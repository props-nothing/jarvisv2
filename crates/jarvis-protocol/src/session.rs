use std::time::Duration;

use jarvis_core::{
    ClientCredential, ClientId, CorrelationId, ErrorCode, RequestId, SafeMessage, SystemIdGenerator,
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::time::timeout;

use crate::{
    ClientHandshake, ClientKind, Command, DaemonHandshake, FrameError, HandshakeError,
    HandshakeOutcome, MAX_SUPPORTED_PROTOCOL, MIN_SUPPORTED_PROTOCOL, NegotiationError,
    PROTOCOL_VERSION, Reply, Request, Response, ServerHandshake, WireError, read_frame,
    write_frame,
};

/// Deadline for completing the client handshake before the connection is closed.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Maximum number of requests served on one client connection.
pub const MAX_REQUESTS_PER_CONNECTION: u32 = 256;

/// Explains why a local protocol session ended early.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// The peer closed the connection before completing the handshake.
    #[error("the peer closed the connection during the handshake")]
    ClosedBeforeHandshake,
    /// The peer closed the connection while a response was outstanding.
    #[error("the peer closed the connection before replying")]
    ClosedBeforeReply,
    /// The handshake did not complete inside the deadline.
    #[error("the client handshake exceeded its deadline")]
    HandshakeTimeout,
    /// The client handshake metadata was invalid.
    #[error(transparent)]
    Handshake(#[from] HandshakeError),
    /// The client and daemon had no common protocol version.
    #[error(transparent)]
    Negotiation(#[from] NegotiationError),
    /// A frame could not be encoded, decoded, or transferred.
    #[error(transparent)]
    Frame(#[from] FrameError),
    /// The daemon rejected the handshake with a bounded reason.
    #[error("the daemon rejected the handshake: {0}")]
    Rejected(WireError),
    /// The daemon returned a stable error for the request.
    #[error("the daemon returned an error: {0}")]
    Request(WireError),
    /// The reply did not echo the request identity that was sent.
    #[error("the daemon reply did not echo the request identity")]
    RequestMismatch,
    /// The peer exceeded the per-connection request budget.
    #[error("the client exceeded the per-connection request budget")]
    RequestBudgetExceeded,
}

/// Answers one admitted client's commands.
///
/// Implementations read daemon state and must not perform durable effects.
pub trait Responder: Send + Sync {
    /// Produces the reply for a negotiated command.
    fn respond(&self, command: Command) -> Reply;
}

/// State the daemon needs to admit and serve local clients.
pub struct ServerContext {
    credential: ClientCredential,
    daemon: DaemonHandshake,
    responder: std::sync::Arc<dyn Responder>,
}

impl ServerContext {
    /// Builds a server session context.
    ///
    /// The accepted handshake reports the daemon's newest supported protocol
    /// version; the negotiated value is written per connection.
    #[must_use]
    pub fn new(
        credential: ClientCredential,
        daemon: DaemonHandshake,
        responder: std::sync::Arc<dyn Responder>,
    ) -> Self {
        Self {
            credential,
            daemon,
            responder,
        }
    }
}

/// Metadata about a client that completed the handshake.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdmittedClient {
    /// Stable client identity.
    pub client_id: ClientId,
    /// Client kind.
    pub client: ClientKind,
    /// Negotiated protocol version.
    pub protocol_version: u16,
    /// Capability tokens the client claimed.
    pub capabilities: Vec<String>,
}

/// Serves the handshake and request loop for one local connection.
///
/// # Errors
///
/// Returns [`SessionError`] when the handshake fails, the peer violates the
/// protocol, or the transport fails.
pub async fn serve<S>(
    stream: &mut S,
    context: &ServerContext,
) -> Result<AdmittedClient, SessionError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let presented = timeout(HANDSHAKE_TIMEOUT, read_frame::<_, ClientHandshake>(stream))
        .await
        .map_err(|_| SessionError::HandshakeTimeout)??;
    let presented = presented.ok_or(SessionError::ClosedBeforeHandshake)?;

    if let Err(error) = presented.validate() {
        return reject(stream, ErrorCode::Validation, error.to_string()).await;
    }
    if !context.credential.matches(&presented.credential) {
        return reject(
            stream,
            ErrorCode::Authentication,
            "local client credential was rejected".to_owned(),
        )
        .await;
    }
    let negotiated = match crate::negotiate(presented.protocol_minimum, presented.protocol_maximum)
    {
        Ok(version) => version,
        Err(error) => return reject(stream, ErrorCode::Conflict, error.to_string()).await,
    };

    let mut accepted = context.daemon.clone();
    accepted.protocol_version = negotiated;
    write_frame(
        stream,
        &ServerHandshake {
            outcome: HandshakeOutcome::Accepted(accepted),
        },
    )
    .await?;

    let admitted = AdmittedClient {
        client_id: presented.client_id,
        client: presented.client,
        protocol_version: negotiated,
        capabilities: presented.capabilities,
    };

    for _ in 0..MAX_REQUESTS_PER_CONNECTION {
        let Some(request) = read_frame::<_, Request>(stream).await? else {
            return Ok(admitted);
        };
        let reply = context.responder.respond(request.command);
        write_frame(
            stream,
            &Response {
                request_id: request.request_id,
                outcome: crate::Outcome::Ok(reply),
            },
        )
        .await?;
    }
    Err(SessionError::RequestBudgetExceeded)
}

async fn reject<S>(
    stream: &mut S,
    code: ErrorCode,
    message: String,
) -> Result<AdmittedClient, SessionError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let error = safe_error(code, &message);
    write_frame(
        stream,
        &ServerHandshake {
            outcome: HandshakeOutcome::Rejected(error.clone()),
        },
    )
    .await?;
    Err(SessionError::Rejected(error))
}

fn safe_error(code: ErrorCode, message: &str) -> WireError {
    let bounded =
        if message.is_empty() || message.len() > 512 || message.chars().any(char::is_control) {
            SafeMessage::new(format!("local session refused with {code}"))
                .unwrap_or_else(|_| fallback_message())
        } else {
            SafeMessage::new(message.to_owned()).unwrap_or_else(|_| fallback_message())
        };
    WireError::new(
        code,
        bounded,
        CorrelationId::generate(&SystemIdGenerator).unwrap_or_default(),
    )
}

fn fallback_message() -> SafeMessage {
    SafeMessage::new("local session refused").unwrap_or_else(|_| {
        // The literal below is non-empty, control-free, and short, so this arm is unreachable.
        SafeMessage::new("local session refused")
            .unwrap_or_else(|error| panic!("static fallback message must be valid: {error}"))
    })
}

/// How a local client introduces itself to the daemon.
pub struct ClientContext {
    credential: ClientCredential,
    client_id: ClientId,
    client: ClientKind,
    client_version: String,
    capabilities: Vec<String>,
}

impl ClientContext {
    /// Builds a client session context.
    ///
    /// # Errors
    ///
    /// Returns [`HandshakeError`] when the client metadata is unsafe to send.
    pub fn new(
        credential: ClientCredential,
        client: ClientKind,
        client_version: impl Into<String>,
        capabilities: Vec<String>,
    ) -> Result<Self, HandshakeError> {
        let context = Self {
            credential,
            client_id: ClientId::generate(&SystemIdGenerator).unwrap_or_default(),
            client,
            client_version: client_version.into(),
            capabilities,
        };
        context.validate()
    }

    /// Rejects client metadata that must never be placed on the wire.
    fn validate(self) -> Result<Self, HandshakeError> {
        if !crate::is_safe_token(&self.client_version) {
            return Err(HandshakeError::InvalidVersion);
        }
        if self.capabilities.len() > crate::MAX_CLIENT_CAPABILITIES
            || !self
                .capabilities
                .iter()
                .all(|token| crate::is_safe_token(token))
        {
            return Err(HandshakeError::InvalidCapabilities);
        }
        Ok(self)
    }
}

/// An established local client session.
pub struct ClientSession<S> {
    stream: S,
    daemon: DaemonHandshake,
}

impl<S> ClientSession<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    /// Performs the client half of the handshake on an open stream.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError`] when the handshake times out, the daemon
    /// rejects it, or the handshake frames cannot be exchanged.
    pub async fn connect(mut stream: S, context: &ClientContext) -> Result<Self, SessionError> {
        write_frame(
            &mut stream,
            &ClientHandshake {
                credential: context.credential.expose().to_owned(),
                client_id: context.client_id,
                client: context.client,
                client_version: context.client_version.clone(),
                protocol_minimum: MIN_SUPPORTED_PROTOCOL,
                protocol_maximum: MAX_SUPPORTED_PROTOCOL,
                capabilities: context.capabilities.clone(),
            },
        )
        .await?;

        let response = timeout(
            HANDSHAKE_TIMEOUT,
            read_frame::<_, ServerHandshake>(&mut stream),
        )
        .await
        .map_err(|_| SessionError::HandshakeTimeout)??;
        let response = response.ok_or(SessionError::ClosedBeforeHandshake)?;

        match response.outcome {
            HandshakeOutcome::Accepted(daemon) => {
                if daemon.protocol_version != PROTOCOL_VERSION {
                    return Err(SessionError::Negotiation(NegotiationError::ClientTooNew {
                        client_minimum: PROTOCOL_VERSION,
                        supported_maximum: daemon.protocol_version,
                    }));
                }
                Ok(Self { stream, daemon })
            }
            HandshakeOutcome::Rejected(error) => Err(SessionError::Rejected(error)),
        }
    }

    /// Returns the daemon handshake accepted for this session.
    #[must_use]
    pub const fn daemon(&self) -> &DaemonHandshake {
        &self.daemon
    }

    /// Sends one command and returns its reply.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError`] when the daemon replies with an error, the reply
    /// does not echo the request identity, or the transport fails.
    pub async fn request(&mut self, command: Command) -> Result<Reply, SessionError> {
        let request_id = RequestId::generate(&SystemIdGenerator).unwrap_or_default();
        write_frame(
            &mut self.stream,
            &Request {
                request_id,
                command,
            },
        )
        .await?;

        let response = read_frame::<_, Response>(&mut self.stream)
            .await?
            .ok_or(SessionError::ClosedBeforeReply)?;
        if response.request_id != request_id {
            return Err(SessionError::RequestMismatch);
        }
        match response.outcome {
            crate::Outcome::Ok(reply) => Ok(reply),
            crate::Outcome::Err(error) => Err(SessionError::Request(error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DaemonHandshake, HealthReply, StatusReply};
    use jarvis_core::{ClientCredential, DaemonRunId, SystemClock, UtcTimestamp};

    fn daemon_handshake() -> DaemonHandshake {
        DaemonHandshake {
            protocol_version: PROTOCOL_VERSION,
            daemon_version: "0.1.0".to_owned(),
            target_os: "test".to_owned(),
            target_arch: "test".to_owned(),
            config_schema: 1,
            database_schema: test_database_schema(),
            daemon_id: DaemonRunId::generate(&SystemIdGenerator).unwrap_or_default(),
            started_at: UtcTimestamp::now(&SystemClock),
        }
    }

    /// The schema version a wire fixture should report.
    ///
    /// `jarvis-protocol` cannot depend on `jarvis-storage`, because the storage adapter
    /// already depends on this crate, so the constant cannot be imported. Naming it here is
    /// what keeps every fixture in this module reading from one place: as bare literals these
    /// stayed at 2 after the storage schema moved to 3, leaving the fixtures describing a
    /// database version that no longer existed while every test still passed.
    fn test_database_schema() -> i64 {
        3
    }

    struct StubResponder;

    impl Responder for StubResponder {
        fn respond(&self, command: Command) -> Reply {
            match command {
                Command::Status => Reply::Status(StatusReply {
                    phase: "ready".to_owned(),
                    live: true,
                    ready: true,
                    daemon_version: "0.1.0".to_owned(),
                    protocol_version: PROTOCOL_VERSION,
                    config_schema: 1,
                    database_schema: test_database_schema(),
                    daemon_id: DaemonRunId::generate(&SystemIdGenerator).unwrap_or_default(),
                    started_at: UtcTimestamp::now(&SystemClock),
                }),
                Command::Health => Reply::Health(HealthReply {
                    live: true,
                    ready: true,
                    phase: "ready".to_owned(),
                }),
            }
        }
    }

    fn server_context(credential: ClientCredential) -> ServerContext {
        ServerContext::new(
            credential,
            daemon_handshake(),
            std::sync::Arc::new(StubResponder),
        )
    }

    fn client_context(credential: ClientCredential) -> ClientContext {
        ClientContext::new(credential, ClientKind::Cli, "0.1.0", Vec::new())
            .unwrap_or_else(|error| panic!("client context: {error}"))
    }

    fn credential() -> ClientCredential {
        ClientCredential::generate().unwrap_or_else(|error| panic!("credential: {error}"))
    }

    #[tokio::test]
    async fn a_matching_credential_is_admitted_and_commands_are_served() {
        let secret = credential();
        let context = server_context(secret.clone());
        let (client, mut server) = tokio::io::duplex(8192);
        let server_task = tokio::spawn(async move { serve(&mut server, &context).await });

        let mut session = ClientSession::connect(client, &client_context(secret))
            .await
            .unwrap_or_else(|error| panic!("handshake: {error}"));
        assert_eq!(session.daemon().protocol_version, PROTOCOL_VERSION);

        let reply = session
            .request(Command::Health)
            .await
            .unwrap_or_else(|error| panic!("request: {error}"));
        assert_eq!(
            reply,
            Reply::Health(HealthReply {
                live: true,
                ready: true,
                phase: "ready".to_owned()
            })
        );
        drop(session);

        let admitted = server_task
            .await
            .unwrap_or_else(|error| panic!("join: {error}"))
            .unwrap_or_else(|error| panic!("serve: {error}"));
        assert_eq!(admitted.client, ClientKind::Cli);
        assert_eq!(admitted.protocol_version, PROTOCOL_VERSION);
    }

    async fn rejected_by(
        presented: ClientHandshake,
    ) -> (HandshakeOutcome, Result<AdmittedClient, SessionError>) {
        let context = server_context(credential());
        let (mut client, mut server) = tokio::io::duplex(8192);
        let server_task = tokio::spawn(async move { serve(&mut server, &context).await });

        write_frame(&mut client, &presented)
            .await
            .unwrap_or_else(|error| panic!("write handshake: {error}"));
        let response: ServerHandshake = read_frame(&mut client)
            .await
            .unwrap_or_else(|error| panic!("read handshake: {error}"))
            .unwrap_or_else(|| panic!("server sent no handshake"));
        let outcome = server_task
            .await
            .unwrap_or_else(|error| panic!("join: {error}"));

        (response.outcome, outcome)
    }

    fn presented_handshake(credential_text: &str, minimum: u16, maximum: u16) -> ClientHandshake {
        ClientHandshake {
            credential: credential_text.to_owned(),
            client_id: ClientId::generate(&SystemIdGenerator).unwrap_or_default(),
            client: ClientKind::Cli,
            client_version: "0.1.0".to_owned(),
            protocol_minimum: minimum,
            protocol_maximum: maximum,
            capabilities: Vec::new(),
        }
    }

    #[tokio::test]
    async fn a_wrong_credential_is_rejected_and_serves_no_commands() {
        let (outcome, served) = rejected_by(presented_handshake("00", 1, 1)).await;

        match outcome {
            HandshakeOutcome::Rejected(error) => {
                assert_eq!(error.code, ErrorCode::Authentication);
                assert!(!error.retryable);
            }
            HandshakeOutcome::Accepted(_) => panic!("a wrong credential must not be admitted"),
        }
        assert!(matches!(served, Err(SessionError::Rejected(_))));
    }

    #[tokio::test]
    async fn an_incompatible_protocol_window_is_rejected() {
        let secret = credential();
        let (response_outcome, admitted) = {
            let context = ServerContext::new(
                secret.clone(),
                daemon_handshake(),
                std::sync::Arc::new(StubResponder),
            );
            let (mut client, mut server) = tokio::io::duplex(8192);
            let server_task = tokio::spawn(async move { serve(&mut server, &context).await });

            write_frame(&mut client, &presented_handshake(secret.expose(), 0, 0))
                .await
                .unwrap_or_else(|error| panic!("write handshake: {error}"));
            let response: ServerHandshake = read_frame(&mut client)
                .await
                .unwrap_or_else(|error| panic!("read handshake: {error}"))
                .unwrap_or_else(|| panic!("server sent no handshake"));
            let outcome = server_task
                .await
                .unwrap_or_else(|error| panic!("join: {error}"));
            (response.outcome, outcome)
        };

        match response_outcome {
            HandshakeOutcome::Rejected(error) => assert_eq!(error.code, ErrorCode::Conflict),
            HandshakeOutcome::Accepted(_) => panic!("too-old protocol must not be admitted"),
        }
        assert!(matches!(admitted, Err(SessionError::Rejected(_))));
    }

    #[tokio::test]
    async fn unsafe_client_metadata_is_rejected_before_authentication() {
        let mut presented = presented_handshake("00", 1, 1);
        presented.client_version = "0.1.0\nforged".to_owned();
        let (outcome, _) = rejected_by(presented).await;

        match outcome {
            HandshakeOutcome::Rejected(error) => assert_eq!(error.code, ErrorCode::Validation),
            HandshakeOutcome::Accepted(_) => panic!("unsafe metadata must not be admitted"),
        }
    }

    #[test]
    fn a_client_cannot_construct_an_unsafe_context() {
        assert!(matches!(
            ClientContext::new(credential(), ClientKind::Cli, String::new(), Vec::new()),
            Err(HandshakeError::InvalidVersion)
        ));
        assert!(matches!(
            ClientContext::new(
                credential(),
                ClientKind::Cli,
                "0.1.0",
                vec!["bad\ncapability".to_owned()]
            ),
            Err(HandshakeError::InvalidCapabilities)
        ));
    }
}
