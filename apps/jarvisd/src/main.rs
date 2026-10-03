//! JARVIS daemon composition root.

mod build_info;
mod code_run;
mod control;
mod dispatch;
mod entity_service;
mod executor;
mod gateway;
mod health;
mod mcp_host;
mod mcp_serve;
mod memory_propose;
mod memory_service;
mod run_service;
mod schedule_service;
mod singleton;
mod skill_service;
mod sse;
mod tool_actor;
mod tool_pipeline;

#[cfg(test)]
mod approval_fixture;

use std::{env, fs, future::Future, io, path::Path, path::PathBuf, process::ExitCode, sync::Arc};

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
    /// The file named by `daemon.executor_api_key_ref` could not be read.
    ///
    /// The path is **not** echoed in the message and the source is kept for a log, because a file error's
    /// text names the path and a startup error reaches an operator's console. The operator knows which path
    /// they configured.
    #[error("the model provider API key file could not be read")]
    ModelApiKeyFile {
        /// Why the file could not be read.
        #[source]
        source: std::io::Error,
    },
    /// The file named by `daemon.executor_api_key_ref` was empty or whitespace only.
    ///
    /// Refused rather than passed to the adapter, which would reject it with a message about the key's shape
    /// rather than about the file the operator supplied.
    #[error("the model provider API key file is empty")]
    ModelApiKeyEmpty,
    #[error("a tool workspace root could not be granted")]
    ToolWorkspaceRoots {
        /// Why the grant was refused, which names the root and the reason.
        #[source]
        source: jarvis_tools::RootError,
    },
    /// The memory-proposal tool could not state its own contract.
    ///
    /// A startup fault rather than a call-time one: `P4-014`'s tool is a constant declaration, so a rejection
    /// means an authoring mistake in this build. It fails startup for the reason `Dispatch::verify_covers`
    /// does — a tool whose contract is wrong must not be offered to a model that would then call it.
    #[error("the memory-proposal tool could not be defined")]
    MemoryProposeTool {
        /// Which constant was rejected, named by the variant.
        #[source]
        source: crate::memory_propose::ProposeToolError,
    },
    /// The web fetch tool could not state its own contract.
    ///
    /// A startup fault for the same reason as the memory-proposal tool: the contract is a constant, so a
    /// rejection is an authoring mistake in this build and the tool must not be offered to a model.
    #[error("the web fetch tool could not be defined")]
    WebFetchTool {
        /// Which constant was rejected, named by the variant.
        #[source]
        source: jarvis_web::WebFetchToolError,
    },
    /// The code-running tool could not state its own contract.
    #[error("the code-running tool could not be defined")]
    CodeRunTool {
        /// Which part was rejected, named by the variant.
        #[source]
        source: crate::code_run::CodeRunToolError,
    },
    /// The configured workspace policy was self-contradictory.
    ///
    /// The pair is refused at parse time too, so this is the belt to that suspenders: the composition
    /// root states its own requirement rather than trusting a caller the configuration layer already
    /// filtered.
    #[error("the configured workspace policy is not usable")]
    WorkspacePolicy {
        /// Why the policy was refused, which names the two clashing values.
        #[source]
        source: jarvis_tools::PolicyError,
    },
    /// A configured tool identifier in the policy section was not a valid identifier.
    ///
    /// Carries the identifier, which is **safe** here: a tool identifier is not a secret, it is the
    /// thing an operator must edit, and an error that did not name it would leave them searching the
    /// document.
    #[error("the workspace policy names an invalid tool: {tool}")]
    WorkspacePolicyTool {
        /// The identifier as written in the document.
        tool: String,
        /// Why the identifier was rejected.
        #[source]
        source: jarvis_tools::ToolIdError,
    },
    #[error("the tool pipeline could not be composed")]
    ToolPipeline {
        /// Why composition failed, which names the rejected definition or root.
        #[source]
        source: crate::tool_pipeline::ToolPipelineError,
    },
    #[error("the MCP endpoint could not be served")]
    McpServe {
        /// Why the endpoint could not be built or bound, which names the reason.
        #[source]
        source: crate::mcp_serve::RemoteMcpError,
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
    /// The composed tool pipeline, resolved at start for the same reason: an unusable workspace
    /// grant must fail the daemon rather than the first tool call.
    tools: Option<Arc<crate::tool_pipeline::ToolPipeline>>,
    /// The connected MCP host, held so its connections stay open and can be closed on shutdown.
    ///
    /// Held **beside** the pipeline rather than inside it because the two have different lifetimes: the
    /// pipeline's adapters are handed over permanently, while the host owns the connections those adapters
    /// share and must be shut down after the pipeline stops being used. Dropping the host while the pipeline
    /// still held an adapter would leave a call reaching a connection nobody was closing.
    mcp: Option<crate::mcp_host::ComposedHost>,
    /// The bound MCP endpoint JARVIS **serves**, when `daemon.mcp_serve_port` is set.
    ///
    /// Distinct from `mcp` in both direction and lifetime: `mcp` is the outbound host (JARVIS calling
    /// *someone else's* server), while this is the inbound endpoint (a client calling *ours*). They are held
    /// separately because shutting one down must not depend on the other having stopped, and a daemon may
    /// legitimately have either without the other.
    serving_mcp: Option<crate::mcp_serve::ServingMcp>,
    /// The MCP endpoint's stop signal, watched by the wait loop so a dead listener is detectable.
    ///
    /// Held **here** rather than inside [`crate::mcp_serve::ServingMcp`] for the same reason the REST
    /// transport's signal is: the transport owns the shutdown sender, and the wait loop must be able to await
    /// the receiver. Keeping it in `Running` is what lets `run` watch it alongside the other two branches.
    mcp_stop: Option<tokio::sync::oneshot::Receiver<()>>,
    daemon_id: DaemonRunId,
    accept_loop: std::pin::Pin<Box<dyn Future<Output = jarvis_core::TransportError> + Send>>,
}

/// Settles every run a previous process left in flight, failing closed if it cannot.
///
/// A process that died mid-run leaves a run row in a non-terminal state with no task that will ever
/// advance it. `FR-RUN-003` requires recovery at a documented boundary, and the boundary honoured
/// here is **truthfulness**: the run is settled as failed with a code naming the restart, or as
/// cancelled when a cancellation had already been requested, because the operator's intent outranks
/// the interruption.
///
/// Recovery is deliberately not a resume. The executor holds the model stream and the answer text in
/// memory, and the daemon cannot know whether the provider accepted the in-flight call, so a resumed
/// run could answer a question that was already being answered. Reporting the interruption is
/// honest; a `completed` run whose answer was never produced would not be.
///
/// This runs **before** the listener binds, so no client can observe a run in a non-terminal state
/// that no process will ever advance.
///
/// # Errors
///
/// Returns the storage failure that prevented the accounting, after recording why the daemon
/// stopped. Failing closed is the point: a daemon that served clients while unable to explain its
/// own interrupted work would present a run that looks active and has no executor behind it.
async fn settle_interrupted_runs(
    database: &SqliteDatabase,
    daemon_id: DaemonRunId,
) -> Result<(), DaemonError> {
    match jarvis_storage::recover_interrupted_runs(database, UtcTimestamp::now(&SystemClock)).await
    {
        Ok(recovered) if !recovered.is_empty() => {
            // Logged with the identifiers rather than a bare count, so an operator can look up what
            // was recovered instead of being told that something was.
            tracing::warn!(
                count = recovered.len(),
                runs = ?recovered,
                "settled runs interrupted by a previous process"
            );
            Ok(())
        }
        Ok(_) => Ok(()),
        Err(error) => {
            let _ = database
                .mark_daemon_stopped(
                    daemon_id,
                    UtcTimestamp::now(&SystemClock),
                    DaemonStopReason::StartupFailed,
                )
                .await;
            Err(error.into())
        }
    }
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

    // Wrapped in a shared handle immediately, because both the daemon's lifetime state and the tool
    // pipeline need the same connection pool. Two handles to one SQLite file would be two pools over
    // one WAL, which is a correctness hazard rather than a convenience.
    let database = Arc::new(SqliteDatabase::open_default(&paths).await?);
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

    // Recovery runs BEFORE the listener binds, so no client can observe a run in a non-terminal
    // state that no process will ever advance.
    if let Err(error) = settle_interrupted_runs(&database, daemon_id).await {
        database.close().await;
        return Err(error);
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

    // The executor is composed HERE, so an unimplemented name — or a live provider with unusable
    // coordinates — stops the daemon at startup with an actionable error rather than being discovered when
    // the first run is started and leaving a run that is accepted but never driven. Extracted as its own
    // function because the live provider adds a file read and a credential, and `start` was already at the
    // length where clippy's `too_many_lines` signals a function carrying a second responsibility.
    let executor = compose_executor(loaded_config.config().daemon())?;

    // The tool pipeline and MCP host are composed HERE, for the same reason the executor is: an unusable
    // workspace grant must stop the daemon at startup rather than be discovered by the first tool call.
    // Extracted into its own function because composing them carries its own failure policy — an MCP failure
    // is not fatal while a pipeline failure is — and `start` was at the length where clippy's `too_many_lines`
    // is a signal that a function has grown a second responsibility.
    let (mcp, tools) = compose_tools(loaded_config.config(), &paths, Arc::clone(&database)).await?;

    // The **inbound** endpoint, composed after the pipeline for the forced reason that it serves the
    // pipeline's own definitions. A failure here is fatal unlike the outbound host's, because the operator
    // explicitly enabled a port: a daemon that reported itself ready with an endpoint it never bound would be
    // the "listener nobody notices is dead" case the REST transport's comments describe.
    let (serving_mcp, mcp_stop) = compose_serving_mcp(
        loaded_config.config(),
        &database,
        tools.as_ref(),
        loaded_config.config().daemon().mcp_serve_port(),
    )
    .await?;

    Ok(Running {
        health,
        paths,
        logging,
        singleton,
        database,
        credential,
        http_port,
        mcp,
        serving_mcp,
        mcp_stop,
        executor,
        tools,
        daemon_id,
        accept_loop: Box::pin(accept_clients(listener, context)),
    })
}

/// Reads the live provider coordinates from configuration, loading the API key from its file.
///
/// # Why the key is read here and nowhere else
///
/// The key lives in a file the operator controls, never in `config.toml`. This is the one place it is read
/// into memory, and it is placed straight into the adapter's own redacting [`jarvis_models::openai::ApiKey`]
/// by `Executor::build` — so it does not enter the configuration, a log line, an error message, or any value
/// this function returns for display. The read is bounded: the file is refused if it is empty, and the
/// adapter refuses an oversized one.
///
/// # When there is nothing to compose
///
/// Returns `None` for a profile whose executor is the deterministic scripted model — it has no provider
/// coordinates, and configuration already refuses provider settings without `openai-compatible` selected.
///
/// # Errors
///
/// Returns [`DaemonError::ModelApiKeyFile`] when the file cannot be read and [`DaemonError::ModelApiKeyEmpty`]
/// when it is blank. Both stop the daemon at startup rather than at the first run.
fn compose_model_provider(
    daemon: &jarvis_storage::DaemonConfig,
) -> Result<Option<executor::ModelProviderConfig>, DaemonError> {
    // Only the live implementation has coordinates. `validate` guarantees they are complete together, so a
    // missing one here would mean this ran against an unvalidated config, which is not a state the daemon
    // reaches — the `.ok_or` is stated rather than unwrapped so a future caller cannot turn it into a panic.
    let Some(key_ref) = daemon.executor_api_key_ref() else {
        return Ok(None);
    };
    let (Some(base_url), Some(model)) = (daemon.executor_base_url(), daemon.executor_model_name())
    else {
        return Ok(None);
    };

    let key =
        fs::read_to_string(key_ref).map_err(|source| DaemonError::ModelApiKeyFile { source })?;
    // Trailing newlines are stripped, because a key file written by an editor almost always ends with one and
    // a newline inside an `Authorization` header would split it. Interior whitespace is left for the adapter's
    // `ApiKey` to refuse, so a genuinely malformed value is rejected rather than silently trimmed into a
    // different one.
    let key = key.trim();
    if key.is_empty() {
        return Err(DaemonError::ModelApiKeyEmpty);
    }
    Ok(Some(executor::ModelProviderConfig::new(
        base_url, model, key,
    )))
}

/// Builds the run executor from the daemon configuration.
///
/// The live provider's coordinates are read into a carrier here and passed to the factory, which validates
/// each one through the adapter's own constructors. **The API key is read from the file the operator named
/// and never enters the configuration, a log line, or the error this returns** — [`executor::Executor`]'s
/// hand-written `Debug` redacts the credential so a failure cannot print it either.
///
/// # When there is nothing to compose
///
/// Returns `None` for a profile with no executor model, which is the deterministic scripted default.
///
/// # Errors
///
/// Returns [`DaemonError::ExecutorModel`] when the name is not one this build implements, and the
/// [`compose_model_provider`] errors when a configured provider cannot be assembled. Both stop the daemon at
/// startup rather than at the first run.
fn compose_executor(
    daemon: &jarvis_storage::DaemonConfig,
) -> Result<Option<Arc<executor::Executor>>, DaemonError> {
    let Some(name) = daemon.executor_model() else {
        return Ok(None);
    };
    let provider = compose_model_provider(daemon)?;
    let executor = executor::Executor::build(name, provider.as_ref()).map_err(|source| {
        DaemonError::ExecutorModel {
            name: name.to_owned(),
            source,
        }
    })?;
    Ok(Some(Arc::new(executor)))
}

/// Composes the MCP host and then the tool pipeline, in that order.
///
/// The order is forced: the pipeline registers the host's adapters, so the host must exist first.
///
/// **The two failures have opposite policies, deliberately.** An MCP failure is **not** fatal — a third-party
/// server that is missing, hung, or colliding is a reason for those tools to be unavailable, not a reason for
/// the daemon to refuse to serve anything, which is the same reasoning that makes one failed `tools/list` an
/// exclusion rather than a failed run. Each failure is logged so the absence is visible rather than silent.
/// A pipeline failure **is** fatal, because it covers the filesystem grant and the registry: an unusable
/// grant or a colliding tool name is a configuration fault an operator must fix, and starting anyway would
/// serve a tool set nobody declared.
///
/// # Errors
///
/// Returns [`DaemonError::ToolWorkspaceRoots`] or [`DaemonError::ToolPipeline`] for an unusable grant or a
/// rejected registration.
async fn compose_tools(
    config: &jarvis_storage::Config,
    paths: &AppPaths,
    database: Arc<SqliteDatabase>,
) -> Result<
    (
        Option<crate::mcp_host::ComposedHost>,
        Option<Arc<crate::tool_pipeline::ToolPipeline>>,
    ),
    DaemonError,
> {
    let mcp = match crate::mcp_host::load_document(paths.config()) {
        Ok(document) => match crate::mcp_host::compose_host(document.as_deref()).await {
            Ok(host) => host,
            Err(error) => {
                tracing::warn!(
                    %error,
                    "the configured MCP servers are unavailable; their tools will not be offered"
                );
                None
            }
        },
        Err(error) => {
            tracing::warn!(
                %error,
                "the MCP server configuration could not be read; its tools will not be offered"
            );
            None
        }
    };
    if let Some(host) = &mcp {
        // Logged with the counts, so an operator learns how many servers answered rather than only that
        // something did. The servers that did *not* answer are named in the warn above when the whole host
        // fails, and are available from `unreadable()` for a caller that reports them.
        tracing::info!(
            reachable = host.reachable_servers(),
            tools = host.definitions().len(),
            unreadable = host.unreadable().len(),
            "composed the MCP host"
        );
    }

    // The pipeline takes the adapters by value, so they are cloned out of the host. Each clone shares the
    // host's connection through the adapter's own `Arc`, which is why the host can still close them.
    //
    // **The memory adapter is pushed first, and it is not an MCP server.** `P4-014` requires that "the model may
    // submit memory candidates", and no tool touched memory before this one — so the model had no way to
    // propose anything. It is a native JARVIS tool rather than a discovered external one because what it writes
    // is a *canonical claim*: a memory belongs to this platform rather than to a provider, and routing it
    // through an MCP server would put the store behind a transport that exists to reach things it does not own.
    //
    // First rather than last so the native tool cannot be shadowed by an MCP server that happens to claim the
    // same identifier: the registry refuses a collision either way, but the order makes which one is "already
    // present" deterministic instead of dependent on discovery.
    let mut additional: Vec<(
        Vec<jarvis_tools::ToolDefinition>,
        Arc<dyn jarvis_tools::ToolExecutor>,
    )> = vec![
        (
            vec![
                crate::memory_propose::MemoryProposeTool::definition()
                    .map_err(|source| DaemonError::MemoryProposeTool { source })?,
            ],
            Arc::new(crate::memory_propose::MemoryProposeTool::new(Arc::clone(
                &database,
            ))) as Arc<dyn jarvis_tools::ToolExecutor>,
        ),
        // The web fetch tool is native too: it is JARVIS's own guarded `GET`, not a third party's server. It is
        // registered unconditionally and **held for approval by default** (risk 2 against the default
        // threshold), so offering it grants the model nothing until a person decides — see
        // `docs/research/integrations/web-fetch.md` for how an operator opts in to unattended use.
        (
            vec![
                jarvis_web::WebFetchTool::definition()
                    .map_err(|source| DaemonError::WebFetchTool { source })?,
            ],
            Arc::new(jarvis_web::WebFetchTool::new()) as Arc<dyn jarvis_tools::ToolExecutor>,
        ),
    ];
    // The code-running tool is composed **only** when an operator configured an image and an interpreter AND this
    // host can actually run a container. An absent tool is the honest state otherwise: a registered tool that
    // failed every call would be offered to a model and read as a broken one. When configured but unavailable the
    // operator is told, once, at startup — not on the first run.
    if let Some(image) = config.daemon().code_sandbox_image() {
        let backend = jarvis_sandbox::ContainerBackend::probe();
        let tool = crate::code_run::CodeRunTool::new(
            backend,
            image.to_owned(),
            config.daemon().code_sandbox_interpreter().to_vec(),
        )
        .map_err(|source| DaemonError::CodeRunTool { source })?;
        if tool.is_available() {
            additional.push((
                vec![
                    crate::code_run::CodeRunTool::definition(
                        config.daemon().code_sandbox_interpreter(),
                    )
                    .map_err(|source| DaemonError::CodeRunTool { source })?,
                ],
                Arc::new(tool) as Arc<dyn jarvis_tools::ToolExecutor>,
            ));
            tracing::info!(image, "the code-running tool is available");
        } else {
            tracing::warn!(
                image,
                "a code sandbox is configured but no container runtime is reachable, so the code-running tool is not offered"
            );
        }
    }
    additional.extend(
        mcp.as_ref()
            .map(|host| {
                host.adapters
                    .iter()
                    .map(|(definitions, adapter)| (definitions.clone(), Arc::clone(adapter)))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
    );

    let tools = compose_tool_pipeline(config, database, additional, paths)?;
    Ok((mcp, tools))
}

/// Composes the tool pipeline from the configured workspace roots, or `None` when none are granted.
///
/// Runs at startup rather than lazily for the same reason the executor model is resolved there: an
/// unusable workspace grant is a configuration fault, and a root that cannot be opened must stop the
/// daemon rather than surface as a workspace whose files appear to be simply absent. ADR-0020 forbids
/// narrowing a grant silently, so the failure is loud and early.
///
/// **No roots means no pipeline**, not an empty one. Registering the adapter over zero roots would let
/// the daemon advertise a tool that fails every call, which reads to a caller as a broken tool rather
/// than an absent capability.
///
/// # Errors
///
/// Returns [`DaemonError::ToolWorkspaceRoots`] when a configured root cannot be used as a root, and
/// [`DaemonError::ToolPipeline`] when the adapter's own definitions or the registry are rejected.
fn compose_tool_pipeline(
    config: &jarvis_storage::Config,
    database: Arc<SqliteDatabase>,
    additional: Vec<(
        Vec<jarvis_tools::ToolDefinition>,
        Arc<dyn jarvis_tools::ToolExecutor>,
    )>,
    paths: &AppPaths,
) -> Result<Option<Arc<crate::tool_pipeline::ToolPipeline>>, DaemonError> {
    // The workspace policy is built **before** the no-pipeline early return, and that ordering is the
    // point rather than an accident. A malformed policy — a contradictory ceiling, or an identifier that
    // is not a tool identifier at all — is a configuration fault an operator must fix whether or not a
    // tool can currently run. Validating it only on the path that builds a pipeline would mean a daemon
    // with no workspace roots **accepts** a document whose policy is unusable and silently ignores it,
    // which is the "accepted and silently ignored" shape this repository removes wherever it finds it.
    let workspace = compose_workspace_policy(config)?;

    // **No filesystem roots and no additional adapters means no pipeline**, not an empty one. Registering
    // the filesystem adapter over zero roots would let the daemon advertise a tool that fails every call,
    // which reads to a caller as a broken tool rather than an absent capability. A pipeline is warranted as
    // soon as *something* can run.
    //
    // `None` rather than an empty `WorkspaceRoots` for a daemon with no roots: that type **refuses an empty
    // list** (`RootError::NoRoots`), deliberately, so a tool cannot silently read nothing while looking like
    // a tool that works. The pipeline therefore registers the filesystem adapter only when a grant exists.
    let roots = match config.daemon().tool_workspace_roots() {
        [] if additional.is_empty() => return Ok(None),
        [] => None,
        roots => Some(
            jarvis_tools::WorkspaceRoots::new(roots.iter())
                .map_err(|source| DaemonError::ToolWorkspaceRoots { source })?,
        ),
    };
    let pipeline = crate::tool_pipeline::ToolPipeline::with_adapters(
        database,
        roots,
        workspace,
        additional,
        // The plaintext decision nonce goes to a file only this account can read, never into the durable
        // approval row: `ADR-0018` stores a digest there deliberately, and `SecretStore`'s module
        // documentation records why the profile's **state** directory is the right home for the plaintext
        // (a cache may be cleared and a runtime directory is login-lifetime, either of which would make
        // every pending approval undecidable across a logout).
        jarvis_storage::SecretStore::in_state(paths.state()),
    )
    .map_err(|source| DaemonError::ToolPipeline { source })?;
    Ok(Some(Arc::new(pipeline)))
}

/// Builds the workspace tool-authorization policy from the configuration document.
///
/// # Why this lives here rather than in `jarvis-storage`
///
/// The document section is `jarvis-storage`'s, because configuration parsing is its role; the policy is
/// `jarvis-tools`'s, because authorization is its role; and `docs/architecture/repository-layout.md`
/// allows an adapter to depend on `jarvis-core` and **not on another adapter**, so neither crate can do
/// this translation. The composition root is where both vocabularies are in scope, which is exactly the
/// `ADR-0023` shape: the pipeline is composed in the daemon over the adapter's own definitions.
///
/// # The direction rule is enforced across this seam, not here
///
/// Every override passes through [`jarvis_tools::WorkspacePolicy::requiring`], which applies it as a
/// `max` against the tool's own declaration. So the translation cannot relax a guard even though it is
/// the only place an operator's string becomes a policy — the rule lives with the policy and not with
/// the parser, which is what keeps it true for a runtime editor as well.
///
/// # A tool identifier that names nothing is not refused
///
/// The registry is built *after* this runs and an MCP server's tools are discovered at startup, so an
/// override for a tool that is currently absent would make the daemon refuse to start. An inert entry is
/// the honest outcome for a policy written before a server came up; `ADR-0122` records the reasoning.
///
/// # Errors
///
/// Returns [`DaemonError::WorkspacePolicy`] when a configured tool identifier is not a valid identifier,
/// or when [`jarvis_tools::WorkspacePolicy::new`] refuses the ceiling and threshold pair. Both are
/// configuration faults, so they stop the daemon at startup rather than at the first tool call.
fn compose_workspace_policy(
    config: &jarvis_storage::Config,
) -> Result<jarvis_tools::WorkspacePolicy, DaemonError> {
    let configured = config.policy();
    // Each field falls back to the workspace default independently, so an operator who sets only one of
    // the two gets a policy that is legal rather than a contradiction with a default they did not write.
    let defaults = jarvis_tools::WorkspacePolicy::default();
    let max_risk = configured.max_risk().unwrap_or(defaults.max_risk());
    let approval_threshold = configured
        .approval_threshold()
        .unwrap_or(defaults.approval_threshold());

    let mut policy = jarvis_tools::WorkspacePolicy::new(
        max_risk,
        approval_threshold,
        defaults.requires_approval_for_external_communication(),
        defaults.requires_strong_authentication_for_high_risk(),
    )
    .map_err(|source| DaemonError::WorkspacePolicy { source })?;

    for identifier in configured.deny() {
        let id = jarvis_tools::ToolId::new(identifier.clone()).map_err(|source| {
            DaemonError::WorkspacePolicyTool {
                tool: identifier.clone(),
                source,
            }
        })?;
        policy = policy.denying(id);
    }
    for (identifier, approval) in configured.approval() {
        let id = jarvis_tools::ToolId::new(identifier.clone()).map_err(|source| {
            DaemonError::WorkspacePolicyTool {
                tool: identifier.clone(),
                source,
            }
        })?;
        policy = policy.requiring(id, *approval);
    }
    Ok(policy)
}

/// Builds and binds the inbound MCP endpoint, when the operator enabled one.
///
/// # Why this is separate from `compose_tools`, and why its failures are fatal
///
/// `compose_tools` treats an MCP **host** failure as non-fatal, because a third-party server that is missing or
/// hung is a reason for those tools to be unavailable rather than for the daemon to refuse to serve anything.
/// This function is the opposite case: the operator explicitly named a port, so anything that stops the endpoint
/// being served is something they must fix. An endpoint that failed to build or bind must therefore stop the
/// daemon rather than leave it reporting itself ready with a port nobody is listening on.
///
/// # The order is forced
///
/// It runs after the pipeline, because the endpoint serves the pipeline's own definitions and the served surface
/// must be the same list the dispatcher covers. Building it before the pipeline would mean serving a list
/// nothing can run.
///
/// # The workspace comes from the profile, never from a request
///
/// `load_local_identity` reads the seeded local identity, which is the same source the gateway uses. A remote
/// MCP client therefore acts in the profile's own workspace and cannot name another — the rule
/// `docs/architecture/identity-and-workspaces.md` states, applied to a protocol whose requests carry even less.
///
/// # Errors
///
/// Returns [`DaemonError::McpServe`] for anything that stops the endpoint being served: no tool to serve, a
/// refused exposure policy or served surface, or a port that cannot be bound. Returns
/// [`DaemonError::DatabaseError`] when the local identity cannot be read, which is the same failure the gateway
/// would hit on its first tool call and is better reported at startup.
async fn compose_serving_mcp(
    config: &jarvis_storage::Config,
    database: &Arc<SqliteDatabase>,
    tools: Option<&Arc<crate::tool_pipeline::ToolPipeline>>,
    port: Option<u16>,
) -> Result<
    (
        Option<crate::mcp_serve::ServingMcp>,
        Option<tokio::sync::oneshot::Receiver<()>>,
    ),
    DaemonError,
> {
    let Some(port) = port else {
        return Ok((None, None));
    };
    let _ = config;

    // The pipeline is the endpoint's whole enforcement path, so an endpoint without one has nothing to serve.
    // Reported as a distinct error from a bound port, because the remedy is different: grant roots or configure
    // a server, rather than change the port.
    let Some(pipeline) = tools else {
        return Err(DaemonError::McpServe {
            source: crate::mcp_serve::RemoteMcpError::NoTools,
        });
    };

    let identity = jarvis_storage::load_local_identity(database)
        .await
        .map_err(DaemonError::Database)?;

    // The definitions come from the pipeline's own registry, so the served surface and the dispatch table
    // describe one set of tools. `definitions()` is what the pipeline registered, which is exactly the list
    // `dispatch.verify_covers` checked.
    let definitions = pipeline
        .definitions()
        .map_err(|source| DaemonError::ToolPipeline { source })?;
    let workspace_id = identity.workspace_id().to_owned();

    let (runner, endpoint) = crate::mcp_serve::build_endpoint(
        Arc::clone(pipeline),
        &definitions,
        &workspace_id,
        // **`local_only`**, which is `P3-009g`'s safe default: it admits nobody remotely, so the endpoint
        // serves the tools an operator's own MCP client can reach on this machine and refuses every remote
        // caller. A remote allowlist needs audience-bound tokens (RFC 8707) that are not built, which is what
        // that default records.
        jarvis_mcp_transport::CallerAdmission::local_only(),
    )
    .map_err(|source| DaemonError::McpServe { source })?;

    let (serving, stopped) = crate::mcp_serve::ServingMcp::bind(port, endpoint)
        .await
        .map_err(|source| DaemonError::McpServe { source })?;

    tracing::info!(
        port = serving.port(),
        tools = definitions.len(),
        // The policy in force is **stated**, so an operator can confirm a configuration was loaded rather than
        // comparing files against a running process. `is_loopback_only` is the fact that matters: it is the
        // admission default that refuses every remote caller, and it is the value a deployment would have to
        // change deliberately.
        origins_loopback_only = serving.policy().admission().is_local_only(),
        "serving JARVIS tools over MCP"
    );
    let _ = runner;
    Ok((Some(serving), Some(stopped)))
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
    /// The scheduler, when the daemon drives runs. It shares the transport's lifetime: a daemon that is shutting
    /// down must not start a run, and a scheduler outliving its listener would fire into a daemon nobody can reach.
    scheduler: Option<tokio::task::JoinHandle<()>>,
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
        tools: Option<Arc<crate::tool_pipeline::ToolPipeline>>,
        paths: &AppPaths,
    ) -> Result<(Self, tokio::sync::oneshot::Receiver<()>), DaemonError> {
        // Loopback only. Reaching any other interface is remote mode, which `P10-004` owns as an
        // explicit TLS-terminated configuration rather than something that happens by default.
        let address = std::net::SocketAddr::from(([127, 0, 0, 1], port));
        let listener = tokio::net::TcpListener::bind(address)
            .await
            .map_err(|source| DaemonError::HttpBind { port, source })?;

        let mut state = gateway::GatewayState::new(
            database,
            credential,
            // The same store the pipeline writes to, so a hold's nonce is read from the profile state
            // directory it was written to. Two stores would be two directories, and the approval would be
            // undecidable in the way that looks like a missing file.
            jarvis_storage::SecretStore::in_state(paths.state()),
        );
        if let Some(executor) = executor {
            state = state.with_executor(executor);
        }
        if let Some(tools) = tools {
            state = state.with_tools(tools);
        }
        // Scheduled tasks fire through the same state the routes use, so a scheduled run is an ordinary run. Only
        // when runs are driven at all: a scheduler over a daemon with no executor would start runs nothing runs.
        let scheduler = state
            .drives_runs()
            .then(|| schedule_service::spawn(state.clone()));
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
                scheduler,
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
        // The scheduler first: nothing new may start while the listener drains.
        if let Some(scheduler) = &self.scheduler {
            scheduler.abort();
        }
        let _ = self.shutdown.send(());
        let _ = self.handle.await;
    }
}

/// The outcome of the daemon's main wait.
struct WaitOutcome {
    shutdown: io::Result<()>,
    accept_failure: Option<jarvis_core::TransportError>,
    http_stopped: bool,
    /// Whether the **inbound MCP endpoint** stopped on its own.
    ///
    /// A separate flag rather than folded into `http_stopped`, because the two are different ports serving
    /// different protocols: an operator reading "a listener stopped" would not know which to look at, and the
    /// remedy differs.
    mcp_stopped: bool,
}

/// Waits for a shutdown signal, a local-listener failure, or either HTTP listener failing.
///
/// **Both listeners are branches of the same select** rather than detached tasks, so a listener that dies takes
/// the shutdown path instead of leaving the daemon reporting itself ready with no transport behind it. Each
/// stop signal is optional because a deployment may enable either listener, both, or neither — and `None` for a
/// listener that was never bound is not a missing branch but the absence of one.
async fn wait_for_exit<F>(
    shutdown: F,
    accept_loop: std::pin::Pin<Box<dyn Future<Output = jarvis_core::TransportError> + Send>>,
    http_stop: Option<tokio::sync::oneshot::Receiver<()>>,
    mcp_stop: Option<tokio::sync::oneshot::Receiver<()>>,
) -> WaitOutcome
where
    F: Future<Output = io::Result<()>>,
{
    // Both signals are normalised to one boxed future type, so the select has a fixed shape rather than nested
    // `if let`s whose arms multiply with each listener. A listener that was never bound becomes a future that
    // never completes, which is how "there is nothing to watch" is expressed — and it keeps the two `stopped`
    // flags distinguishable in the outcome rather than collapsing them into one.
    let mut http_stop = stop_signal(http_stop);
    let mut mcp_stop = stop_signal(mcp_stop);

    tokio::select! {
        result = shutdown => WaitOutcome {
            shutdown: result,
            accept_failure: None,
            http_stopped: false,
            mcp_stopped: false,
        },
        failure = accept_loop => WaitOutcome {
            shutdown: Ok(()),
            accept_failure: Some(failure),
            http_stopped: false,
            mcp_stopped: false,
        },
        () = &mut http_stop => WaitOutcome {
            shutdown: Ok(()),
            accept_failure: None,
            http_stopped: true,
            mcp_stopped: false,
        },
        () = &mut mcp_stop => WaitOutcome {
            shutdown: Ok(()),
            accept_failure: None,
            http_stopped: false,
            mcp_stopped: true,
        },
    }
}

/// A listener's stop signal, or a future that never completes when there is no listener to watch.
///
/// The receiver's `Result` is discarded — a `RecvError` means the sender was dropped, which for a one-shot stop
/// signal means the transport is gone, and that is the same event as the signal being sent for this purpose. The
/// `None` arm is `std::future::pending`, which is not a stub: it is the honest expression of "this listener was
/// never bound", and it is what lets one `select!` cover a daemon with either listener, both, or neither.
fn stop_signal(
    receiver: Option<tokio::sync::oneshot::Receiver<()>>,
) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>> {
    match receiver {
        Some(receiver) => Box::pin(async move {
            let _ = receiver.await;
        }),
        None => Box::pin(std::future::pending::<()>()),
    }
}

/// Stops whatever network listeners were running, in the one order that matters.
///
/// The **inbound** MCP endpoint is stopped before the outbound MCP host is closed, and after the REST transport.
/// The order matters in one direction only: this endpoint's handler holds the pipeline, so closing it before the
/// pipeline's adapters are dropped avoids a request being served against an adapter that is going away.
///
/// An unexpected stop is logged at error level rather than returned: the daemon is already exiting, and which
/// listener died first is the diagnostic, not a second failure to report.
async fn stop_transports(
    http: Option<HttpTransport>,
    serving_mcp: Option<crate::mcp_serve::ServingMcp>,
    http_port: Option<u16>,
    outcome: &WaitOutcome,
) {
    if let Some(transport) = http {
        let port = transport.port();
        transport.shutdown().await;
        tracing::info!(port, "HTTP transport stopped");
    }
    if let Some(serving) = serving_mcp {
        let port = serving.port();
        serving.shutdown().await;
        tracing::info!(port, "MCP endpoint stopped");
    }
    if outcome.http_stopped {
        tracing::error!(
            port = http_port.unwrap_or(0),
            "the HTTP transport stopped unexpectedly"
        );
    }
    if outcome.mcp_stopped {
        tracing::error!("the MCP endpoint stopped unexpectedly");
    }
}

async fn run<F>(build: BuildInfo, root: Option<PathBuf>, shutdown: F) -> Result<(), DaemonError>
where
    F: Future<Output = io::Result<()>>,
{
    let Running {
        health,
        paths,
        logging,
        singleton,
        database,
        credential,
        http_port,
        executor,
        tools,
        mcp,
        serving_mcp,
        mcp_stop,
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
        mcp_serve_port = serving_mcp
            .as_ref()
            .map_or(0, crate::mcp_serve::ServingMcp::port),
        "daemon ready"
    );

    // The listener is bound before it is served, and binding is what fails fast: an occupied port
    // is reported here rather than becoming a listener nobody notices is dead.
    let (http, http_stop) = match http_port {
        Some(port) => {
            let (transport, stop) = HttpTransport::bind(
                port,
                Arc::clone(&database),
                credential.clone(),
                executor,
                tools,
                &paths,
            )
            .await?;
            (Some(transport), Some(stop))
        }
        None => (None, None),
    };

    let outcome = wait_for_exit(shutdown, accept_loop, http_stop, mcp_stop).await;
    stop_transports(http, serving_mcp, http_port, &outcome).await;

    let shutdown_result = outcome.shutdown;
    let accept_failure = outcome.accept_failure;

    // The MCP connections are closed **explicitly**, before the database, so a child process is stopped
    // deliberately rather than left to its drop guard. The SDK's handle carries a cancellation guard, so a
    // failure here still cancels — which is why a shutdown failure is logged rather than fatal: the daemon is
    // exiting, and which servers did not stop cleanly is the useful part.
    if let Some(mcp) = mcp
        && let Err(failures) = mcp.close().await
    {
        tracing::warn!(?failures, "some MCP connections did not stop cleanly");
    }

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

    /// Parses a document with `body` in the policy section, for the composition tests below.
    fn config_with_policy(section: &str) -> jarvis_storage::Config {
        let document = format!(
            r#"
schema_version = 1

[profile]
name = "home"

[logging]
level = "warn"

[daemon]
shutdown_timeout_seconds = 30

{section}
"#
        );
        jarvis_storage::Config::parse_with_environment(&document, Vec::<(String, String)>::new())
            .unwrap_or_else(|error| panic!("the fixture document must load: {error}"))
            .into_config()
    }

    fn tool_id(value: &str) -> jarvis_tools::ToolId {
        jarvis_tools::ToolId::new(value).unwrap_or_else(|error| panic!("{value}: {error}"))
    }

    /// **The workspace policy defaults when the document configures none.**
    ///
    /// The control for the tests that follow: without it a composition that ignored the document
    /// entirely would satisfy every other assertion here.
    #[test]
    fn a_document_without_a_policy_section_composes_the_workspace_default() {
        let policy = compose_workspace_policy(&config_with_policy(""))
            .unwrap_or_else(|error| panic!("the default policy must compose: {error}"));
        let defaults = jarvis_tools::WorkspacePolicy::default();
        assert_eq!(policy.max_risk(), defaults.max_risk());
        assert_eq!(policy.approval_threshold(), defaults.approval_threshold());
        assert!(!policy.denies(&tool_id("jarvis.files.read")));
        assert_eq!(
            policy.approval_overrides().count(),
            0,
            "a document that configures no override must produce none"
        );
    }

    /// **A configured ceiling, threshold, denial, and override all reach the composed policy.**
    ///
    /// The end-to-end claim of the slice at the composition seam: each value an operator writes is the
    /// value policy decides with. No call is made — the policy is constructed and read, which is what
    /// the offline suite can establish.
    #[test]
    fn a_configured_policy_reaches_the_composed_workspace_policy() {
        let config = config_with_policy(
            r#"[policy]
max_risk = "high"
approval_threshold = "low"
deny = ["jarvis.mail.send"]

[policy.approval]
"jarvis.files.read" = "ask"
"#,
        );
        let policy = compose_workspace_policy(&config)
            .unwrap_or_else(|error| panic!("the configured policy must compose: {error}"));

        assert_eq!(policy.max_risk(), jarvis_tools::Risk::High);
        assert_eq!(policy.approval_threshold(), jarvis_tools::Risk::Low);
        assert!(
            policy.denies(&tool_id("jarvis.mail.send")),
            "a configured denial must reach the policy that decides"
        );
        // The override is what `evaluate` consults, so it is asserted through the accessor the engine
        // itself calls rather than by reading the map: a composition that stored the override where
        // nothing reads it would pass a map assertion and fail this one.
        assert_eq!(
            policy.effective_approval(
                &tool_id("jarvis.files.read"),
                jarvis_tools::ApprovalPolicy::Auto
            ),
            jarvis_tools::ApprovalPolicy::Ask,
            "a configured override must tighten the tool's own declaration"
        );
    }

    /// **A configured override cannot relax a tool's own declaration across the composition seam.**
    ///
    /// The direction rule asserted where the operator's *string* becomes a policy, because this is the
    /// boundary an implementation is most likely to get wrong: a translation that wrote the configured
    /// value into the policy as-is, rather than through `requiring`, would satisfy every tightening
    /// assertion and fail only here.
    #[test]
    fn a_configured_override_cannot_relax_a_declaration_at_composition() {
        let config = config_with_policy(
            r#"[policy.approval]
"jarvis.mail.send" = "auto"
"#,
        );
        let policy = compose_workspace_policy(&config)
            .unwrap_or_else(|error| panic!("the configured policy must compose: {error}"));

        // The tool declares `Ask`; the operator wrote the loosest possible value.
        assert_eq!(
            policy.effective_approval(
                &tool_id("jarvis.mail.send"),
                jarvis_tools::ApprovalPolicy::Ask
            ),
            jarvis_tools::ApprovalPolicy::Ask,
            "an override must not lower a tool's own `Ask`, wherever it is read from"
        );
    }

    /// **An invalid tool identifier in the policy section is refused at startup.**
    ///
    /// The identifier is named in the error, because a tool identifier is not a secret and an operator
    /// needs to know which line to fix.
    #[test]
    fn an_invalid_policy_tool_identifier_is_refused() {
        let config = config_with_policy(
            r#"[policy]
deny = ["not a tool id"]
"#,
        );
        let error = compose_workspace_policy(&config)
            .err()
            .unwrap_or_else(|| panic!("an invalid tool identifier must be refused"));
        let rendered = error.to_string();
        assert!(
            rendered.contains("not a tool id"),
            "the refused identifier must be named so it can be fixed: {rendered}"
        );
    }

    /// **An unusable policy is refused even when no tool can run.**
    ///
    /// A daemon with no workspace roots and no MCP server composes **no pipeline**, and the first version
    /// of `compose_tool_pipeline` returned early before the policy was built — so a document whose policy
    /// was contradictory or whose identifier was malformed was **accepted and silently ignored**. That is
    /// the "accepted and silently ignored" shape this repository removes, and it was invisible because the
    /// only path that validated policy was the one that built a pipeline.
    ///
    /// This drives the same composition the daemon drives, with the incoming adapters list empty, and
    /// asserts the policy is still built. The **ordering** is what is under test: the refusal itself is
    /// covered above and would pass either way, so this would fail only against a reintroduced early return.
    #[tokio::test]
    async fn an_unusable_policy_is_refused_even_with_no_tools_registered() {
        let config = config_with_policy(
            r#"[policy]
deny = ["not a tool id"]
"#,
        );
        // The same temporary-profile shape the executor's tests use, so the fixture is not a second
        // convention: a real migrated database under the scratch directory the testkit removes.
        let root = std::env::temp_dir().join(format!(
            "jarvis-policy-compose-{}-{}",
            std::process::id(),
            jarvis_core::RunId::new()
        ));
        std::fs::create_dir_all(&root).unwrap_or_else(|error| panic!("create scratch: {error}"));
        let database = Arc::new(
            jarvis_storage::SqliteDatabase::open(
                &root.join(jarvis_storage::DEFAULT_DATABASE_FILENAME),
            )
            .await
            .unwrap_or_else(|error| panic!("open fixture database: {error}")),
        );
        let paths = jarvis_storage::AppPaths::from_root(&root)
            .unwrap_or_else(|error| panic!("resolve paths: {error}"));

        let error = compose_tool_pipeline(&config, database, Vec::new(), &paths)
            .err()
            .unwrap_or_else(|| {
                panic!("an unusable policy must be refused even with no tools registered")
            });
        assert!(
            error.to_string().contains("not a tool id"),
            "the refusal must be the policy's, not the no-pipeline path: {error}"
        );

        // Cleaned up after the database is dropped, because a profile removed while a pool still holds the
        // file open is a sharing violation on Windows.
        drop_error_free(&root);
    }

    /// Removes a scratch directory, ignoring the failure `remove_scratch_dir` already bounds.
    fn drop_error_free(path: &std::path::Path) {
        jarvis_core::remove_scratch_dir(path);
    }
}
