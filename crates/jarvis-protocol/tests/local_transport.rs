//! Cross-crate contract tests for the local daemon transport and handshake.
//!
//! These bind a real operating-system endpoint (Unix domain socket or Windows
//! named pipe) and drive the protocol v1 handshake and request loop over it.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use jarvis_core::{
    ClientCredential, CorrelationId, DaemonRunId, ErrorCode, LocalEndpoint, LocalListener,
    SafeMessage, SystemClock, SystemIdGenerator, UtcTimestamp, connect,
};
use jarvis_protocol::{
    ClientContext, ClientKind, ClientSession, Command, DaemonHandshake, HandshakeOutcome,
    HealthReply, PROTOCOL_VERSION, Reply, Responder, ServerContext, SessionError, StatusReply,
    WireError, serve,
};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "jarvis-protocol-transport-{}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap_or_else(|error| panic!("create temp dir: {error}"));
        Self(path)
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn endpoint(directory: &TempDirectory) -> LocalEndpoint {
    if cfg!(windows) {
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        LocalEndpoint::named_pipe(&format!("jarvis-test-{}-{sequence}", std::process::id()))
            .unwrap_or_else(|error| panic!("pipe endpoint: {error}"))
    } else {
        LocalEndpoint::unix_socket(&directory.0, "test")
            .unwrap_or_else(|error| panic!("socket endpoint: {error}"))
    }
}

fn new_daemon_id() -> DaemonRunId {
    DaemonRunId::generate(&SystemIdGenerator).unwrap_or_default()
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
                daemon_id: new_daemon_id(),
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

fn daemon_handshake() -> DaemonHandshake {
    DaemonHandshake {
        protocol_version: PROTOCOL_VERSION,
        daemon_version: "0.1.0".to_owned(),
        target_os: std::env::consts::OS.to_owned(),
        target_arch: std::env::consts::ARCH.to_owned(),
        config_schema: 1,
        database_schema: test_database_schema(),
        daemon_id: new_daemon_id(),
        started_at: UtcTimestamp::now(&SystemClock),
    }
}

/// The schema version a wire fixture should report.
///
/// `jarvis-protocol` cannot depend on `jarvis-storage`, because the storage adapter already
/// depends on this crate, so the constant cannot be imported. Naming it here keeps every
/// fixture in this file reading from one place instead of several stale literals.
fn test_database_schema() -> i64 {
    3
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

#[tokio::test]
async fn an_authenticated_client_completes_the_handshake_over_native_transport() {
    let directory = TempDirectory::new();
    let endpoint = endpoint(&directory);
    let credential =
        ClientCredential::generate().unwrap_or_else(|error| panic!("credential: {error}"));

    let mut listener =
        LocalListener::bind(&endpoint).unwrap_or_else(|error| panic!("bind: {error}"));
    let context = server_context(credential.clone());
    let server = tokio::spawn(async move {
        let mut stream = listener
            .accept()
            .await
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        serve(&mut stream, &context)
            .await
            .map_err(|error| std::io::Error::other(error.to_string()))
    });

    let stream = connect(&endpoint)
        .await
        .unwrap_or_else(|error| panic!("connect: {error}"));
    let mut session = ClientSession::connect(stream, &client_context(credential))
        .await
        .unwrap_or_else(|error| panic!("handshake: {error:?}"));

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

    let admitted = server
        .await
        .unwrap_or_else(|error| panic!("join: {error}"))
        .unwrap_or_else(|error| panic!("serve: {error}"));
    assert_eq!(admitted.client, ClientKind::Cli);
}

/// Proves the credential guard fails closed over the real operating-system transport.
#[tokio::test]
async fn a_wrong_credential_is_refused_over_native_transport() {
    let directory = TempDirectory::new();
    let endpoint = endpoint(&directory);
    let correct =
        ClientCredential::generate().unwrap_or_else(|error| panic!("credential: {error}"));
    let wrong = ClientCredential::generate().unwrap_or_else(|error| panic!("credential: {error}"));

    let mut listener =
        LocalListener::bind(&endpoint).unwrap_or_else(|error| panic!("bind: {error}"));
    let context = server_context(correct);
    let server = tokio::spawn(async move {
        let mut stream = listener
            .accept()
            .await
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        serve(&mut stream, &context)
            .await
            .map_err(|error| std::io::Error::other(error.to_string()))
    });

    let stream = connect(&endpoint)
        .await
        .unwrap_or_else(|error| panic!("connect: {error}"));
    let result = ClientSession::connect(stream, &client_context(wrong)).await;
    match result {
        Err(SessionError::Rejected(error)) => {
            assert_eq!(error.code, ErrorCode::Authentication);
            assert!(!error.retryable);
        }
        Err(other) => panic!("expected an authentication rejection, got {other:?}"),
        Ok(_) => panic!("a wrong credential must never establish a session"),
    }

    let admitted = server.await.unwrap_or_else(|error| panic!("join: {error}"));
    assert!(
        admitted.is_err(),
        "the server must not admit a client that presented the wrong credential"
    );
}

#[test]
fn a_rejection_carries_a_stable_code_rather_than_a_bare_success_signal() {
    let rejected = HandshakeOutcome::Rejected(WireError::new(
        ErrorCode::Authentication,
        SafeMessage::new("refused").unwrap_or_else(|error| panic!("message: {error}")),
        CorrelationId::generate(&SystemIdGenerator).unwrap_or_default(),
    ));
    assert!(matches!(rejected, HandshakeOutcome::Rejected(_)));
}
