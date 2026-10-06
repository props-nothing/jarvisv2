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
//! - **It does not answer an approval.** A held decision returns
//!   [`ToolPipelineOutcome::AwaitingApproval`] after writing a **durable approval request**, so a
//!   decision has something to bind to and a caller can name the approval it applies to. Deciding it and
//!   resuming the call — rebuilding a receipt that cites the decision and re-driving the admitted call —
//!   is the next slice (`P3-012b`), because resuming past a terminal outcome is where a duplicate
//!   delivery would become a second effect.
//! - **It does not write `run_events`.** The call row is the audit record for a tool call; linking calls
//!   to the event log is `P3-012`.

use std::str::FromStr;
use std::sync::Arc;

use jarvis_core::{ApprovalId, ApprovalRequest, ApprovalRequestParts, RunId, WorkspaceId};
use jarvis_core::{
    CanonicalIntentHash, CorrelationId, EventSummary, RunEventKind, RunEventPayload, SystemClock,
    UtcTimestamp,
};
use jarvis_storage::{
    CallBinding, CallOrigin, CallTarget, DatabaseError, NewRunEvent, NewToolCall, SqliteDatabase,
    StoredToolCall, admit_tool_call, advance_tool_call, append_run_event, create_approval,
    find_approval, find_tool_call, link_tool_call_approval, record_tool_outcome,
};
use serde_json::Value;

use crate::tool_actor::ToolActor;
use jarvis_tools::ToolId;
use jarvis_tools::ToolOutcome;
use jarvis_tools::ToolRegistry;
use jarvis_tools::{AdapterError, ToolExecutionRequest, ToolExecutionRequestParts};
use jarvis_tools::{
    ApprovalPolicy, PolicyDecision, PolicyRequest, Risk, TargetAssessment, ToolSource,
    WorkspacePolicy, evaluate,
};
use jarvis_tools::{
    AuthorizationReceipt, AuthorizationReceiptParts, IdempotencyKey, ToolCallResult,
};
use jarvis_tools::{FilesystemTool, FilesystemToolError};
use jarvis_tools::{RootError, WorkspaceRoots};
use jarvis_tools::{SchemaError, SchemaViolation};
// The port's methods are reached through the trait, so it must be in scope at the call site. Without
// it, `Arc<FilesystemTool>` appears to have no `execute` at all, which reads as a broken adapter
// rather than a missing import.
use jarvis_tools::ToolExecutor;

use crate::dispatch::{Dispatch, DispatchError};

/// The policy-version label a **preview** carries.
///
/// A stored receipt's label is a claim about the policy in force when a call was admitted, and a preview
/// admits nothing — so it must not borrow a recorded label or invent one that looks recorded. This
/// constant exists so the preview path names what it is doing rather than passing an empty string that
/// reads as a missing value.
pub const PREVIEW_POLICY_VERSION: &str = "preview";

/// One tool in the operator-facing policy inventory.
///
/// # Why this type exists beside `jarvis_tools::ToolInventoryEntry`
///
/// The registry's entry answers "what is registered and what is wrong with it". This one answers the
/// question an operator configuring policy has: **what will this workspace do with it**. It therefore
/// carries the declared policy, the effective policy, and the two facts that explain a difference
/// between them — an override and a denial — as well as the scopes and effects a person needs to judge
/// whether the posture is right.
///
/// It is a **domain** value rather than a wire type: the REST shape is `jarvis_protocol::ToolReply`, and
/// the gateway maps between them. Keeping the two apart is the rule `rest.rs` states, so a change here
/// does not silently change a client-visible contract.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PolicyInventoryEntry {
    /// The canonical identifier, which is also the name a policy override addresses.
    pub id: String,
    /// The operator- and model-facing title.
    pub title: String,
    /// The declared behaviour/schema version.
    pub version: String,
    /// Which class of source the tool came from.
    pub source: ToolSource,
    /// The declared baseline risk.
    pub risk: Risk,
    /// The effects the tool may have, as stable names, in the adapter's own order.
    pub effects: Vec<String>,
    /// The capability scopes the tool requires.
    pub required_scopes: Vec<String>,
    /// The approval policy the tool's own definition declares.
    pub declared_approval: ApprovalPolicy,
    /// The approval policy in force after any workspace override.
    pub effective_approval: ApprovalPolicy,
    /// Whether an override is currently raising the declared policy.
    pub overridden: bool,
    /// Whether the workspace refuses this tool outright.
    pub denied: bool,
    /// Whether the tool can currently run.
    pub callable: bool,
    /// Why it cannot, when it cannot.
    pub unavailable_reason: Option<String>,
    /// Whether a call is held for the owner's yes or no before it runs.
    pub asks_first: bool,
}

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
    /// Carries the three things a caller needs to act: the admitted call the approval would authorize,
    /// the durable approval that was written so a decision has something to bind to, and the reason the
    /// call is waiting.
    AwaitingApproval {
        /// The admitted call, which is what an approval would bind to.
        call_id: String,
        /// The durable approval request that was persisted for this hold.
        ///
        /// Present rather than implied: a caller must be able to **name** the approval a decision
        /// applies to, and reading it back out of the database by run would be guesswork when a run
        /// has held more than one call.
        approval_id: String,
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
    /// A call that needs an answer repeats one this run already asked about.
    ///
    /// One approval exists per action per run (`approvals_run_intent_idx`), so a repeat is refused with a
    /// sentence the model can act on rather than surfacing as a database error.
    #[error(
        "this exact call was already asked about in this run and is {state}; it will not be asked again. \
         Change the arguments if a different action is meant, or tell the user"
    )]
    RepeatedRequest {
        /// The earlier approval's state, such as `approved` or `denied`.
        state: String,
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
    /// No call has the identifier a resume named.
    ///
    /// Distinct from [`Self::Storage`] because the caller's next step is different: an unknown call is a
    /// wrong identifier, while a storage fault is a broken database. Reported as its own variant so a
    /// caller is not told the database failed when it named a call that does not exist.
    #[error("no tool call exists for the requested identifier: {call_id}")]
    UnknownCall {
        /// The identifier that was named.
        call_id: String,
    },
    /// A resume was refused, and the reason names the check that fired.
    ///
    /// # Why one variant with a reason rather than five variants
    ///
    /// Unlike a policy refusal — where the reason code is a **stable contract** a client switches on — these
    /// are programming-level answers whose full text is an internal explanation, not a wire value. One
    /// variant keeps the arm count in a match low while the reason still names which check fired, and it is
    /// deliberately not a stable code because nothing outside this crate branches on it.
    #[error("the call {call_id} cannot be resumed: {reason}")]
    ResumeRefused {
        /// The call the resume named.
        call_id: String,
        /// Which check refused it, in words an operator can act on.
        reason: &'static str,
    },
    /// A stored identifier an approval needs could not be rebuilt.
    ///
    /// An approval records its workspace and run as typed identifiers, and a pipeline holds them as
    /// the strings its call rows use. A string that is not a `UUIDv7` is an authoring or corruption
    /// fault rather than a decision, so it is reported rather than substituted — a substituted
    /// identifier would write an approval against a workspace that does not exist.
    #[error("the {field} on a held call is not a valid identifier")]
    ApprovalIdentity {
        /// Which identifier was refused.
        field: &'static str,
    },
    /// The domain refused a field of the approval request.
    ///
    /// Carried as the domain's own field-naming error so a reader learns **which** invariant failed
    /// without the message forwarding a preview or a length, which `P3-004` deliberately keeps out.
    #[error("the approval request was refused: {field}")]
    Approval {
        /// The domain error naming the offending field.
        field: jarvis_core::InvalidApprovalField,
    },
    /// A run event's fields were rejected by their own constructors.
    ///
    /// The event log and the call row are written on the same path, and a rejected summary or payload is
    /// an authoring error in a fixed literal rather than a fact about the request — so it is reported
    /// rather than retried with the event skipped. Skipping would make the log silently incomplete, which
    /// is the failure an audit trail exists to prevent.
    #[error("the run event for {kind} was rejected: {source}")]
    Event {
        /// The event kind whose fields were refused.
        kind: &'static str,
        /// The domain error, which names the rejected field without forwarding its content.
        source: jarvis_core::InvalidRunEvent,
    },
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

/// Appends one run event, naming the kind in any rejection so the failure says which event failed.
///
/// # Why the event is written here rather than by the executor
///
/// The executor writes events for a **run's own** lifecycle — its states, its answer, its settlement. A
/// tool call is a different kind of fact: it is recorded against a run but produced by the tool path, and
/// the `P3-005` call row is its audit entry. `P3-012c` links the two so a client reading the stream learns
/// that a call happened without having to read a second table — which matters because the tool call is the
/// point at which an effect may have occurred, and a stream that omitted it would show a run that did
/// something with no trace of it.
///
/// # Why the summary and payload are fixed literals
///
/// Nothing here renders tool arguments. `tool_calls` deliberately stores no argument column (`0007`), and a
/// payload is a durable record excluded from that exclusion would have undone it. The identifiers are the
/// run's own, the tool, and the call, so the event is linkable without carrying a payload.
///
/// # Errors
///
/// Returns [`ToolPipelineError::Event`] when a fixed literal is rejected — which is an authoring error
/// rather than a fact about the request, so it is reported instead of the event being skipped.
///
/// # Why the label is derived rather than passed
///
/// An earlier shape took the kind **and** a `&'static str` label for the error message, which is two
/// values that can disagree about one fact — the defect class `P3-006a` found between a receipt's risk and
/// its decision. The label is now `kind.as_str()`, so a rejection names the kind that was actually refused.
async fn append_event(
    database: &SqliteDatabase,
    run_id: &str,
    kind: RunEventKind,
    summary: &str,
    payload: &str,
    correlation_id: CorrelationId,
    at: UtcTimestamp,
) -> Result<(), ToolPipelineError> {
    let reject = |source| ToolPipelineError::Event {
        kind: kind.as_str(),
        source,
    };
    let event = NewRunEvent::new(
        RunId::new().to_string(),
        run_id,
        kind,
        Some(EventSummary::new(summary).map_err(reject)?),
        // Parsed rather than bound as a literal, so a malformed payload is a failure at the call site
        // instead of a row that a reader cannot decode.
        RunEventPayload::new(payload).map_err(reject)?,
        correlation_id,
        at,
    )?;
    append_run_event(database, &event).await?;
    Ok(())
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

/// What admitting a call produced.
///
/// Two variants rather than `Option`, because the two cases carry **different** values: a runnable call
/// has a receipt and an idempotency key, and a held call has neither — it has the approval that was
/// written and the intent that approval binds to. An `Option<PreparedCall>` would force the hold's own
/// facts into a type that describes something else.
enum Admission {
    /// The call may run now.
    Runnable(Box<PreparedCall>),
    /// The call waits on a durable approval.
    Held(Box<PreparedHold>),
}

/// An admitted call whose policy decision requires a human approval.
///
/// # Why this is a distinct type rather than a `PreparedCall` with a flag
///
/// A held call has no `receipt` and cannot produce one: `AuthorizationReceipt::new` refuses a
/// `RequireApproval` decision with no approval cited (`P3-006a`), because a receipt is what an adapter
/// treats as permission and no decision has been taken yet. So the value a hold produces carries
/// **less** than a runnable call — the admitted call, the intent an approval must bind to, and the
/// preview/arguments the owner will review — and giving it the runnable type would mean filling in
/// fields only the runnable case has.
struct PreparedHold {
    /// The durable call identifier, which is what a resumption re-reads.
    call_id: String,
    /// The durable approval that was written for this hold, and which a decision names.
    approval_id: String,
    /// The workspace the run belongs to, taken from the actor rather than from the request.
    workspace_id: String,
    /// The run that asked for the action, which is the approval's **requester**.
    run_id: String,
    /// The tool identifier the approval binds to.
    tool: String,
    /// The tool version the approval binds to, because an authority for one version is not an
    /// authority for a later one.
    tool_version: String,
    /// The intent an approval must bind to, computed over the same tool, version, and arguments the
    /// receipt would have covered, so a decision binds to the action rather than to a description of it.
    intent: CanonicalIntentHash,
    /// The risk the decision was taken at, which is what a stored approval records.
    risk: jarvis_tools::Risk,
    /// A human-readable preview of what is being authorized.
    preview: String,
    /// The arguments the intent was computed over, held on the approval **while it is pending** so the person
    /// deciding can see what they are approving (`ADR-0130`). The intent, not this value, is the binding.
    arguments: serde_json::Value,
    /// The correlation identity shared with the originating request.
    correlation_id: CorrelationId,
    /// When the call was admitted.
    issued_at: UtcTimestamp,
}

/// Records the durable approval a held call is waiting on.
///
/// # Errors
///
/// Returns [`ToolPipelineError::Storage`] when the row cannot be written, and
/// [`ToolPipelineError::Approval`] when the domain refuses a field — both faults rather than decisions.
fn new_approval_request(
    hold: &PreparedHold,
    actor_id: &str,
    id: ApprovalId,
    expires_at: UtcTimestamp,
) -> Result<ApprovalRequest, ToolPipelineError> {
    let workspace_id = WorkspaceId::from_str(&hold.workspace_id).map_err(|_| {
        ToolPipelineError::ApprovalIdentity {
            field: "workspace_id",
        }
    })?;
    let run_id = RunId::from_str(&hold.run_id)
        .map_err(|_| ToolPipelineError::ApprovalIdentity { field: "run_id" })?;

    ApprovalRequest::new(ApprovalRequestParts {
        id,
        workspace_id,
        run_id,
        actor_id: actor_id.to_owned(),
        tool: hold.tool.clone(),
        tool_version: hold.tool_version.clone(),
        intent: hold.intent,
        preview: hold.preview.clone(),
        risk_level: hold.risk.level(),
        correlation_id: hold.correlation_id,
        created_at: hold.issued_at,
        expires_at,
    })
    .map_err(|field| ToolPipelineError::Approval { field })
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
            let filesystem_definitions = FilesystemTool::definitions()?;
            registry.define_all(filesystem_definitions.clone())?;
            sources.push((
                filesystem_definitions,
                Arc::new(FilesystemTool::new(roots)) as Arc<dyn ToolExecutor>,
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
            available: definition.availability().is_available(),
            target: TargetAssessment::none(),
        });
        if decision.is_denied() {
            return Ok(ToolPipelineOutcome::Refused {
                reason_code: decision.reason_code(),
            });
        }

        // The call is about to become a durable row, so the run's stream says so before the row exists.
        // A refusal above writes nothing at all, deliberately: a stream entry for every denied attempt
        // would let a caller fill the log by asking for tools it may not use, which is a write a refusal
        // must not perform. What is emitted here is emitted for a call that is about to be admitted.
        append_event(
            &self.database,
            actor.run_id(),
            RunEventKind::ToolRequested,
            &format!("Requested {tool}"),
            &format!(
                r#"{{"tool":{},"tool_version":{}}}"#,
                serde_json::Value::String(tool.to_owned()),
                serde_json::Value::String(definition.version().to_owned())
            ),
            correlation_id,
            UtcTimestamp::now(&SystemClock),
        )
        .await?;

        // 3-5. Bind the authority, admit the call, and stop if a human must decide first.
        let prepared = self
            .authorize_and_admit(
                tool,
                &definition,
                &arguments,
                actor,
                &decision,
                correlation_id,
            )
            .await?;

        let prepared = match prepared {
            Admission::Runnable(prepared) => prepared,
            // A held decision: the call is admitted, a **durable approval** is written, and the caller
            // learns what an approval would need and which approval to decide.
            Admission::Held(hold) => {
                return Ok(ToolPipelineOutcome::AwaitingApproval {
                    call_id: hold.call_id,
                    approval_id: hold.approval_id,
                    reason_code: decision.reason_code(),
                });
            }
        };

        // 6-7. Execute through the adapter and record what it established.
        self.execute_and_record(*prepared, tool, &definition, correlation_id)
            .await
    }

    /// Runs a **held** call that an approval has since approved, re-checking the decision before it runs.
    ///
    /// # What this closes
    ///
    /// `P3-012a` recorded a durable approval for a held call; `P3-012b` made that approval decidable and
    /// `P3-016` linked the call to it. None of them re-drove the call, so an approved action sat at
    /// `requested` with an authority nothing acted on — an approval that changes a row and nothing else.
    /// This is the step that turns the decision into the effect the human authorized.
    ///
    /// # Why the arguments are supplied by the caller, and why that is not a hole
    ///
    /// `tool_calls` deliberately stores **no arguments** (`0007`): the intent digest is the binding and
    /// storing the payload would put tool arguments into a durable record. So a resume does not read the
    /// arguments back — it takes them from the caller and re-derives the digest over them, which is then
    /// compared against the stored call's `intent_hash`. A caller that supplies different arguments is
    /// **refused**, not run: the digest must match the one the approval was decided against. That makes the
    /// absence of a stored payload safe rather than merely convenient, and the comparison is the control
    /// rather than the argument itself.
    ///
    /// # The checks, and each one is an authority a caller cannot manufacture
    ///
    /// 1. the caller's run owns the call, so a call cannot be resumed by naming its identifier;
    /// 2. the call is linked to an approval, so an unlinked call is never run under one;
    /// 3. the approval **authorizes at this instant** — `authorizes_at`, which is false for a denial, a
    ///    cancellation, a lapsed approval, and an undecided one. A denial therefore cannot be resumed by
    ///    asking again, which is the confused-deputy refusal rather than a missing check;
    /// 4. the approval's tool, version, and intent match the call's, so a decision about one action cannot
    ///    release another;
    /// 5. the call is **not already past authorization**. This is the duplicate-delivery guard: a resumed
    ///    call that has reached `submitted` or a terminal outcome is **refused**, because running it again
    ///    is how one effect becomes two.
    ///
    /// # The duplicate guard is two independent refusals, and the falsification run is what showed it
    ///
    /// Check 5 refuses a resume whose stored outcome is no longer `requested`. Underneath it,
    /// `execute_and_record` advances the call to `authorized` and then `submitted` **before** it reaches the
    /// adapter, and `record_tool_outcome` refuses to replace a terminal outcome — so removing check 5 was
    /// measured to leave the route test **green**, with the adapter still reached exactly once, because the
    /// transition table refuses the re-drive on its own.
    ///
    /// That is worth stating rather than quietly keeping both. Check 5 is not redundant *in general* — it
    /// refuses before building a receipt for a call that has already run, which is the cheaper and clearer
    /// answer — but its removal is **not** observable through the single-effect property, and a test written
    /// as if it were would be claiming a guarantee two mechanisms are actually providing. The assertion that
    /// carries the property is the adapter's own call **count**, which is why the route test asserts it
    /// rather than only the statuses.
    ///
    /// # Errors
    ///
    /// Returns [`ToolPipelineError::ResumeRefused`] for any of the five refusals, naming which one fired, so
    /// a caller learns whether it named a call it does not own, named an unlinked call, resumed a
    /// non-authorized approval, supplied arguments the decision was not about, or asked twice.
    ///
    /// # This does not yet re-evaluate policy, and that is a recorded limit
    ///
    /// The approval was decided against the policy in force when the call was held. A resume does not
    /// re-evaluate, so a policy tightened *after* a decision does not refuse a resume. Whether it should is a
    /// real question — re-evaluating would make an approval lapse silently, not re-evaluating lets a decided
    /// action run under a superseded policy — and it is deliberately **not** decided here. What *is* enforced
    /// is that the approval has not lapsed, which is the time bound `P3-004` already commits to.
    pub async fn resume(
        &self,
        call_id: &str,
        arguments: Value,
        actor: &ToolActor,
        correlation_id: CorrelationId,
    ) -> Result<ToolPipelineOutcome, ToolPipelineError> {
        let stored = self.read_call(call_id).await?;

        // 1. The caller's run must own the call. Checked against the **stored** row, so a caller cannot
        //    resume a call by knowing its identifier.
        if stored.run_id() != actor.run_id() {
            return Err(ToolPipelineError::ResumeRefused {
                call_id: call_id.to_owned(),
                reason: "the call belongs to a different run",
            });
        }

        // 2. A call with no approval link is never run here. `P3-016` writes the link for every hold, so an
        //    unlinked `requested` call is one whose hold predates that write — and resuming it would be
        //    running an action under an authority nothing recorded.
        let Some(approval_id) = stored.approval_id() else {
            return Err(ToolPipelineError::ResumeRefused {
                call_id: call_id.to_owned(),
                reason: "the call is not linked to an approval",
            });
        };
        let approval = find_approval(&self.database, approval_id)
            .await
            .map_err(ToolPipelineError::Storage)?;

        // 3. The time-and-outcome gate, in the domain. `authorizes_at` is false for a denial, a cancel, an
        //    expiry, and an undecided approval, so a denial cannot be resumed and neither can a lapsed one.
        let now = UtcTimestamp::now(&SystemClock);
        if !approval.authorizes_at(now) {
            return Err(ToolPipelineError::ResumeRefused {
                call_id: call_id.to_owned(),
                reason: "the approval does not authorize at this instant",
            });
        }

        // 4. The decision must be about **this** action. The tool and version are compared as stored text,
        //    and the intent is recomputed from the supplied arguments so a caller cannot substitute a
        //    different payload for the one that was decided.
        if approval.tool() != stored.tool() || approval.tool_version() != stored.tool_version() {
            return Err(ToolPipelineError::ResumeRefused {
                call_id: call_id.to_owned(),
                reason: "the approval is for a different tool or version",
            });
        }
        let intent = CanonicalIntentHash::compute(stored.tool(), stored.tool_version(), &arguments)
            .map_err(|_| ToolPipelineError::UnintelligibleIntent {
                tool: stored.tool().to_owned(),
            })?;
        if intent != approval.intent() || intent.to_hex() != stored.intent_hash() {
            return Err(ToolPipelineError::ResumeRefused {
                call_id: call_id.to_owned(),
                reason: "the arguments do not match the intent the approval decided",
            });
        }

        // 5. The duplicate-delivery guard. A call that already passed `authorized` is refused: `requested`
        //    is the only state a resume may act from, which is what stops a retried decision from becoming a
        //    second effect. Checked on the **stored** outcome rather than a version, because the state is
        //    the fact and a version could be bumped by an unrelated write.
        if stored.outcome() != ToolOutcome::Requested {
            return Err(ToolPipelineError::ResumeRefused {
                call_id: call_id.to_owned(),
                reason: "the call has already been authorized or run",
            });
        }

        // The receipt cites the decision. `AuthorizationReceipt::new` accepts a `RequireApproval` decision
        // **only** when an approval is cited, which is why the citation is built from the stored row rather
        // than restated: the approver and the expiry are facts about the decision, so a resume cannot invent
        // them.
        let decision = approval
            .decision()
            .ok_or(ToolPipelineError::ResumeRefused {
                call_id: call_id.to_owned(),
                // Unreachable: `authorizes_at` is false without a decision. Reported rather than unwrapped
                // so a future change to `authorizes_at` cannot turn this into a panic.
                reason: "the approval carries no decision",
            })?;
        let citation = jarvis_tools::ApprovalCitation {
            approval_id: approval.id().to_string(),
            approver_id: decision.approver_id().to_owned(),
            approved_at: decision.decided_at(),
            expires_at: approval.expires_at(),
        };

        let definition = self.definition_for(stored.tool())?;

        // The decision the receipt derives from. It is **reconstructed** from the approval rather than
        // re-evaluated, and the reason is the honest one: policy is a function of the request, and the
        // request was evaluated when the call was held. Re-evaluating here would silently turn a policy
        // edit into a refusal of something an operator already approved — the question this method's own
        // limits section records as deliberately unsettled.
        let decision = jarvis_tools::PolicyDecision::held_by_approval(approval.risk_level());

        let receipt = AuthorizationReceipt::new(AuthorizationReceiptParts {
            receipt_id: stored.id().to_owned(),
            tool: ToolId::new(stored.tool())?,
            tool_version: stored.tool_version().to_owned(),
            arguments: arguments.clone(),
            intent_hash: intent,
            policy_version: stored.policy_version().to_owned(),
            decision,
            // The citation is what makes a `RequireApproval` decision acceptable to the constructor.
            approval: Some(citation),
            correlation_id,
            issued_at: now,
        })?;

        let prepared = PreparedCall {
            call_id: stored.id().to_owned(),
            receipt,
            // The key is **re-read from the stored row** rather than regenerated, so the adapter forwards
            // the same key the original admission used. A fresh key would make a provider deduplicate
            // nothing, which is the one thing the key exists for.
            key: IdempotencyKey::parse(stored.idempotency_key())?,
            arguments,
            issued_at: now,
        };

        self.execute_and_record(prepared, stored.tool(), &definition, correlation_id)
            .await
    }

    /// Returns the registry the pipeline resolves tools against.
    ///
    /// Offered so the executor can offer the model exactly the tools the pipeline can run, from the
    /// same source dispatch was verified against. Deriving the model-facing list any other way would be
    /// a second statement of which tools exist, and the two could disagree in the direction that
    /// matters: a tool offered to a model that the pipeline then refuses as unknown.
    #[must_use]
    pub const fn registry(&self) -> &ToolRegistry {
        &self.registry
    }

    /// Reads one stored call, which is what a resume begins from.
    ///
    /// # Errors
    ///
    /// Returns [`ToolPipelineError::UnknownCall`] when no call has that identifier, so a caller learns it
    /// named a call that does not exist rather than that something failed.
    async fn read_call(&self, call_id: &str) -> Result<StoredToolCall, ToolPipelineError> {
        find_tool_call(&self.database, call_id)
            .await
            .map_err(|error| match error {
                DatabaseError::ToolCallNotFound => ToolPipelineError::UnknownCall {
                    call_id: call_id.to_owned(),
                },
                other => ToolPipelineError::Storage(other),
            })
    }

    /// Resolves a definition by identifier, reporting an unknown tool the way `call_tool` does.
    ///
    /// # Errors
    ///
    /// Returns [`ToolPipelineError::UnknownTool`] when the registry does not hold it. Reachable here for a
    /// call stored by a build that registered a tool this one does not — a profile whose MCP server was
    /// removed between the hold and the decision — which is refused rather than run on a stale definition.
    fn definition_for(
        &self,
        tool: &str,
    ) -> Result<jarvis_tools::ToolDefinition, ToolPipelineError> {
        let id = ToolId::new(tool)?;
        self.registry
            .get(&id)
            .cloned()
            .map_err(|_| ToolPipelineError::UnknownTool {
                tool: tool.to_owned(),
            })
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

    /// Returns every registered tool with the policy that is **in force** for it.
    ///
    /// # Why this is not just `registry.inventory()`
    ///
    /// The registry reports what each tool *declares*. An operator configuring policy needs to see what
    /// the workspace will actually do with it, and those differ in three ways this method resolves:
    /// a per-tool approval override, an outright denial, and the effective approval after both. Reading
    /// the registry directly would show the declaration and leave an operator unable to tell whether
    /// their configuration took effect — the exact question `P3-025` exists to make answerable.
    ///
    /// # Why it lives here rather than in a route
    ///
    /// It needs the workspace policy, and the pipeline is the value that holds it. A route that rebuilt
    /// the policy from configuration would be a second composition of the same grant, and the two could
    /// disagree — so the surface a client reads is the surface `call_tool` decides with.
    ///
    /// # Errors
    ///
    /// Returns [`ToolPipelineError`] when an identifier from the registry cannot be parsed back. That is
    /// unreachable for a key the registry itself produced, so it is reported rather than skipped: a
    /// skipped tool would be a hole in the list an operator is reviewing.
    pub fn policy_inventory(&self) -> Result<Vec<PolicyInventoryEntry>, ToolPipelineError> {
        let mut entries = Vec::new();
        for entry in self.registry.inventory() {
            let id = ToolId::new(&entry.id)?;
            let definition = self.registry.get(&id).cloned()?;
            let declared = definition.approval();
            let effective = self.workspace.effective_approval(&id, declared);
            entries.push(PolicyInventoryEntry {
                id: entry.id,
                title: definition.title().to_owned(),
                version: entry.version,
                source: entry.source,
                risk: entry.risk,
                effects: entry
                    .effects
                    .iter()
                    .map(|effect| effect.as_str().to_owned())
                    .collect(),
                required_scopes: definition
                    .required_scopes()
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
                declared_approval: declared,
                effective_approval: effective,
                // Derived rather than stored, so the flag cannot disagree with the two policies it
                // describes — a stored flag would be a third value to keep in step.
                overridden: effective != declared,
                denied: self.workspace.denies(&id),
                callable: entry.callable,
                unavailable_reason: entry.unavailable_reason,
                asks_first: self.workspace.asks_first(&definition),
            });
        }
        Ok(entries)
    }

    /// Evaluates what a call to a tool **would** decide, without recording or running anything.
    ///
    /// # Why this is a decision and not a prediction
    ///
    /// [`evaluate`] is a pure function of declared facts and the supplied context, so this computes the
    /// answer rather than estimating it. The distinction matters for how the result may be described: a
    /// prediction would be a claim about a future state, while this is the same function `call_tool`
    /// calls, given the same facts. It is exact **for the context supplied** — a caller that omits an
    /// escalation signal gets the decision for a call without that signal.
    ///
    /// # Why this writes nothing
    ///
    /// A preview that recorded a call would let an operator fill the ledger by looking at it, and one
    /// that consumed an idempotency key would make the real call a duplicate. It therefore takes no
    /// `run_id` and touches no table: the whole value is the decision.
    ///
    /// # Errors
    ///
    /// Returns [`ToolPipelineError::UnknownTool`] for an identifier the registry does not hold, so a
    /// caller learns it named a tool that does not exist rather than that a decision was denied.
    pub fn preview_call(
        &self,
        tool: &str,
        actor: &ToolActor,
        target: &TargetAssessment,
    ) -> Result<PolicyDecision, ToolPipelineError> {
        let definition = self.definition_for(tool)?;
        Ok(evaluate(&PolicyRequest {
            definition: &definition,
            actor: actor.authority(),
            workspace: &self.workspace,
            available: definition.availability().is_available(),
            target: target.clone(),
        }))
    }

    /// Returns the workspace policy this pipeline decides with.
    ///
    /// Offered so a caller can render the configured ceiling and threshold beside the per-tool entries
    /// from [`Self::policy_inventory`], reading the same value `call_tool` consults rather than
    /// recomposing it from configuration — the two would otherwise be able to disagree, and the one a
    /// client displayed would be the one nothing enforced.
    #[must_use]
    pub const fn workspace_policy(&self) -> &WorkspacePolicy {
        &self.workspace
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
    /// Returns [`Admission::Held`] for a decision that requires a human, which is not an error: the
    /// call is admitted and recorded as `requested` and a **durable approval request is written**, so a
    /// decision has something to bind to. Running it instead would be the confused-deputy shape
    /// `docs/architecture/security.md` refuses.
    ///
    /// # Why the call is admitted before the receipt is built
    ///
    /// A held call produces no receipt at all. The receipt's own constructor refuses a
    /// `RequireApproval` decision with nothing cited (`P3-006a`), because a receipt is what an adapter
    /// treats as **permission** and no permission exists yet — so building one here would require
    /// inventing a citation for a decision nobody has made, which is the shape that guard exists to
    /// prevent. The call row therefore records the call's own identifier as its receipt binding (the
    /// two are the same value for a call that goes on to run), which is why `CallBinding` takes the
    /// identifier as a parameter rather than reading it off a receipt.
    ///
    /// # Errors
    ///
    /// Returns [`ToolPipelineError`] for a fault: an uncomputable intent, a rejected receipt, a failed
    /// key generation, or a durable write that did not succeed.
    async fn authorize_and_admit(
        &self,
        tool: &str,
        definition: &jarvis_tools::ToolDefinition,
        arguments: &Value,
        actor: &ToolActor,
        decision: &jarvis_tools::PolicyDecision,
        correlation_id: CorrelationId,
    ) -> Result<Admission, ToolPipelineError> {
        // The receipt's risk and intent are DERIVED from the decision and the arguments, so the
        // authority an adapter acts on cannot disagree with the decision (`P3-006a`).
        let intent =
            CanonicalIntentHash::compute(tool, definition.version(), arguments).map_err(|_| {
                ToolPipelineError::UnintelligibleIntent {
                    tool: tool.to_owned(),
                }
            })?;
        let now = UtcTimestamp::now(&SystemClock);
        let call_id = correlation_id.to_string();

        if decision.is_held() {
            let earlier =
                jarvis_storage::read_run_approvals(&self.database, actor.run_id()).await?;
            if let Some(previous) = earlier
                .iter()
                .find(|approval| approval.intent().to_hex() == intent.to_hex())
            {
                return Err(ToolPipelineError::RepeatedRequest {
                    state: previous.stored_state().as_str().to_owned(),
                });
            }
        }

        // The key is generated here rather than derived from the intent: the digest must be
        // deterministic because the receipt binds to it, while the key must be unique per logical
        // call, so a deliberate second identical call is a second call (`P3-005`).
        let key = IdempotencyKey::generate()?;
        let admitted = admit_tool_call(
            &self.database,
            &NewToolCall::new(
                call_id.clone(),
                CallOrigin::new(actor.workspace_id(), actor.run_id(), None),
                CallTarget::new(tool, definition.version()),
                CallBinding::new(
                    intent.to_hex(),
                    key.as_str(),
                    &call_id,
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
            // The hold is assembled here and `record_hold` takes it as one value, rather than the
            // function taking nine arguments. That is the remedy `P3-005` and `P3-008b` both applied
            // for this lint: adjacent opaque arguments a caller could transpose are better grouped into
            // the value that already describes them.
            let hold = PreparedHold {
                call_id,
                approval_id: String::new(),
                workspace_id: actor.workspace_id().to_owned(),
                run_id: actor.run_id().to_owned(),
                tool: tool.to_owned(),
                tool_version: definition.version().to_owned(),
                intent,
                risk: decision.effective_risk(),
                preview: format!("{tool} {}", definition.version()),
                arguments: arguments.clone(),
                correlation_id,
                issued_at: now,
            };
            return self
                .record_hold(hold)
                .await
                .map(|hold| Admission::Held(Box::new(hold)));
        }

        let receipt = AuthorizationReceipt::new(AuthorizationReceiptParts {
            receipt_id: call_id.clone(),
            tool: ToolId::new(tool)?,
            tool_version: definition.version().to_owned(),
            arguments: arguments.clone(),
            intent_hash: intent,
            policy_version: actor.policy_version().to_owned(),
            decision: decision.clone(),
            // No approval: this is the path a decision allowed directly, so there is nothing to cite.
            approval: None,
            correlation_id,
            issued_at: now,
        })?;

        Ok(Admission::Runnable(Box::new(PreparedCall {
            call_id,
            receipt,
            key,
            arguments: arguments.clone(),
            issued_at: now,
        })))
    }

    /// Writes the durable approval a held call is waiting on.
    ///
    /// # The approval's requester is the run, and the approver is not
    ///
    /// `new_approval_request` carries the argument for using the run as the requester. The consequence
    /// worth stating at the call site is that a decision naming the run as its approver is **refused by
    /// the domain**, which is what stops the agent that asked for an action from answering for it.
    ///
    /// # The preview names the tool and not its arguments, deliberately
    ///
    /// The arguments are bound by the intent digest, which is what a decision is checked against, so a
    /// preview is **additional** context rather than the binding. Rendering arbitrary arguments into
    /// it would put unbounded model-authored text into a record `P3-004` bounds at 512 characters and
    /// could fail the hold for a tool whose arguments are simply large. Naming the tool, its version,
    /// and the requesting run is what an operator needs to find the call; a rich preview is the
    /// connector slice's job (`A11`), where the content is a message or an event this layer can
    /// summarize meaningfully.
    ///
    /// # Errors
    ///
    /// Returns [`ToolPipelineError::ApprovalIdentity`] for an identifier that will not parse,
    /// [`ToolPipelineError::Approval`] when the domain refuses a field, and
    /// [`ToolPipelineError::Storage`] when the row cannot be written.
    async fn record_hold(&self, mut hold: PreparedHold) -> Result<PreparedHold, ToolPipelineError> {
        // The requester is the hold's own run, and that is the identity decision this slice turns on:
        // `new_approval_request` carries the argument. Reading it from the hold rather than taking it as
        // a second argument is deliberate — a caller that could pass a *different* identity here would
        // be able to record one requester while the row's run says another.
        let requester = hold.run_id.clone();

        let expiry = jarvis_core::ApprovalRequest::default_expiry(&SystemClock);
        let approval = ApprovalId::new();
        let request = new_approval_request(&hold, &requester, approval, expiry)?;

        // The approval row is written before the call is linked to it, so a crash can leave an approval that
        // nothing resumes automatically but never a call pointing at an approval that does not exist.
        create_approval(&self.database, &request).await?;

        // The link is what makes the hold **findable**: without it the decision route can move the
        // approval to `approved` and nothing can tell which call was waiting, because the two rows share
        // no column. Written after the approval (the identifier does not exist before). It is write-once,
        // so a re-drive cannot repoint it at a different approval.
        link_tool_call_approval(&self.database, &hold.call_id, &request.id().to_string()).await?;

        // The arguments are held on the approval so the person deciding can see them, and so the call can be
        // resumed by a client that never had them (`ADR-0130`). They are not part of the binding — the intent is —
        // and a payload that does not fit is simply not held, which leaves the approval undecidable from a client
        // that must show what it is approving: the fail-closed direction.
        let arguments_text = serde_json::to_string(&hold.arguments).ok();
        let stored_arguments = match arguments_text {
            Some(text) => {
                jarvis_storage::attach_approval_arguments(
                    &self.database,
                    &request.id().to_string(),
                    &text,
                )
                .await?
            }
            None => false,
        };
        if !stored_arguments {
            tracing::warn!(
                approval_id = %request.id(),
                "a held call's arguments are too large to show a person, so the approval cannot be decided from a client that must display them"
            );
        }
        // The stream records that a human is now the thing holding this run up, which is the one fact a
        // client cannot infer from the tool events around it.
        append_event(
            &self.database,
            &hold.run_id,
            RunEventKind::ApprovalRequested,
            &format!("Approval needed for {}", hold.tool),
            &format!(
                r#"{{"approval_id":{},"call_id":{}}}"#,
                serde_json::Value::String(approval.to_string()),
                serde_json::Value::String(hold.call_id.clone())
            ),
            hold.correlation_id,
            UtcTimestamp::now(&SystemClock),
        )
        .await?;

        hold.approval_id = approval.to_string();
        Ok(hold)
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
