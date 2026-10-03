# Domain And API Contracts

These sketches constrain meaning, not exact syntax. Implementation may refine names through an ADR, but provider SDK types must not leak into these contracts.

## Common Context

```rust
pub struct ActorContext {
    pub actor_id: ActorId,
    pub user_id: Option<UserId>,
    pub client_id: ClientId,
    pub workspace_id: WorkspaceId,
    pub authenticated_at: DateTime<Utc>,
    pub auth_strength: AuthStrength,
    pub scopes: ScopeSet,
    pub correlation_id: CorrelationId,
    pub trace_id: Option<TraceId>,
}
```

Every command/query that touches workspace data receives `ActorContext`; it never accepts a free-standing workspace ID as proof of access.

## Local Client Protocol v1 (implemented, P1-008)

The first versioned surface is the local transport used by `jarvis-cli` and the desktop shell. It is framed as a 4-byte big-endian length followed by UTF-8 JSON, capped at 64 KiB per frame.

```rust
pub struct ClientHandshake {
    pub credential: String,      // profile-bound secret, never logged
    pub client_id: ClientId,
    pub client: ClientKind,      // Cli | Desktop | Integration
    pub client_version: String,
    pub protocol_minimum: u16,
    pub protocol_maximum: u16,
    pub capabilities: Vec<String>,
}

pub struct DaemonHandshake {
    pub protocol_version: u16,   // negotiated value
    pub daemon_version: String,
    pub target_os: String,
    pub target_arch: String,
    pub config_schema: u32,
    pub database_schema: i64,
    pub daemon_id: DaemonRunId,
    pub started_at: UtcTimestamp,
}

pub enum HandshakeOutcome {
    Accepted(DaemonHandshake),
    Rejected(WireError),         // never a bare success signal
}

pub struct Request { pub request_id: RequestId, pub command: Command }
pub enum Command { Status, Health }

pub struct Response { pub request_id: RequestId, pub outcome: Outcome }
pub enum Outcome { Ok(Reply), Err(WireError) }
pub enum Reply { Status(StatusReply), Health(HealthReply) }

pub struct WireError {
    pub code: ErrorCode,          // stable, provider-neutral
    pub message: SafeMessage,     // bounded, secret-free
    pub retryable: bool,          // derived from ErrorCode::is_retryable
    pub correlation_id: CorrelationId,
}
```

Negotiation selects the highest version common to both windows. An inverted window, a client newer than the supported maximum, or a client older than the supported minimum each fail closed with a distinct error. Every connection is bound to a profile by its credential; filesystem locality is not authorization.

## Runs

```rust
pub struct StartRun {
    pub session_id: SessionId,
    pub objective: ContentEnvelope,
    pub runtime_policy: RuntimePolicy,
    pub model_requirements: ModelRequirements,
    pub idempotency_key: Option<IdempotencyKey>,
}

pub enum RunState {
    Received,
    ContextBuilding,
    Planning,
    AwaitingApproval,
    Executing,
    Observing,
    Responding,
    Completed,
    Failed,
    Cancelled,
}

pub enum RunEvent {
    StateChanged { from: RunState, to: RunState },
    ActivityUpdated { code: ActivityCode, summary: String },
    OutputDelta { channel: OutputChannel, text: String },
    ToolRequested { intent: ToolIntent },
    ApprovalRequested { approval_id: ApprovalId },
    ArtifactCreated { artifact_id: ArtifactId },
    UsageUpdated { usage: NormalizedUsage },
    Completed { outcome: RunOutcome },
    Failed { error: PublicError },
}
```

Events are persisted/ordered at application boundaries. Transports wrap them with stream protocol metadata.

### Run Stream As Implemented (`P2-007`, `P2-008`)

The sketch above is the eventual shape. What exists is `run_events`: one durable row per run event,
scoped to its run with a monotonic per-run `sequence` and `UNIQUE (run_id, sequence)`, written in the
same transaction as the state transition that produced it (ADR-0011). The wire form of one event as a
client receives it:

```json
{
  "event_id": "...",
  "sequence": 1,
  "kind": "state_changed",
  "summary": "run accepted",
  "payload": { "state": "received", "version": 1 },
  "correlation_id": "...",
  "occurred_at": "..."
}
```

`kind` is a closed set (the twelve values of `jarvis_core::RunEventKind`), and it is the same code in the
durable row, the REST page, and the SSE `event:` name — one naming scheme, so the streaming and
historical views of one event cannot disagree.

On the stream each event is an SSE frame whose `id:` is the **sequence**, not the event UUID, because
the cursor a client must send back is a position in the stream:

```text
id: 1
event: state_changed
data: { ...the object above... }

```

A mid-stream failure is an `error` event carrying the shared `WireError` envelope, so one error shape
spans the stream, the REST routes, and local IPC. Keep-alives are SSE **comment** frames (`:`), never
`heartbeat` events: a heartbeat event would occupy a sequence number and enter the durable log, so a
later replay would replay it as though the run had done something. A client resuming with `Last-Event-ID`
asks for everything after a position it holds; a position beyond what the daemon holds is an explicit
`409` resync rather than an empty success.

## Model Port

```rust
#[async_trait]
pub trait ModelGateway: Send + Sync {
    async fn select(
        &self,
        actor: &ActorContext,
        requirements: &ModelRequirements,
    ) -> Result<ModelSelection, ModelError>;

    async fn stream(
        &self,
        request: ModelRequest,
        cancellation: CancellationToken,
    ) -> Result<ModelStream, ModelError>;
}
```

`ModelRequest` contains normalized content, tool definitions, structured-output schema, context manifest reference, privacy policy, deadline, and correlation IDs. It never contains a provider API key.

## Runtime Port

See [runtime-and-models.md](../architecture/runtime-and-models.md). The transport-facing worker protocol mirrors the domain operations but adds protocol version, sequence, heartbeat, and opaque native binding fields.

External runtime tool use is represented only as `RuntimeEvent::ToolRequested`; the runtime has no `ToolExecutor` credential.

## Tool Contracts

```rust
pub struct ToolDefinition {
    pub id: ToolId,
    pub version: ToolVersion,
    pub description: String,
    pub input_schema: JsonSchema,
    pub output_schema: JsonSchema,
    pub effects: EffectSet,
    pub risk: RiskLevel,
    pub required_scopes: ScopeSet,
    pub approval_policy: ApprovalPolicy,
    pub timeout: Duration,
    pub retry: RetryPolicy,
    pub idempotency: IdempotencySupport,
    pub source: ToolSource,
    pub sensitivity: DataPolicy,
}

pub struct ToolIntent {
    pub tool_id: ToolId,
    pub tool_version: ToolVersion,
    pub arguments: serde_json::Value,
    pub requested_by: RunId,
    pub idempotency_key: Option<IdempotencyKey>,
}

#[async_trait]
pub trait ToolExecutor: Send + Sync {
    async fn execute(
        &self,
        authorization: AuthorizationReceipt,
        intent: ValidatedToolIntent,
        cancellation: CancellationToken,
    ) -> Result<ToolExecutionResult, ToolExecutionError>;
}
```

Only the application tool gateway can create `AuthorizationReceipt` and `ValidatedToolIntent`. Adapter implementations should be unable to bypass construction invariants accidentally.

### Tool Control Plane (`P3-025`, `P3-026` — implemented)

Two routes let a CLI or a control-plane UI read the authorization posture in force and preview a decision
without making a call. They exist because `P3-025` made the policy configurable per tool: a configuration
file is the **input**, while the workspace policy is what the engine enforces, so a document cannot answer
*"did my configuration take effect"*.

```text
GET  /api/v1/tools                          -> ToolListReply
POST /api/v1/tools/{tool}/preview           -> ToolPreviewReply
```

The list reports, per tool, the **declared** approval policy and the **effective** one, plus `overridden`
(whether an override is raising the declaration) and `denied` (whether the workspace refuses it outright).
Both policies are reported because they differ for a reason the operator needs to see: a tool held by the
workspace threshold looks identical to one held by its own declaration unless both are visible. The reply
also carries the workspace's `max_risk` and `approval_threshold`, so a single tool's posture is readable
against the ceiling it sits under.

The preview reports the `decision`, its stable `reason` code, the `effective_risk` **beside** the
`declared_risk`, the `escalated_by` signals, and the `required_strength` an approval would need. It is a
**decision and not a prediction**: the daemon computes it with the same pure `evaluate` the tool-call path
uses, so it is exact for the context supplied. It writes no `tool_calls` row and consumes no idempotency
key, so an operator cannot fill the ledger by looking at it.

**What a caller may supply, and what it may not.** A preview request carries only the `channel`, the
`claimed_strength`, and the `escalation` signals — the properties of the call that only the caller knows.
Scopes, the workspace policy, and the tool definition come from the daemon, and `deny_unknown_fields` makes
an attempt to name them a `422` rather than an ignored value. A preview must not be a way to ask *"what if I
had different permissions"*.

**Three distinctions the statuses keep.** An unknown tool is `404` rather than a refusal, because a typo and
a policy denial have opposite remedies. A daemon with **no tool surface at all** answers `404` rather than an
empty list, because "nothing is configured" and "this daemon cannot serve tools" are different deployment
facts. And a **denied** tool is still reported as `callable: true` — the denial is a policy fact, not an
availability one, and conflating them would send an operator to fix a grant when the remedy is a
configuration line.

## Policy And Approval

```rust
pub enum PolicyVerdict {
    Allow { obligations: Vec<Obligation> },
    RequireApproval { request: ApprovalSpec },
    Deny { reasons: Vec<PolicyReason> },
}

pub struct ApprovalDecision {
    pub approval_id: ApprovalId,
    pub intent_hash: IntentHash,
    pub decision: ApproveOrDeny,
    pub reason: Option<String>,
}
```

**There is no `expected_version` on a decision, and there must not be one.** An earlier revision of this
document carried the field, and `docs/adr/0022-cancellation-carries-no-version.md` wrongly recorded it as
implemented; neither was true. The rule the ADR established for cancellation generalises: **where a durable
identity already names the subject of the decision — here a stable `ApprovalId` whose own transition rule
refuses a second decision — a version expectation adds no safety, and can only add a refusal.** The identity
plus the state machine already exclude a lost update, so a version token would be an additional way to
reject a decision that is perfectly valid, which is exactly the defect `ADR-0022` removed from cancellation.

Approval resolution receives `ActorContext`, checks eligibility/auth strength/expiry/nonce, and re-evaluates policy before issuing an authorization receipt.

## Memory

```rust
#[async_trait]
pub trait MemoryRepository: Send + Sync {
    async fn add_candidate(&self, candidate: MemoryCandidate) -> Result<MemoryId, MemoryError>;
    async fn get(&self, scope: MemoryScope, id: MemoryId) -> Result<Option<Memory>, MemoryError>;
    async fn search(&self, query: MemoryQuery) -> Result<Vec<MemoryHit>, MemoryError>;
    async fn correct(&self, command: CorrectMemory) -> Result<MemoryId, MemoryError>;
    async fn forget(&self, command: ForgetMemory) -> Result<DeletionReceipt, MemoryError>;
}
```

`MemoryQuery` includes workspace, eligible types/sensitivity, temporal/entity constraints, ranking budget, and caller purpose. Repositories enforce workspace eligibility; they do not accept a globally unscoped search.

## Memory Control Plane (`P4-008` — implemented; admission by `P4-014`)

The `FR-MEM-005` lifecycle over the canonical store: list, inspect, search, remember, correct, **confirm**,
forget, and export.

| Route | Verb | What it answers |
| --- | --- | --- |
| `/api/v1/memories` | `GET` | the workspace's claims as **references** — no claim text |
| `/api/v1/memories` | `POST` | records a claim the caller states, through the candidate pipeline |
| `/api/v1/memories/search` | `POST` | ranked matches, with each signal's contribution |
| `/api/v1/memories/export` | `GET` | every claim **with** text, including archived and deleted |
| `/api/v1/memories/{id}` | `GET` | one claim, with its text, claim triple, and admission |
| `/api/v1/memories/{id}/correct` | `POST` | replaces the text, linking `supersedes` |
| `/api/v1/memories/{id}/confirm` | `POST` | **accepts a proposal**, recording who accepted it |
| `/api/v1/memories/{id}/forget` | `POST` | deletes it, returning a `DeletionReceipt` |

### Session summaries

`P4-015`. The route is under `sessions`, because the **session** is what is being summarized and its identifier
is what the path names; that the result is stored as a memory is the daemon's concern.

| Route | Verb | What it answers |
| --- | --- | --- |
| `/api/v1/sessions/{id}/summaries` | `POST` | records a compressed summary of a span of turns |
| `/api/v1/sessions/{id}/summaries` | `GET` | the session's **current** summaries, largest span first |
| `/api/v1/sessions/{id}/summaries/retire` | `POST` | applies the retention rule, archiving them |

**A summary's trust is not a request field.** `SummarizeSessionRequest` has no `confidence`, `status`, or
`trust`, and there is no `MemorySourceKind` in it either: a summary is always `Conversation` + `Document`, hence
`Derived`. A caller able to send `authoritative` could present a model's compression as the user's own words,
which is the one outcome the slice names as forbidden.

**The span is required and is checked against the transcript.** `first_sequence`, `last_sequence`, and
`turns_covered` are all in the body; the last is validated **against** the span's width rather than derived from
it, so a producer that read one range and reported another is refused instead of storing a compression ratio
computed from the wrong denominator. `source_chars` is optional, and absent means "not measured" — no ratio is
reported from a zero, because a fabricated compression figure is worse than an absent one.

**A span is refused `422` when it names turns the session does not have, when its turn count disagrees with its
width, or when it overlaps a stored span** — and the overlap refusal **names the stored range**, because a
producer summarizing a long session in pieces has to know which turns are done. `422` rather than `503`: all
three are requests a caller can fix, and a retryable status would send it to retry a request that can never
succeed.

**The complement is on the reply.** `unsummarized` lists the ranges no summary covers, oldest first, because
that is what a producer needs in order to continue — and deriving it from the stored spans is the subtraction
that omits the range between two adjacent covered spans. The same list appears once on the list reply rather
than per summary, since it is a fact about the session.

**The two write verbs that change trust require `expected_version`**, as the skill verbs do, and every read
returns it — a guard value a client cannot obtain is one it cannot send.

**`confirm` carries the version and nothing else.** It is the one place a decision is recorded, and the
approver is **not** a field: the daemon reads its own seeded identity, so a caller cannot attribute a decision
to somebody else. `admitted_by_actor_id` and `admitted_at` appear on the reference and travel together, because
the domain refuses half a decision — a row carrying one without the other is a stored-row error rather than an
answer.

**Only a proposal may be confirmed.** A claim is `proposed` when the domain's own derivation says it must be:
a `relationship` claim (the document's "high-impact" class), and any `model_inference` at any confidence. `422`
for a claim that is not a proposal; `409` for a stale version, because the remedy is a re-read and it outranks
the state refusal — a caller has to re-read before it can act at all.

**There is deliberately no self-admission refusal.** `ADR-0117` §4 refuses a promotion by a procedure's own
author, and the symmetry does not transfer: the document requires "explicit user confirmation" of a high-impact
inference, so the person confirming **is** the person whose statement produced the candidate. With one seeded
identity the two are always the same value. What holds instead is that the approver is never client-supplied —
see `ADR-0124`.

### Entities

`P4-016`. An entity is what a memory is **about**, and this is the surface that makes a remember possible at
all: every claim must name a subject, and until this slice existed there was no way to obtain one. See
`ADR-0126`.

| Route | Verb | What it answers |
| --- | --- | --- |
| `/api/v1/entities` | `GET` | the workspace's **usable** entities — merged and archived excluded |
| `/api/v1/entities` | `POST` | creates one, returning the identifier a remember names |
| `/api/v1/entities/lookup` | `GET` | every entity a **label** or an **alias** denotes, with the evidence |
| `/api/v1/entities/{id}` | `GET` | one entity with its aliases, and how many claims link to it |
| `/api/v1/entities/{id}/aliases` | `POST` | attaches a name to it |
| `/api/v1/entities/{id}/merge` | `POST` | merges it **into** the identifier in the body |

**A lookup returns candidates, never one row.** The architecture's rule is that ambiguous aliases "remain
separate candidates", so a reply whose `matches` held one entry would turn a guess into an identity and every
later claim would inherit it. Each match carries the aliases that produced it and a `verified` flag, and the flag
is the **conjunction** over those aliases: an entity holding a verified name *and* a probabilistic one for the
same value is not verified.

**A lookup with no selector is `400`, not an empty workspace.** `?label=` and `?alias_kind=`/`?alias_value=` are
the two selectors; neither means `GET /entities`, so the absent case is refused rather than defaulted. Half an
alias is refused for the same reason — an empty kind searches every kind and an empty value matches nothing.

**`verification` and `source_kind` are both request fields, and the schema's cross-field rule is restated as a
`422`.** A `confirmed` alias must come from `user_statement` or `user_correction`; a `probabilistic` one cannot
be held as `confirmed`. The refusal names the rule, because the constraint it mirrors names a table.

**The merge's direction is in words.** The path names the entity merged **away** and the body names the one kept,
and the reply is the winner — its `merged_into`-pointing loser is reachable by `GET /entities/{id}`, so a
reversal is visible in the receipt rather than only in the database. Each reply carries `linked_memories`,
because a merge moves no links and an operator needs to know which side is used.

**No deletion.** `merge` is the only operation that retires an entity, and an entity created in error stays
listed until it is merged into another. Recorded as a limit in `ADR-0126` rather than left as an omission.


## Skill Control Plane (`P4-013` — implemented)

The `FR-MEM-005` lifecycle applied to a **stored procedure**: list, inspect, create, promote, disable, enable,
forget, and export. `ADR-0117` fixes the trust boundary, and three of its rules shape the contract.

| Route | Verb | What it answers |
| --- | --- | --- |
| `/api/v1/skills` | `GET` | the workspace's revisions as **references** — no procedure text |
| `/api/v1/skills` | `POST` | records a new revision, or a declared correction |
| `/api/v1/skills/export` | `GET` | every revision **with** text, plus what the export excludes |
| `/api/v1/skills/{id}` | `GET` | one revision, with its prose and steps |
| `/api/v1/skills/{id}` | `DELETE` | removes it, returning a `SkillDeletionReceipt` |
| `/api/v1/skills/{id}/promote` | `POST` | the approval that makes a proposal usable |
| `/api/v1/skills/{id}/disable` | `POST` | archives it: retained for inspection, not offered |
| `/api/v1/skills/{id}/enable` | `POST` | returns it to the state its promotion record implies |

**No request carries a workspace, a granted tool, a scope, or a pre-approval.** `deny_unknown_fields` makes an
attempt a `422` rather than an ignored value, and there is no field for authority to arrive in — so creating a
skill cannot be privilege escalation.

**Every control verb requires `expected_version`**, the counter the caller observed. A listing and a detail
read both return it (`version_counter`), because a guard value a client cannot obtain is one it cannot send.
The counter is the platform's optimistic-locking value and is **not** the author's `author_version` string,
which is content and is not unique across workspaces.

**`promote` is the one verb that names an actor** (`approver_actor_id`), because `ADR-0117` §4 makes promotion
an approval and `ADR-0043` requires it to be attributable. The daemon refuses an approver equal to the
revision's author; the author comes from the authenticated session and cannot be supplied, which is what makes
that refusal meaningful.

**A creation records a `user_statement` source and an `active` state** — a person stating how something is
done needs no approval. A model-authored revision reaches the store as `proposed` and needs a promotion. A
client cannot choose which path it is on, so the promotion rule is not a flag.

**Status codes:** `201` for a creation and `200` for a correction (a correction is not a creation); `409` for a
stale counter, because the remedy is a re-read; `422` for an unknown tool, a malformed step, a mismatch between
a correction and the skill it names, a self-approval, or a refused transition; `404` for an absent revision —
including one in another workspace, because "not yours" would confirm that something exists.

## Events

```rust
pub struct EventEnvelope<T> {
    pub event_id: EventId,
    pub event_type: EventType,
    pub schema_version: SchemaVersion,
    pub workspace_id: WorkspaceId,
    pub source: EventSource,
    pub occurred_at: DateTime<Utc>,
    pub received_at: DateTime<Utc>,
    pub causation_id: Option<EventId>,
    pub correlation_id: CorrelationId,
    pub dedupe_key: DedupeKey,
    pub sensitivity: Sensitivity,
    pub payload: T,
}
```

Provider webhooks convert to an envelope only after verification. Internal producers write through a unit of work so outbox insertion is atomic with their state mutation.

## Workflows

```rust
#[async_trait]
pub trait WorkflowEngine: Send + Sync {
    async fn start(&self, command: StartWorkflow) -> Result<WorkflowRunId, WorkflowError>;
    async fn signal(&self, signal: WorkflowSignal) -> Result<(), WorkflowError>;
    async fn cancel(&self, command: CancelWorkflow) -> Result<(), WorkflowError>;
    async fn status(&self, id: WorkflowRunId) -> Result<WorkflowSnapshot, WorkflowError>;
}
```

Workflow step handlers receive a leased attempt/fencing token and return a normalized result or wait instruction. They do not mutate workflow state directly.

## Connectors

```rust
#[async_trait]
pub trait Connector: Send + Sync {
    fn manifest(&self) -> &ConnectorManifest;
    async fn begin_auth(&self, request: BeginAuth) -> Result<AuthChallenge, ConnectorError>;
    async fn complete_auth(&self, callback: AuthCallback) -> Result<VerifiedAccount, ConnectorError>;
    async fn health(&self, account: ConnectorAccountId) -> ConnectorHealth;
    async fn revoke(&self, account: ConnectorAccountId) -> Result<(), ConnectorError>;
    fn tools(&self, account: ConnectorAccountId) -> Vec<ToolDefinition>;
}
```

Provider operations can be internal adapter traits or canonical tool executors. Token refresh is transparent but observable; reauthorization is a user-facing state.

## Voice

```rust
#[async_trait]
pub trait VoiceProvider: Send + Sync {
    async fn start_outbound_call(&self, request: OutboundCallRequest)
        -> Result<ProviderCall, VoiceError>;
    async fn get_call(&self, id: ProviderCallId) -> Result<ProviderCallState, VoiceError>;
    async fn end_call(&self, id: ProviderCallId) -> Result<(), VoiceError>;
    async fn verify_webhook(&self, request: RawWebhookRequest)
        -> Result<VerifiedVoiceEvent, VoiceError>;
}
```

The application creates/authorizes a JARVIS call before invoking the provider. Provider IDs never replace `VoiceCallId`.

## Turn Detection And Interruption

Turn detection is a separate port from `VoiceProvider`, because it is a distinct capability that a speech provider may or may not expose, and because its output is what defines the canonical transcript boundary ([ADR-0010](../adr/0010-model-based-turn-detection.md)).

```rust
pub enum TurnEvent {
    EndOfTurn { confidence: Option<TurnConfidence> },
    EagerEndOfTurn { confidence: Option<TurnConfidence> },
    TurnResumed,
    FalseInterruption,
    Backchannel,
}

#[async_trait]
pub trait TurnDetector: Send + Sync {
    /// Reports which events this provider can express. An absent event is
    /// reported as unsupported, never substituted with a similar-looking one.
    fn capabilities(&self) -> TurnCapabilities;
    /// Returns the next canonical turn event for one session.
    async fn next_turn_event(&mut self) -> Result<TurnEvent, VoiceError>;
}
```

Rules:

- The adapter maps its own vocabulary onto these five events. A provider that cannot distinguish a false interruption reports that capability as unsupported rather than emitting `EndOfTurn` or a generic interruption, because a noise event reported as a real interruption silently cancels valid speech.
- Confidence is carried as evidence and is **not** comparable across providers.
- The configured silence timeout is a maximum-silence cap that forces a boundary, not the turn mechanism, and is recorded on the session.
- Turn detection, eager-turn speculation, and interruption resume are JARVIS-owned session events. A provider may advise a boundary; JARVIS records it.
- Eager-turn speculation produces drafts only. Every speculative result that is discarded is recorded, because it costs model calls.

## Repositories And Unit Of Work

Repository ports are aggregate/use-case focused, not a generic CRUD abstraction. A unit of work exposes the repositories required for one transaction and commits domain events to the outbox atomically.

Avoid leaking SQL transactions into handlers or retaining them across awaits that call external systems.

## Public Error Contract

```text
code              stable machine-readable identifier
message           safe human-readable summary
retryable         whether the same request may be retried
retry_after       optional duration/timestamp
correlation_id
field_violations  optional safe validation details
```

Provider body, SQL text, stack trace, secret-bearing URL/header, and internal file path are excluded. The internal diagnostic error retains a redacted cause chain linked by correlation ID.

## Contract Test Rule

Every adapter has a shared contract suite for the port it implements. Unit tests may mock the port; adapter contract tests prove normalization; live tests prove external ownership. A passing fake is never evidence that a provider contract works.