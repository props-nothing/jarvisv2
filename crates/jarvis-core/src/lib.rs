//! Provider-neutral domain vocabulary, invariants, and ports for JARVIS.

mod context;
mod credential;
mod endpoint;
mod error;
mod id;
mod loglevel;
mod run;
mod secret;
mod sensitivity;
mod timestamp;
#[cfg(any(unix, windows))]
mod transport;

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
    ClientId, CorrelationId, DaemonRunId, IdGenerator, InvalidId, InvalidIdReason, ProfileId,
    RequestId, SystemIdGenerator,
};
pub use loglevel::LogLevel;
pub use run::{
    ExpectedRunState, InvalidRunErrorCode, InvalidRunState, MAX_RUN_ERROR_CODE_CHARS, RunErrorCode,
    RunOutcome, RunState, RunTransition, RunTransitionError,
};
pub use secret::{SecretRef, SecretRefValidationError};
pub use sensitivity::{InvalidSensitivity, Sensitivity};
pub use timestamp::{Clock, InvalidTimestamp, SystemClock, UtcTimestamp};
#[cfg(any(unix, windows))]
pub use transport::{LocalListener, LocalStream, TransportError, connect};
