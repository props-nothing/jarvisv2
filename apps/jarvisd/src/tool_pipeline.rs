//! The tool pipeline: the composition root that joins the five tool slices together.
//!
//! # Why this module exists
//!
//! `P3-001` through `P3-006` each built one part of the tool path — a contract, a registry, a policy
//! engine, a durable approval, a call lifecycle, and one adapter — and **nothing composed them**. The
//! only `AuthorizationReceipt` constructions in the workspace were test fixtures, and no caller ever
//! admitted a tool call.
//!
//! Closing that gap found a security defect (`P3-006a`: a receipt could declare `Risk::Minimal` for a
//! call policy had decided at `High`) and another a slice later (`P3-006b`: a request could name a tool
//! its receipt did not cover). Both lived in the space **between** two correct modules, which is the
//! space no unit test covers. This module is that space, made explicit and testable.
//!
//! # The pipeline
//!
//! ```text
//! resolve the definition from the registry            P3-002
//! validate against the definition's input schema      P3-001
//! evaluate policy                                     P3-003  -> Allow | RequireApproval | Deny
//! build the receipt FROM the decision                  P3-006a (derived, not stated)
//! admit the call to the idempotency ledger             P3-005
//! move it to `authorized`, then `submitted`            P3-005
//! build the execution request, bound to the receipt    P3-006b
//! execute through the adapter                          P3-006
//! record the outcome                                   P3-005
//! ```
//!
//! `docs/architecture/tools-and-connectors.md` gives this order, and `AGENTS.md` states the rule it
//! implements: "The model may request an effect; deterministic Rust policy decides whether it may
//! happen." The adapter is reached only after every gate has passed.
//!
//! # What this deliberately does not do
//!
//! - **It decides nothing.** Every refusal is a value a previous slice produced; this module sequences
//!   them and reports which one fired, by its stable reason code.
//! - **It does not request or answer an approval.** A held decision returns
//!   [`ToolPipelineOutcome::AwaitingApproval`] carrying the admitted call and the strength an approval
//!   must be established with. The approval round-trip — persisting the request, a human deciding,
//!   resuming the call — is the **next** slice, because resuming needs a run to park in
//!   `awaiting_approval`, which is a run-state change rather than a tool one.
//! - **It does not write `run_events`.** The call row is the audit record for a tool call; linking calls
//!   to the event log is `P3-012`.

use std::sync::Arc;

use jarvis_core::{CanonicalIntentHash, CorrelationId, SystemClock, UtcTimestamp};
use jarvis_storage::{
    CallBinding, CallOrigin, CallTarget, DatabaseError, NewToolCall, SqliteDatabase,
    admit_tool_call, advance_tool_call, record_tool_outcome,
};
#[cfg(test)]
use jarvis_storage::{StoredToolCall, find_tool_call};
use serde_json::Value;

use crate::tool_actor::ToolActor;
use jarvis_tools::ToolId;
use jarvis_tools::ToolOutcome;
use jarvis_tools::ToolRegistry;
use jarvis_tools::{AdapterError, ToolExecutionRequest, ToolExecutionRequestParts};
use jarvis_tools::{
    AuthenticationStrength, PolicyRequest, TargetAssessment, WorkspacePolicy, evaluate,
};
use jarvis_tools::{
    AuthorizationReceipt, AuthorizationReceiptParts, IdempotencyKey, ToolCallResult,
};
use jarvis_tools::{FilesystemReadTool, FilesystemToolError};
use jarvis_tools::{RootError, WorkspaceRoots};
use jarvis_tools::{SchemaError, SchemaViolation};
// The port's methods are reached through the trait, so it must be in scope at the call site. Without
// it, `Arc<FilesystemReadTool>` appears to have no `execute` at all, which reads as a broken adapter
// rather than a missing import.
use jarvis_tools::ToolExecutor;

use crate::dispatch::{Dispatch, DispatchError};

/// What one pipeline call produced.
///
/// Three variants, and each is a different answer to "what should the caller do next": report the
/// result, ask a human, or stop. A single `Result` would collapse the middle case into one of the other
/// two, and the middle case is the one `docs/architecture/security.md` most depends on being visible.
#[derive(Clone, Debug)]
pub enum ToolPipelineOutcome {
    /// The call ran, and this is what the adapter established.
    Executed(Box<ToolCallResult>),
    /// The call was **authorized and recorded** but not run, because a human must decide first.
    ///
    /// Carries the two things a caller needs to act: the admitted call the approval would authorize,
    /// and the strength the approval must be established with — which `P3-003` computed and which a
    /// caller must not be free to lower.
    AwaitingApproval {
        /// The admitted call, which is what an approval would bind to.
        call_id: String,
        /// The authentication strength an approval must carry.
        required_strength: AuthenticationStrength,
        /// The stable reason code the decision was held at.
        reason_code: &'static str,
    },
    /// The call was refused, and nothing was recorded.
    Refused {
        /// The stable reason code, so a caller can distinguish a missing scope from a workspace denial.
        reason_code: &'static str,
    },
}

/// Explains why the pipeline could not complete a call.
///
/// Every variant is a **fault** rather than a decision. A refusal is
/// [`ToolPipelineOutcome::Refused`], because refusing is a correct answer to a request rather than a
/// failure of the pipeline — and mixing the two would make a workspace that denies a tool
/// indistinguishable from a broken database.
#[derive(Debug, thiserror::Error)]
pub enum ToolPipelineError {
    /// The tool identifier was not well-formed.
    #[error("the tool identifier is invalid: {0}")]
    InvalidToolId(#[from] jarvis_tools::ToolIdError),
    /// The registry does not hold the tool.
    ///
    /// Reported with the identifier a caller asked for rather than as the registry's own error, because
    /// "the registry does not hold X" is the actionable form and the registry's message is addressed to
    /// whoever registered tools rather than to whoever called one.
    #[error("the registry does not hold {tool}")]
    UnknownTool {
        /// The tool that was asked for.
        tool: String,
    },
    /// The arguments did not validate against the tool's input schema.
    #[error("the arguments are not valid for {tool}: {violations}")]
    InvalidArguments {
        /// The tool the arguments were checked against.
        tool: String,
        /// The bounded violations, rendered so a caller learns which keyword failed where.
        violations: String,
    },
    /// The arguments could not be canonicalized for the intent digest.
    #[error("the arguments for {tool} could not be turned into an intent digest")]
    UnintelligibleIntent {
        /// The tool the arguments were for.
        tool: String,
    },
    /// A call was already admitted under the same run and key.
    ///
    /// The `P3-005` ledger's answer to a re-drive, reported separately from a generic storage fault
    /// because the caller's response is specific: **adopt the existing call** rather than making a
    /// second one, which is what stops one action becoming two.
    #[error("this call was already admitted as {existing_call_id}")]
    DuplicateCall {
        /// The existing call to adopt.
        existing_call_id: String,
    },
    /// The adapter's own definitions were rejected, which is an authoring error.
    #[error("the filesystem adapter's definitions are inconsistent: {0}")]
    Adapter(#[from] FilesystemToolError),
    /// The registry rejected a definition it was given.
    ///
    /// A configuration fault rather than a call-time one: it means the adapter's own fixed definitions
    /// collide with something already registered, so the pipeline must not start at all.
    #[error("the registry rejected a definition: {0}")]
    Registration(#[from] jarvis_tools::RegistrationError),
    /// A registered tool could not be read back, which means the registry and its own inventory disagree.
    #[error("a registered tool could not be resolved: {0}")]
    Registry(#[from] jarvis_tools::RegistryError),
    /// The granted roots could not be opened.
    #[error(transparent)]
    Roots(#[from] RootError),
    /// The call row could not be written or read.
    #[error(transparent)]
    Storage(#[from] DatabaseError),
    /// The execution request was rejected by its own constructor.
    #[error(transparent)]
    Request(#[from] jarvis_tools::ExecutionRequestError),
    /// The authorization receipt was rejected, which means the decision and the intent disagree.
    #[error(transparent)]
    Receipt(#[from] jarvis_tools::ReceiptError),
    /// An idempotency key could not be generated.
    ///
    /// Propagated rather than replaced with a fallback: a key that could collide would make the ledger
    /// adopt an unrelated call, which is worse than refusing this one (`P3-005`).
    #[error(transparent)]
    Key(#[from] jarvis_tools::IdempotencyKeyError),
    /// The adapter could not establish an outcome.
    #[error(transparent)]
    AdapterCall(#[from] AdapterError),
    /// A schema validator was rejected, which is an authoring error.
    #[error(transparent)]
    Schema(#[from] SchemaError),
    /// The dispatch table is unusable.
    ///
    /// A configuration fault rather than a call-time one: two adapters claiming one tool, or a registered
    /// tool no adapter can run, means the pipeline must not start. Resolved here because the alternative is a
    /// call that is authorized, admitted durably, and then fails for a reason unrelated to the request.
    #[error(transparent)]
    Dispatch(#[from] DispatchError),
}

/// Returns the instant a call started now must stop by.
///
/// Computed here rather than on `ToolDefinition` because the deadline is a **property of a call**
/// rather than of a tool: the same definition produces a different deadline on every call, and a
/// method on the definition would invite a caller to treat the tool's declared timeout as an
/// instant. The seconds are the definition's declaration, which is what `P3-001` validated.
fn deadline_after(start: UtcTimestamp, timeout_seconds: u32) -> UtcTimestamp {
    let nanos = i128::from(timeout_seconds) * 1_000_000_000;
    // The nanosecond range of an `i128` admits every instant `time` can represent plus any timeout the
    // contract allows (bounded at 600 seconds by `P3-001`), so this cannot overflow in practice. A
    // failure falls back to `start`, which makes the deadline already passed — failing **closed** by
    // refusing the call rather than granting an unbounded deadline.
    UtcTimestamp::from_unix_nanos(start.unix_nanos().saturating_add(nanos)).unwrap_or(start)
}

/// An admitted call that is ready to run.
///
/// Groups the values `authorize_and_admit` produces and `execute_and_record` consumes, so the two steps
/// cannot be called with a receipt from one call and a key from another. That is the same reasoning
/// `P3-006a` and `P3-006b` applied to the receipt and the request: a set of values that must agree is
/// safer as one value than as five parameters.
struct PreparedCall {
    /// The durable call identifier the ledger assigned.
    call_id: String,
    /// The authority, bound to this intent by its own constructor.
    receipt: AuthorizationReceipt,
    /// The ledger key, which the request carries so the provider can deduplicate.
    key: IdempotencyKey,
    /// The validated arguments, carried so the request hashes to the same intent the receipt bound.
    arguments: Value,
    /// When the call was admitted, from which the deadline is computed.
    issued_at: UtcTimestamp,
}

/// Renders schema violations as one bounded, reader-facing line.
///
/// `instance_path` and `keyword` are separate fields rather than a message, deliberately
/// (`P3-001`): both are locations and identifiers, so neither can carry prose an attacker authored.
/// Joining them here is what makes the error readable without reintroducing free text at the source.
fn describe(violations: &[SchemaViolation]) -> String {
    violations
        .iter()
        .map(|violation| {
            let path = if violation.instance_path.is_empty() {
                "(the whole input)".to_owned()
            } else {
                violation.instance_path.clone()
            };
            format!("{path} failed `{}`", violation.keyword)
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// The composed tool path: a registry, a policy engine, a call lifecycle, and the adapters that run tools.
///
/// Holds the registry by value so no caller can register into a registry this pipeline is not reading —
/// a shared registry would let a tool appear to a caller that the pipeline cannot actually resolve.
pub struct ToolPipeline {
    database: Arc<SqliteDatabase>,
    registry: ToolRegistry,
    /// Which adapter runs which tool, resolved by canonical identifier.
    ///
    /// A table rather than one adapter because the pipeline already serves more than one tool area, and the
    /// filesystem adapter is no longer the only one — the MCP host brings a second. The lookup is keyed by the
    /// identifier the receipt binds, so the adapter that runs is the one the definition was authorized as
    /// (see `dispatch.rs` for why the table refuses a duplicate claim and an uncovered registration).
    dispatch: Dispatch,
    /// The granted workspace policy, held so every call is decided against the same grants.
    ///
    /// Held rather than passed per call because a caller-supplied policy would let the handler that
    /// derives the actor from the stored run also weaken the grants the daemon was configured with —
    /// the two would then be independently negotiable.
    workspace: WorkspacePolicy,
}

impl ToolPipeline {
    /// Builds the pipeline over granted workspace roots, with the filesystem adapter registered.
    ///
    /// Registers the filesystem adapter's **own** definitions rather than restating them, so policy
    /// reads the same contract the adapter enforces. A second copy of the schema or the risk level here
    /// would be a copy that can disagree, and the definition is what `P3-003` decides about — a
    /// disagreement would mean policy deciding about a tool other than the one being run.
    ///
    /// `#[cfg(test)]` because the daemon composes through [`Self::with_adapters`] — the MCP host may add
    /// adapters, so a production caller always has an `additional` list even when it is empty. Keeping a
    /// second public constructor nothing calls is how a surface grows a method with no consumer, and the
    /// tests want a three-argument form because they register no MCP servers.
    ///
    /// # Errors
    ///
    /// Returns [`ToolPipelineError`] when the adapter's fixed definitions are rejected or the registry
    /// refuses a registration. Both are configuration faults, so this fails at startup rather than on
    /// the first call.
    #[cfg(test)]
    pub fn new(
        database: Arc<SqliteDatabase>,
        roots: WorkspaceRoots,
        workspace: WorkspacePolicy,
    ) -> Result<Self, ToolPipelineError> {
        Self::with_adapters(database, Some(roots), workspace, Vec::new())
    }

    /// Builds the pipeline over granted workspace roots **and any additional adapters**.
    ///
    /// `additional` carries adapters whose definitions are supplied by their own source — the MCP host's,
    /// one per configured server. Each entry is `(definitions, adapter)`, so a caller states which contract
    /// an adapter runs and the registry is populated from the same pair the dispatch table is. Reusing one
    /// pair for both is what keeps the registry and the dispatch table from disagreeing about what exists.
    ///
    /// # An absent filesystem grant is not a filesystem tool
    ///
    /// `roots` is an `Option` rather than a possibly-empty `WorkspaceRoots`, because that type **refuses an
    /// empty list**: `RootError::NoRoots` exists precisely so a tool cannot silently read nothing while
    /// looking like a tool that works. A daemon configured only with MCP servers is therefore `None` here,
    /// and the filesystem adapter is **not registered at all** — which is the honest shape: the tool is
    /// absent rather than present-and-failing, exactly the reasoning `compose_tool_pipeline` already used
    /// when it returned no pipeline over zero roots.
    ///
    /// The first version of this constructor took a `WorkspaceRoots` and tried to build one from an empty
    /// list for the MCP-only case, which is how the contradiction surfaced: a *test* asserting a routing
    /// property failed with `NoRoots`, and the failure was the composition being wrong rather than the test.
    ///
    /// # Errors
    ///
    /// Returns [`ToolPipelineError`] for an unusable grant, a rejected definition, a registry refusal, a
    /// duplicate claim between two adapters, or a registered tool no adapter can run. Every one is a
    /// configuration fault, so all of them fail startup rather than a call.
    pub fn with_adapters(
        database: Arc<SqliteDatabase>,
        roots: Option<WorkspaceRoots>,
        workspace: WorkspacePolicy,
        additional: Vec<(Vec<jarvis_tools::ToolDefinition>, Arc<dyn ToolExecutor>)>,
    ) -> Result<Self, ToolPipelineError> {
        let mut registry = ToolRegistry::new();
        let mut sources: Vec<(Vec<jarvis_tools::ToolDefinition>, Arc<dyn ToolExecutor>)> =
            Vec::new();

        // Registered first when granted, so the filesystem tools are the native ones a later MCP tool cannot
        // shadow — a collision is refused by the registry either way, but the order makes which one is
        // "already present" deterministic rather than dependent on the caller's list order.
        if let Some(roots) = roots {
            let filesystem_definitions = FilesystemReadTool::definitions()?;
            registry.define_all(filesystem_definitions.clone())?;
            sources.push((
                filesystem_definitions,
                Arc::new(FilesystemReadTool::new(roots)) as Arc<dyn ToolExecutor>,
            ));
        }

        // The MCP definitions are registered from the **host's own** list, which came from the catalog, so
        // the risk and effects policy reads are the ones the catalog derived from the operator's posture.
        for (definitions, adapter) in additional {
            registry.define_all(definitions.clone())?;
            sources.push((definitions, adapter));
        }

        // The dispatch table is built from what each adapter declares, and then checked against the
        // registry's tool list. The order matters: coverage is verified against what the registry actually
        // holds, so a definition that failed to register cannot leave a hole this check would miss.
        let dispatch = Dispatch::new(sources)?;
        // Coverage is checked against the identifiers the **registry** holds, parsed back from its operator-facing
        // inventory. Parsing can only fail for a key the registry itself produced, so a failure here is an
        // authoring error rather than a configuration one — and it is reported rather than skipped, because a
        // skipped identifier would be a hole the check exists to find.
        let registered: Vec<ToolId> = registry
            .inventory()
            .iter()
            .map(|entry| ToolId::new(&entry.id))
            .collect::<Result<_, _>>()?;
        dispatch.verify_covers(&registered)?;

        Ok(Self {
            database,
            registry,
            dispatch,
            workspace,
        })
    }

    /// Returns the database handle the call rows are written through.
    ///
    /// Offered so a test can assert a property of the **stored** row rather than only of the returned
    /// result: the ledger's refusal of a later writer is a property storage enforces, and proving it
    /// needs the same handle the pipeline wrote with.
    #[cfg(test)]
    #[must_use]
    pub fn database(&self) -> &SqliteDatabase {
        &self.database
    }

    /// Returns how many tools are dispatchable.
    ///
    /// `#[cfg(test)]` because nothing in the product asks: a request names one tool and the pipeline resolves
    /// it. The count is what a **routing** test needs — a table with one entry finds the right adapter however
    /// it is looked up, so an assertion that dispatch works requires knowing there was more than one candidate.
    /// The falsification run that made this accessor necessary is recorded on
    /// `a_call_reaches_the_adapter_that_owns_its_definition`.
    #[cfg(test)]
    #[must_use]
    pub fn dispatchable_tools(&self) -> usize {
        self.dispatch.len()
    }

    /// Runs one tool call through every gate.
    ///
    /// # Errors
    ///
    /// Returns [`ToolPipelineError`] for a **fault**. A refusal — an unknown tool, invalid arguments, a
    /// workspace denial, a missing scope — is [`ToolPipelineOutcome::Refused`], because those are correct
    /// answers to a request.
    ///
    /// # The order, and why
    ///
    /// Resolution precedes validation because the schema to validate against comes from the definition.
    /// Validation precedes policy because policy reads declared fields, so a decision about arguments
    /// nothing checked would be a decision about a different call. Policy precedes admission because
    /// admitting a call the workspace denies would leave a durable record of an action that was never
    /// permitted. Admission precedes the execution request because the request carries the admitted call
    /// identifier, and the ledger's idempotency is keyed on the run and the key.
    pub async fn call_tool(
        &self,
        tool: &str,
        arguments: Value,
        actor: &ToolActor,
        correlation_id: CorrelationId,
    ) -> Result<ToolPipelineOutcome, ToolPipelineError> {
        let id = ToolId::new(tool)?;
        let definition = match self.registry.get(&id) {
            Ok(definition) => definition.clone(),
            Err(_) => {
                return Err(ToolPipelineError::UnknownTool {
                    tool: tool.to_owned(),
                });
            }
        };

        // 1. Validate. `P3-001` compiled the validator at registration, so this is a check, not a parse.
        Self::validate(&definition, tool, &arguments)?;

        // 2. Decide. A pure function over the borrowed definition: no clock, no I/O, no repository.
        let decision = evaluate(&PolicyRequest {
            definition: &definition,
            actor: actor.authority(),
            workspace: &self.workspace,
            channel: actor.channel(),
            claimed_strength: actor.claimed_strength(),
            available: definition.availability().is_available(),
            target: TargetAssessment::none(),
        });
        if decision.is_denied() {
            return Ok(ToolPipelineOutcome::Refused {
                reason_code: decision.reason_code(),
            });
        }

        // 3-5. Bind the authority, admit the call, and stop if a human must decide first.
        let Some(prepared) = self
            .authorize_and_admit(
                tool,
                &definition,
                &arguments,
                actor,
                &decision,
                correlation_id,
            )
            .await?
        else {
            // A held decision: the call is admitted and the caller learns what an approval would need.
            return Ok(ToolPipelineOutcome::AwaitingApproval {
                call_id: correlation_id.to_string(),
                required_strength: decision
                    .required_strength()
                    .unwrap_or(AuthenticationStrength::Absent),
                reason_code: decision.reason_code(),
            });
        };

        // 6-7. Execute through the adapter and record what it established.
        self.execute_and_record(prepared, tool, &definition, correlation_id)
            .await
    }

    /// Returns every registered definition, in the registry's stable order.
    ///
    /// Offered so a caller that must state *what this daemon can do* — the inbound MCP endpoint's served
    /// surface — reads the same list dispatch was verified against. Deriving it any other way would be a second
    /// statement of which tools exist, and the two could disagree in the direction that matters: a tool served
    /// with no adapter to run it.
    ///
    /// # Errors
    ///
    /// Returns [`ToolPipelineError`] when an inventory entry cannot be resolved back to its definition. That is
    /// unreachable for a key the registry itself produced — the same reasoning
    /// [`Self::with_adapters`] gives when it parses identifiers back from the inventory — so a failure here is
    /// reported rather than skipped, because a skipped tool would be a hole in the served surface.
    pub fn definitions(&self) -> Result<Vec<jarvis_tools::ToolDefinition>, ToolPipelineError> {
        let ids: Vec<ToolId> = self
            .registry
            .inventory()
            .iter()
            .map(|entry| ToolId::new(&entry.id))
            .collect::<Result<_, _>>()?;
        ids.iter()
            .map(|id| {
                self.registry
                    .get(id)
                    .cloned()
                    .map_err(ToolPipelineError::Registry)
            })
            .collect()
    }

    /// Runs one tool call for a **remote MCP caller**, applying every gate a local call gets and recording no
    /// call row.
    ///
    /// # Why this exists beside `call_tool` rather than as a flag on it
    ///
    /// A remote MCP call cannot be a `tool_calls` row: `0007_tool_calls.sql` declares
    /// `run_id TEXT NOT NULL REFERENCES agent_runs (id)`, and an MCP request has no run. So the steps that
    /// write rows — `authorize_and_admit` and `execute_and_record` — cannot be used, and the honest shape is a
    /// second entrypoint rather than a `record: bool` the caller could set wrongly.
    ///
    /// **What is shared, and that is the point.** The definition comes from the same registry, and
    /// [`Self::validate`] and [`evaluate`] are the *same functions* `call_tool` calls, over the same
    /// `WorkspacePolicy` and the same dispatcher. A remote call therefore reaches the identical adapter with
    /// the identical confinement (`ADR-0020`) and the identical policy decision, and the only difference is
    /// that nothing is persisted.
    ///
    /// # Errors
    ///
    /// Returns [`ToolPipelineError`] for a fault, and [`ToolPipelineOutcome::Refused`] for a decision. A
    /// decision requiring an approval is **refused** rather than returned as
    /// [`ToolPipelineOutcome::AwaitingApproval`]: there is no run to park, and handing a caller an approval
    /// shape it cannot complete would be a promise nothing can keep.
    ///
    /// # This records nothing, which is a recorded limit
    ///
    /// A remote call is **not** in the ledger, so it is observable in a log and not in the audit trail.
    /// `P3-012` owns linking calls to a durable log; until then the absence is stated here rather than left for
    /// a reader to discover.
    pub async fn call_remote_tool(
        &self,
        tool: &str,
        arguments: Value,
        actor: &ToolActor,
        correlation_id: CorrelationId,
    ) -> Result<ToolPipelineOutcome, ToolPipelineError> {
        let id = ToolId::new(tool)?;
        let definition = match self.registry.get(&id) {
            Ok(definition) => definition.clone(),
            Err(_) => {
                return Err(ToolPipelineError::UnknownTool {
                    tool: tool.to_owned(),
                });
            }
        };

        // 1. Validate, with the same function the local path uses. A remote caller does not get a weaker
        //    argument check than an operator's own call, which is the failure a second implementation would
        //    produce.
        Self::validate(&definition, tool, &arguments)?;

        // 2. Decide, with the same engine over the same definitions.
        let decision = evaluate(&PolicyRequest {
            definition: &definition,
            actor: actor.authority(),
            workspace: &self.workspace,
            channel: actor.channel(),
            claimed_strength: actor.claimed_strength(),
            available: definition.availability().is_available(),
            // `none`, deliberately: an MCP request carries no target and this module does not read one out of
            // free-form arguments. A target assessment invented from arguments would be a second classifier
            // deciding risk, which is the thing `TargetAssessment` exists to keep singular.
            target: TargetAssessment::none(),
        });
        if decision.is_denied() {
            return Ok(ToolPipelineOutcome::Refused {
                reason_code: decision.reason_code(),
            });
        }
        // A held decision is a refusal on this path, and the reason code says so rather than reporting the
        // workspace's `require_approval` code as if an approval were obtainable.
        if decision.decision() == jarvis_tools::Decision::RequireApproval {
            return Ok(ToolPipelineOutcome::Refused {
                reason_code: "mcp_call_cannot_hold_an_approval",
            });
        }
        // A second, independent statement, because `ApprovalPolicy` is a declaration on the **tool** and the
        // engine's threshold is a **workspace** setting. A tool declaring an approval its workspace would not
        // ask for must still not run here, and a test varying only the workspace would not catch removing this.
        if definition.approval() != jarvis_tools::ApprovalPolicy::Auto {
            return Ok(ToolPipelineOutcome::Refused {
                reason_code: "mcp_call_cannot_hold_an_approval",
            });
        }

        // 3-4. Build the authority and run, with no admission and no outcome write. The intent is computed and
        //      the receipt carried, so the adapter checks the same binding it always does.
        let intent =
            CanonicalIntentHash::compute(tool, definition.version(), &arguments).map_err(|_| {
                ToolPipelineError::UnintelligibleIntent {
                    tool: tool.to_owned(),
                }
            })?;
        let now = UtcTimestamp::now(&SystemClock);
        let receipt = AuthorizationReceipt::new(AuthorizationReceiptParts {
            receipt_id: correlation_id.to_string(),
            tool: id.clone(),
            tool_version: definition.version().to_owned(),
            arguments: arguments.clone(),
            intent_hash: intent,
            policy_version: actor.policy_version().to_owned(),
            decision: decision.clone(),
            // No approval, and none is possible: a held decision returned above. A citation here would be
            // inventing authority this path never obtained.
            approval: None,
            correlation_id,
            issued_at: now,
        })?;

        let key = IdempotencyKey::generate()?;
        let request = ToolExecutionRequest::new(ToolExecutionRequestParts {
            call_id: correlation_id.to_string(),
            tool: id,
            tool_version: definition.version().to_owned(),
            arguments,
            receipt,
            idempotency_key: key,
            deadline: deadline_after(now, definition.timeout_seconds()),
            correlation_id,
        })?;

        match self.dispatch.adapter_for(request.tool()) {
            Some(adapter) => Ok(ToolPipelineOutcome::Executed(Box::new(
                adapter.execute(&request).await?,
            ))),
            // Unreachable through this path for the same reason as the local one: coverage was verified at
            // construction. Reported as a fault rather than defaulted.
            None => Err(ToolPipelineError::Dispatch(DispatchError::Uncovered {
                tool: tool.to_owned(),
            })),
        }
    }

    /// Validates arguments against the definition's compiled input schema.
    ///
    /// # Errors
    ///
    /// Returns [`ToolPipelineError::InvalidArguments`] with the bound violations, so a caller learns
    /// which keyword failed where rather than only that something did.
    fn validate(
        definition: &jarvis_tools::ToolDefinition,
        tool: &str,
        arguments: &Value,
    ) -> Result<(), ToolPipelineError> {
        let report = definition.input_schema().validate(arguments)?;
        if report.is_valid() {
            return Ok(());
        }
        Err(ToolPipelineError::InvalidArguments {
            tool: tool.to_owned(),
            violations: describe(report.violations()),
        })
    }

    /// Builds the authority, admits the call, and reports whether it may run.
    ///
    /// Returns `Ok(None)` for a **held** decision, which is not an error: the call is admitted and
    /// recorded as `requested` so an approval has something to bind to, and running it would be the
    /// confused-deputy shape `docs/architecture/security.md` refuses.
    ///
    /// # Errors
    ///
    /// Returns [`ToolPipelineError`] for a fault: an uncomputable intent, a rejected receipt, a
    /// failed key generation, or a durable write that did not succeed.
    async fn authorize_and_admit(
        &self,
        tool: &str,
        definition: &jarvis_tools::ToolDefinition,
        arguments: &Value,
        actor: &ToolActor,
        decision: &jarvis_tools::PolicyDecision,
        correlation_id: CorrelationId,
    ) -> Result<Option<PreparedCall>, ToolPipelineError> {
        // The receipt's risk and intent are DERIVED from the decision and the arguments, so the
        // authority an adapter acts on cannot disagree with the decision (`P3-006a`).
        let intent =
            CanonicalIntentHash::compute(tool, definition.version(), arguments).map_err(|_| {
                ToolPipelineError::UnintelligibleIntent {
                    tool: tool.to_owned(),
                }
            })?;
        let id = ToolId::new(tool)?;
        let now = UtcTimestamp::now(&SystemClock);
        let call_id = correlation_id.to_string();
        let receipt = AuthorizationReceipt::new(AuthorizationReceiptParts {
            receipt_id: call_id.clone(),
            tool: id,
            tool_version: definition.version().to_owned(),
            arguments: arguments.clone(),
            intent_hash: intent,
            policy_version: actor.policy_version().to_owned(),
            decision: decision.clone(),
            // No approval: a held decision returns before this receipt is used for anything but the
            // call row, and fabricating a citation would be inventing authority.
            approval: None,
            correlation_id,
            issued_at: now,
        })?;

        // The key is generated here rather than derived from the intent: the digest must be
        // deterministic because the receipt binds to it, while the key must be unique per logical
        // call, so a deliberate second identical call is a second call (`P3-005`).
        let key = IdempotencyKey::generate()?;
        let admitted = admit_tool_call(
            &self.database,
            &NewToolCall::new(
                call_id,
                CallOrigin::new(actor.workspace_id(), actor.run_id(), None),
                CallTarget::new(tool, definition.version()),
                CallBinding::new(
                    intent.to_hex(),
                    key.as_str(),
                    receipt.receipt_id(),
                    actor.policy_version(),
                    None,
                ),
                correlation_id,
                now,
            )?,
        )
        .await;
        let call_id = match admitted {
            Ok(admitted) => admitted,
            Err(DatabaseError::ToolCallDuplicate { existing_call_id }) => {
                // The ledger's answer to a re-drive: adopt rather than duplicate.
                return Err(ToolPipelineError::DuplicateCall { existing_call_id });
            }
            Err(error) => return Err(ToolPipelineError::Storage(error)),
        };

        if decision.is_held() {
            return Ok(None);
        }
        Ok(Some(PreparedCall {
            call_id,
            receipt,
            key,
            arguments: arguments.clone(),
            issued_at: now,
        }))
    }

    /// Moves the call to `authorized` then `submitted`, runs the adapter, and records the outcome.
    ///
    /// The two `advance_tool_call`s happen **before** the adapter is reached, so a crash between here
    /// and the result leaves a row that `must_not_repeat` is true for rather than one that looks
    /// untouched (`P3-005`). The order matters: `requested -> submitted` is refused by the transition
    /// table, because a call that never passed `authorized` has no receipt.
    ///
    /// # Errors
    ///
    /// Returns [`ToolPipelineError`] when the request is rejected, a transition is refused, the adapter
    /// cannot establish an outcome, or the outcome cannot be recorded.
    async fn execute_and_record(
        &self,
        prepared: PreparedCall,
        tool: &str,
        definition: &jarvis_tools::ToolDefinition,
        correlation_id: CorrelationId,
    ) -> Result<ToolPipelineOutcome, ToolPipelineError> {
        // The request is bound to the receipt by its own constructor (`P3-006b`), so a mismatch between
        // what was authorized and what is asked for cannot reach the adapter.
        let request = ToolExecutionRequest::new(ToolExecutionRequestParts {
            call_id: prepared.call_id.clone(),
            tool: ToolId::new(tool)?,
            tool_version: definition.version().to_owned(),
            arguments: prepared.arguments,
            receipt: prepared.receipt,
            idempotency_key: prepared.key,
            deadline: deadline_after(prepared.issued_at, definition.timeout_seconds()),
            correlation_id,
        })?;

        advance_tool_call(
            &self.database,
            &prepared.call_id,
            ToolOutcome::Authorized,
            prepared.issued_at,
        )
        .await?;
        advance_tool_call(
            &self.database,
            &prepared.call_id,
            ToolOutcome::Submitted,
            UtcTimestamp::now(&SystemClock),
        )
        .await?;

        let result = match self.dispatch.adapter_for(request.tool()) {
            Some(adapter) => adapter.execute(&request).await?,
            // Unreachable through `call_tool`, which resolves the definition from the registry first and
            // coverage was verified at construction — so a registered tool always has an adapter. Reported as
            // a fault rather than defaulted, because the alternative would be running a call on an adapter
            // that never claimed the tool.
            None => {
                return Err(ToolPipelineError::Dispatch(DispatchError::Uncovered {
                    tool: tool.to_owned(),
                }));
            }
        };

        // Written once and not replaceable, so a re-drive cannot downgrade an `Unknown` to a `Failed` —
        // which is how one sent message becomes two (`P3-005`).
        record_tool_outcome(
            &self.database,
            &prepared.call_id,
            result.record(),
            result
                .output()
                .map(|output| (output.content(), output.is_truncated())),
            result.reported_at(),
        )
        .await?;

        Ok(ToolPipelineOutcome::Executed(Box::new(result)))
    }

    /// Reads one stored call.
    ///
    /// # Errors
    ///
    /// Returns [`ToolPipelineError::Storage`] when the call is absent or cannot be decoded.
    #[cfg(test)]
    pub async fn call(&self, call_id: &str) -> Result<StoredToolCall, ToolPipelineError> {
        Ok(find_tool_call(&self.database, call_id).await?)
    }
}

impl std::fmt::Debug for ToolPipeline {
    /// Names the registered tool count and the adapters, and nothing else.
    ///
    /// A pipeline holds a database handle and directory handles, neither of which belongs in a
    /// formatted value.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ToolPipeline")
            .field("tools", &self.registry.len())
            .field("dispatch", &self.dispatch)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
#[path = "tool_pipeline_tests.rs"]
mod tests;
