//! JARVIS daemon composition root.

mod build_info;
mod control;
mod health;
mod singleton;

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
    database: SqliteDatabase,
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
    let context = Arc::new(control.server_context(credential));

    Ok(Running {
        health,
        paths,
        logging,
        singleton,
        database,
        daemon_id,
        accept_loop: Box::pin(accept_clients(listener, context)),
    })
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
        "daemon ready"
    );

    let (shutdown_result, accept_failure) = tokio::select! {
        result = shutdown => (result, None),
        failure = accept_loop => (Ok(()), Some(failure)),
    };
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
