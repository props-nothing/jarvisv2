//! Acceptance evidence for A02: "Doctor Explains A Broken Install".
//!
//! `docs/quality/acceptance-tests.md` requires that, for each deliberately broken
//! case, doctor produces a stable finding code, safe evidence, a specific
//! remediation, no secret leakage, and **no mutation unless repair was explicitly
//! requested**.
//!
//! These tests therefore assert three things per case, not one:
//!   1. the expected stable code appears in the report,
//!   2. the aggregate outcome is blocking,
//!   3. the broken artefact is byte-for-byte unchanged afterwards.
//!
//! Assertion 3 is the one that matters most: a doctor that "fixes" while merely
//! diagnosing would pass 1 and 2 while violating the contract.

use std::{
    fs,
    path::{Path, PathBuf},
};

use jarvis_diagnostics::{FindingCode, Report, ReportOutcome, Severity, diagnose, repair};
use jarvis_storage::AppPaths;
use sqlx::{ConnectOptions, Connection, Executor, sqlite::SqliteConnectOptions};

struct TempProfile(PathBuf);

impl TempProfile {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("jarvis-doctor-a02-{}", jarvis_core::scratch_tag()));
        fs::create_dir_all(&path).unwrap_or_else(|error| panic!("create temp dir: {error}"));
        Self(path)
    }

    fn paths(&self) -> AppPaths {
        AppPaths::from_root(&self.0).unwrap_or_else(|error| panic!("fixture paths: {error}"))
    }

    fn database(&self) -> PathBuf {
        self.0.join("data").join("jarvis.sqlite3")
    }

    fn config(&self) -> PathBuf {
        self.0.join("config").join("config.toml")
    }
}

impl Drop for TempProfile {
    fn drop(&mut self) {
        jarvis_core::remove_scratch_dir(&self.0);
    }
}

/// Writes a minimal database with explicit JARVIS markers.
async fn write_database(path: &Path, statements: &[&'static str]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|error| panic!("create data dir: {error}"));
    }
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .disable_statement_logging();
    let mut connection = options
        .connect()
        .await
        .unwrap_or_else(|error| panic!("open fixture: {error}"));
    for statement in statements {
        connection
            .execute(*statement)
            .await
            .unwrap_or_else(|error| panic!("execute {statement:?}: {error}"));
    }
    connection
        .close()
        .await
        .unwrap_or_else(|error| panic!("close fixture: {error}"));
}

fn snapshot(path: &Path) -> Vec<u8> {
    fs::read(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

fn codes(report: &Report) -> Vec<&'static str> {
    report
        .checks
        .iter()
        .flat_map(|check| check.findings.iter())
        .map(|finding| finding.code().as_str())
        .collect()
}

fn assert_has_code(report: &Report, expected: FindingCode) {
    let found = codes(report);
    assert!(
        found.contains(&expected.as_str()),
        "expected {} in {found:?}",
        expected.as_str()
    );
}

#[tokio::test]
async fn a_newer_schema_is_explained_without_touching_the_database() {
    let profile = TempProfile::new();
    let paths = profile.paths();
    paths
        .prepare()
        .unwrap_or_else(|error| panic!("prepare: {error}"));

    write_database(
        &profile.database(),
        &[
            "PRAGMA application_id = 1245794902",
            "PRAGMA user_version = 99",
            "CREATE TABLE jarvis_storage_metadata (singleton INTEGER PRIMARY KEY, schema_version INTEGER NOT NULL)",
            "INSERT INTO jarvis_storage_metadata VALUES (1, 99)",
        ],
    )
    .await;

    let before = snapshot(&profile.database());
    let report = diagnose(&paths)
        .await
        .unwrap_or_else(|error| panic!("diagnose: {error}"));

    assert_has_code(&report, FindingCode::DatabaseSchemaTooNew);
    assert_eq!(report.outcome(), ReportOutcome::Failed);

    // The evidence must name both versions so the operator can act.
    let finding = report
        .checks
        .iter()
        .flat_map(|check| check.findings.iter())
        .find(|finding| finding.code() == FindingCode::DatabaseSchemaTooNew)
        .unwrap_or_else(|| panic!("finding not present"));
    let evidence: Vec<&str> = finding
        .evidence()
        .iter()
        .map(|(key, _)| key.as_str())
        .collect();
    assert!(evidence.contains(&"found"));
    assert!(evidence.contains(&"supported"));
    assert!(!finding.remediation().is_empty());
    assert_ne!(finding.remediation(), "No action required.");

    // No mutation: diagnosis must not migrate, rewrite, or delete the file.
    assert_eq!(
        before,
        snapshot(&profile.database()),
        "diagnosis modified the database"
    );
}

#[tokio::test]
async fn repair_does_not_touch_a_newer_schema_database() {
    let profile = TempProfile::new();
    let paths = profile.paths();
    paths
        .prepare()
        .unwrap_or_else(|error| panic!("prepare: {error}"));

    write_database(
        &profile.database(),
        &[
            "PRAGMA application_id = 1245794902",
            "PRAGMA user_version = 99",
        ],
    )
    .await;

    let before = snapshot(&profile.database());
    // Repair is restricted to idempotent, JARVIS-owned directory permissions; it
    // must never migrate, downgrade, or remove a database.
    let _ = repair(&paths);

    assert_eq!(
        before,
        snapshot(&profile.database()),
        "repair modified a database it is not allowed to touch"
    );
    let report = diagnose(&paths)
        .await
        .unwrap_or_else(|error| panic!("diagnose: {error}"));
    assert_has_code(&report, FindingCode::DatabaseSchemaTooNew);
}

#[tokio::test]
async fn a_corrupt_database_is_reported_as_unreadable() {
    let profile = TempProfile::new();
    let paths = profile.paths();
    paths
        .prepare()
        .unwrap_or_else(|error| panic!("prepare: {error}"));

    fs::write(profile.database(), b"not a sqlite database")
        .unwrap_or_else(|error| panic!("write fixture: {error}"));

    let before = snapshot(&profile.database());
    let report = diagnose(&paths)
        .await
        .unwrap_or_else(|error| panic!("diagnose: {error}"));

    assert_has_code(&report, FindingCode::DatabaseUnreadable);
    assert_eq!(report.outcome(), ReportOutcome::Failed);
    assert_eq!(before, snapshot(&profile.database()));
}

#[tokio::test]
async fn an_invalid_configuration_is_reported_with_a_remediation() {
    let profile = TempProfile::new();
    let paths = profile.paths();
    paths
        .prepare()
        .unwrap_or_else(|error| panic!("prepare: {error}"));

    fs::write(profile.config(), "schema_version = 1\nunknown_key = 5\n")
        .unwrap_or_else(|error| panic!("write fixture: {error}"));

    let before = snapshot(&profile.config());
    let report = diagnose(&paths)
        .await
        .unwrap_or_else(|error| panic!("diagnose: {error}"));

    assert_has_code(&report, FindingCode::ConfigInvalid);
    assert_eq!(report.outcome(), ReportOutcome::Failed);

    let finding = report
        .checks
        .iter()
        .flat_map(|check| check.findings.iter())
        .find(|finding| finding.code() == FindingCode::ConfigInvalid)
        .unwrap_or_else(|| panic!("finding not present"));
    assert!(
        finding.remediation().contains("config.toml"),
        "remediation should name the file to change: {}",
        finding.remediation()
    );

    // Doctor must not rewrite a broken configuration to a valid one.
    assert_eq!(before, snapshot(&profile.config()));
}

#[tokio::test]
async fn a_healthy_profile_reports_no_blocking_findings() {
    let profile = TempProfile::new();
    let paths = profile.paths();
    paths
        .prepare()
        .unwrap_or_else(|error| panic!("prepare: {error}"));

    let report = diagnose(&paths)
        .await
        .unwrap_or_else(|error| panic!("diagnose: {error}"));

    assert!(
        !report.has_errors(),
        "a prepared profile should not report errors: {:?}",
        codes(&report)
    );
    assert_has_code(&report, FindingCode::DatabaseAbsent);
    // The self-test must actually run and pass, not be skipped.
    assert_has_code(&report, FindingCode::RedactionSelfTestPassed);

    // A profile that has never been started is not "passed": there is no daemon,
    // no credential, and no log record yet. Reporting warnings is the honest
    // answer, and claiming success would be a false reassurance.
    assert_eq!(report.outcome(), ReportOutcome::Warnings);
    assert_has_code(&report, FindingCode::DaemonNotRunning);
    assert_has_code(&report, FindingCode::CredentialMissing);

    let json = report.to_json();
    assert_eq!(json["outcome"], "warnings");
    assert!(json["checks"].is_array());
}

#[tokio::test]
async fn the_report_never_contains_a_secret_value() {
    let profile = TempProfile::new();
    let paths = profile.paths();
    paths
        .prepare()
        .unwrap_or_else(|error| panic!("prepare: {error}"));

    // A realistic secret placed where doctor reads: the configuration.
    let canary = "ghp_doctorcanary4f8b2c1d9e7a6350";
    fs::write(
        profile.config(),
        format!("schema_version = 1\napi_key = \"{canary}\"\n"),
    )
    .unwrap_or_else(|error| panic!("write fixture: {error}"));

    let report = diagnose(&paths)
        .await
        .unwrap_or_else(|error| panic!("diagnose: {error}"));
    let json = report.to_json().to_string();

    assert!(
        !json.contains(canary),
        "doctor output leaked a secret value: {json}"
    );

    // The human rendering must not leak it either.
    let mut human = String::new();
    for check in &report.checks {
        for finding in &check.findings {
            human.push_str(finding.summary().as_str());
            for (_, value) in finding.evidence() {
                human.push_str(value);
            }
            human.push_str(finding.remediation());
        }
    }
    assert!(!human.contains(canary), "human rendering leaked a secret");
}

#[test]
fn finding_severities_are_bounded_and_ordered() {
    // Error is the only severity that makes the report blocking.
    assert!(Severity::Info < Severity::Warning);
    assert!(Severity::Warning < Severity::Error);
}
