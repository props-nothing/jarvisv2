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
    pub expected_version: u64,
    pub decision: ApproveOrDeny,
    pub reason: Option<String>,
}
```

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