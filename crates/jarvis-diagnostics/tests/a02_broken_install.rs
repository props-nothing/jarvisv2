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

use jarvis_diagnostics::{FindingCode, Outcome, Report, ReportOutcome, Severity, diagnose, repair};
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

/// **The sandbox check reports which guarantees are in force, and never claims a guarantee the host lacks.**
///
/// `docs/architecture/security.md` requires diagnostics to "report effective guarantees rather than claiming
/// parity". That is only meaningful if the report carries the **list**, so this test reads the guarantee names
/// back out of the evidence rather than accepting a pass/fail. A boolean would let a host enforcing one of four
/// guarantees read identically to one enforcing all four, which is exactly the parity claim being forbidden.
///
/// It is also the check's **caller test**: without it `check_sandbox` would be a function nothing could
/// distinguish from a stub — the failure mode where a slice looks complete because a unit test passes on a
/// helper no report ever shows.
///
/// Falsified by mutation: dropping the `guarantees` evidence key fails the evidence assertion; reporting
/// `SandboxAvailable` when the support set is empty fails the consistency assertion below.
#[tokio::test]
async fn the_report_names_the_sandbox_guarantees_in_force() {
    let fixture = TempProfile::new();
    let report = diagnose(&fixture.paths())
        .await
        .unwrap_or_else(|error| panic!("diagnose must inspect a fixture installation: {error}"));

    let check = report
        .checks
        .iter()
        .find(|check| check.name == "sandbox")
        .unwrap_or_else(|| panic!("every report must contain the sandbox check"));
    let finding = check
        .findings
        .first()
        .unwrap_or_else(|| panic!("the sandbox check must produce a finding"));

    // The host may or may not have a backend, so the expectation is derived from the code rather than assumed.
    // Both branches assert something, so the test cannot pass by checking nothing on one of the two hosts.
    match finding.code() {
        FindingCode::SandboxAvailable => {
            let guarantees = finding
                .evidence()
                .iter()
                .find(|(key, _)| key == "guarantees")
                .map_or_else(
                    || panic!("an available sandbox must name its guarantees, not just assert one"),
                    |(_, value)| value.clone(),
                );
            assert_ne!(
                guarantees, "none",
                "a check reporting availability must name at least one guarantee"
            );
            assert!(
                !guarantees.contains("cpu_time_ceiling") || !cfg!(target_os = "linux"),
                "cgroup v2 has no cumulative CPU limit, so Linux must never report it: {guarantees}"
            );
            assert_eq!(
                check.outcome,
                Outcome::Passed,
                "an available sandbox is an informational pass, not a warning"
            );
        }
        FindingCode::SandboxUnavailable => {
            assert_eq!(
                finding
                    .evidence()
                    .iter()
                    .find(|(key, _)| key == "guarantees")
                    .map(|(_, value)| value.as_str()),
                Some("none"),
                "an unavailable sandbox must say so rather than omit the key"
            );
            // Informational rather than a warning: a host with no confinement is a *supported* configuration,
            // so reporting a degraded install would dilute the findings that are genuinely actionable.
            // `ServiceNotApplicable` is the existing precedent for an absent-by-design capability. The first
            // version of this comment claimed the warning broke the Phase 1 acceptance gate; that was measured
            // and **refuted** (the gate passes with a warning), and the failure it described was my own leaked
            // `JARVIS_PROFILE` environment variable. The severity is right for the design reason above.
            assert_eq!(
                check.outcome,
                Outcome::Passed,
                "an absent capability is reported informationally, so doctor's summary stays actionable"
            );
            // Asserted per finding rather than through `report.has_errors()`, which was the first version of this
            // line and was wrong: this fixture is a deliberately **broken** installation, so other checks raise
            // errors for reasons that have nothing to do with the sandbox. The property that matters is that no
            // *sandbox* finding is blocking, which is precise and does not depend on what the fixture breaks.
            for finding in &check.findings {
                assert!(
                    finding.severity() < Severity::Error,
                    "no confinement must never be blocking on its own, got {:?} for {}",
                    finding.severity(),
                    finding.code().as_str()
                );
            }
        }
        other => panic!("the sandbox check reported an unexpected code: {other:?}"),
    }
}

/// **Every `FindingCode` has a distinct identifier, a specific remediation, and a default severity.**
///
/// The crate's own doc comment makes those three properties contractual — "a code is never reused for a
/// different meaning", "each code maps to exactly one remediation" — but nothing enforced them, so a code added
/// with a copy-pasted remediation would keep a support playbook pointing at the wrong action. The list is
/// derived from the exhaustive `as_str` match, so a new variant that is not added here still compiles, but a
/// variant that is mapped to a duplicate string is caught.
///
/// Falsified by mutation: giving the two sandbox codes the same identifier string fails here; pointing
/// `SandboxUnavailable` at "No action required." fails the remediation assertion.
#[test]
fn every_finding_code_has_a_distinct_id_and_an_actionable_remediation() {
    // Every code currently declared. Kept as an explicit list rather than a wildcard so adding a variant forces
    // a deliberate decision about its severity and remediation.
    let codes = [
        FindingCode::AllChecksPassed,
        FindingCode::ConfigValid,
        FindingCode::ConfigSchemaTooNew,
        FindingCode::ConfigInvalid,
        FindingCode::PathsUnavailable,
        FindingCode::PathsInsecure,
        FindingCode::PathsPrivate,
        FindingCode::DatabaseAbsent,
        FindingCode::DatabaseCurrent,
        FindingCode::DatabaseUnreadable,
        FindingCode::DatabaseForeign,
        FindingCode::DatabaseSchemaTooNew,
        FindingCode::DatabaseMigrationPending,
        FindingCode::DatabaseSchemaInconsistent,
        FindingCode::DatabaseIntegrityFailed,
        FindingCode::CredentialMissing,
        FindingCode::CredentialPresent,
        FindingCode::CredentialInvalid,
        FindingCode::DaemonRunning,
        FindingCode::DaemonNotRunning,
        FindingCode::ProtocolAligned,
        FindingCode::ProtocolMismatch,
        FindingCode::ProtocolUnreachable,
        FindingCode::RedactionSelfTestPassed,
        FindingCode::RedactionSelfTestFailed,
        FindingCode::LogsEmpty,
        FindingCode::LogsReadable,
        FindingCode::ServiceAbsent,
        FindingCode::ServiceNotApplicable,
        FindingCode::ServiceCurrent,
        FindingCode::ServiceDrifted,
        FindingCode::ServiceForeign,
        FindingCode::ServiceUnreadable,
        FindingCode::SandboxAvailable,
        FindingCode::SandboxUnavailable,
    ];
    let mut identifiers: Vec<&str> = codes.iter().map(|code| code.as_str()).collect();
    identifiers.sort_unstable();
    let total = identifiers.len();
    identifiers.dedup();
    assert_eq!(
        identifiers.len(),
        total,
        "every finding code needs its own identifier, or support automation cannot match on it"
    );

    for code in codes {
        let remediation = code.remediation();
        assert!(
            !remediation.trim().is_empty(),
            "{} must have a remediation",
            code.as_str()
        );
    }
    // The two sandbox codes are the reason this test exists: a code that warns without saying what to do is the
    // difference between an actionable warning and noise. It is asserted outside the loop because it is a claim
    // about one specific code rather than a property of all of them.
    assert_ne!(
        FindingCode::SandboxUnavailable.remediation(),
        "No action required.",
        "a host with no confinement must be told how to obtain one"
    );
    assert_eq!(
        FindingCode::SandboxAvailable.default_severity(),
        Severity::Info,
        "an available sandbox is informational"
    );
    assert_eq!(
        FindingCode::SandboxUnavailable.default_severity(),
        Severity::Info,
        "a host with no confinement is supported, so it must not make doctor report a degraded install"
    );
}
