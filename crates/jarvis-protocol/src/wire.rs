use jarvis_core::{
    ClientId, CorrelationId, DaemonRunId, ErrorCode, RequestId, SafeMessage, UtcTimestamp,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Bounded maximum number of capability tokens a client may claim.
pub const MAX_CLIENT_CAPABILITIES: usize = 64;
/// Bounded maximum length of a single capability token or version label.
pub const MAX_TOKEN_BYTES: usize = 64;

/// Client kinds recognized by protocol v1.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientKind {
    /// The `jarvis` command-line client.
    Cli,
    /// The desktop application.
    Desktop,
    /// A first-party integration or test harness.
    Integration,
}

/// Explains why a handshake was rejected before any command ran.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum HandshakeError {
    /// The client build label was empty, oversized, or unsafe.
    #[error("the client version label is invalid")]
    InvalidVersion,
    /// The client claimed too many or unsafe capabilities.
    #[error("the client capability list is invalid")]
    InvalidCapabilities,
}

/// First client message on a local control connection.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ClientHandshake {
    /// Profile-bound local client credential presented for authentication.
    pub credential: String,
    /// Stable client identity for audit and least privilege.
    pub client_id: ClientId,
    /// Client kind.
    pub client: ClientKind,
    /// Client build version.
    pub client_version: String,
    /// Oldest protocol version the client can speak.
    pub protocol_minimum: u16,
    /// Newest protocol version the client can speak.
    pub protocol_maximum: u16,
    /// Capability tokens the client claims.
    pub capabilities: Vec<String>,
}

impl ClientHandshake {
    /// Validates bounded, non-secret handshake metadata.
    ///
    /// # Errors
    ///
    /// Returns [`HandshakeError`] for an unsafe version label or capability list.
    pub fn validate(&self) -> Result<(), HandshakeError> {
        if !is_safe_token(&self.client_version) {
            return Err(HandshakeError::InvalidVersion);
        }
        if self.capabilities.len() > MAX_CLIENT_CAPABILITIES
            || !self.capabilities.iter().all(|token| is_safe_token(token))
        {
            return Err(HandshakeError::InvalidCapabilities);
        }
        Ok(())
    }
}

/// Daemon's accepted handshake response, sent before any command is served.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DaemonHandshake {
    /// Negotiated protocol version.
    pub protocol_version: u16,
    /// Daemon build version.
    pub daemon_version: String,
    /// Target operating system of the daemon build.
    pub target_os: String,
    /// Target architecture of the daemon build.
    pub target_arch: String,
    /// Configuration schema understood by the daemon.
    pub config_schema: u32,
    /// Database schema owned by the daemon.
    pub database_schema: i64,
    /// Lifecycle ID of the running daemon process.
    pub daemon_id: DaemonRunId,
    /// When this daemon lifecycle started.
    pub started_at: UtcTimestamp,
}

/// The daemon's first message: an accepted negotiation or a bounded rejection.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ServerHandshake {
    /// Whether the daemon admitted the client.
    pub outcome: HandshakeOutcome,
}

/// Result of the daemon's half of the local handshake.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HandshakeOutcome {
    /// The client was authenticated and a protocol version was negotiated.
    Accepted(DaemonHandshake),
    /// The client was refused; no command will be served on this connection.
    Rejected(WireError),
}

/// One versioned local protocol request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Request {
    /// Client-generated request identity used to correlate the response.
    pub request_id: RequestId,
    /// Requested command.
    pub command: Command,
}

/// Commands available in protocol v1.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Command {
    /// Report daemon lifecycle and build status.
    Status,
    /// Report liveness and readiness only.
    Health,
}

/// Terminal reply for one request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Response {
    /// Echoes the request identity.
    pub request_id: RequestId,
    /// Outcome of the command.
    pub outcome: Outcome,
}

/// Either a successful reply or a stable bounded error.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// The command succeeded.
    Ok(Reply),
    /// The command failed with a stable, safe error envelope.
    Err(WireError),
}

/// Successful protocol v1 replies.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reply {
    /// Daemon status snapshot.
    Status(StatusReply),
    /// Daemon liveness/readiness snapshot.
    Health(HealthReply),
}

/// Daemon lifecycle and build status without secrets.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StatusReply {
    /// Current lifecycle phase name.
    pub phase: String,
    /// Whether the process is live.
    pub live: bool,
    /// Whether the daemon is ready to serve clients.
    pub ready: bool,
    /// Daemon build version.
    pub daemon_version: String,
    /// Negotiated protocol version.
    pub protocol_version: u16,
    /// Configuration schema version.
    pub config_schema: u32,
    /// Database schema version.
    pub database_schema: i64,
    /// Lifecycle ID of this daemon process.
    pub daemon_id: DaemonRunId,
    /// When this lifecycle started.
    pub started_at: UtcTimestamp,
}

/// Minimal liveness and readiness snapshot.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HealthReply {
    /// Whether the process is live.
    pub live: bool,
    /// Whether the daemon is ready to serve clients.
    pub ready: bool,
    /// Current lifecycle phase name.
    pub phase: String,
}

/// Stable client-visible error envelope.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WireError {
    /// Stable provider-neutral error category.
    pub code: ErrorCode,
    /// Bounded, secret-free diagnostic message.
    pub message: SafeMessage,
    /// Whether a bounded retry is permitted by default.
    pub retryable: bool,
    /// Correlation identity assigned by the daemon.
    pub correlation_id: CorrelationId,
}

impl WireError {
    /// Builds a wire error that derives retryability from its category.
    #[must_use]
    pub const fn new(code: ErrorCode, message: SafeMessage, correlation_id: CorrelationId) -> Self {
        Self {
            code,
            message,
            retryable: code.is_retryable(),
            correlation_id,
        }
    }
}

impl std::fmt::Display for WireError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let retry = if self.retryable { " retryable" } else { "" };
        write!(
            formatter,
            "{}: {}{retry} correlation={}",
            self.code, self.message, self.correlation_id
        )
    }
}

/// Reports whether a token is non-empty, bounded, and free of control characters.
#[must_use]
pub fn is_safe_token(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= MAX_TOKEN_BYTES
        && !token.chars().any(char::is_control)
        && token.bytes().all(|byte| byte.is_ascii_graphic())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PROTOCOL_VERSION;
    use jarvis_core::{SystemClock, SystemIdGenerator};

    fn handshake(capabilities: Vec<String>) -> ClientHandshake {
        ClientHandshake {
            credential: "not-a-real-secret".to_owned(),
            client_id: ClientId::generate(&SystemIdGenerator).unwrap_or_default(),
            client: ClientKind::Cli,
            client_version: "0.1.0".to_owned(),
            protocol_minimum: 1,
            protocol_maximum: 1,
            capabilities,
        }
    }

    #[test]
    fn handshake_validation_rejects_unsafe_metadata() {
        assert!(handshake(Vec::new()).validate().is_ok());

        let forged = ClientHandshake {
            client_version: "0.1.0\nforged".to_owned(),
            ..handshake(Vec::new())
        };
        assert_eq!(forged.validate(), Err(HandshakeError::InvalidVersion));

        let oversized = handshake(vec!["x".repeat(MAX_TOKEN_BYTES + 1)]);
        assert_eq!(
            oversized.validate(),
            Err(HandshakeError::InvalidCapabilities)
        );
    }

    #[test]
    fn wire_types_serialize_without_exposing_provider_types() {
        let response = Response {
            request_id: RequestId::generate(&SystemIdGenerator).unwrap_or_default(),
            outcome: Outcome::Ok(Reply::Health(HealthReply {
                live: true,
                ready: false,
                phase: "booting".to_owned(),
            })),
        };
        let encoded = serde_json::to_string(&response).unwrap_or_default();
        assert!(encoded.contains("\"health\""));
        assert!(encoded.contains("\"booting\""));

        let error = WireError::new(
            ErrorCode::Authentication,
            SafeMessage::new("local client credential was rejected")
                .unwrap_or_else(|_| panic!("valid safe message")),
            CorrelationId::generate(&SystemIdGenerator).unwrap_or_default(),
        );
        assert!(!error.retryable);
        assert_eq!(error.code, ErrorCode::Authentication);

        let status = StatusReply {
            phase: "ready".to_owned(),
            live: true,
            ready: true,
            daemon_version: "0.1.0".to_owned(),
            protocol_version: PROTOCOL_VERSION,
            config_schema: 1,
            database_schema: test_database_schema(),
            daemon_id: DaemonRunId::generate(&SystemIdGenerator).unwrap_or_default(),
            started_at: UtcTimestamp::now(&SystemClock),
        };
        let encoded = serde_json::to_string(&status).unwrap_or_default();
        let decoded: Result<StatusReply, _> = serde_json::from_str(&encoded);
        assert_eq!(decoded.ok(), Some(status));
    }

    /// The schema version a wire fixture should report.
    ///
    /// This crate cannot read `jarvis_storage::CURRENT_SCHEMA_VERSION` without inverting
    /// the dependency direction (`jarvis-storage` already depends on `jarvis-protocol`), so
    /// the value is deliberately named rather than inlined. A literal here previously stayed
    /// at 2 after the storage schema moved to 3, which made a fixture describe a database
    /// version that no longer existed and no test noticed.
    fn test_database_schema() -> i64 {
        3
    }
}
