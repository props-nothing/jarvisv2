//! JARVIS daemon composition root.

mod build_info;
mod control;
mod executor;
mod gateway;
mod health;
mod run_service;
mod singleton;
mod sse;

use std::{env, future::Future, io, path::Path, path::PathBuf, process::ExitCode, sync::Arc};

use jarvis_core::{
    DAEMON_LOCK_FILE_NAME, DaemonRunId, LocalEndpoint, LocalListener, SystemClock, UtcTimestamp,
};
use jarvis_observability::{Logging, LoggingError, Redactor};
use jarvis_storage::{
    AppPaths, ConfigError, ConfigStore, CredentialStore, CredentialStoreError, DaemonInstanceStart,
    DaemonStopReason, DatabaseError, PathError, SqliteDatabase, portable_layout,
};
use thiserror::Error;

use crate::{
    build_info::BuildInfo,
    control::{ControlPlane, accept_clients},
    health::{HealthState, LifecycleError},
    singleton::{SingletonError, SingletonGuard},
};

/// Filename of the credential file inside the configuration directory.
const LOCAL_CREDENTIAL_FILE_NAME: &str = "client.credential";

#[derive(Debug, Error)]
enum DaemonError {
    #[error(transparent)]
    Path(#[from] PathError),
    #[error(transparent)]
    Singleton(#[from] SingletonError),
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Credential(#[from] CredentialStoreError),
    #[error(transparent)]
    Logging(#[from] LoggingError),
    #[error(transparent)]
    Database(#[from] DatabaseError),
    #[error(transparent)]
    Lifecycle(#[from] LifecycleError),
    #[error(transparent)]
    LocalEndpoint(#[from] jarvis_core::EndpointError),
    #[error("failed to bind the local control endpoint")]
    LocalTransport(#[from] jarvis_core::TransportError),
    #[error("the portable --root path is not absolute: {0}")]
    RelativeRoot(String),
    #[error("the portable --root argument requires a directory path")]
    MissingRootValue,
    #[error("unknown jarvisd argument: {0}")]
    UnknownArgument(String),
    #[error("failed to listen for a shutdown signal")]
    Signal(#[source] io::Error),
    #[error("failed to bind the loopback HTTP transport on port {port}")]
    HttpBind {
        /// The configured port, without the resolved address.
        port: u16,
        /// The underlying operating-system error.
        #[source]
        source: io::Error,
    },
    #[error("daemon.executor_model names a model this build does not implement: {name}")]
    ExecutorModel {
        /// The configured name, echoed so the fix is one edit.
        name: String,
        /// Why the name was refused.
        #[source]
        source: executor::ExecutorBuildError,
    },
}

/// How the daemon was told to locate its profile.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Invocation {
    /// Print build identity and exit.
    Version,
    /// Use OS-native per-user directories, or an explicit portable root.
    Run {
        /// Explicit portable root, when one was supplied.
        root: Option<PathBuf>,
    },
}

/// Parses the command line without a dependency, keeping the surface explicit.
fn parse_invocation(arguments: &[String]) -> Result<Invocation, DaemonError> {
    let mut root = None;
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--version" | "-V" => return Ok(Invocation::Version),
            "--root" => {
                let value = arguments
                    .get(index + 1)
                    .ok_or(DaemonError::MissingRootValue)?;
                let candidate = PathBuf::from(value);
                if !candidate.is_absolute() {
                    return Err(DaemonError::RelativeRoot(value.clone()));
                }
                root = Some(candidate);
                index += 2;
            }
            other => {
                return Err(DaemonError::UnknownArgument(other.to_owned()));
            }
        }
    }
    Ok(Invocation::Run { root })
}

/// Resolves the profile directories for the selected mode.
fn resolve_paths(root: Option<&Path>) -> Result<AppPaths, DaemonError> {
    match root {
        Some(root) => portable_layout(root)
            .ok_or_else(|| DaemonError::RelativeRoot(root.display().to_string())),
        None => AppPaths::resolve_native().map_err(DaemonError::from),
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let build = BuildInfo::current();
    let arguments: Vec<String> = env::args().skip(1).collect();
    let invocation = match parse_invocation(&arguments) {
        Ok(invocation) => invocation,
        Err(error) => {
            eprintln!("jarvisd usage error: {error}");
            eprintln!("usage: jarvisd [--version] [--root <absolute-directory>]");
            return ExitCode::from(2);
        }
    };

    let root = match invocation {
        Invocation::Version => {
            println!("{}", build.display_line());
            return ExitCode::SUCCESS;
        }
        Invocation::Run { root } => root,
    };

    match run(build, root, shutdown_signal()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("jarvisd startup failed: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Everything the daemon must hold for its lifetime, constructed in startup order.
struct Running {
    health: Arc<HealthState>,
    paths: AppPaths,
    logging: Logging,
    singleton: SingletonGuard,
    database: Arc<SqliteDatabase>,
    credential: jarvis_core::ClientCredential,
    http_port: Option<u16>,
    /// The configured executor model, resolved at start so an unimplemented name fails the
    /// daemon rather than being discovered when a run is started.
    executor: Option<Arc<executor::Executor>>,
    daemon_id: DaemonRunId,
    accept_loop: std::pin::Pin<Box<dyn Future<Output = jarvis_core::TransportError> + Send>>,
}

/// Establishes every owned resource, failing closed before the daemon serves.
///
/// Order matters: the credential is read before logging so the redactor can mask
/// it from the first recorded line, and the singleton lock is taken before the
/// database so a second daemon never touches state.
async fn start(build: BuildInfo, root: Option<&Path>) -> Result<Running, DaemonError> {
    let health = Arc::new(HealthState::new());
    let paths = resolve_paths(root)?;
    paths.prepare()?;

    let config_store = ConfigStore::from_paths(&paths);
    let loaded_config = config_store.load()?;
    if loaded_config.migration().is_some() {
        config_store.save(loaded_config.config())?;
    }

    // The daemon is the sole issuer of the profile credential.
    let credential_store = CredentialStore::at(paths.config().join(LOCAL_CREDENTIAL_FILE_NAME));
    let credential = credential_store.load_or_create(paths.config())?;
    let logging = Logging::init(
        paths.logs(),
        loaded_config.config().logging().level(),
        Redactor::new([credential.expose()]),
    )?;

    let lock_path = paths.runtime().join(DAEMON_LOCK_FILE_NAME);
    let singleton = SingletonGuard::acquire(&lock_path, build)?;

    let database = SqliteDatabase::open_default(&paths).await?;
    let daemon_id = DaemonRunId::new();
    let started_at = UtcTimestamp::now(&SystemClock);
    let instance = DaemonInstanceStart::new(
        daemon_id,
        std::process::id(),
        build.version(),
        build.target_os(),
        build.target_arch(),
        started_at,
    )?;
    if let Err(error) = database.record_daemon_start(&instance).await {
        database.close().await;
        return Err(error.into());
    }
    if let Err(error) = database
        .mark_daemon_ready(daemon_id, UtcTimestamp::now(&SystemClock))
        .await
    {
        let _ = database
            .mark_daemon_stopped(
                daemon_id,
                UtcTimestamp::now(&SystemClock),
                DaemonStopReason::StartupFailed,
            )
            .await;
        database.close().await;
        return Err(error.into());
    }

    health.mark_ready()?;
    let endpoint = LocalEndpoint::scoped(paths.runtime(), loaded_config.config().profile().name())?;
    let listener = LocalListener::bind(&endpoint)?;
    let control = Arc::new(ControlPlane::new(
        Arc::clone(&health),
        build,
        daemon_id,
        started_at,
    ));
    let context = Arc::new(control.server_context(credential.clone()));

    // ADR-0011 makes the HTTP transport separately enabled and loopback-bound. Reading the
    // setting here rather than inside the gateway keeps the gateway itself free of policy about
    // whether it should exist.
    let http_port = loaded_config
        .config()
        .daemon()
        .http_enabled()
        .then(|| loaded_config.config().daemon().http_port());

    // The executor model is resolved HERE, so an unimplemented name stops the daemon at startup
    // with an actionable error rather than being discovered when the first run is started and
    // leaving a run that is accepted but never driven.
    let executor = match loaded_config.config().daemon().executor_model() {
        Some(name) => Some(Arc::new(executor::Executor::build(name).map_err(
            |error| DaemonError::ExecutorModel {
                name: name.to_owned(),
                source: error,
            },
        )?)),
        None => None,
    };

    Ok(Running {
        health,
        paths,
        logging,
        singleton,
        database: Arc::new(database),
        credential,
        http_port,
        executor,
        daemon_id,
        accept_loop: Box::pin(accept_clients(listener, context)),
    })
}

/// A bound and served loopback HTTP transport.
///
/// Owning the serving task rather than dropping it into a detached spawn is what makes a dead
/// listener detectable. `axum::serve` never returns an error and retries socket errors itself, so
/// its return value carries no information; the task's **completion** does. [`Self::bind`]
/// therefore also hands back a stop signal the caller can await.
struct HttpTransport {
    port: u16,
    shutdown: tokio::sync::oneshot::Sender<()>,
    handle: tokio::task::JoinHandle<()>,
}

impl HttpTransport {
    /// Binds loopback, starts serving the gateway, and returns the transport and its stop signal.
    ///
    /// # Errors
    ///
    /// Returns [`DaemonError::HttpBind`] when the loopback address cannot be bound. Binding is the
    /// step that fails fast: an occupied port is reported here rather than becoming a listener the
    /// daemon believes it has.
    async fn bind(
        port: u16,
        database: Arc<SqliteDatabase>,
        credential: jarvis_core::ClientCredential,
        executor: Option<Arc<executor::Executor>>,
    ) -> Result<(Self, tokio::sync::oneshot::Receiver<()>), DaemonError> {
        // Loopback only. Reaching any other interface is remote mode, which `P10-004` owns as an
        // explicit TLS-terminated configuration rather than something that happens by default.
        let address = std::net::SocketAddr::from(([127, 0, 0, 1], port));
        let listener = tokio::net::TcpListener::bind(address)
            .await
            .map_err(|source| DaemonError::HttpBind { port, source })?;

        let mut state = gateway::GatewayState::new(database, credential);
        if let Some(executor) = executor {
            state = state.with_executor(executor);
        }
        let app = gateway::router(state);
        let (shutdown, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let (stopped_tx, stopped) = tokio::sync::oneshot::channel::<()>();
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.await;
                })
                .await;
            // Sent explicitly, because this is the only observable evidence that the listener
            // stopped. `axum::serve`'s own return value says nothing about why.
            let _ = stopped_tx.send(());
        });

        Ok((
            Self {
                port,
                shutdown,
                handle,
            },
            stopped,
        ))
    }

    /// Returns the bound loopback port.
    const fn port(&self) -> u16 {
        self.port
    }

    /// Stops the transport, letting in-flight responses close.
    ///
    /// Graceful rather than abrupt so an SSE stream ends at an event boundary. A severed stream
    /// looks to a client like a lost event, leaving it to decide whether to resume from its last
    /// sequence or discard its position.
    async fn shutdown(self) {
        let _ = self.shutdown.send(());
        let _ = self.handle.await;
    }
}

/// The outcome of the daemon's main wait.
struct WaitOutcome {
    shutdown: io::Result<()>,
    accept_failure: Option<jarvis_core::TransportError>,
    http_stopped: bool,
}

/// Waits for a shutdown signal, a local-listener failure, or an HTTP-listener failure.
///
/// The HTTP listener is a third branch of the same select rather than a detached task, so a
/// listener that dies takes the shutdown path instead of leaving the daemon reporting itself ready
/// with no transport behind it.
async fn wait_for_exit<F>(
    shutdown: F,
    accept_loop: std::pin::Pin<Box<dyn Future<Output = jarvis_core::TransportError> + Send>>,
    http_stop: Option<tokio::sync::oneshot::Receiver<()>>,
) -> WaitOutcome
where
    F: Future<Output = io::Result<()>>,
{
    // A pinned receiver so the select polls it by reference, keeping ownership inside this
    // function. Awaiting it by value in an arm would move it out of the `if let`, and the other
    // arms would then have to return it.
    if let Some(receiver) = http_stop {
        let mut stop = Box::pin(receiver);
        tokio::select! {
            result = shutdown => WaitOutcome {
                shutdown: result,
                accept_failure: None,
                http_stopped: false,
            },
            failure = accept_loop => WaitOutcome {
                shutdown: Ok(()),
                accept_failure: Some(failure),
                http_stopped: false,
            },
            _ = &mut stop => WaitOutcome {
                shutdown: Ok(()),
                accept_failure: None,
                http_stopped: true,
            },
        }
    } else {
        let (shutdown, accept_failure) = tokio::select! {
            result = shutdown => (result, None),
            failure = accept_loop => (Ok(()), Some(failure)),
        };
        WaitOutcome {
            shutdown,
            accept_failure,
            http_stopped: false,
        }
    }
}

async fn run<F>(build: BuildInfo, root: Option<PathBuf>, shutdown: F) -> Result<(), DaemonError>
where
    F: Future<Output = io::Result<()>>,
{
    let Running {
        health,
        paths: _paths,
        logging,
        singleton,
        database,
        credential,
        http_port,
        executor,
        daemon_id,
        accept_loop,
    } = start(build, root.as_deref()).await?;

    let ready = health.snapshot();
    tracing::info!(
        phase = ready.phase,
        live = ready.live,
        ready = ready.ready,
        version = build.version(),
        target_os = build.target_os(),
        target_arch = build.target_arch(),
        config_schema = build.config_schema(),
        database_schema = build.database_schema(),
        mode = if root.is_some() { "portable" } else { "native" },
        secrets_masked = logging.secret_count(),
        http_port = http_port.unwrap_or(0),
        "daemon ready"
    );

    // The listener is bound before it is served, and binding is what fails fast: an occupied port
    // is reported here rather than becoming a listener nobody notices is dead.
    let (http, http_stop) = match http_port {
        Some(port) => {
            let (transport, stop) =
                HttpTransport::bind(port, Arc::clone(&database), credential.clone(), executor)
                    .await?;
            (Some(transport), Some(stop))
        }
        None => (None, None),
    };

    let outcome = wait_for_exit(shutdown, accept_loop, http_stop).await;
    if let Some(transport) = http {
        let port = transport.port();
        transport.shutdown().await;
        tracing::info!(port, "HTTP transport stopped");
    }
    if outcome.http_stopped {
        tracing::error!(
            port = http_port.unwrap_or(0),
            "the HTTP transport stopped unexpectedly"
        );
    }

    let shutdown_result = outcome.shutdown;
    let accept_failure = outcome.accept_failure;
    health.begin_shutdown()?;
    let stop_reason = if shutdown_result.is_ok() && accept_failure.is_none() {
        DaemonStopReason::Signal
    } else {
        DaemonStopReason::ShutdownFailed
    };
    if let Some(failure) = accept_failure {
        tracing::error!(error = %failure, "local listener stopped");
    }
    let persistence_result = database
        .mark_daemon_stopped(daemon_id, UtcTimestamp::now(&SystemClock), stop_reason)
        .await;
    database.close().await;
    health.mark_stopped()?;
    let lock_result = singleton.release();

    shutdown_result.map_err(DaemonError::Signal)?;
    persistence_result?;
    lock_result?;
    let stopped = health.snapshot();
    tracing::info!(
        phase = stopped.phase,
        live = stopped.live,
        ready = stopped.ready,
        version = build.version(),
        "daemon stopped"
    );
    logging.flush()?;
    Ok(())
}

#[cfg(unix)]
async fn shutdown_signal() -> io::Result<()> {
    use tokio::signal::unix::{SignalKind, signal};

    let mut terminate = signal(SignalKind::terminate())?;
    tokio::select! {
        result = tokio::signal::ctrl_c() => result,
        event = terminate.recv() => signal_event(event),
    }
}

#[cfg(windows)]
async fn shutdown_signal() -> io::Result<()> {
    use tokio::signal::windows;

    let mut ctrl_break = windows::ctrl_break()?;
    let mut ctrl_close = windows::ctrl_close()?;
    let mut ctrl_logoff = windows::ctrl_logoff()?;
    let mut ctrl_shutdown = windows::ctrl_shutdown()?;
    tokio::select! {
        result = tokio::signal::ctrl_c() => result,
        event = ctrl_break.recv() => signal_event(event),
        event = ctrl_close.recv() => signal_event(event),
        event = ctrl_logoff.recv() => signal_event(event),
        event = ctrl_shutdown.recv() => signal_event(event),
    }
}

fn signal_event(event: Option<()>) -> io::Result<()> {
    event.ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "signal stream closed"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn injected_shutdown_future_is_awaitable_without_os_signal() {
        let result = async { Ok::<(), io::Error>(()) }.await;
        assert!(result.is_ok());
    }

    #[test]
    fn closed_signal_stream_is_an_error() {
        assert_eq!(
            signal_event(None).map_err(|error| error.kind()),
            Err(io::ErrorKind::BrokenPipe)
        );
    }
}
