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

mod output;

use std::{io, path::PathBuf, process::ExitCode};

use jarvis_core::{ClientCredential, LocalEndpoint, connect};
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

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let status = match arguments.first().map(String::as_str) {
        Some("--version" | "-V" | "version") => {
            println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
            ExitStatus::Ok
        }
        Some("status") => run(Command::Status, &arguments).await,
        Some("health") => run(Command::Health, &arguments).await,
        Some("logs") => logs(&arguments),
        Some("doctor") => doctor(&arguments).await,
        Some("service") => service(&arguments),
        Some(other) => {
            eprintln!("jarvis: unknown command {other:?}");
            eprintln!("{}", usage());
            ExitStatus::Usage
        }
        None => {
            eprintln!("{}", usage());
            ExitStatus::Usage
        }
    };
    status.into()
}

const fn usage() -> &'static str {
    "usage: jarvis <status|health|logs|doctor|service|version> [--json] [--lines N] [--repair] [--root DIR]"
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

    // The service must launch the daemon, not this client. `current_exe()` is the
    // client, so the daemon is resolved as its sibling.
    let client = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => return fail("locate the client binary", &error),
    };
    let Some(binary) = jarvis_diagnostics::daemon_binary_beside(&client) else {
        eprintln!(
            "jarvis: could not find jarvisd beside {}; the service would launch the client",
            client.display()
        );
        return ExitStatus::Unavailable;
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
}
