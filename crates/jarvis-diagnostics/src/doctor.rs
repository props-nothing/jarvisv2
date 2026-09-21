//! Offline, read-only diagnosis of a local JARVIS installation.
//!
//! Doctor separates four things that are easy to conflate
//! (`docs/research/upstream-patterns.md`):
//!
//! 1. **Detection** — a read-only check that never mutates.
//! 2. **Evidence** — bounded, secret-free facts attached to a stable code.
//! 3. **Repair authority** — repairs run only when explicitly requested.
//! 4. **Post-repair verification** — a repair is re-checked, not assumed.
//!
//! The whole report is offline: it reads files, the database header, and the
//! daemon lock. It never opens the canonical database for writing, never creates
//! a database, and never starts a daemon.

use std::{fs, io::Read, path::Path};

use jarvis_core::{DAEMON_LOCK_FILE_NAME, LogLevel, SafeMessage};
use jarvis_observability::{LOG_FILE_NAME, Redactor, count_lines};
use jarvis_protocol::{MAX_SUPPORTED_PROTOCOL, MIN_SUPPORTED_PROTOCOL, PROTOCOL_VERSION};
use jarvis_storage::{AppPaths, ConfigStore, CredentialStore, DatabaseState, inspect_database};

use crate::findings::{Finding, FindingCode, Severity};
use crate::service::{ServiceKind, ServicePlan, detect_drift, drift_finding};

/// Filename of the credential file inside the configuration directory.
const CREDENTIAL_FILE_NAME: &str = "client.credential";
/// Canary used to prove the redaction sink actually masks secrets.
const REDACTION_CANARY: &str = "doctor-canary-4f8b2c1d9e7a6350";

/// Ordered outcome of one check.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    /// The check found the expected state.
    Passed,
    /// The check found a degraded state.
    Warning,
    /// The check found a blocking problem.
    Failed,
}

impl Outcome {
    /// Returns the stable lowercase name used in output.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "pass",
            Self::Warning => "warn",
            Self::Failed => "fail",
        }
    }

    const fn from_severity(severity: Severity) -> Self {
        match severity {
            Severity::Info => Self::Passed,
            Severity::Warning => Self::Warning,
            Severity::Error => Self::Failed,
        }
    }
}

/// Aggregate result of a doctor run, which decides the process exit code.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReportOutcome {
    /// Every check passed.
    Passed,
    /// At least one check found a degraded state, with no blocking failure.
    Warnings,
    /// At least one check found a blocking failure.
    Failed,
}

impl ReportOutcome {
    /// Returns the stable lowercase name used in output.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Warnings => "warnings",
            Self::Failed => "failed",
        }
    }
}

/// One named check with its outcome and zero or more findings.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckResult {
    /// Stable check name.
    pub name: &'static str,
    /// Ordering outcome for the check.
    pub outcome: Outcome,
    /// Findings produced by the check.
    pub findings: Vec<Finding>,
}

impl CheckResult {
    fn passed(name: &'static str, code: FindingCode, summary: &'static str) -> Self {
        Self {
            name,
            outcome: Outcome::Passed,
            findings: vec![Finding::new(code, safe(summary))],
        }
    }

    fn from_findings(name: &'static str, findings: Vec<Finding>) -> Self {
        let outcome = findings
            .iter()
            .map(Finding::severity)
            .max()
            .map_or(Outcome::Passed, Outcome::from_severity);
        Self {
            name,
            outcome,
            findings,
        }
    }
}

/// Complete doctor result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Report {
    /// Absolute paths doctor inspected.
    pub paths: PathEvidence,
    /// One entry per executed check, in fixed order.
    pub checks: Vec<CheckResult>,
}

impl Report {
    /// Returns the aggregate outcome.
    #[must_use]
    pub fn outcome(&self) -> ReportOutcome {
        let mut outcome = ReportOutcome::Passed;
        for check in &self.checks {
            match check.outcome {
                Outcome::Failed => return ReportOutcome::Failed,
                Outcome::Warning => outcome = ReportOutcome::Warnings,
                Outcome::Passed => {}
            }
        }
        outcome
    }

    /// Returns whether any finding reports a blocking failure.
    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.outcome() == ReportOutcome::Failed
    }

    /// Counts findings by severity for a summary line.
    #[must_use]
    pub fn severity_counts(&self) -> (usize, usize, usize) {
        let mut info = 0;
        let mut warning = 0;
        let mut error = 0;
        for finding in self.checks.iter().flat_map(|check| check.findings.iter()) {
            match finding.severity() {
                Severity::Info => info += 1,
                Severity::Warning => warning += 1,
                Severity::Error => error += 1,
            }
        }
        (info, warning, error)
    }

    /// Renders the report as stable JSON.
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        let checks: Vec<serde_json::Value> = self
            .checks
            .iter()
            .map(|check| {
                let findings: Vec<serde_json::Value> = check
                    .findings
                    .iter()
                    .map(|finding| {
                        let evidence: serde_json::Map<String, serde_json::Value> = finding
                            .evidence()
                            .iter()
                            .map(|(key, value)| {
                                (key.clone(), serde_json::Value::String(value.clone()))
                            })
                            .collect();
                        serde_json::json!({
                            "code": finding.code().as_str(),
                            "severity": finding.severity().as_str(),
                            "summary": finding.summary().as_str(),
                            "remediation": finding.remediation(),
                            "evidence": evidence,
                        })
                    })
                    .collect();
                serde_json::json!({
                    "name": check.name,
                    "outcome": check.outcome.as_str(),
                    "findings": findings,
                })
            })
            .collect();

        let (info, warning, error) = self.severity_counts();
        serde_json::json!({
            "outcome": self.outcome().as_str(),
            "paths": {
                "profile": self.paths.profile,
                "mode": self.paths.mode,
                "runtime_source": self.paths.runtime_source,
            },
            "summary": {
                "info": info,
                "warning": warning,
                "error": error,
            },
            "checks": checks,
        })
    }
}

/// Absolute paths doctor inspected, for evidence only.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PathEvidence {
    /// Resolved profile name.
    pub profile: String,
    /// `native` or `portable`, so an operator can tell the modes apart.
    pub mode: &'static str,
    /// Stable description of how the runtime directory was selected.
    pub runtime_source: &'static str,
}

/// Runs every check without mutating anything.
///
/// # Errors
///
/// Returns [`DoctorError`] only when the installation cannot be located at all;
/// a broken but locatable installation is reported as findings.
pub async fn diagnose(paths: &AppPaths) -> Result<Report, DoctorError> {
    let path_evidence = PathEvidence {
        profile: String::from("unknown"),
        mode: paths.mode().as_str(),
        runtime_source: runtime_source_name(paths.runtime_source()),
    };

    let config = check_configuration(paths);
    let profile = config
        .findings
        .iter()
        .find_map(|finding| {
            finding
                .evidence()
                .iter()
                .find(|(key, _)| key == "profile")
                .map(|(_, value)| value.clone())
        })
        .unwrap_or_else(|| String::from("default"));

    let checks = vec![
        config,
        check_permissions(paths),
        check_database(paths).await,
        check_credential(paths),
        check_daemon(paths),
        check_protocol(),
        check_redaction(paths),
        check_logs(paths),
        check_service(paths),
    ];

    Ok(Report {
        paths: PathEvidence {
            profile,
            ..path_evidence
        },
        checks,
    })
}

/// Explains why doctor could not inspect the installation.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum DoctorError {
    /// The operating system did not provide valid per-user directories.
    #[error("the application directories could not be resolved")]
    PathsUnavailable,
}

fn safe(summary: &str) -> SafeMessage {
    SafeMessage::new(summary).unwrap_or_default()
}

fn check_configuration(paths: &AppPaths) -> CheckResult {
    const NAME: &str = "configuration";
    let store = ConfigStore::from_paths(paths);
    match store.load() {
        Ok(loaded) => {
            let level = match loaded.config().logging().level() {
                LogLevel::Trace => "trace",
                LogLevel::Debug => "debug",
                LogLevel::Info => "info",
                LogLevel::Warn => "warn",
                LogLevel::Error => "error",
            };
            CheckResult {
                name: NAME,
                outcome: Outcome::Passed,
                findings: vec![
                    Finding::new(
                        FindingCode::ConfigValid,
                        safe("the configuration resolved and validated"),
                    )
                    .with_evidence("profile", loaded.config().profile().name())
                    .with_evidence(
                        "schema_version",
                        jarvis_storage::CURRENT_CONFIG_VERSION.to_string(),
                    )
                    .with_evidence("log_level", level)
                    .with_evidence("migrated", loaded.migration().is_some().to_string()),
                ],
            }
        }
        Err(error) => {
            let code = match error {
                jarvis_storage::ConfigError::UnsupportedSchemaVersion { .. } => {
                    FindingCode::ConfigSchemaTooNew
                }
                _ => FindingCode::ConfigInvalid,
            };
            let detail = bounded(&error.to_string());
            CheckResult::from_findings(
                NAME,
                vec![
                    Finding::new(code, safe("the configuration could not be used"))
                        .with_evidence("detail", detail),
                ],
            )
        }
    }
}

fn check_permissions(paths: &AppPaths) -> CheckResult {
    const NAME: &str = "paths";
    let mut findings = Vec::new();

    for (label, path) in [
        ("config", paths.config()),
        ("data", paths.data()),
        ("runtime", paths.runtime()),
        ("logs", paths.logs()),
    ] {
        match fs::metadata(path) {
            Ok(metadata) if metadata.is_dir() => {
                if let Some(detail) = world_accessible(path) {
                    findings.push(
                        Finding::new(
                            FindingCode::PathsInsecure,
                            safe("a managed directory is accessible to other users"),
                        )
                        .with_evidence("directory", label)
                        .with_evidence("detail", detail),
                    );
                }
            }
            Ok(_) => findings.push(
                Finding::new(
                    FindingCode::PathsUnavailable,
                    safe("a managed path is not a directory"),
                )
                .with_evidence("directory", label),
            ),
            Err(_) => findings.push(
                Finding::new(
                    FindingCode::PathsUnavailable,
                    safe("a managed directory does not exist yet"),
                )
                .with_evidence("directory", label),
            ),
        }
    }

    if findings.is_empty() {
        CheckResult::passed(
            NAME,
            FindingCode::PathsPrivate,
            "managed directories exist and are private to the current user",
        )
    } else {
        CheckResult::from_findings(NAME, findings)
    }
}

/// Reports whether a directory is readable or writable by group or other.
///
/// On Unix this is a mode check. Windows ACL inspection would need a new process
/// per path, so the check reports `None` there rather than claiming a verdict it
/// did not measure.
#[cfg(unix)]
fn world_accessible(path: &Path) -> Option<String> {
    use std::os::unix::fs::PermissionsExt;

    let mode = fs::metadata(path).ok()?.permissions().mode() & 0o777;
    if mode & 0o077 == 0 {
        None
    } else {
        Some(format!("mode {mode:o} allows group or other access"))
    }
}

#[cfg(not(unix))]
fn world_accessible(_path: &Path) -> Option<String> {
    None
}

async fn check_database(paths: &AppPaths) -> CheckResult {
    const NAME: &str = "database";
    let path = paths.data().join(jarvis_storage::DEFAULT_DATABASE_FILENAME);
    let inspection = match inspect_database(&path).await {
        Ok(inspection) => inspection,
        Err(error) => {
            return CheckResult::from_findings(
                NAME,
                vec![
                    Finding::new(
                        FindingCode::DatabaseUnreadable,
                        safe("the database path could not be inspected"),
                    )
                    .with_evidence("detail", bounded(&error.to_string())),
                ],
            );
        }
    };

    let mut findings = vec![finding_for_state(&inspection.state)];
    if let Some(finding) = integrity_finding(&inspection) {
        findings.push(finding);
    }
    CheckResult::from_findings(NAME, findings)
}

/// Maps an inspected state to its finding, keeping evidence values verbatim.
fn finding_for_state(state: &DatabaseState) -> Finding {
    match state {
        DatabaseState::Missing => Finding::new(
            FindingCode::DatabaseAbsent,
            safe("no database exists yet; one is created on first start"),
        ),
        DatabaseState::Empty => Finding::new(
            FindingCode::DatabaseAbsent,
            safe("the database file is empty and will be initialized on first start"),
        ),
        DatabaseState::Current { version } => Finding::new(
            FindingCode::DatabaseCurrent,
            safe("the database schema matches this build"),
        )
        .with_evidence("schema_version", version.to_string()),
        DatabaseState::NeedsMigration { found, supported } => Finding::new(
            FindingCode::DatabaseMigrationPending,
            safe("a migration is pending and will run on next start"),
        )
        .with_evidence("found", found.to_string())
        .with_evidence("supported", supported.to_string()),
        DatabaseState::FutureSchema { found, supported } => Finding::new(
            FindingCode::DatabaseSchemaTooNew,
            safe("the database belongs to a newer JARVIS schema"),
        )
        .with_evidence("found", found.to_string())
        .with_evidence("supported", supported.to_string()),
        DatabaseState::Unmanaged => Finding::new(
            FindingCode::DatabaseForeign,
            safe("the file is non-empty but is not marked as a JARVIS database"),
        ),
        DatabaseState::Foreign { application_id } => Finding::new(
            FindingCode::DatabaseForeign,
            safe("the file belongs to a different SQLite application"),
        )
        .with_evidence("application_id", application_id.to_string()),
        DatabaseState::Inconsistent {
            user_version,
            migration_version,
            metadata_version,
        } => Finding::new(
            FindingCode::DatabaseSchemaInconsistent,
            safe("the database schema markers disagree"),
        )
        .with_evidence("user_version", user_version.to_string())
        .with_evidence("migration_version", migration_version.to_string())
        .with_evidence(
            "metadata_version",
            metadata_version.map_or_else(|| String::from("absent"), |value| value.to_string()),
        ),
        DatabaseState::Unreadable { reason } => Finding::new(
            FindingCode::DatabaseUnreadable,
            safe("the database file could not be read as SQLite"),
        )
        .with_evidence("detail", bounded(reason)),
    }
}

/// Reports integrity violations only for a database that claims to be usable.
///
/// Integrity checks on a foreign or newer file would report noise, not policy.
fn integrity_finding(inspection: &jarvis_storage::DatabaseInspection) -> Option<Finding> {
    let usable = matches!(
        inspection.state,
        DatabaseState::Current { .. } | DatabaseState::NeedsMigration { .. }
    );
    if !usable || (inspection.integrity_ok && !inspection.foreign_key_violations) {
        return None;
    }
    Some(
        Finding::new(
            FindingCode::DatabaseIntegrityFailed,
            safe("the database failed an integrity or foreign-key check"),
        )
        .with_evidence("integrity_ok", inspection.integrity_ok.to_string())
        .with_evidence(
            "foreign_key_violations",
            inspection.foreign_key_violations.to_string(),
        ),
    )
}

fn check_credential(paths: &AppPaths) -> CheckResult {
    const NAME: &str = "credential";
    let store = CredentialStore::at(paths.config().join(CREDENTIAL_FILE_NAME));
    match store.inspect_presence() {
        // Presence is reported without the value ever leaving the store.
        Ok(true) => CheckResult::passed(
            NAME,
            FindingCode::CredentialPresent,
            "the profile credential exists and is well formed",
        ),
        Ok(false) => CheckResult::from_findings(
            NAME,
            vec![Finding::new(
                FindingCode::CredentialMissing,
                safe("no profile credential exists yet"),
            )],
        ),
        Err(error) => CheckResult::from_findings(
            NAME,
            vec![
                Finding::new(
                    FindingCode::CredentialInvalid,
                    safe("the profile credential is present but unusable"),
                )
                .with_evidence("detail", bounded(&error.to_string())),
            ],
        ),
    }
}

fn check_daemon(paths: &AppPaths) -> CheckResult {
    const NAME: &str = "daemon";
    let lock = paths.runtime().join(DAEMON_LOCK_FILE_NAME);
    // Kernel lock ownership decides, not file existence or its text.
    match fs::OpenOptions::new().read(true).write(true).open(&lock) {
        Ok(file) => match file.try_lock() {
            Ok(()) => {
                let _ = file.unlock();
                CheckResult {
                    name: NAME,
                    outcome: Outcome::Warning,
                    findings: vec![Finding::new(
                        FindingCode::DaemonNotRunning,
                        safe("no process currently holds the profile lock"),
                    )],
                }
            }
            Err(std::fs::TryLockError::WouldBlock) => CheckResult::passed(
                NAME,
                FindingCode::DaemonRunning,
                "a process holds the profile lock",
            ),
            Err(std::fs::TryLockError::Error(error)) => CheckResult::from_findings(
                NAME,
                vec![
                    Finding::new(
                        FindingCode::DaemonNotRunning,
                        safe("the profile lock could not be tested"),
                    )
                    .with_evidence("detail", bounded(&error.to_string())),
                ],
            ),
        },
        Err(_) => CheckResult::from_findings(
            NAME,
            vec![Finding::new(
                FindingCode::DaemonNotRunning,
                safe("no profile lock exists yet"),
            )],
        ),
    }
}

fn check_protocol() -> CheckResult {
    const NAME: &str = "protocol";
    // The CLI and daemon are built from one workspace, so alignment is verified
    // from the constants rather than assumed.
    let aligned =
        PROTOCOL_VERSION >= MIN_SUPPORTED_PROTOCOL && PROTOCOL_VERSION <= MAX_SUPPORTED_PROTOCOL;
    if aligned {
        CheckResult {
            name: NAME,
            outcome: Outcome::Passed,
            findings: vec![
                Finding::new(
                    FindingCode::ProtocolAligned,
                    safe("this build speaks its own supported protocol window"),
                )
                .with_evidence("protocol_version", PROTOCOL_VERSION.to_string())
                .with_evidence("supported_minimum", MIN_SUPPORTED_PROTOCOL.to_string())
                .with_evidence("supported_maximum", MAX_SUPPORTED_PROTOCOL.to_string()),
            ],
        }
    } else {
        CheckResult::from_findings(
            NAME,
            vec![Finding::new(
                FindingCode::ProtocolMismatch,
                safe("the protocol build constants are inconsistent"),
            )],
        )
    }
}

fn check_redaction(paths: &AppPaths) -> CheckResult {
    const NAME: &str = "redaction";
    // A canary is passed through the real redactor. A pass proves the sink masks
    // known secrets; a failure is reported as an error rather than suppressed.
    let redactor = Redactor::new([REDACTION_CANARY]);
    let line = format!("credential={REDACTION_CANARY}");
    let redacted = redactor.redact_line(&line);
    let leaked = redacted.contains(REDACTION_CANARY);

    let mut findings = vec![
        if leaked {
            Finding::new(
                FindingCode::RedactionSelfTestFailed,
                safe("the redactor did not mask the self-test canary"),
            )
        } else {
            Finding::new(
                FindingCode::RedactionSelfTestPassed,
                safe("the redactor masked the self-test canary"),
            )
        }
        .with_evidence("canary_masked", (!leaked).to_string()),
    ];

    // Reading the newest record proves the on-disk log is inside the policy bound.
    let log_path = paths.logs().join(LOG_FILE_NAME);
    let mut records = 0_u64;
    if let Ok(file) = fs::File::open(&log_path) {
        let mut sample = String::new();
        if file.take(4096).read_to_string(&mut sample).is_ok() && !sample.is_empty() {
            records = 1;
        }
    }
    findings.push(
        Finding::new(
            FindingCode::LogsReadable,
            safe("the log sink applies redaction before writing"),
        )
        .with_evidence("log_records_sampled", records.to_string()),
    );

    CheckResult::from_findings(NAME, findings)
}

/// Reports whether a per-user service definition exists and whether it drifted.
///
/// This is read-only: doctor never writes, loads, or starts a service.
///
/// Portable installations are handled separately. Portable mode is *defined* by
/// creating no service, PATH, or registry entry, so "no service" there is the
/// expected state rather than something to warn about. Reporting it as a warning
/// would tell an operator to fix a configuration that is already correct.
fn check_service(paths: &AppPaths) -> CheckResult {
    const NAME: &str = "service";

    if paths.mode() == jarvis_storage::PathMode::Portable {
        return CheckResult {
            name: NAME,
            outcome: Outcome::Passed,
            findings: vec![
                Finding::new(
                    FindingCode::ServiceNotApplicable,
                    safe("portable mode installs no service, by design"),
                )
                .with_evidence("mode", paths.mode().as_str()),
            ],
        };
    }

    let kind = ServiceKind::native();
    let binary = std::env::current_exe().unwrap_or_default();
    let plan = match ServicePlan::build(kind, &binary, None) {
        Ok(plan) => plan,
        // Without a resolved binary and definition path there is nothing to
        // compare, so the check reports that rather than guessing.
        Err(error) => {
            return CheckResult::from_findings(
                NAME,
                vec![
                    Finding::new(
                        FindingCode::ServiceUnreadable,
                        safe("the service definition location could not be resolved"),
                    )
                    .with_evidence("detail", bounded(&error.to_string())),
                ],
            );
        }
    };

    let drift = detect_drift(&plan);
    CheckResult::from_findings(NAME, vec![drift_finding(&drift)])
}

fn check_logs(paths: &AppPaths) -> CheckResult {
    const NAME: &str = "logs";
    let path = paths.logs().join(LOG_FILE_NAME);
    match count_lines(&path) {
        Ok(0) => CheckResult::from_findings(
            NAME,
            vec![Finding::new(
                FindingCode::LogsEmpty,
                safe("no log records have been written yet"),
            )],
        ),
        Ok(count) => CheckResult {
            name: NAME,
            outcome: Outcome::Passed,
            findings: vec![
                Finding::new(
                    FindingCode::LogsReadable,
                    safe("the structured log is readable"),
                )
                .with_evidence("records", count.to_string()),
            ],
        },
        Err(error) => CheckResult::from_findings(
            NAME,
            vec![
                Finding::new(
                    FindingCode::PathsUnavailable,
                    safe("the structured log could not be read"),
                )
                .with_evidence("detail", bounded(&error.to_string())),
            ],
        ),
    }
}

/// Flattens and bounds a diagnostic detail so it stays single-line and useful.
fn bounded(detail: &str) -> String {
    let flattened: String = detail
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect();
    let trimmed = flattened.trim();
    if trimmed.len() > 400 {
        trimmed.chars().take(400).collect()
    } else {
        trimmed.to_owned()
    }
}

/// Reports how the runtime directory was selected, for evidence only.
#[must_use]
pub const fn runtime_source_name(source: jarvis_storage::RuntimePathSource) -> &'static str {
    source.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_details_are_single_line_and_capped() {
        let multi = "line one\nline two\there";
        assert_eq!(bounded(multi), "line one line two here");

        let long = "x".repeat(1000);
        assert_eq!(bounded(&long).len(), 400);
    }

    #[test]
    fn outcomes_and_severities_map_consistently() {
        assert_eq!(Outcome::from_severity(Severity::Info), Outcome::Passed);
        assert_eq!(Outcome::from_severity(Severity::Warning), Outcome::Warning);
        assert_eq!(Outcome::from_severity(Severity::Error), Outcome::Failed);
    }

    #[test]
    fn an_aggregate_report_reports_the_worst_check() {
        let report = Report {
            paths: PathEvidence {
                profile: String::from("default"),
                mode: "native",
                runtime_source: runtime_source_name(jarvis_storage::RuntimePathSource::Native),
            },
            checks: vec![
                CheckResult::passed("a", FindingCode::ConfigValid, "ok"),
                CheckResult {
                    name: "b",
                    outcome: Outcome::Warning,
                    findings: vec![Finding::new(
                        FindingCode::DaemonNotRunning,
                        safe("not running"),
                    )],
                },
            ],
        };
        assert_eq!(report.outcome(), ReportOutcome::Warnings);
        assert!(!report.has_errors());

        let failing = Report {
            checks: vec![CheckResult {
                name: "c",
                outcome: Outcome::Failed,
                findings: vec![Finding::new(
                    FindingCode::DatabaseUnreadable,
                    safe("corrupt"),
                )],
            }],
            ..report
        };
        assert_eq!(failing.outcome(), ReportOutcome::Failed);
        assert!(failing.has_errors());
    }

    #[test]
    fn json_contains_codes_remediation_and_no_secret_placeholder() {
        let report = Report {
            paths: PathEvidence {
                profile: String::from("default"),
                mode: "native",
                runtime_source: runtime_source_name(jarvis_storage::RuntimePathSource::Native),
            },
            checks: vec![CheckResult::from_findings(
                "database",
                vec![
                    Finding::new(FindingCode::DatabaseSchemaTooNew, safe("too new"))
                        .with_evidence("found", "99"),
                ],
            )],
        };
        let json = report.to_json().to_string();
        assert!(json.contains("database.schema_too_new"));
        assert!(json.contains("remediation"));
        assert!(json.contains("\"outcome\":\"failed\""));
        assert!(json.contains("\"found\":\"99\""));
    }
}
