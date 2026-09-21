//! Stable finding identifiers for `jarvis doctor`.
//!
//! Codes are contractual: support automation and tests match on them, so a code
//! is never reused for a different meaning and never renamed once released
//! (`docs/operations/install-and-release.md`).
//!
//! Each code maps to exactly one remediation, which is what keeps a finding
//! actionable instead of merely descriptive.

use jarvis_core::SafeMessage;

/// Severity of a doctor finding, ordered from informational to blocking.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Severity {
    /// Expected state that needs no action.
    Info,
    /// Degraded or risky, but the installation can still run.
    Warning,
    /// Broken; the installation cannot work until this is resolved.
    Error,
}

impl Severity {
    /// Returns the stable lowercase name used in human and JSON output.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

/// A stable, machine-matchable doctor finding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FindingCode {
    /// Everything checked is in the expected state.
    AllChecksPassed,
    /// Configuration resolved and validated.
    ConfigValid,
    /// The configuration document targets a newer unsupported schema.
    ConfigSchemaTooNew,
    /// The configuration document is malformed or has unknown keys.
    ConfigInvalid,
    /// A managed directory is missing or could not be created.
    PathsUnavailable,
    /// A managed path is not private to the current user.
    PathsInsecure,
    /// Every managed directory exists and is private to the current user.
    PathsPrivate,
    /// No database file exists yet.
    DatabaseAbsent,
    /// The database schema matches this build.
    DatabaseCurrent,
    /// The SQLite file is corrupt or cannot be read.
    DatabaseUnreadable,
    /// The SQLite file is not a JARVIS database.
    DatabaseForeign,
    /// The SQLite file belongs to a newer unsupported schema.
    DatabaseSchemaTooNew,
    /// A supported older schema is pending migration.
    DatabaseMigrationPending,
    /// The schema markers disagree with each other.
    DatabaseSchemaInconsistent,
    /// The database reported integrity or foreign-key violations.
    DatabaseIntegrityFailed,
    /// No local client credential exists for this profile.
    CredentialMissing,
    /// The profile credential exists and is well formed.
    CredentialPresent,
    /// The local client credential is present but unusable.
    CredentialInvalid,
    /// A daemon currently owns the profile singleton lock.
    DaemonRunning,
    /// No daemon currently owns the profile singleton lock.
    DaemonNotRunning,
    /// The daemon binary and the CLI share a protocol version.
    ProtocolAligned,
    /// The daemon speaks a protocol version this build does not.
    ProtocolMismatch,
    /// A daemon appears to be running but did not complete a local handshake.
    ProtocolUnreachable,
    /// A redaction canary was masked by the logging sink.
    RedactionSelfTestPassed,
    /// A redaction canary survived and would have leaked.
    RedactionSelfTestFailed,
    /// The log directory contains no records yet.
    LogsEmpty,
    /// The structured log is readable.
    LogsReadable,
    /// No per-user service definition exists.
    ServiceAbsent,
    /// A service is not applicable to this installation mode.
    ServiceNotApplicable,
    /// The per-user service definition matches the desired state.
    ServiceCurrent,
    /// The per-user service definition differs from the desired state.
    ServiceDrifted,
    /// A service definition exists that JARVIS does not own.
    ServiceForeign,
    /// The per-user service definition could not be read.
    ServiceUnreadable,
}

impl FindingCode {
    /// Returns the stable identifier string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AllChecksPassed => "doctor.all_checks_passed",
            Self::ConfigValid => "config.valid",
            Self::ConfigSchemaTooNew => "config.schema_too_new",
            Self::ConfigInvalid => "config.invalid",
            Self::PathsUnavailable => "paths.unavailable",
            Self::PathsInsecure => "paths.insecure",
            Self::PathsPrivate => "paths.private",
            Self::DatabaseAbsent => "database.absent",
            Self::DatabaseCurrent => "database.current",
            Self::DatabaseUnreadable => "database.unreadable",
            Self::DatabaseForeign => "database.foreign",
            Self::DatabaseSchemaTooNew => "database.schema_too_new",
            Self::DatabaseMigrationPending => "database.migration_pending",
            Self::DatabaseSchemaInconsistent => "database.schema_inconsistent",
            Self::DatabaseIntegrityFailed => "database.integrity_failed",
            Self::CredentialMissing => "credential.missing",
            Self::CredentialPresent => "credential.present",
            Self::CredentialInvalid => "credential.invalid",
            Self::DaemonRunning => "daemon.running",
            Self::DaemonNotRunning => "daemon.not_running",
            Self::ProtocolAligned => "protocol.aligned",
            Self::ProtocolMismatch => "protocol.mismatch",
            Self::ProtocolUnreachable => "protocol.unreachable",
            Self::RedactionSelfTestPassed => "redaction.self_test_passed",
            Self::RedactionSelfTestFailed => "redaction.self_test_failed",
            Self::LogsEmpty => "logs.empty",
            Self::LogsReadable => "logs.readable",
            Self::ServiceAbsent => "service.absent",
            Self::ServiceNotApplicable => "service.not_applicable",
            Self::ServiceCurrent => "service.current",
            Self::ServiceDrifted => "service.drifted",
            Self::ServiceForeign => "service.foreign",
            Self::ServiceUnreadable => "service.unreadable",
        }
    }

    /// Returns the default severity for this code.
    #[must_use]
    pub const fn default_severity(self) -> Severity {
        match self {
            Self::DatabaseMigrationPending
            | Self::DaemonNotRunning
            | Self::LogsEmpty
            | Self::ServiceAbsent
            | Self::ServiceDrifted
            | Self::CredentialMissing => Severity::Warning,
            Self::AllChecksPassed
            | Self::ConfigValid
            | Self::PathsPrivate
            | Self::DatabaseAbsent
            | Self::DatabaseCurrent
            | Self::CredentialPresent
            | Self::DaemonRunning
            | Self::ProtocolAligned
            | Self::ServiceCurrent
            | Self::ServiceNotApplicable
            | Self::LogsReadable
            | Self::RedactionSelfTestPassed => Severity::Info,
            Self::ConfigSchemaTooNew
            | Self::ConfigInvalid
            | Self::PathsUnavailable
            | Self::PathsInsecure
            | Self::DatabaseUnreadable
            | Self::DatabaseForeign
            | Self::DatabaseSchemaTooNew
            | Self::DatabaseSchemaInconsistent
            | Self::DatabaseIntegrityFailed
            | Self::CredentialInvalid
            | Self::ProtocolMismatch
            | Self::ProtocolUnreachable
            | Self::ServiceForeign
            | Self::ServiceUnreadable
            | Self::RedactionSelfTestFailed => Severity::Error,
        }
    }

    /// Returns the specific remediation for this code.
    ///
    /// A remediation names the action and the command, so an operator does not
    /// have to infer what to do from the code.
    #[must_use]
    pub const fn remediation(self) -> &'static str {
        match self {
            Self::AllChecksPassed
            | Self::ConfigValid
            | Self::PathsPrivate
            | Self::DatabaseAbsent
            | Self::DatabaseCurrent
            | Self::CredentialPresent
            | Self::LogsReadable
            | Self::ProtocolAligned
            | Self::ServiceCurrent
            | Self::ServiceNotApplicable
            | Self::RedactionSelfTestPassed => "No action required.",
            Self::ConfigSchemaTooNew => {
                "Install the JARVIS build that supports this configuration schema, or restore an older config.toml."
            }
            Self::ConfigInvalid => {
                "Fix or remove the invalid keys in config.toml, then run `jarvis doctor` again."
            }
            Self::PathsUnavailable => {
                "Check that the application directories exist and are writable by the current user."
            }
            Self::PathsInsecure => {
                "Re-run `jarvisd` once to reapply user-only permissions, or fix the ownership of the reported path."
            }
            Self::DatabaseUnreadable => {
                "Move or delete the corrupt database and restore from a verified backup, then run `jarvisd`."
            }
            Self::DatabaseForeign => {
                "Point the data directory at the JARVIS database; this file belongs to another application."
            }
            Self::DatabaseSchemaTooNew => {
                "Install the newer JARVIS build that owns this schema. Do not delete the database."
            }
            Self::DatabaseMigrationPending => {
                "Start `jarvisd` once to apply the pending migration; a verified backup is taken first."
            }
            Self::DatabaseSchemaInconsistent => {
                "Restore the database from the verified pre-migration backup printed above."
            }
            Self::DatabaseIntegrityFailed => {
                "Restore the database from a verified backup; do not continue using this file."
            }
            Self::CredentialMissing => {
                "Start `jarvisd` once to issue the profile credential, then run `jarvis doctor` again."
            }
            Self::CredentialInvalid => {
                "Delete the client credential file and restart `jarvisd` to reissue it."
            }
            Self::DaemonRunning => "No action required unless the daemon should be stopped.",
            Self::DaemonNotRunning => "Start `jarvisd` to serve local clients.",
            Self::ProtocolMismatch => {
                "Replace the older of the CLI and `jarvisd` so both speak the same protocol version."
            }
            Self::ProtocolUnreachable => {
                "Check that the running daemon is healthy and that its socket or pipe is not owned by another account."
            }
            Self::RedactionSelfTestFailed => {
                "Stop using this build and report a secret-redaction defect; diagnostics may leak secrets."
            }
            Self::LogsEmpty => "No action required; records appear once the daemon runs.",
            Self::ServiceAbsent => {
                "No action required unless this installation should start automatically at logon."
            }
            Self::ServiceDrifted => {
                "Reconcile the stored definition so it points at the current daemon binary."
            }
            Self::ServiceForeign => {
                "Inspect the existing definition and remove it yourself; JARVIS never overwrites a definition it does not own."
            }
            Self::ServiceUnreadable => {
                "Check the definition file's permissions and contents before deciding whether to replace it."
            }
        }
    }
}

/// One doctor finding: a stable code, bounded facts, and a specific remediation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Finding {
    code: FindingCode,
    severity: Severity,
    summary: SafeMessage,
    evidence: Vec<(String, String)>,
}

impl Finding {
    /// Builds a finding with the code's default severity.
    #[must_use]
    pub fn new(code: FindingCode, summary: SafeMessage) -> Self {
        Self {
            code,
            severity: code.default_severity(),
            summary,
            evidence: Vec::new(),
        }
    }

    /// Builds a finding with an explicit severity override.
    #[must_use]
    pub fn with_severity(mut self, severity: Severity) -> Self {
        self.severity = severity;
        self
    }

    /// Attaches one bounded, secret-free fact.
    #[must_use]
    pub fn with_evidence(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.evidence.push((key.into(), value.into()));
        self
    }

    /// Returns the stable code.
    #[must_use]
    pub const fn code(&self) -> FindingCode {
        self.code
    }

    /// Returns the severity.
    #[must_use]
    pub const fn severity(&self) -> Severity {
        self.severity
    }

    /// Returns the bounded summary.
    #[must_use]
    pub const fn summary(&self) -> &SafeMessage {
        &self.summary
    }

    /// Returns the attached facts.
    #[must_use]
    pub fn evidence(&self) -> &[(String, String)] {
        &self.evidence
    }

    /// Returns the specific remediation for this finding's code.
    #[must_use]
    pub const fn remediation(&self) -> &'static str {
        self.code.remediation()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    const ALL_CODES: &[FindingCode] = &[
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
    ];

    #[test]
    fn codes_are_unique_and_namespaced() {
        let mut seen = BTreeSet::new();
        for code in ALL_CODES {
            let name = code.as_str();
            assert!(seen.insert(name), "duplicate finding code: {name}");
            assert!(
                name.contains('.'),
                "finding code {name} should be namespaced by area"
            );
            assert!(!name.is_empty());
        }
        assert_eq!(seen.len(), ALL_CODES.len());
    }

    #[test]
    fn every_code_has_a_specific_remediation() {
        for code in ALL_CODES {
            let remediation = code.remediation();
            assert!(!remediation.is_empty(), "{code:?} has no remediation");
            assert!(
                remediation.ends_with('.'),
                "{code:?} remediation should be a sentence"
            );
            // A remediation that only restates the problem is not actionable.
            if code.default_severity() == Severity::Error {
                assert_ne!(
                    remediation, "No action required.",
                    "{code:?} is an error but claims no action is required"
                );
            }
        }
    }

    #[test]
    fn severity_orders_from_informational_to_blocking() {
        assert!(Severity::Info < Severity::Warning);
        assert!(Severity::Warning < Severity::Error);
        assert_eq!(Severity::Error.as_str(), "error");
    }
}
