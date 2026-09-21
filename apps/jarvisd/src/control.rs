//! Local control-plane surface served to authenticated local clients.

use std::sync::Arc;

use jarvis_core::{DaemonRunId, LocalListener, TransportError, UtcTimestamp};
use jarvis_protocol::{
    Command, DaemonHandshake, HealthReply, PROTOCOL_VERSION, Reply, Responder, ServerContext,
    StatusReply, serve,
};

use crate::{build_info::BuildInfo, health::HealthState};

/// Answers protocol v1 commands from live daemon state.
pub(crate) struct ControlPlane {
    health: Arc<HealthState>,
    build: BuildInfo,
    daemon_id: DaemonRunId,
    started_at: UtcTimestamp,
}

impl ControlPlane {
    pub(crate) const fn new(
        health: Arc<HealthState>,
        build: BuildInfo,
        daemon_id: DaemonRunId,
        started_at: UtcTimestamp,
    ) -> Self {
        Self {
            health,
            build,
            daemon_id,
            started_at,
        }
    }

    /// Describes this daemon in the protocol handshake.
    pub(crate) fn handshake(&self) -> DaemonHandshake {
        DaemonHandshake {
            protocol_version: PROTOCOL_VERSION,
            daemon_version: self.build.version().to_owned(),
            target_os: self.build.target_os().to_owned(),
            target_arch: self.build.target_arch().to_owned(),
            config_schema: self.build.config_schema(),
            database_schema: self.build.database_schema(),
            daemon_id: self.daemon_id,
            started_at: self.started_at,
        }
    }

    /// Builds the shared server context for the local listener.
    pub(crate) fn server_context(
        self: &Arc<Self>,
        credential: jarvis_core::ClientCredential,
    ) -> ServerContext {
        ServerContext::new(
            credential,
            self.handshake(),
            Arc::clone(self) as Arc<dyn Responder>,
        )
    }
}

impl Responder for ControlPlane {
    fn respond(&self, command: Command) -> Reply {
        let snapshot = self.health.snapshot();
        match command {
            Command::Health => Reply::Health(HealthReply {
                live: snapshot.live,
                ready: snapshot.ready,
                phase: snapshot.phase.to_owned(),
            }),
            Command::Status => Reply::Status(StatusReply {
                phase: snapshot.phase.to_owned(),
                live: snapshot.live,
                ready: snapshot.ready,
                daemon_version: self.build.version().to_owned(),
                protocol_version: PROTOCOL_VERSION,
                config_schema: self.build.config_schema(),
                database_schema: self.build.database_schema(),
                daemon_id: self.daemon_id,
                started_at: self.started_at,
            }),
        }
    }
}

/// Accepts local clients until the listener fails.
///
/// Each connection is served on its own task so one slow client cannot block
/// the accept loop. The function only returns when the listener itself fails;
/// shutdown is handled by the caller cancelling this future.
pub(crate) async fn accept_clients(
    mut listener: LocalListener,
    context: Arc<ServerContext>,
) -> TransportError {
    loop {
        let stream = match listener.accept().await {
            Ok(stream) => stream,
            Err(error) => return error,
        };
        let context = Arc::clone(&context);
        tokio::spawn(async move {
            let mut stream = stream;
            if let Err(error) = serve(&mut stream, &context).await {
                // The error text is a stable code plus a bounded, secret-free
                // message, so it is safe for the operator log.
                eprintln!("jarvisd local client session ended: {error}");
            }
        });
    }
}
