//! Provider-neutral domain vocabulary, invariants, and ports for JARVIS.

mod context;
mod credential;
mod endpoint;
mod error;
mod id;
mod loglevel;
mod loopback;
mod message;
mod run;
mod run_event;
mod secret;
mod sensitivity;
mod session;
mod testkit;
mod timestamp;
#[cfg(any(unix, windows))]
mod transport;

mod approval;
mod memory;
mod secretbytes;
mod tool_outcome;

pub use approval::{
    ApprovalChannel, ApprovalDecision, ApprovalDecisionOutcome, ApprovalRequest,
    ApprovalRequestParts, ApprovalState, AuthenticationStrength, CanonicalIntentHash,
    DecisionNonce, IntentError, InvalidApprovalField, MAX_APPROVAL_LIFETIME_SECONDS,
    MAX_APPROVAL_PREVIEW_CHARS, MAX_CANONICAL_INTENT_CHARS, NonceError,
};
pub use context::{
    ContextBudget, ContextError, ContextItem, ContextManifest, ContextPriority, ContextSource,
    ContextSourceKind, ContextTrust, ExcludedContext, ExclusionReason, InclusionReason,
    MAX_RETAINED_SOURCES, MAX_SOURCE_REFERENCE_CHARS, assemble_context,
};
pub use credential::{CREDENTIAL_BYTES, CREDENTIAL_CHARS, ClientCredential, CredentialError};
pub use endpoint::{
    DAEMON_LOCK_FILE_NAME, EndpointError, LocalEndpoint, MAX_PROFILE_NAME_BYTES,
    UNIX_SOCKET_FILE_NAME, WINDOWS_PIPE_PREFIX, is_valid_profile,
};
pub use error::{DomainError, ErrorCode, SafeMessage, UnsafeMessage, UnsafeMessageReason};
pub use id::{
    ApprovalId, ClientId, CorrelationId, DaemonRunId, EntityId, IdGenerator, InvalidId,
    InvalidIdReason, MemoryId, ProfileId, RequestId, RunId, SessionId, SystemIdGenerator,
    WorkspaceId,
};
pub use loglevel::LogLevel;
pub use loopback::{InvalidLoopbackHost, LOOPBACK_ADDRESS, LoopbackHost};
pub use memory::{
    EffectiveMemoryStatus, EntityMatch, EntityRef, InvalidMemory, MAX_CLAIM_PART_CHARS,
    MAX_MEMORY_CONTENT_CHARS, MAX_MEMORY_ENTITIES, MAX_MEMORY_IMPORTANCE, MAX_SEARCH_KEY_CHARS,
    MAX_SEARCH_KEY_WORD_CHARS, MAX_SEARCH_KEY_WORDS, MAX_SOURCE_LOCATOR_CHARS,
    MAX_STRUCTURED_CLAIM_BYTES, MemoryConfidence, MemoryRecord, MemoryRecordParts, MemorySearchKey,
    MemorySource, MemorySourceKind, MemoryStatus, MemoryTrust, MemoryType, StructuredClaim,
};
pub use message::{
    InvalidMessage, MAX_MESSAGE_CONTENT_BYTES, MessageRole, MessageSource, NewMessage,
};
pub use run::{
    ExpectedRunState, InvalidRunErrorCode, InvalidRunState, MAX_RUN_ERROR_CODE_CHARS, RunErrorCode,
    RunOutcome, RunState, RunTransition, RunTransitionError,
};
pub use run_event::{
    EventSummary, InvalidRunEvent, MAX_EVENT_PAYLOAD_BYTES, MAX_EVENT_SUMMARY_CHARS,
    MAX_REPLAY_EVENTS, ReplayRequest, RunEventKind, RunEventPayload, RunEventSequence,
};
pub use secret::{SecretRef, SecretRefValidationError};
pub use sensitivity::{InvalidSensitivity, Sensitivity};
pub use session::{InvalidSessionField, MAX_SESSION_TITLE_CHARS, SessionChannel, SessionStatus};
pub use testkit::{
    MAX_SCRATCH_REMOVAL_ATTEMPTS, SCRATCH_REMOVAL_INTERVAL, remove_scratch_dir, scratch_tag,
};
pub use timestamp::{Clock, InvalidTimestamp, SystemClock, UtcTimestamp};
pub use tool_outcome::{
    InvalidToolOutcome, MAX_OUTCOME_DETAIL_CHARS, ToolOutcome, ToolOutcomeError, ToolOutcomeRecord,
};
#[cfg(any(unix, windows))]
pub use transport::{LocalListener, LocalStream, TransportError, connect};
