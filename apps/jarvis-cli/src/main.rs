//! Thin command-line client for the JARVIS daemon.
//!
//! The client never touches the database or a provider directly. It locates the
//! daemon's native local endpoint, presents the profile credential the daemon
//! issued, and renders the versioned protocol reply as human text or stable JSON.
//!
//! The client only ever reads the profile credential. Minting one here would let
//! an unprivileged process choose the secret the daemon trusts, so a missing
//! credential is an actionable daemon-not-started error, not something to fix by
//! generating a value.

mod api_client;
mod approvals;
mod cancel;
mod chat;
mod checks;
mod connector;
mod contacts;
mod digest;
mod entity;
mod eval;
mod hud;
mod init;
mod install;
mod lifecycle;
mod memory;
mod output;
mod project;
mod push;
mod schedule;
mod service_install;
mod settings;
mod shell_path;
mod skills;
mod tools;
mod watch;

use std::{io, path::PathBuf, process::ExitCode};

use jarvis_core::{ClientCredential, LocalEndpoint, LoopbackHost, connect};
use jarvis_diagnostics::{ServiceKind, ServicePlan, detect_drift, drift_finding};
use jarvis_observability::{DEFAULT_TAIL_LINES, read_tail};
use jarvis_protocol::{
    ClientContext, ClientKind, ClientSession, Command, HealthReply, Reply, SessionError,
    StatusReply,
};
use jarvis_storage::{
    AppPaths, ConfigStore, CredentialStore, CredentialStoreError, portable_layout,
};
use output::{ExitStatus, Fields};

/// Filename of the credential file inside the configuration directory.
const CREDENTIAL_FILE_NAME: &str = "client.credential";
/// Filename of the daemon log inside the log directory.
const LOG_FILE_NAME: &str = "jarvisd.jsonl";

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    // The daemon is this same executable (`ADR-0144`), and it brings its own multi-threaded runtime.
    if arguments.first().map(String::as_str) == Some("daemon") {
        return jarvisd::run_blocking(arguments.into_iter().skip(1).collect());
    }
    match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime.block_on(client_main(arguments)),
        Err(error) => {
            eprintln!("jarvis: could not start the async runtime: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn client_main(arguments: Vec<String>) -> ExitCode {
    let status = match arguments.first().map(String::as_str) {
        Some("--version" | "-V" | "version") => {
            println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
            ExitStatus::Ok
        }
        Some("status") => run(Command::Status, &arguments).await,
        Some("health") => run(Command::Health, &arguments).await,
        Some("ask") => ask(&arguments).await,
        Some("chat") => chat(&arguments).await,
        Some("logs") => logs(&arguments),
        Some("memory") => memory_command(&arguments).await,
        // `entity` is the `P4-016` surface: the subject a memory is about. Before it, every claim needed an
        // entity identifier and nothing could produce one — which is what `P4-008` recorded as "a remember is
        // still unreachable by a user of the shipped product".
        Some("entity") => entity_command(&arguments).await,
        Some("tools") => tools_command(&arguments).await,
        // `approvals` is how a person releases or refuses an action a tool call was held on: without it a held
        // call parked its run and nothing in the shipped product could finish the conversation.
        // `init` and `start` are first-run: a configuration written for you, and a daemon started for you.
        Some("init") => match resolve_paths(&arguments) {
            Ok(paths) => init::init(&paths, &arguments).await,
            Err(status) => status,
        },
        Some("start") => match resolve_paths(&arguments) {
            Ok(paths) => start_and_open(&paths, &arguments),
            Err(status) => status,
        },
        Some("config") => match resolve_paths(&arguments) {
            Ok(paths) => settings::config(&paths, &arguments),
            Err(status) => status,
        },
        Some("keys") => match resolve_paths(&arguments) {
            Ok(paths) => {
                // Only `keys test voice` needs the daemon's credential, so it is not loaded (and complained about) otherwise.
                let credential = arguments
                    .windows(2)
                    .any(|pair| pair[0] == "test" && pair[1] == "voice")
                    .then(|| load_credential(&paths).ok().map(|c| c.expose().to_owned()))
                    .flatten();
                settings::keys(&paths, &arguments, credential).await
            }
            Err(status) => status,
        },
        Some("stop") => stop_command(&arguments, false).await,
        Some("restart") => stop_command(&arguments, true).await,
        Some("cancel") => cancel_command(&arguments).await,
        Some("watch") => watch_command(&arguments).await,
        Some("hud") => hud_command(&arguments),
        Some("approvals") => approvals_command(&arguments).await,
        // `schedule` and `runs` are the proactive half of the product: tasks that run while you are away, and
        // where you read what they said.
        Some("schedule") => schedule_command(&arguments, false).await,
        Some("runs") => schedule_command(&arguments, true).await,
        // Projects: the goal, guidance, folder and journal of long-running work (`ADR-0151`).
        Some("project") => project_command(&arguments).await,
        Some("digest") => digest_command(&arguments).await,
        // Run a suite of prompts against the daemon and score the answers, so a change can be measured (`ADR-0166`).
        Some("eval") => eval_command(&arguments).await,
        Some("contacts") => contacts_command(&arguments).await,
        Some("push") => push_command(&arguments).await,
        // `skills` is the `P4-013` inspection and control surface: the `FR-MEM-005` lifecycle applied to a
        // stored procedure, which before this verb group was reachable only from a test.
        Some("skills") => skills_command(&arguments).await,
        Some("connector") => connector::run(&arguments),
        Some("doctor") => doctor(&arguments).await,
        Some("service") => service(&arguments),
        // `path` puts `jarvis` on the search path, so the commands JARVIS suggests work from any terminal.
        Some("path") => shell_path::run(&arguments),
        // `install` puts the one program in a per-user folder and on the path; `uninstall` takes it back out (`ADR-0147`).
        Some("install") => install::install(&arguments),
        Some("uninstall") => install::uninstall(&arguments),
        // No arguments (or `launch`) is the one command that gets a person to a working console: set up on the first
        // run, then start, then open. Without a terminal to ask questions on it still just explains itself.
        Some("--help" | "-h" | "help") => {
            println!("{}", usage());
            ExitStatus::Ok
        }
        // Flags alone (`jarvis --root DIR`, `jarvis --no-open`) are a launch with options.
        Some("launch") | None => launch_command(&arguments).await,
        Some(flag) if flag.starts_with("--") => launch_command(&arguments).await,
        Some(other) => {
            eprintln!("jarvis: unknown command {other:?}");
            eprintln!("{}", usage());
            ExitStatus::Usage
        }
    };
    status.into()
}

/// Starts the daemon and opens the console, the way launching an app opens its window.
///
/// `--no-open` skips the window. A browser that cannot be opened is reported by `hud_command` and does not make a
/// started daemon a failure.
fn start_and_open(paths: &AppPaths, arguments: &[String]) -> ExitStatus {
    let status = init::start(paths, arguments);
    if status == ExitStatus::Ok
        && let Some(tip) = shell_path::hint()
    {
        println!("{tip}");
    }
    if status == ExitStatus::Ok && !arguments.iter().any(|argument| argument == "--no-open") {
        let _ = hud_command(arguments);
    }
    status
}

/// `jarvis` or `jarvis launch`: set up if this is the first run, then start and open the console.
async fn launch_command(arguments: &[String]) -> ExitStatus {
    let paths = match resolve_paths(arguments) {
        Ok(paths) => paths,
        Err(status) => return status,
    };
    if !init::is_configured(&paths) {
        if !init::can_ask() {
            eprintln!(
                "jarvis: not set up yet, and there is no terminal to ask questions on; run `jarvis init` from a terminal"
            );
            eprintln!("{}", usage());
            return ExitStatus::Usage;
        }
        println!("Welcome to JARVIS. A few questions, then it starts and opens in your browser.");
        let status = init::init(&paths, &[]).await;
        if status != ExitStatus::Ok {
            return status;
        }
    }
    start_and_open(&paths, arguments)
}

const fn usage() -> &'static str {
    "usage: jarvis [launch] | jarvis <init|start|stop|restart|config|keys|service|status|health|ask|chat|logs|memory|tools|approvals|cancel|watch|hud|schedule|runs|project|digest|eval|contacts|push|skills|connector|doctor|path|version> [--json] [--lines N] [--repair] [--root DIR]\n       jarvis ask <objective...> [--root DIR]\n       jarvis chat [--root DIR]\n       jarvis start [--no-open] [--root DIR]\n       jarvis memory <list|show|search|remember|correct|confirm|forget|export> [...]\n       jarvis tools <list|preview> [...]\n       jarvis approvals <list|approve|deny|resume> [...]
       jarvis schedule <add|list|pause|resume|remove> [...]
       jarvis project <add|list|show|set|pause|resume|done|note|remove> [...]   # long-running work with its own goal, guidance and journal; `jarvis ask --project NAME ...`
       jarvis digest [HOURS] [--json]   # what JARVIS did while you were away: runs, outcomes, project decisions, failures, tokens
       jarvis eval <run|history> [...]  # score a suite of prompts against the daemon and compare with the last run: `jarvis eval run evals/basics.toml`
       jarvis contacts <list|stats|add|remove|export> [...]   # the people and companies JARVIS is working with; `export` writes a CSV
       jarvis push test                 # send one test message to your ntfy topic (daemon.push_topic)
       jarvis runs [list] [--limit N] [--full]\n       jarvis skills <list|show|create|promote|disable|enable|forget|export> [...]\n       jarvis connector <new|check|items> [...]\n       jarvis path <install|uninstall|status>      # make `jarvis` work from any terminal\n       jarvis stop|restart [--wait] [--force]      # a stop that would interrupt working tasks is refused unless --force (or waits with --wait)\n       jarvis install [--service] [--dir DIR]      # install this one program for the current user (and start it at login)\n       jarvis uninstall                            # remove it again (your settings and memory stay)"
}

/// Runs one `jarvis memory` verb against the daemon's HTTP API.
///
/// Shares `run_client` with `ask` and `chat`, because the memory surface lives on the same HTTP API and
/// needs the same four facts. A separate client builder here would be the second place a configured port is
/// read, which is how two commands come to target different ports.
async fn memory_command(arguments: &[String]) -> ExitStatus {
    let root = match requested_root(arguments) {
        Ok(Some(root)) => vec!["--root".to_owned(), root.display().to_string()],
        Ok(None) => Vec::new(),
        Err(status) => return status,
    };
    let client = match run_client(&root) {
        Ok(client) => client,
        Err(status) => return status,
    };
    memory::run(&client, arguments).await
}

/// Runs one `jarvis entity` verb over the daemon's HTTP API.
///
/// Shares `run_client` with `memory_command` and `tools_command`, for the reason that function's own comment
/// gives: the entity surface lives on the same API and needs the same four facts, and a second client builder
/// would be a second place a configured port is read — which is how two commands come to target different
/// ports.
async fn entity_command(arguments: &[String]) -> ExitStatus {
    let root = match requested_root(arguments) {
        Ok(Some(root)) => vec!["--root".to_owned(), root.display().to_string()],
        Ok(None) => Vec::new(),
        Err(status) => return status,
    };
    let client = match run_client(&root) {
        Ok(client) => client,
        Err(status) => return status,
    };
    entity::run(&client, arguments).await
}

/// Runs one `jarvis tools` verb over the daemon's HTTP API.
///
/// Shares `run_client` with `memory_command`, `ask`, and `chat`, for the reason that function's own comment
/// gives: the tool surface lives on the same API and needs the same four facts, and a second client builder
/// would be a second place a configured port is read — which is how two commands come to target different
/// ports.
async fn tools_command(arguments: &[String]) -> ExitStatus {
    let root = match requested_root(arguments) {
        Ok(Some(root)) => vec!["--root".to_owned(), root.display().to_string()],
        Ok(None) => Vec::new(),
        Err(status) => return status,
    };
    let client = match run_client(&root) {
        Ok(client) => client,
        Err(status) => return status,
    };
    tools::run(&client, arguments).await
}

/// Runs one `jarvis project` verb over the daemon's HTTP API.
async fn project_command(arguments: &[String]) -> ExitStatus {
    let root = match requested_root(arguments) {
        Ok(Some(root)) => vec!["--root".to_owned(), root.display().to_string()],
        Ok(None) => Vec::new(),
        Err(status) => return status,
    };
    let client = match run_client(&root) {
        Ok(client) => client,
        Err(status) => return status,
    };
    project::run_project(&client, arguments).await
}

/// Runs `jarvis digest` over the daemon's HTTP API.
async fn digest_command(arguments: &[String]) -> ExitStatus {
    let root = match requested_root(arguments) {
        Ok(Some(root)) => vec!["--root".to_owned(), root.display().to_string()],
        Ok(None) => Vec::new(),
        Err(status) => return status,
    };
    let client = match run_client(&root) {
        Ok(client) => client,
        Err(status) => return status,
    };
    digest::run_digest(&client, arguments).await
}

/// Runs one `jarvis eval` verb: the suite is a file, the answers come from the daemon, the history lives in the data directory.
async fn eval_command(arguments: &[String]) -> ExitStatus {
    let root = match requested_root(arguments) {
        Ok(Some(root)) => vec!["--root".to_owned(), root.display().to_string()],
        Ok(None) => Vec::new(),
        Err(status) => return status,
    };
    let paths = match resolve_paths(arguments) {
        Ok(paths) => paths,
        Err(status) => return status,
    };
    let client = match run_client(&root) {
        Ok(client) => client,
        Err(status) => return status,
    };
    eval::run_eval(&client, &paths, arguments).await
}

/// Runs one `jarvis contacts` verb over the daemon API.
async fn contacts_command(arguments: &[String]) -> ExitStatus {
    let root = match requested_root(arguments) {
        Ok(Some(root)) => vec!["--root".to_owned(), root.display().to_string()],
        Ok(None) => Vec::new(),
        Err(status) => return status,
    };
    let client = match run_client(&root) {
        Ok(client) => client,
        Err(status) => return status,
    };
    contacts::run_contacts(&client, arguments).await
}

/// Runs `jarvis push test` over the daemon API.
async fn push_command(arguments: &[String]) -> ExitStatus {
    let root = match requested_root(arguments) {
        Ok(Some(root)) => vec!["--root".to_owned(), root.display().to_string()],
        Ok(None) => Vec::new(),
        Err(status) => return status,
    };
    let client = match run_client(&root) {
        Ok(client) => client,
        Err(status) => return status,
    };
    push::run_push(&client, arguments).await
}

/// Takes `--project NAME` out of the arguments, so `ask` and `chat` can start runs inside a project.
fn take_project(arguments: &[String]) -> (Vec<String>, Option<String>) {
    let mut rest = Vec::with_capacity(arguments.len());
    let mut project = None;
    let mut index = 0;
    while index < arguments.len() {
        if arguments[index] == "--project" && index + 1 < arguments.len() {
            project = Some(arguments[index + 1].clone());
            index += 2;
            continue;
        }
        rest.push(arguments[index].clone());
        index += 1;
    }
    (rest, project)
}

/// Runs one `jarvis schedule` or `jarvis runs` verb over the daemon's HTTP API.
async fn schedule_command(arguments: &[String], runs: bool) -> ExitStatus {
    let root = match requested_root(arguments) {
        Ok(Some(root)) => vec!["--root".to_owned(), root.display().to_string()],
        Ok(None) => Vec::new(),
        Err(status) => return status,
    };
    let client = match run_client(&root) {
        Ok(client) => client,
        Err(status) => return status,
    };
    if runs {
        schedule::run_runs(&client, arguments).await
    } else {
        schedule::run_schedule(&client, arguments).await
    }
}

/// `jarvis stop` ends the background daemon gracefully; `jarvis restart` stops it (if running) and starts it again.
async fn stop_command(arguments: &[String], restart: bool) -> ExitStatus {
    let paths = match resolve_paths(arguments) {
        Ok(paths) => paths,
        Err(status) => return status,
    };
    let loaded = match ConfigStore::from_paths(&paths).load() {
        Ok(loaded) => loaded,
        Err(error) => return fail("configuration", &error),
    };
    let port = loaded.config().daemon().http_port();
    let credential = match load_credential(&paths) {
        Ok(credential) => credential,
        // Never started, so there is nothing running to stop.
        Err(_) if !restart => {
            println!("not running");
            return ExitStatus::Ok;
        }
        Err(status) => return status,
    };
    let force = arguments.iter().any(|argument| argument == "--force");
    let wait = arguments.iter().any(|argument| argument == "--wait");
    // A stop never silently throws away work: tasks the daemon is driving are named, and the daemon is left running unless the
    // caller says `--force` (interrupt them) or `--wait` (go ahead once they have finished, for up to an hour).
    let patience = std::time::Instant::now() + std::time::Duration::from_secs(3600);
    let mut last_told = None::<std::time::Instant>;
    loop {
        match lifecycle::stop(port, credential.expose(), force).await {
            Ok(lifecycle::StopOutcome::NotRunning) if !restart => println!("not running"),
            Ok(lifecycle::StopOutcome::NotRunning) => {}
            Ok(lifecycle::StopOutcome::Stopped) => println!("stopped"),
            Ok(lifecycle::StopOutcome::Busy(message)) => {
                if !wait || std::time::Instant::now() >= patience {
                    eprintln!("jarvis: {message}.");
                    eprintln!(
                        "        Wait with `--wait`, or interrupt them with `--force`; `jarvis runs` lists what is working."
                    );
                    return ExitStatus::Rejected;
                }
                if last_told.is_none_or(|told| told.elapsed().as_secs() >= 60) {
                    eprintln!("jarvis: {message}; waiting for them to finish (Ctrl-C to give up)");
                    last_told = Some(std::time::Instant::now());
                }
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                continue;
            }
            Err(status) => return status,
        }
        break;
    }
    if restart {
        // The console is usually already open in a tab, so a restart does not open another.
        return init::start(&paths, arguments);
    }
    ExitStatus::Ok
}

/// Runs `jarvis hud`: opens the heads-up display in the default browser.
fn hud_command(arguments: &[String]) -> ExitStatus {
    let root = match requested_root(arguments) {
        Ok(Some(root)) => vec!["--root".to_owned(), root.display().to_string()],
        Ok(None) => Vec::new(),
        Err(status) => return status,
    };
    let paths = match resolve_paths(&root) {
        Ok(paths) => paths,
        Err(status) => return status,
    };
    let loaded = match ConfigStore::from_paths(&paths).load() {
        Ok(loaded) => loaded,
        Err(error) => return fail("configuration", &error),
    };
    if !loaded.config().daemon().http_enabled() {
        eprintln!(
            "jarvis: the HTTP API is off in this configuration, so there is nothing to display; run `jarvis init`"
        );
        return ExitStatus::Unavailable;
    }
    let credential = match load_credential(&paths) {
        Ok(credential) => credential,
        Err(status) => return status,
    };
    hud::open(
        loaded.config().daemon().http_port(),
        credential.expose(),
        arguments.iter().any(|argument| argument == "--no-open"),
        arguments.iter().any(|argument| argument == "--print-url"),
    )
}

/// Runs `jarvis watch`, the live view of what the assistant is doing.
async fn watch_command(arguments: &[String]) -> ExitStatus {
    let root = match requested_root(arguments) {
        Ok(Some(root)) => vec!["--root".to_owned(), root.display().to_string()],
        Ok(None) => Vec::new(),
        Err(status) => return status,
    };
    match run_client(&root) {
        Ok(client) => watch::run_watch(&client, arguments).await,
        Err(status) => status,
    }
}

/// Runs `jarvis cancel`, the kill switch.
async fn cancel_command(arguments: &[String]) -> ExitStatus {
    let root = match requested_root(arguments) {
        Ok(Some(root)) => vec!["--root".to_owned(), root.display().to_string()],
        Ok(None) => Vec::new(),
        Err(status) => return status,
    };
    match run_client(&root) {
        Ok(client) => cancel::run_cancel(&client, arguments).await,
        Err(status) => status,
    }
}

/// Runs one `jarvis approvals` verb over the daemon's HTTP API.
async fn approvals_command(arguments: &[String]) -> ExitStatus {
    let root = match requested_root(arguments) {
        Ok(Some(root)) => vec!["--root".to_owned(), root.display().to_string()],
        Ok(None) => Vec::new(),
        Err(status) => return status,
    };
    let client = match run_client(&root) {
        Ok(client) => client,
        Err(status) => return status,
    };
    approvals::run(&client, arguments).await
}

/// Runs one `jarvis skills` verb over the daemon's HTTP API.
///
/// Shares `run_client` with `memory_command` and `tools_command`, for the reason that function's own comment
/// gives: the skill surface lives on the same API and needs the same four facts, and a second client builder
/// would be a second place a configured port is read — which is how two commands come to target different
/// ports.
async fn skills_command(arguments: &[String]) -> ExitStatus {
    let root = match requested_root(arguments) {
        Ok(Some(root)) => vec!["--root".to_owned(), root.display().to_string()],
        Ok(None) => Vec::new(),
        Err(status) => return status,
    };
    let client = match run_client(&root) {
        Ok(client) => client,
        Err(status) => return status,
    };
    skills::run(&client, arguments).await
}

fn json_requested(arguments: &[String]) -> bool {
    arguments.iter().any(|argument| argument == "--json")
}

/// Reads `--root`, requiring an absolute directory when present.
///
/// Portable mode must never silently fall back to the current directory, so a
/// relative root is a usage error rather than a resolved path.
fn requested_root(arguments: &[String]) -> Result<Option<PathBuf>, ExitStatus> {
    let mut index = 0;
    while index < arguments.len() {
        if arguments[index] == "--root" {
            let Some(value) = arguments.get(index + 1) else {
                eprintln!("jarvis: --root requires a directory");
                return Err(ExitStatus::Usage);
            };
            let candidate = PathBuf::from(value);
            if !candidate.is_absolute() {
                eprintln!("jarvis: --root must be an absolute directory, got {value:?}");
                return Err(ExitStatus::Usage);
            }
            if !candidate.is_dir() {
                eprintln!("jarvis: --root directory does not exist: {value:?}");
                return Err(ExitStatus::Usage);
            }
            return Ok(Some(candidate));
        }
        index += 1;
    }
    Ok(None)
}

fn resolve_paths(arguments: &[String]) -> Result<AppPaths, ExitStatus> {
    match requested_root(arguments)? {
        Some(root) => portable_layout(&root).ok_or_else(|| {
            eprintln!("jarvis: --root must be an absolute directory");
            ExitStatus::Usage
        }),
        None => AppPaths::resolve_native().map_err(|error| fail("locate", &error)),
    }
}

/// Plans or inspects the per-user service definition.
///
/// This is read-only. It reports what a reconcile would do and detects drift or
/// foreign ownership; it never writes or starts a service, because installation
/// is a separate operator action.
fn service(arguments: &[String]) -> ExitStatus {
    let json = json_requested(arguments);
    let kind = ServiceKind::native();

    // The service launches this executable as `jarvis daemon` (`ADR-0144`).
    let binary = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => return fail("locate this program", &error),
    };

    // A portable root is deliberately refused: portable mode creates no service.
    let root = match requested_root(arguments) {
        Ok(root) => root,
        Err(status) => return status,
    };

    let plan = match ServicePlan::build(kind, &binary, root.as_deref()) {
        Ok(plan) => plan,
        Err(error) => {
            eprintln!("jarvis: {error}");
            return ExitStatus::Rejected;
        }
    };
    match arguments.get(1).map(String::as_str) {
        Some("install") => return service_install::install(&plan),
        Some("uninstall") => return service_install::uninstall(&plan),
        _ => {}
    }
    let drift = detect_drift(&plan);
    let finding = drift_finding(&drift);
    if json {
        println!(
            "{}",
            serde_json::json!({
                "kind": "service",
                "service_kind": plan.kind.as_str(),
                "definition": plan.definition_path.display().to_string(),
                "binary": plan.binary_path.display().to_string(),
                "arguments": plan.arguments(),
                "drift": drift.as_str(),
                "reconcilable": drift.is_safe_to_reconcile(),
                "code": finding.code().as_str(),
                "remediation": finding.remediation(),
            })
        );
    } else {
        println!(
            "service kind={} drift={} reconcilable={}",
            plan.kind.as_str(),
            drift.as_str(),
            drift.is_safe_to_reconcile()
        );
        println!("  definition: {}", plan.definition_path.display());
        println!("  launch:     {}", plan.arguments().join(" "));
        println!("  remediation: {}", finding.remediation());
    }

    if drift.is_safe_to_reconcile() {
        ExitStatus::Ok
    } else {
        ExitStatus::Rejected
    }
}

/// The local credential if there is one, without complaining when there is not (the daemon may never have run).
fn load_credential_quietly(paths: &AppPaths) -> Option<String> {
    CredentialStore::at(paths.config().join(CREDENTIAL_FILE_NAME))
        .load()
        .ok()
        .map(|credential| credential.expose().to_owned())
}

/// Runs the offline doctor checks, optionally applying safe repairs.
///
/// Doctor is non-mutating unless `--repair` is passed, and any repair is
/// re-verified before its result is reported.
async fn doctor(arguments: &[String]) -> ExitStatus {
    let json = json_requested(arguments);
    let wants_repair = arguments.iter().any(|argument| argument == "--repair");

    let paths = match resolve_paths(arguments) {
        Ok(paths) => paths,
        Err(status) => return status,
    };

    if wants_repair {
        let report = jarvis_diagnostics::repair(&paths);
        if json {
            println!(
                "{}",
                serde_json::json!({
                    "kind": "repair",
                    "name": report.name,
                    "outcome": report.outcome.as_str(),
                    "code": report.finding.code().as_str(),
                    "summary": report.finding.summary().as_str(),
                })
            );
        } else {
            println!(
                "repair {} outcome={} code={} {}",
                report.name,
                report.outcome.as_str(),
                report.finding.code().as_str(),
                report.finding.summary()
            );
        }
        if report.outcome.is_failure() {
            return ExitStatus::RepairFailed;
        }
    }

    let report = match jarvis_diagnostics::diagnose(&paths).await {
        Ok(report) => report,
        Err(error) => return fail("diagnose", &error),
    };

    if json {
        println!("{}", report.to_json());
    } else {
        render_report(&report);
    }

    if arguments.iter().any(|argument| argument == "--live") {
        let credential = load_credential_quietly(&paths);
        let findings = checks::run(&paths, credential.as_deref()).await;
        let worst =
            findings
                .iter()
                .map(|finding| finding.level)
                .fold(checks::Level::Ok, |worst, level| match (worst, level) {
                    (checks::Level::Fail, _) | (_, checks::Level::Fail) => checks::Level::Fail,
                    (checks::Level::Warn, _) | (_, checks::Level::Warn) => checks::Level::Warn,
                    _ => checks::Level::Ok,
                });
        if json {
            let items: Vec<_> = findings
                .iter()
                .map(|f| serde_json::json!({ "check": f.name, "level": f.level.word(), "detail": f.detail, "fix": f.fix }))
                .collect();
            println!("{}", serde_json::json!({ "kind": "live", "checks": items }));
        } else {
            println!("live checks:");
            for f in &findings {
                println!("  [{}] {}: {}", f.level.word(), f.name, f.detail);
                if let Some(fix) = &f.fix {
                    let label = if f.level == checks::Level::Ok {
                        "to change"
                    } else {
                        "fix"
                    };
                    println!("        {label}: {fix}");
                }
            }
        }
        if worst == checks::Level::Fail {
            return ExitStatus::DoctorFailed;
        }
    }

    match report.outcome() {
        jarvis_diagnostics::ReportOutcome::Passed => ExitStatus::Ok,
        jarvis_diagnostics::ReportOutcome::Warnings => ExitStatus::DoctorWarnings,
        jarvis_diagnostics::ReportOutcome::Failed => ExitStatus::DoctorFailed,
    }
}

fn render_report(report: &jarvis_diagnostics::Report) {
    let (info, warning, error) = report.severity_counts();
    println!(
        "doctor profile={} mode={} runtime_source={} outcome={} info={info} warning={warning} error={error}",
        report.paths.profile,
        report.paths.mode,
        report.paths.runtime_source,
        report.outcome().as_str(),
    );
    for check in &report.checks {
        for finding in &check.findings {
            if finding.severity() == jarvis_diagnostics::Severity::Info {
                continue;
            }
            println!(
                "  [{}] {} {} — {}",
                finding.severity().as_str(),
                check.name,
                finding.code().as_str(),
                finding.summary()
            );
            if !finding.evidence().is_empty() {
                let facts: Vec<String> = finding
                    .evidence()
                    .iter()
                    .map(|(key, value)| format!("{key}={value}"))
                    .collect();
                println!("      evidence: {}", facts.join(" "));
            }
            println!("      remediation: {}", finding.remediation());
        }
    }
}

/// Parses `--lines N`, clamping to the documented bound.
///
/// This reads the structured log directly rather than going through the daemon:
/// logs must remain diagnosable precisely when the daemon cannot start.
fn requested_lines(arguments: &[String]) -> Result<usize, ExitStatus> {
    let mut requested = DEFAULT_TAIL_LINES;
    let mut index = 0;
    while index < arguments.len() {
        if arguments[index] == "--lines" {
            let Some(value) = arguments.get(index + 1) else {
                eprintln!("jarvis: --lines requires a count");
                return Err(ExitStatus::Usage);
            };
            match value.parse::<usize>() {
                Ok(parsed) if parsed > 0 => requested = parsed,
                _ => {
                    eprintln!("jarvis: --lines must be a positive integer");
                    return Err(ExitStatus::Usage);
                }
            }
            index += 2;
            continue;
        }
        index += 1;
    }
    Ok(requested)
}

fn logs(arguments: &[String]) -> ExitStatus {
    let lines = match requested_lines(arguments) {
        Ok(lines) => lines,
        Err(status) => return status,
    };
    let paths = match resolve_paths(arguments) {
        Ok(paths) => paths,
        Err(status) => return status,
    };
    let path = paths.logs().join(LOG_FILE_NAME);
    let tail = match read_tail(&path, lines) {
        Ok(tail) => tail,
        Err(error) => return fail("read logs", &error),
    };

    if tail.is_empty() {
        eprintln!("jarvis: no log records yet at {}", path.display());
        return ExitStatus::Ok;
    }
    for line in tail {
        println!("{}", line.text);
    }
    ExitStatus::Ok
}

async fn run(command: Command, arguments: &[String]) -> ExitStatus {
    match query(command, arguments).await {
        Ok(reply) => {
            let fields = match reply {
                Reply::Status(status) => status_fields(&status),
                Reply::Health(health) => health_fields(&health),
            };
            if json_requested(arguments) {
                fields.render_json();
            } else {
                fields.render_human();
            }
            ExitStatus::Ok
        }
        Err(status) => status,
    }
}

async fn query(command: Command, arguments: &[String]) -> Result<Reply, ExitStatus> {
    let paths = resolve_paths(arguments)?;
    let credential = load_credential(&paths)?;

    let config = ConfigStore::from_paths(&paths)
        .load()
        .map_err(|error| fail("configuration", &error))?;
    let endpoint = LocalEndpoint::scoped(paths.runtime(), config.config().profile().name())
        .map_err(|error| fail("endpoint", &error))?;

    let stream = connect(&endpoint)
        .await
        .map_err(|error| fail("connect", &error))?;
    let context = ClientContext::new(
        credential,
        ClientKind::Cli,
        env!("CARGO_PKG_VERSION"),
        Vec::new(),
    )
    .map_err(|error| fail("client", &error))?;

    let mut session = ClientSession::connect(stream, &context)
        .await
        .map_err(session_status)?;
    session.request(command).await.map_err(session_status)
}

fn load_credential(paths: &AppPaths) -> Result<ClientCredential, ExitStatus> {
    let store = CredentialStore::at(paths.config().join(CREDENTIAL_FILE_NAME));
    match store.load() {
        Ok(credential) => Ok(credential),
        Err(CredentialStoreError::Read { source }) if source.kind() == io::ErrorKind::NotFound => {
            eprintln!(
                "jarvis: no local credential for this profile; start jarvisd once to issue one"
            );
            Err(ExitStatus::Unavailable)
        }
        Err(error) => {
            eprintln!("jarvis: refused to use the local credential: {error}");
            Err(ExitStatus::Denied)
        }
    }
}

/// Runs one objective through the daemon's HTTP API and renders its stream.
///
/// # Why this needs the daemon's configuration
///
/// The HTTP transport is separately enabled and its port is configuration, so the CLI reads the
/// same profile configuration the daemon does and targets whatever the daemon was told to bind. A
/// hard-coded port would work on a default install and silently target nothing on a configured one.
async fn ask(arguments: &[String]) -> ExitStatus {
    let (arguments, project) = take_project(arguments);
    let Some((objective, root)) = split_ask_arguments(&arguments) else {
        eprintln!("jarvis: ask requires an objective, for example: jarvis ask summary of my inbox");
        return ExitStatus::Usage;
    };

    let paths = match resolve_paths(&root) {
        Ok(paths) => paths,
        Err(status) => return status,
    };
    let client = match run_client(&root) {
        Ok(client) => client.in_project(project),
        Err(status) => return status,
    };
    chat::drive(&client, &paths, &objective).await
}

/// Runs an interactive conversation against the daemon's API.
///
/// A conversation is an operator-driven sequence of runs in one session, so this command differs
/// from `ask` only in that it reads turns from standard input and remembers the session. It contains
/// no orchestration for the same reason `ask` does not: what history a model sees is the daemon's
/// context assembly, and its result is the manifest the daemon records.
async fn chat(arguments: &[String]) -> ExitStatus {
    let (arguments, project) = take_project(arguments);
    let arguments = arguments.as_slice();
    let root = match requested_root(arguments) {
        Ok(Some(root)) => vec!["--root".to_owned(), root.display().to_string()],
        Ok(None) => Vec::new(),
        Err(status) => return status,
    };

    let paths = match resolve_paths(&root) {
        Ok(paths) => paths,
        Err(status) => return status,
    };
    let client = match run_client(&root) {
        Ok(client) => client.in_project(project),
        Err(status) => return status,
    };
    chat::converse(&client, &paths).await
}

/// Builds an authenticated client for this profile's daemon.
///
/// Shared by `ask` and `chat`, because both need the same four facts: the configured port, the
/// profile credential, the loopback-only endpoint type, and whether HTTP is enabled at all. Duplicated
/// per command, one of them would eventually read a different configuration and target a port the
/// daemon is not listening on.
fn run_client(root: &[String]) -> Result<api_client::ApiClient, ExitStatus> {
    let paths = resolve_paths(root)?;

    let loaded = match ConfigStore::from_paths(&paths).load() {
        Ok(loaded) => loaded,
        Err(error) => return Err(fail("configuration", &error)),
    };
    let daemon = loaded.config().daemon();
    if !daemon.http_enabled() {
        // Reported rather than worked around: the HTTP transport is off by default because a
        // listening port is a larger surface than an OS-protected pipe, and turning it on is the
        // operator's decision. The message names the exact keys so the fix is one edit.
        //
        // ⭐ The environment form spells out `true` because the daemon parses **`bool`**, not a
        // truthiness rule — so `JARVIS_HTTP_ENABLED=1`, which a reader reasonably tries first, is a
        // startup failure whose message ("invalid value for configuration environment variable") does
        // not say what would be valid. A hint that names a value the daemon refuses sends the operator
        // to the configuration file to look for a problem that is in the hint.
        eprintln!(
            "jarvis: the daemon HTTP API is not enabled for this profile, so runs are unreachable"
        );
        eprintln!(
            "jarvis: set daemon.http_enabled = true in config.toml (or JARVIS_HTTP_ENABLED=true, \
             which parses as a strict boolean) and restart jarvisd"
        );
        return Err(ExitStatus::Unavailable);
    }

    let credential = load_credential(&paths)?;
    // Port 0 is refused by configuration validation, so this can only fail if a stored config was
    // edited outside the validated path. It is still handled rather than unwrapped.
    let host = match LoopbackHost::new(daemon.http_port()) {
        Ok(host) => host,
        Err(error) => {
            eprintln!("jarvis: the configured HTTP port cannot be used: {error}");
            return Err(ExitStatus::Unavailable);
        }
    };

    api_client::ApiClient::new(host, credential.expose().to_owned())
        .map_err(|error| fail("client", &error))
}

/// Splits `ask` arguments into an objective and an optional `--root` directory.
///
/// `--root` is extracted from anywhere in the arguments rather than required to come first, and the
/// remaining words are joined so an unquoted objective works. Returns `None` for an empty objective,
/// which is a usage error rather than a run with no objective.
fn split_ask_arguments(arguments: &[String]) -> Option<(String, Vec<String>)> {
    let mut objective_words: Vec<String> = Vec::new();
    let mut root: Vec<String> = Vec::new();
    let mut index = 1;
    while index < arguments.len() {
        if arguments[index] == "--root" {
            let value = arguments.get(index + 1)?;
            root.push("--root".to_owned());
            root.push(value.clone());
            index += 2;
            continue;
        }
        objective_words.push(arguments[index].clone());
        index += 1;
    }
    let objective = objective_words.join(" ");
    if objective.trim().is_empty() {
        return None;
    }
    Some((objective, root))
}

fn fail(operation: &str, error: &impl std::fmt::Display) -> ExitStatus {
    eprintln!("jarvis: {operation} failed: {error}");
    ExitStatus::Unavailable
}

fn session_status(error: SessionError) -> ExitStatus {
    match error {
        SessionError::Rejected(wire) | SessionError::Request(wire) => {
            eprintln!("jarvis: daemon error: {wire}");
            ExitStatus::from_code(wire.code)
        }
        other => fail("protocol", &other),
    }
}

fn status_fields(status: &StatusReply) -> Fields {
    Fields::new("status")
        .with("phase", status.phase.clone())
        .with("live", status.live.to_string())
        .with("ready", status.ready.to_string())
        .with("daemon_version", status.daemon_version.clone())
        .with("protocol_version", status.protocol_version.to_string())
        .with("config_schema", status.config_schema.to_string())
        .with("database_schema", status.database_schema.to_string())
        .with("daemon_id", status.daemon_id.to_string())
        .with("started_at", status.started_at.to_string())
}

fn health_fields(health: &HealthReply) -> Fields {
    Fields::new("health")
        .with("phase", health.phase.clone())
        .with("live", health.live.to_string())
        .with("ready", health.ready.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_flag_is_recognized_after_the_command() {
        assert!(json_requested(&["status".to_owned(), "--json".to_owned()]));
        assert!(!json_requested(&["status".to_owned()]));
    }

    #[test]
    fn distinct_daemon_error_codes_map_to_distinct_exit_statuses() {
        use jarvis_core::ErrorCode;

        assert_ne!(
            ExitStatus::from_code(ErrorCode::Authentication),
            ExitStatus::from_code(ErrorCode::Validation)
        );
        assert_ne!(
            ExitStatus::from_code(ErrorCode::Internal),
            ExitStatus::from_code(ErrorCode::Conflict)
        );
    }

    /// **The preview flags become the request body the daemon expects.**
    ///
    /// Asserted on the request rather than on rendered output, because the flag parsing is what can be wrong:
    /// a preview that ignored `--escalation` would render a decision for a call *without* that signal and
    /// look entirely plausible, which is the failure mode a signal exists to prevent.
    #[test]
    fn preview_flags_parse_into_a_request() {
        let arguments = [
            "tools".to_owned(),
            "preview".to_owned(),
            "jarvis.files.read".to_owned(),
            "--escalation".to_owned(),
            "bulk".to_owned(),
        ];
        let request = tools::preview_request_for_test(&arguments)
            .unwrap_or_else(|_| panic!("the flags must parse"));
        assert_eq!(
            request.escalation,
            vec![jarvis_core::EscalationSignal::Bulk]
        );
    }

    /// **An unknown closed-set value is a usage error rather than an ignored flag.**
    ///
    /// The direction that matters. Silently dropping `--escalation loud` would compute the preview for a
    /// call without that signal, which reads as more permissive than the user asked about.
    #[test]
    fn unknown_preview_flags_are_refused() {
        let (flag, value) = ("--escalation", "loud");
        let arguments = [
            "tools".to_owned(),
            "preview".to_owned(),
            "jarvis.files.read".to_owned(),
            flag.to_owned(),
            value.to_owned(),
        ];
        assert!(
            tools::preview_request_for_test(&arguments).is_err(),
            "{flag} {value} must be refused rather than ignored"
        );
    }

    /// **`--json` is accepted by the preview parser and contributes nothing to the body.**
    ///
    /// The control for the test above: a parser that refused every flag would satisfy it, and `--json` is a
    /// flag this command must tolerate because `json_requested` reads it separately.
    #[test]
    fn the_json_flag_is_tolerated_by_the_preview_parser() {
        let arguments = [
            "tools".to_owned(),
            "preview".to_owned(),
            "jarvis.files.read".to_owned(),
            "--json".to_owned(),
        ];
        let request = tools::preview_request_for_test(&arguments)
            .unwrap_or_else(|_| panic!("--json must be tolerated"));
        assert_eq!(request, jarvis_protocol::ToolPreviewRequest::default());
    }
}
