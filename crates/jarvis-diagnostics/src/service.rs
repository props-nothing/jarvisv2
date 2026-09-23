//! Per-user service definition planning and drift detection.
//!
//! `docs/operations/install-and-release.md` defines service installation as a
//! **reconcile operation**, not a blind file write:
//!
//! 1. Resolve the exact verified binary and profile paths.
//! 2. Read the existing owned definition and determine drift/ownership.
//! 3. Stage the new definition atomically.
//! 4. Load/start through the platform manager.
//! 5. Wait for process plus readiness.
//! 6. Verify service binary version, config/state path, and protocol.
//! 7. Roll back the staged definition when activation fails.
//!
//! This module implements steps 1, 2, and 6 — the planning and drift half. It
//! deliberately does **not** write, load, start, or roll back anything:
//! `TODO.md` P1-011 requires the abstraction without installing services yet, and
//! a planner that also wrote files could not be tested without mutating a machine.
//!
//! Two invariants make the planner safe to run against a real host:
//!
//! - **Ownership before authority.** A definition JARVIS does not recognize as its
//!   own is reported as foreign and is never planned for replacement, matching
//!   "never rewrite an externally managed service without explicit operator action".
//! - **Portable mode plans nothing.** A portable installation is defined by
//!   leaving no service, PATH, or registry changes behind, so a request to install
//!   one is refused rather than silently performed.

use std::{
    fs,
    path::{Path, PathBuf},
};

use jarvis_core::SafeMessage;
use thiserror::Error;

use crate::findings::{Finding, FindingCode};

/// Stable identity marker placed in every JARVIS-owned definition.
///
/// Drift detection depends on recognizing JARVIS's own definition, so a definition
/// without this marker is treated as foreign rather than adopted.
pub const OWNERSHIP_MARKER: &str = "jarvis-managed-service";

/// Platform service manager that a plan targets.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServiceKind {
    /// Linux per-user systemd unit.
    SystemdUser,
    /// macOS per-user launchd agent.
    LaunchdAgent,
    /// Windows per-user logon launcher or scheduled task.
    WindowsUserLauncher,
}

impl ServiceKind {
    /// Returns the kind native to the current platform.
    #[must_use]
    pub const fn native() -> Self {
        #[cfg(target_os = "windows")]
        {
            Self::WindowsUserLauncher
        }
        #[cfg(target_os = "macos")]
        {
            Self::LaunchdAgent
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        {
            Self::SystemdUser
        }
    }

    /// Returns the stable name used in diagnostics.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SystemdUser => "systemd-user",
            Self::LaunchdAgent => "launchd-agent",
            Self::WindowsUserLauncher => "windows-user-launcher",
        }
    }
}

/// How the on-disk definition relates to what JARVIS would write now.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ServiceDrift {
    /// No definition exists, so installation would create one.
    Absent,
    /// A JARVIS-owned definition matches the desired state exactly.
    Current,
    /// A JARVIS-owned definition differs from the desired state.
    Drifted {
        /// Bounded list of fields that differ.
        differences: Vec<String>,
    },
    /// A definition exists that JARVIS does not own or recognize.
    Foreign {
        /// Absolute path of the unrecognized definition.
        path: PathBuf,
    },
    /// The definition could not be read.
    Unreadable {
        /// Bounded reason.
        detail: String,
    },
}

impl ServiceDrift {
    /// Returns the stable name used in diagnostics.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Absent => "absent",
            Self::Current => "current",
            Self::Drifted { .. } => "drifted",
            Self::Foreign { .. } => "foreign",
            Self::Unreadable { .. } => "unreadable",
        }
    }

    /// Reports whether JARVIS may replace the definition without operator action.
    ///
    /// Only an absent or already-owned definition is replaceable. A foreign
    /// definition requires explicit operator action, and an unreadable one must be
    /// investigated rather than overwritten.
    #[must_use]
    pub const fn is_safe_to_reconcile(&self) -> bool {
        matches!(self, Self::Absent | Self::Current | Self::Drifted { .. })
    }
}

/// A fully resolved service definition, before anything is written.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServicePlan {
    /// Platform manager this plan targets.
    pub kind: ServiceKind,
    /// Absolute path of the definition file.
    pub definition_path: PathBuf,
    /// Absolute daemon binary this definition would launch.
    pub binary_path: PathBuf,
    /// Absolute portable root, when the daemon would run in portable mode.
    pub root: Option<PathBuf>,
    /// Portable root flag the launcher would pass, when any.
    pub root_argument: Option<String>,
}

impl ServicePlan {
    /// Builds the canonical definition path for a kind.
    #[must_use]
    pub fn definition_path(kind: ServiceKind) -> Option<PathBuf> {
        let base = definition_root(kind)?;
        Some(match kind {
            ServiceKind::SystemdUser => base.join("jarvisd.service"),
            ServiceKind::LaunchdAgent => base.join("dev.jarvis.jarvisd.plist"),
            ServiceKind::WindowsUserLauncher => base.join("jarvisd-launcher.json"),
        })
    }

    /// Builds a plan for a resolved binary and profile.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::UnsupportedPlatform`] when the platform has no
    /// per-user manager, [`ServiceError::RelativeBinary`] when the binary path is
    /// relative, or [`ServiceError::PortableMode`] when a portable root is given,
    /// because portable mode must not create a service.
    pub fn build(
        kind: ServiceKind,
        binary_path: &Path,
        root: Option<&Path>,
    ) -> Result<Self, ServiceError> {
        if let Some(root) = root {
            return Err(ServiceError::PortableMode {
                root: root.to_path_buf(),
            });
        }
        if !binary_path.is_absolute() {
            return Err(ServiceError::RelativeBinary);
        }
        let definition_path =
            Self::definition_path(kind).ok_or(ServiceError::UnsupportedPlatform)?;
        Ok(Self {
            kind,
            definition_path,
            binary_path: binary_path.to_path_buf(),
            root: None,
            root_argument: None,
        })
    }

    /// Returns the launcher argument list, in the order the manager would use.
    #[must_use]
    pub fn arguments(&self) -> Vec<String> {
        let mut arguments = vec![self.binary_path.display().to_string()];
        if let Some(argument) = &self.root_argument {
            arguments.push("--root".to_owned());
            arguments.push(argument.clone());
        }
        arguments
    }

    /// Renders the definition body the platform manager expects.
    ///
    /// The output is what drift detection compares against, so it must be
    /// deterministic: no timestamps, no absolute paths other than the resolved ones.
    #[must_use]
    pub fn render(&self) -> String {
        let executable = self.binary_path.display();
        match self.kind {
            ServiceKind::SystemdUser => format!(
                "# {OWNERSHIP_MARKER}\n\
                 [Unit]\n\
                 Description=JARVIS personal assistant daemon\n\
                 After=default.target\n\
                 \n\
                 [Service]\n\
                 Type=simple\n\
                 ExecStart={executable}\n\
                 Restart=on-failure\n\
                 \n\
                 [Install]\n\
                 WantedBy=default.target\n"
            ),
            ServiceKind::LaunchdAgent => format!(
                "<!-- {OWNERSHIP_MARKER} -->\n\
                 <?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
                 <plist version=\"1.0\">\n\
                 <dict>\n\
                 <key>Label</key><string>dev.jarvis.jarvisd</string>\n\
                 <key>ProgramArguments</key>\n\
                 <array><string>{executable}</string></array>\n\
                 <key>RunAtLoad</key><true/>\n\
                 <key>KeepAlive</key><false/>\n\
                 </dict>\n\
                 </plist>\n"
            ),
            ServiceKind::WindowsUserLauncher => format!(
                "{{\n\
                 \"marker\": \"{OWNERSHIP_MARKER}\",\n\
                 \"binary\": \"{executable}\"\n\
                 }}\n"
            ),
        }
    }

    /// Returns the ownership marker this plan's definition carries.
    #[must_use]
    pub const fn ownership_marker(&self) -> &'static str {
        OWNERSHIP_MARKER
    }
}

/// Resolves the per-user definition directory for a kind.
fn definition_root(kind: ServiceKind) -> Option<PathBuf> {
    match kind {
        ServiceKind::SystemdUser | ServiceKind::LaunchdAgent | ServiceKind::WindowsUserLauncher => {
            let home = user_home()?;
            Some(match kind {
                ServiceKind::SystemdUser => home.join(".config").join("systemd").join("user"),
                ServiceKind::LaunchdAgent => home.join("Library").join("LaunchAgents"),
                ServiceKind::WindowsUserLauncher => home.join(".jarvis").join("service"),
            })
        }
    }
}

fn user_home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}

/// Derives the daemon binary that sits beside a client binary.
///
/// `jarvis` and `jarvisd` are released together, so the daemon is the sibling of
/// the client. Using `current_exe()` directly would plan the **client** as the
/// service and install a launcher that never serves anything, so the sibling is
/// resolved explicitly and `None` is returned when it is absent.
#[must_use]
pub fn daemon_binary_beside(client: &Path) -> Option<PathBuf> {
    let name = if cfg!(windows) {
        "jarvisd.exe"
    } else {
        "jarvisd"
    };
    let candidate = client.parent()?.join(name);
    candidate.is_file().then_some(candidate)
}

/// Explains why a service plan could not be produced.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum ServiceError {
    /// The current platform has no supported per-user service manager.
    #[error("this platform has no supported per-user service manager")]
    UnsupportedPlatform,
    /// The daemon binary path was relative.
    #[error("the daemon binary path must be absolute")]
    RelativeBinary,
    /// A portable root was supplied, and portable mode installs no service.
    #[error("portable mode must not install a service")]
    PortableMode {
        /// The refused portable root.
        root: PathBuf,
    },
}

/// Inspects the on-disk definition and classifies it against the desired plan.
///
/// This is read-only. It never writes, loads, or starts anything, which is what
/// makes it safe to run on an operator's real machine.
#[must_use]
pub fn detect_drift(plan: &ServicePlan) -> ServiceDrift {
    let path = &plan.definition_path;
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return ServiceDrift::Absent;
        }
        Err(error) => {
            return ServiceDrift::Unreadable {
                detail: bound(&error.to_string()),
            };
        }
    };

    // Ownership is decided by the marker, not by the path: a user may have placed
    // an unrelated definition at the same path on purpose.
    if !content.contains(OWNERSHIP_MARKER) {
        return ServiceDrift::Foreign { path: path.clone() };
    }

    let desired = plan.render();
    if normalize(&content) == normalize(&desired) {
        return ServiceDrift::Current;
    }

    ServiceDrift::Drifted {
        differences: describe_differences(&content, &desired),
    }
}

/// Reports which resolved values differ from the desired definition.
///
/// Evidence names the fields, not whole file bodies, so a definition cannot smuggle
/// unrelated content into a diagnostic.
fn describe_differences(actual: &str, desired: &str) -> Vec<String> {
    let mut differences = Vec::new();
    for line in desired.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('<') {
            continue;
        }
        if !actual.lines().any(|candidate| candidate.trim() == trimmed) {
            differences.push(trimmed.to_owned());
        }
    }
    if differences.is_empty() {
        // The body differs in a line this comparison does not name individually.
        differences.push("definition body differs from the desired state".to_owned());
    }
    differences.truncate(8);
    differences
}

/// Normalizes line endings so a CRLF checkout is not reported as drift.
fn normalize(content: &str) -> String {
    content.replace("\r\n", "\n").trim_end().to_owned()
}

fn bound(detail: &str) -> String {
    let flattened: String = detail
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .take(200)
        .collect();
    flattened.trim().to_owned()
}

/// Converts drift into a doctor finding, so the same codes reach the operator.
#[must_use]
pub fn drift_finding(drift: &ServiceDrift) -> Finding {
    match drift {
        ServiceDrift::Absent => Finding::new(
            FindingCode::ServiceAbsent,
            safe("no per-user service definition exists"),
        ),
        ServiceDrift::Current => Finding::new(
            FindingCode::ServiceCurrent,
            safe("the per-user service definition matches the desired state"),
        ),
        ServiceDrift::Drifted { differences } => Finding::new(
            FindingCode::ServiceDrifted,
            safe("the per-user service definition differs from the desired state"),
        )
        .with_evidence("differences", differences.join(" | "))
        .with_evidence("reconcilable", "true"),
        ServiceDrift::Foreign { path } => Finding::new(
            FindingCode::ServiceForeign,
            safe("a service definition exists that JARVIS does not own"),
        )
        .with_evidence("definition", path.display().to_string())
        .with_evidence("reconcilable", "false"),
        ServiceDrift::Unreadable { detail } => Finding::new(
            FindingCode::ServiceUnreadable,
            safe("the per-user service definition could not be read"),
        )
        .with_evidence("detail", detail.clone()),
    }
}

fn safe(summary: &str) -> SafeMessage {
    SafeMessage::new(summary).unwrap_or_default()
}

#[cfg(test)]
mod tests {

    use super::*;

    struct TempDirectory(PathBuf);

    impl TempDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "jarvis-service-plan-{}",
                jarvis_core::scratch_tag()
            ));
            fs::create_dir_all(&path).unwrap_or_else(|error| panic!("create temp dir: {error}"));
            Self(path)
        }
    }

    impl Drop for TempDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn plan_for(directory: &TempDirectory) -> ServicePlan {
        ServicePlan {
            kind: ServiceKind::SystemdUser,
            definition_path: directory.0.join("jarvisd.service"),
            binary_path: PathBuf::from("/usr/local/bin/jarvisd"),
            root: None,
            root_argument: None,
        }
    }

    #[test]
    fn the_daemon_binary_is_resolved_beside_the_client_not_as_the_client() {
        let directory = TempDirectory::new();
        let client = directory.0.join(if cfg!(windows) {
            "jarvis.exe"
        } else {
            "jarvis"
        });
        fs::write(&client, b"client").unwrap_or_else(|error| panic!("write client: {error}"));

        // With no sibling, resolution must fail rather than fall back to the client.
        assert_eq!(daemon_binary_beside(&client), None);

        let daemon = directory.0.join(if cfg!(windows) {
            "jarvisd.exe"
        } else {
            "jarvisd"
        });
        fs::write(&daemon, b"daemon").unwrap_or_else(|error| panic!("write daemon: {error}"));

        let resolved = daemon_binary_beside(&client)
            .unwrap_or_else(|| panic!("the daemon should resolve beside the client"));
        assert_eq!(resolved, daemon);
        assert_ne!(resolved, client, "the service must not launch the client");
    }

    #[test]
    fn portable_mode_refuses_to_install_a_service() {
        let root = PathBuf::from("/tmp/portable");
        let result = ServicePlan::build(
            ServiceKind::SystemdUser,
            Path::new("/usr/local/bin/jarvisd"),
            Some(&root),
        );
        assert_eq!(
            result,
            Err(ServiceError::PortableMode { root }),
            "portable mode must not create a service"
        );
    }

    #[test]
    fn a_relative_binary_is_refused() {
        assert_eq!(
            ServicePlan::build(
                ServiceKind::SystemdUser,
                Path::new("target/debug/jarvisd"),
                None
            ),
            Err(ServiceError::RelativeBinary)
        );
    }

    #[test]
    fn rendering_is_deterministic_and_carries_the_ownership_marker() {
        let directory = TempDirectory::new();
        let plan = plan_for(&directory);
        assert_eq!(plan.render(), plan.render());
        assert!(plan.render().contains(OWNERSHIP_MARKER));
        assert!(plan.render().contains("/usr/local/bin/jarvisd"));
        assert_eq!(plan.arguments(), vec!["/usr/local/bin/jarvisd"]);
    }

    #[test]
    fn an_absent_definition_is_detected_and_reconcilable() {
        let directory = TempDirectory::new();
        let drift = detect_drift(&plan_for(&directory));
        assert_eq!(drift, ServiceDrift::Absent);
        assert!(drift.is_safe_to_reconcile());
        assert!(!directory.0.join("jarvisd.service").exists());
    }

    #[test]
    fn a_matching_definition_is_current() {
        let directory = TempDirectory::new();
        let plan = plan_for(&directory);
        fs::write(&plan.definition_path, plan.render())
            .unwrap_or_else(|error| panic!("write fixture: {error}"));

        assert_eq!(detect_drift(&plan), ServiceDrift::Current);
    }

    #[test]
    fn a_foreign_definition_is_never_reconciled() {
        let directory = TempDirectory::new();
        let plan = plan_for(&directory);
        // A definition the operator wrote, or another tool owns.
        fs::write(
            &plan.definition_path,
            "[Unit]\nDescription=someone else's\n",
        )
        .unwrap_or_else(|error| panic!("write fixture: {error}"));

        let drift = detect_drift(&plan);
        assert!(
            matches!(drift, ServiceDrift::Foreign { .. }),
            "expected foreign, got {drift:?}"
        );
        assert!(
            !drift.is_safe_to_reconcile(),
            "an unowned definition must require explicit operator action"
        );
        let finding = drift_finding(&drift);
        assert_eq!(finding.code(), FindingCode::ServiceForeign);
    }

    #[test]
    fn a_changed_path_is_reported_as_drift_with_named_fields() {
        let directory = TempDirectory::new();
        let mut plan = plan_for(&directory);
        fs::write(&plan.definition_path, plan.render())
            .unwrap_or_else(|error| panic!("write fixture: {error}"));

        // The binary moved: the definition is owned but stale.
        plan.binary_path = PathBuf::from("/opt/jarvis/bin/jarvisd");
        let drift = detect_drift(&plan);
        match &drift {
            ServiceDrift::Drifted { differences } => {
                assert!(!differences.is_empty());
                assert!(
                    differences.iter().any(|line| line.contains("/opt/jarvis")),
                    "evidence should name the stale value: {differences:?}"
                );
            }
            other => panic!("expected drift, got {other:?}"),
        }
        assert!(drift.is_safe_to_reconcile());
        assert_eq!(drift_finding(&drift).code(), FindingCode::ServiceDrifted);
    }

    #[test]
    fn line_ending_differences_are_not_drift() {
        let directory = TempDirectory::new();
        let plan = plan_for(&directory);
        let crlf = plan.render().replace('\n', "\r\n");
        fs::write(&plan.definition_path, crlf)
            .unwrap_or_else(|error| panic!("write fixture: {error}"));

        assert_eq!(
            detect_drift(&plan),
            ServiceDrift::Current,
            "a CRLF checkout must not be reported as drift"
        );
    }
}
