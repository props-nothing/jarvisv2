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

/// The composed tool path: a registry, a policy engine, a call lifecycle, and one adapter.
///
/// Holds the registry by value so no caller can register into a registry this pipeline is not reading —
/// a shared registry would let a tool appear to a caller that the pipeline cannot actually resolve.
pub struct ToolPipeline {
    database: Arc<SqliteDatabase>,
    registry: ToolRegistry,
    adapter: Arc<FilesystemReadTool>,
    /// The granted workspace policy, held so every call is decided against the same grants.
    ///
    /// Held rather than passed per call because a caller-supplied policy would let the handler that
    /// derives the actor from the stored run also weaken the grants the daemon was configured with —
    /// the two would then be independently negotiable.
    workspace: WorkspacePolicy,
}

impl ToolPipeline {
    /// Builds the pipeline over granted workspace roots.
    ///
    /// Registers the filesystem adapter's **own** definitions rather than restating them, so policy
    /// reads the same contract the adapter enforces. A second copy of the schema or the risk level here
    /// would be a copy that can disagree, and the definition is what `P3-003` decides about — a
    /// disagreement would mean policy deciding about a tool other than the one being run.
    ///
    /// # Errors
    ///
    /// Returns [`ToolPipelineError`] when the adapter's fixed definitions are rejected or the registry
    /// refuses a registration. Both are configuration faults, so this fails at startup rather than on
    /// the first call.
    pub fn new(
        database: Arc<SqliteDatabase>,
        roots: WorkspaceRoots,
        workspace: WorkspacePolicy,
    ) -> Result<Self, ToolPipelineError> {
        let mut registry = ToolRegistry::new();
        registry.define_all(FilesystemReadTool::definitions()?)?;
        Ok(Self {
            database,
            registry,
            adapter: Arc::new(FilesystemReadTool::new(roots)),
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

        let result = self.adapter.execute(&request).await?;

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
    /// Names the registered tool count and the adapter, and nothing else.
    ///
    /// A pipeline holds a database handle and directory handles, neither of which belongs in a
    /// formatted value.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ToolPipeline")
            .field("tools", &self.registry.len())
            .field("adapter", &self.adapter.adapter_id())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
#[path = "tool_pipeline_tests.rs"]
mod tests;
