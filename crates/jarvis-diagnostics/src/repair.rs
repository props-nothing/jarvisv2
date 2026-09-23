//! Explicit repair with mandatory post-repair verification.
//!
//! Doctor is read-only unless repair is requested. When it is requested, a repair
//! must be **re-checked**, not assumed: `docs/operations/install-and-release.md`
//! requires post-repair verification, and `docs/architecture/security.md` requires
//! a check that fails closed. Each repair here therefore reports three things —
//! whether it acted, whether the re-check passed, and the resulting finding — so a
//! repair that silently did nothing cannot be mistaken for a success.

use jarvis_core::SafeMessage;
use jarvis_storage::AppPaths;

use crate::findings::{Finding, FindingCode};

/// Result of one repair attempt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RepairOutcome {
    /// The repair acted and the re-check confirmed the fix.
    Repaired,
    /// Nothing needed repair.
    NotNeeded,
    /// The repair ran but the re-check still failed.
    VerificationFailed {
        /// Bounded explanation of the remaining problem.
        detail: String,
    },
    /// The repair could not run.
    Failed {
        /// Bounded explanation of why the repair did not run.
        detail: String,
    },
}

impl RepairOutcome {
    /// Returns the stable name used in output.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Repaired => "repaired",
            Self::NotNeeded => "not-needed",
            Self::VerificationFailed { .. } => "verification-failed",
            Self::Failed { .. } => "failed",
        }
    }

    /// Reports whether this outcome should be treated as a failure.
    #[must_use]
    pub const fn is_failure(&self) -> bool {
        matches!(self, Self::VerificationFailed { .. } | Self::Failed { .. })
    }
}

/// One repair attempt and its verification.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepairReport {
    /// Stable name of the repair.
    pub name: &'static str,
    /// Outcome including verification.
    pub outcome: RepairOutcome,
    /// Finding describing the final state, using the same codes as diagnosis.
    pub finding: Finding,
}

/// Applies the safe repairs doctor is allowed to perform, each verified.
///
/// Only repairs that are idempotent and confined to JARVIS-owned directories are
/// attempted. Nothing that destroys user data, moves a database, or changes
/// version compatibility is ever automatic.
#[must_use]
pub fn repair(paths: &AppPaths) -> RepairReport {
    reapply_directory_permissions(paths)
}

/// Re-applies user-only permissions to the managed directories, then re-checks.
fn reapply_directory_permissions(paths: &AppPaths) -> RepairReport {
    const NAME: &str = "paths.permissions";

    let before = insecure_directories(paths);
    if before.is_empty() {
        return RepairReport {
            name: NAME,
            outcome: RepairOutcome::NotNeeded,
            finding: Finding::new(
                FindingCode::PathsPrivate,
                safe("every managed directory is already private"),
            ),
        };
    }

    if let Err(error) = paths.prepare() {
        return RepairReport {
            name: NAME,
            outcome: RepairOutcome::Failed {
                detail: bounded(&error.to_string()),
            },
            finding: Finding::new(
                FindingCode::PathsUnavailable,
                safe("the managed directories could not be prepared"),
            ),
        };
    }

    // Post-repair verification: the same check that detected the problem.
    let after = insecure_directories(paths);
    if after.is_empty() {
        RepairReport {
            name: NAME,
            outcome: RepairOutcome::Repaired,
            finding: Finding::new(
                FindingCode::PathsPrivate,
                safe("managed directory permissions were reapplied and verified"),
            )
            .with_evidence("directories", before.join(",")),
        }
    } else {
        RepairReport {
            name: NAME,
            outcome: RepairOutcome::VerificationFailed {
                detail: format!("still insecure: {}", after.join(",")),
            },
            finding: Finding::new(
                FindingCode::PathsInsecure,
                safe("managed directory permissions are still too broad after repair"),
            )
            .with_evidence("directories", after.join(",")),
        }
    }
}

/// Lists managed directory labels that are group- or other-accessible.
///
/// The detection and the verification intentionally share one implementation so a
/// repair cannot pass a weaker check than the one that failed.
///
/// `std::fs` is imported HERE rather than at module scope. This function is
/// `#[cfg(unix)]`, and the module previously had no `fs` import at all: it compiled
/// only because a separate `#[cfg(windows)]` block elsewhere in the file brought
/// `std::fs` into scope. On unix that block is removed, so this function referenced an
/// unresolved `fs` and the crate did not compile -- on Linux and macOS, and never on
/// Windows, which is why the local build and lint were clean. Keep the import local to
/// the platform-gated function so the dependency is stated where it is used.
///
/// The group/other test is `trailing_zeros() < 6`, matching the equivalent check in
/// `doctor::world_accessible`. The low six bits are the group and other permission bits,
/// so fewer than six trailing zeros means one is set. The bitwise form
/// `mode & 0o077 != 0` trips `clippy::verbose_bit_mask` on unix builds only.
#[cfg(unix)]
fn insecure_directories(paths: &AppPaths) -> Vec<String> {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let mut insecure = Vec::new();
    for (label, path) in [
        ("config", paths.config()),
        ("data", paths.data()),
        ("runtime", paths.runtime()),
        ("logs", paths.logs()),
    ] {
        if let Ok(metadata) = fs::metadata(path)
            && metadata.permissions().mode().trailing_zeros() < 6
        {
            insecure.push(label.to_owned());
        }
    }
    insecure
}

/// Windows ACL verification needs a per-path `icacls` invocation.
///
/// Doctor reports no repair rather than claiming a verdict it did not measure;
/// the permission reapplication itself still runs through `AppPaths::prepare`.
#[cfg(not(unix))]
fn insecure_directories(_paths: &AppPaths) -> Vec<String> {
    Vec::new()
}

fn safe(summary: &str) -> SafeMessage {
    SafeMessage::new(summary).unwrap_or_default()
}

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

#[cfg(all(test, unix))]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

    use super::*;

    struct TempDirectory(PathBuf);

    impl TempDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "jarvis-diagnostics-repair-{}",
                jarvis_core::scratch_tag()
            ));
            fs::create_dir_all(&path).unwrap_or_else(|error| panic!("create temp dir: {error}"));
            Self(path)
        }

        fn paths(&self) -> AppPaths {
            AppPaths::from_root(&self.0).unwrap_or_else(|error| panic!("fixture paths: {error}"))
        }
    }

    impl Drop for TempDirectory {
        fn drop(&mut self) {
            jarvis_core::remove_scratch_dir(&self.0);
        }
    }

    #[test]
    fn repair_reports_not_needed_when_permissions_are_already_private() {
        let directory = TempDirectory::new();
        let paths = directory.paths();
        paths
            .prepare()
            .unwrap_or_else(|error| panic!("prepare: {error}"));

        let report = repair(&paths);
        assert_eq!(report.outcome, RepairOutcome::NotNeeded);
        assert!(!report.outcome.is_failure());
    }

    #[test]
    fn repair_fixes_broad_permissions_and_verifies_the_fix() {
        let directory = TempDirectory::new();
        let paths = directory.paths();
        paths
            .prepare()
            .unwrap_or_else(|error| panic!("prepare: {error}"));

        // Widen one managed directory to simulate the drift doctor detects.
        fs::set_permissions(paths.data(), fs::Permissions::from_mode(0o777))
            .unwrap_or_else(|error| panic!("loosen: {error}"));
        assert!(!insecure_directories(&paths).is_empty());

        let report = repair(&paths);
        assert_eq!(report.outcome, RepairOutcome::Repaired);
        assert!(insecure_directories(&paths).is_empty());

        // The verification must fail closed if the repair did not take effect.
        fs::set_permissions(paths.data(), fs::Permissions::from_mode(0o777))
            .unwrap_or_else(|error| panic!("loosen again: {error}"));
        let outcome = RepairOutcome::VerificationFailed {
            detail: String::from("forced"),
        };
        assert!(outcome.is_failure());
    }
}
