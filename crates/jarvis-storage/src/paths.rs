use std::{
    collections::HashSet,
    fmt, fs, io,
    path::{Path, PathBuf},
};

use etcetera::BaseStrategy;
use thiserror::Error;

#[cfg(target_os = "macos")]
use etcetera::base_strategy::Apple as NativeStrategy;
#[cfg(target_os = "windows")]
use etcetera::base_strategy::Windows as NativeStrategy;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
use etcetera::base_strategy::Xdg as NativeStrategy;

/// Identifies one managed application directory without exposing its full path.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PathKind {
    /// Versioned user configuration.
    Config,
    /// Canonical databases and durable local objects.
    Data,
    /// Rebuildable cached data.
    Cache,
    /// Durable machine-local state.
    State,
    /// Login- or process-lifetime coordination files.
    Runtime,
    /// Structural application logs.
    Logs,
}

impl fmt::Display for PathKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Config => "config",
            Self::Data => "data",
            Self::Cache => "cache",
            Self::State => "state",
            Self::Runtime => "runtime",
            Self::Logs => "logs",
        })
    }
}

/// Describes whether the runtime directory came from a native login-lifetime facility.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimePathSource {
    /// The OS supplied a dedicated per-user runtime directory.
    Native,
    /// JARVIS had to use a private persistent-state subdirectory.
    StateFallback,
    /// The runtime directory sits inside an explicit portable root.
    PortableRoot,
}

impl RuntimePathSource {
    /// Returns the stable name used in diagnostics.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::StateFallback => "state-fallback",
            Self::PortableRoot => "portable-root",
        }
    }
}

/// How a profile's directories were selected.
///
/// This is reported by `doctor` and `status` so an operator can tell a portable
/// installation from an installed one without inspecting paths by hand.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PathMode {
    /// OS-native per-user directories.
    Native,
    /// A caller-supplied root directory, used by portable mode and tests.
    Portable,
}

impl PathMode {
    /// Returns the stable name used in diagnostics.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Portable => "portable",
        }
    }
}

/// Returns the canonical absolute layout beneath an explicit root directory.
///
/// Returns `None` when the root is relative, because a relative root would place
/// state under the process working directory. `docs/research/integrations/os-application-directories.md`
/// forbids silently using the current directory, so callers must resolve the root
/// themselves and the failure is explicit.
#[must_use]
pub fn portable_layout(root: &Path) -> Option<AppPaths> {
    if !root.is_absolute() {
        return None;
    }
    Some(AppPaths {
        config: root.join("config"),
        data: root.join("data"),
        cache: root.join("cache"),
        state: root.join("state"),
        runtime: root.join("runtime"),
        logs: root.join("logs"),
        runtime_source: RuntimePathSource::PortableRoot,
    })
}

/// Errors produced while resolving or securing application directories.
#[derive(Debug, Error)]
pub enum PathError {
    /// The operating system did not provide a valid current-user profile.
    #[error("the operating system did not provide a valid user profile")]
    UserProfileUnavailable,
    /// A native base directory was relative and therefore unsafe.
    #[error("the resolved {kind} directory is not absolute")]
    RelativePath {
        /// The affected directory category.
        kind: PathKind,
    },
    /// A managed endpoint was a symbolic link.
    #[error("the managed {kind} directory is a symbolic link")]
    SymbolicLink {
        /// The affected directory category.
        kind: PathKind,
    },
    /// A managed endpoint existed but was not a directory.
    #[error("the managed {kind} path is not a directory")]
    NotDirectory {
        /// The affected directory category.
        kind: PathKind,
    },
    /// A managed file endpoint existed but was not a regular file.
    #[error("the managed {kind} path is not a regular file")]
    NotFile {
        /// The affected file's directory category.
        kind: PathKind,
    },
    /// A filesystem operation failed.
    #[error("failed to {operation} the {kind} directory")]
    Io {
        /// The affected directory category.
        kind: PathKind,
        /// The bounded operation name.
        operation: &'static str,
        /// The underlying operating-system error.
        #[source]
        source: io::Error,
    },
    /// A required native permission tool could not start.
    #[error("could not start the native {tool} permission tool")]
    PermissionToolUnavailable {
        /// The native tool name.
        tool: &'static str,
        /// The process-spawn error.
        #[source]
        source: io::Error,
    },
    /// A native permission command rejected the operation.
    #[error("the native {tool} permission tool failed with exit code {exit_code:?}")]
    PermissionToolFailed {
        /// The native tool name.
        tool: &'static str,
        /// The process exit code, when the OS supplied one.
        exit_code: Option<i32>,
    },
    /// The current Windows access token did not yield a valid SID.
    #[error("the current Windows account SID could not be determined")]
    InvalidWindowsPrincipal,
}

/// Resolved per-user directories used by the local JARVIS profile.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppPaths {
    config: PathBuf,
    data: PathBuf,
    cache: PathBuf,
    state: PathBuf,
    runtime: PathBuf,
    logs: PathBuf,
    runtime_source: RuntimePathSource,
}

impl AppPaths {
    /// Resolves native per-user directories without creating them.
    ///
    /// # Errors
    ///
    /// Returns [`PathError::UserProfileUnavailable`] when the OS cannot resolve
    /// the current user's base paths, or [`PathError::RelativePath`] if a native
    /// facility supplies a relative path.
    pub fn resolve_native() -> Result<Self, PathError> {
        let base = NativeStrategy::new().map_err(|_| PathError::UserProfileUnavailable)?;
        Self::from_base_strategy(&base)
    }

    /// Builds the standard managed layout beneath an explicit root directory.
    ///
    /// This supports isolated test profiles and portable mode, where the whole
    /// installation lives under one directory instead of the OS locations.
    ///
    /// # Errors
    ///
    /// Returns [`PathError::RelativePath`] when the root is not absolute.
    pub fn from_root(root: &Path) -> Result<Self, PathError> {
        portable_layout(root).ok_or(PathError::RelativePath {
            kind: PathKind::Data,
        })
    }

    /// Returns how this profile's directories were selected.
    #[must_use]
    pub const fn mode(&self) -> PathMode {
        match self.runtime_source {
            RuntimePathSource::PortableRoot => PathMode::Portable,
            RuntimePathSource::Native | RuntimePathSource::StateFallback => PathMode::Native,
        }
    }

    /// Creates all managed directories and enforces user-only access.
    ///
    /// This operation is idempotent and rechecks existing endpoints.
    ///
    /// # Errors
    ///
    /// Returns [`PathError`] when a path is a symlink or non-directory, a
    /// filesystem operation fails, or the platform cannot enforce its private
    /// permission policy.
    pub fn prepare(&self) -> Result<(), PathError> {
        let mut managed = self.managed_paths();
        managed.sort_by_key(|(_, path)| path.components().count());

        let mut prepared = HashSet::new();
        for (kind, path) in managed {
            if prepared.insert(path.to_path_buf()) {
                prepare_private_directory(kind, path)?;
            }
        }
        Ok(())
    }

    /// Returns the configuration directory.
    #[must_use]
    pub fn config(&self) -> &Path {
        &self.config
    }

    /// Returns the canonical local data directory.
    #[must_use]
    pub fn data(&self) -> &Path {
        &self.data
    }

    /// Returns the rebuildable cache directory.
    #[must_use]
    pub fn cache(&self) -> &Path {
        &self.cache
    }

    /// Returns the durable local state directory.
    #[must_use]
    pub fn state(&self) -> &Path {
        &self.state
    }

    /// Returns the runtime coordination directory.
    #[must_use]
    pub fn runtime(&self) -> &Path {
        &self.runtime
    }

    /// Returns the structural log directory.
    #[must_use]
    pub fn logs(&self) -> &Path {
        &self.logs
    }

    /// Returns how the runtime directory was selected.
    #[must_use]
    pub const fn runtime_source(&self) -> RuntimePathSource {
        self.runtime_source
    }

    fn from_base_strategy(base: &impl BaseStrategy) -> Result<Self, PathError> {
        let component = application_component();
        let config = native_config_base(base).join(component);
        let data = native_data_base(base).join(component);

        #[cfg(target_os = "windows")]
        let cache = data.join("cache");
        #[cfg(not(target_os = "windows"))]
        let cache = base.cache_dir().join(component);

        #[cfg(target_os = "linux")]
        let state = base
            .state_dir()
            .unwrap_or_else(|| base.data_dir())
            .join(component);
        #[cfg(not(target_os = "linux"))]
        let state = data.clone();

        #[cfg(target_os = "macos")]
        let logs = base.home_dir().join("Library").join("Logs").join(component);
        #[cfg(not(target_os = "macos"))]
        let logs = state.join("logs");

        let (runtime, runtime_source) = match base.runtime_dir() {
            Some(runtime) => (runtime.join(component), RuntimePathSource::Native),
            None => (state.join("runtime"), RuntimePathSource::StateFallback),
        };

        Self {
            config,
            data,
            cache,
            state,
            runtime,
            logs,
            runtime_source,
        }
        .validate()
    }

    fn validate(self) -> Result<Self, PathError> {
        for (kind, path) in self.managed_paths() {
            if !path.is_absolute() {
                return Err(PathError::RelativePath { kind });
            }
        }
        Ok(self)
    }

    fn managed_paths(&self) -> Vec<(PathKind, &Path)> {
        vec![
            (PathKind::Config, &self.config),
            (PathKind::Data, &self.data),
            (PathKind::Cache, &self.cache),
            (PathKind::State, &self.state),
            (PathKind::Logs, &self.logs),
            (PathKind::Runtime, &self.runtime),
        ]
    }
}

#[cfg(target_os = "macos")]
fn native_config_base(base: &impl BaseStrategy) -> PathBuf {
    base.data_dir()
}

#[cfg(not(target_os = "macos"))]
fn native_config_base(base: &impl BaseStrategy) -> PathBuf {
    base.config_dir()
}

#[cfg(target_os = "windows")]
fn native_data_base(base: &impl BaseStrategy) -> PathBuf {
    base.cache_dir()
}

#[cfg(not(target_os = "windows"))]
fn native_data_base(base: &impl BaseStrategy) -> PathBuf {
    base.data_dir()
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
const fn application_component() -> &'static str {
    "JARVIS"
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
const fn application_component() -> &'static str {
    "jarvis"
}

fn ensure_directory_endpoint(kind: PathKind, path: &Path) -> Result<(), PathError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(PathError::SymbolicLink { kind });
        }
        Ok(metadata) if !metadata.is_dir() => return Err(PathError::NotDirectory { kind }),
        Ok(_) => return Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(PathError::Io {
                kind,
                operation: "inspect",
                source,
            });
        }
    }

    create_private_directory(kind, path)?;
    let metadata = fs::symlink_metadata(path).map_err(|source| PathError::Io {
        kind,
        operation: "recheck",
        source,
    })?;
    if metadata.file_type().is_symlink() {
        return Err(PathError::SymbolicLink { kind });
    }
    if !metadata.is_dir() {
        return Err(PathError::NotDirectory { kind });
    }
    Ok(())
}

#[cfg(unix)]
fn create_private_directory(kind: PathKind, path: &Path) -> Result<(), PathError> {
    use std::os::unix::fs::DirBuilderExt;

    let mut builder = fs::DirBuilder::new();
    builder.recursive(true).mode(0o700);
    builder.create(path).map_err(|source| PathError::Io {
        kind,
        operation: "create",
        source,
    })
}

#[cfg(windows)]
fn create_private_directory(kind: PathKind, path: &Path) -> Result<(), PathError> {
    fs::create_dir_all(path).map_err(|source| PathError::Io {
        kind,
        operation: "create",
        source,
    })
}

#[cfg(unix)]
pub(crate) fn prepare_private_directory(kind: PathKind, path: &Path) -> Result<(), PathError> {
    use std::os::unix::fs::PermissionsExt;

    ensure_directory_endpoint(kind, path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|source| PathError::Io {
        kind,
        operation: "secure",
        source,
    })
}

#[cfg(windows)]
pub(crate) fn prepare_private_directory(kind: PathKind, path: &Path) -> Result<(), PathError> {
    ensure_directory_endpoint(kind, path)?;
    harden_windows_path(path, true)
}

#[cfg(unix)]
/// Verifies a managed file endpoint and enforces current-user-only access.
///
/// # Errors
///
/// Returns [`PathError`] when the endpoint is a symlink or non-file, or when
/// the platform cannot enforce its private permission policy.
pub fn secure_private_file(kind: PathKind, path: &Path) -> Result<(), PathError> {
    use std::os::unix::fs::PermissionsExt;

    ensure_file_endpoint(kind, path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|source| PathError::Io {
        kind,
        operation: "secure file",
        source,
    })
}

#[cfg(windows)]
/// Verifies a managed file endpoint and enforces current-user-only access.
///
/// # Errors
///
/// Returns [`PathError`] when the endpoint is a symlink or non-file, or when
/// the platform cannot enforce its private permission policy.
pub fn secure_private_file(kind: PathKind, path: &Path) -> Result<(), PathError> {
    ensure_file_endpoint(kind, path)?;
    harden_windows_path(path, false)
}

fn ensure_file_endpoint(kind: PathKind, path: &Path) -> Result<(), PathError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| PathError::Io {
        kind,
        operation: "inspect file",
        source,
    })?;
    if metadata.file_type().is_symlink() {
        return Err(PathError::SymbolicLink { kind });
    }
    if !metadata.is_file() {
        return Err(PathError::NotFile { kind });
    }
    Ok(())
}

#[cfg(windows)]
fn harden_windows_path(path: &Path, inheritable: bool) -> Result<(), PathError> {
    use std::process::Command;

    let sid = current_windows_sid()?;
    let grant = if inheritable {
        format!("*{sid}:(OI)(CI)F")
    } else {
        format!("*{sid}:F")
    };

    run_permission_command(Command::new("icacls").arg(path).args(["/reset", "/q"]))?;
    run_permission_command(
        Command::new("icacls")
            .arg(path)
            .args(["/grant:r", &grant, "/q"]),
    )?;
    run_permission_command(
        Command::new("icacls")
            .arg(path)
            .args(["/inheritancelevel:r", "/q"]),
    )?;
    run_permission_command(Command::new("icacls").arg(path).args(["/verify", "/q"]))
}

#[cfg(windows)]
fn current_windows_sid() -> Result<String, PathError> {
    use std::process::Command;

    let output = Command::new("whoami")
        .args(["/user", "/fo", "csv", "/nh"])
        .output()
        .map_err(|source| PathError::PermissionToolUnavailable {
            tool: "whoami",
            source,
        })?;
    if !output.status.success() {
        return Err(PathError::PermissionToolFailed {
            tool: "whoami",
            exit_code: output.status.code(),
        });
    }

    extract_sid(&output.stdout).ok_or(PathError::InvalidWindowsPrincipal)
}

#[cfg(windows)]
fn extract_sid(output: &[u8]) -> Option<String> {
    let start = output.windows(4).position(|window| window == b"S-1-")?;
    let bytes: Vec<u8> = output[start..]
        .iter()
        .copied()
        .take_while(|byte| byte.is_ascii_digit() || matches!(*byte, b'S' | b's' | b'-'))
        .take(185)
        .collect();
    if bytes.len() < 5 {
        return None;
    }
    String::from_utf8(bytes).ok()
}

#[cfg(windows)]
fn run_permission_command(command: &mut std::process::Command) -> Result<(), PathError> {
    let output = command
        .output()
        .map_err(|source| PathError::PermissionToolUnavailable {
            tool: "icacls",
            source,
        })?;
    if output.status.success() {
        Ok(())
    } else {
        Err(PathError::PermissionToolFailed {
            tool: "icacls",
            exit_code: output.status.code(),
        })
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!(
                "jarvis-storage-paths-{}",
                jarvis_core::scratch_tag()
            )))
        }

        fn paths(&self) -> AppPaths {
            AppPaths {
                config: self.0.join("config"),
                data: self.0.join("data"),
                cache: self.0.join("cache"),
                state: self.0.join("state"),
                runtime: self.0.join("runtime"),
                logs: self.0.join("logs"),
                runtime_source: RuntimePathSource::StateFallback,
            }
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            jarvis_core::remove_scratch_dir(&self.0);
        }
    }

    #[test]
    fn paths_native_resolution_is_absolute() {
        let base = NativeStrategy::new()
            .unwrap_or_else(|error| panic!("native base paths should resolve: {error}"));
        let paths = AppPaths::resolve_native()
            .unwrap_or_else(|error| panic!("native paths should resolve: {error}"));

        assert!(
            paths
                .managed_paths()
                .iter()
                .all(|(_, path)| path.is_absolute())
        );
        assert_eq!(
            paths.config(),
            native_config_base(&base).join(application_component())
        );
        assert_eq!(
            paths.data(),
            native_data_base(&base).join(application_component())
        );

        #[cfg(target_os = "windows")]
        {
            assert_eq!(paths.cache(), paths.data().join("cache"));
            assert_eq!(paths.state(), paths.data());
            assert_eq!(paths.logs(), paths.data().join("logs"));
            assert_eq!(paths.runtime(), paths.data().join("runtime"));
            assert_eq!(paths.runtime_source(), RuntimePathSource::StateFallback);
        }

        #[cfg(target_os = "macos")]
        {
            assert_eq!(
                paths.cache(),
                base.cache_dir().join(application_component())
            );
            assert_eq!(paths.state(), paths.data());
            assert_eq!(
                paths.logs(),
                base.home_dir()
                    .join("Library")
                    .join("Logs")
                    .join(application_component())
            );
            assert_eq!(paths.runtime(), paths.data().join("runtime"));
            assert_eq!(paths.runtime_source(), RuntimePathSource::StateFallback);
        }

        #[cfg(target_os = "linux")]
        {
            assert_eq!(
                paths.cache(),
                base.cache_dir().join(application_component())
            );
            assert_eq!(
                paths.state(),
                base.state_dir()
                    .unwrap_or_else(|| base.data_dir())
                    .join(application_component())
            );
            // `if let`/`else` rather than a two-arm `match`: the arms have different
            // bodies, so clippy's `single_match_else` rejects the `match` form. That lint
            // compiles only under `#[cfg(target_os = "linux")]`, which is why this file
            // linted clean on Windows and macOS-failing Linux runners caught it. The two
            // shapes are equivalent here; the `if let` form is the one the lint accepts.
            if let Some(runtime) = base.runtime_dir() {
                assert_eq!(paths.runtime(), runtime.join(application_component()));
                assert_eq!(paths.runtime_source(), RuntimePathSource::Native);
            } else {
                assert_eq!(paths.runtime(), paths.state().join("runtime"));
                assert_eq!(paths.runtime_source(), RuntimePathSource::StateFallback);
            }
        }
    }

    #[test]
    fn paths_prepare_is_idempotent() {
        let test_directory = TestDirectory::new();
        let paths = test_directory.paths();

        paths
            .prepare()
            .unwrap_or_else(|error| panic!("initial preparation should succeed: {error}"));
        paths
            .prepare()
            .unwrap_or_else(|error| panic!("repeat preparation should succeed: {error}"));

        assert!(paths.managed_paths().iter().all(|(_, path)| path.is_dir()));
    }

    #[test]
    fn paths_relative_layout_fails_closed() {
        let paths = AppPaths {
            config: PathBuf::from("relative/config"),
            data: PathBuf::from("relative/data"),
            cache: PathBuf::from("relative/cache"),
            state: PathBuf::from("relative/state"),
            runtime: PathBuf::from("relative/runtime"),
            logs: PathBuf::from("relative/logs"),
            runtime_source: RuntimePathSource::StateFallback,
        };

        assert!(matches!(
            paths.validate(),
            Err(PathError::RelativePath { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn paths_prepare_tightens_unix_mode_and_rejects_symlink() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let test_directory = TestDirectory::new();
        let paths = test_directory.paths();
        paths
            .prepare()
            .unwrap_or_else(|error| panic!("initial preparation should succeed: {error}"));

        fs::set_permissions(paths.data(), fs::Permissions::from_mode(0o777))
            .unwrap_or_else(|error| panic!("test should weaken permissions: {error}"));
        paths
            .prepare()
            .unwrap_or_else(|error| panic!("preparation should tighten permissions: {error}"));
        let mode = fs::metadata(paths.data())
            .unwrap_or_else(|error| panic!("test should inspect permissions: {error}"))
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o700);

        fs::remove_dir(paths.cache())
            .unwrap_or_else(|error| panic!("test should remove cache directory: {error}"));
        symlink(paths.data(), paths.cache())
            .unwrap_or_else(|error| panic!("test should create symlink: {error}"));
        assert!(matches!(
            paths.prepare(),
            Err(PathError::SymbolicLink {
                kind: PathKind::Cache
            })
        ));
    }

    #[cfg(windows)]
    #[test]
    fn paths_prepare_removes_windows_everyone_ace() {
        use std::process::Command;

        let test_directory = TestDirectory::new();
        let paths = test_directory.paths();
        paths
            .prepare()
            .unwrap_or_else(|error| panic!("initial preparation should succeed: {error}"));

        run_permission_command(Command::new("icacls").arg(paths.data()).args([
            "/grant:r",
            "*S-1-1-0:(OI)(CI)F",
            "/q",
        ]))
        .unwrap_or_else(|error| panic!("test should grant Everyone: {error}"));
        assert!(windows_sid_lookup_mentions_path(paths.data(), "S-1-1-0"));

        paths
            .prepare()
            .unwrap_or_else(|error| panic!("preparation should remove broad ACE: {error}"));
        assert!(!windows_sid_lookup_mentions_path(paths.data(), "S-1-1-0"));
    }

    #[cfg(windows)]
    fn windows_sid_lookup_mentions_path(path: &Path, sid: &str) -> bool {
        use std::process::Command;

        let output = Command::new("icacls")
            .arg(path)
            .args(["/findsid", &format!("*{sid}"), "/q"])
            .output()
            .unwrap_or_else(|error| panic!("SID lookup should start: {error}"));
        String::from_utf8_lossy(&output.stdout).contains(&path.to_string_lossy().to_string())
    }
}
