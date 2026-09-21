use std::path::{Path, PathBuf};

use thiserror::Error;

/// Maximum accepted profile name length in bytes.
pub const MAX_PROFILE_NAME_BYTES: usize = 64;
/// Unix domain socket filename inside the private runtime directory.
pub const UNIX_SOCKET_FILE_NAME: &str = "jarvis.sock";
/// Windows named-pipe namespace prefix.
pub const WINDOWS_PIPE_PREFIX: &str = r"\\.\pipe\";
/// Daemon singleton lock filename inside the runtime directory.
pub const DAEMON_LOCK_FILE_NAME: &str = "jarvisd.lock";

/// Explains why a local client endpoint could not be derived.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum EndpointError {
    /// The profile name was empty, too long, or contained unsafe characters.
    #[error("the profile name is not a valid local endpoint component")]
    InvalidProfileName,
    /// The runtime directory was relative, so the endpoint would be ambiguous.
    #[error("the runtime directory is not absolute")]
    RelativeRuntimeDirectory,
}

/// One profile's native local client endpoint.
///
/// The daemon and every local client derive the same endpoint from the same
/// runtime directory and profile name; the value is a locator, not authority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LocalEndpoint {
    /// A Unix domain socket file inside the runtime directory.
    UnixSocket(PathBuf),
    /// A Windows named pipe in the local pipe namespace.
    NamedPipe(String),
}

impl LocalEndpoint {
    /// Derives the Unix socket endpoint for a profile.
    ///
    /// # Errors
    ///
    /// Returns [`EndpointError`] when the profile name is unsafe or the runtime
    /// directory is relative.
    pub fn unix_socket(runtime_dir: &Path, profile: &str) -> Result<Self, EndpointError> {
        if !runtime_dir.is_absolute() {
            return Err(EndpointError::RelativeRuntimeDirectory);
        }
        if !is_valid_profile(profile) {
            return Err(EndpointError::InvalidProfileName);
        }
        Ok(Self::UnixSocket(runtime_dir.join(UNIX_SOCKET_FILE_NAME)))
    }

    /// Derives the Windows named-pipe endpoint for a profile.
    ///
    /// # Errors
    ///
    /// Returns [`EndpointError::InvalidProfileName`] when the profile name is
    /// unsafe.
    pub fn named_pipe(profile: &str) -> Result<Self, EndpointError> {
        if !is_valid_profile(profile) {
            return Err(EndpointError::InvalidProfileName);
        }
        Ok(Self::NamedPipe(format!(
            "{WINDOWS_PIPE_PREFIX}jarvis-{profile}"
        )))
    }

    /// Derives the Windows named-pipe endpoint for a profile in a specific root.
    ///
    /// Windows pipe names live in a single global namespace, so a portable profile
    /// named `default` would otherwise collide with a natively installed `default`
    /// profile and the two daemons would fight over one pipe. The root directory is
    /// folded in through a stable hash, so the endpoint stays derived (never
    /// configured in a file) and identical for every client that shares the root.
    ///
    /// # Errors
    ///
    /// Returns [`EndpointError`] when the profile name is unsafe or the runtime
    /// directory is relative.
    pub fn named_pipe_in_root(runtime_dir: &Path, profile: &str) -> Result<Self, EndpointError> {
        if !is_valid_profile(profile) {
            return Err(EndpointError::InvalidProfileName);
        }
        if !runtime_dir.is_absolute() {
            return Err(EndpointError::RelativeRuntimeDirectory);
        }
        // Windows path comparison is case-insensitive, so the digest is computed
        // over a normalized spelling to keep one profile from claiming two pipes.
        let normalized = runtime_dir.to_string_lossy().to_lowercase();
        Ok(Self::NamedPipe(format!(
            "{WINDOWS_PIPE_PREFIX}jarvis-{profile}-{:08x}",
            fnv1a32(normalized.as_bytes())
        )))
    }

    /// Derives the platform-native endpoint for a profile.
    ///
    /// Unix uses a socket inside the runtime directory; Windows uses a named
    /// pipe and ignores the runtime directory, because the profile name already
    /// distinguishes native installations.
    ///
    /// # Errors
    ///
    /// Returns [`EndpointError`] when the profile name is unsafe or, on Unix,
    /// when the runtime directory is relative.
    #[cfg(unix)]
    pub fn native(runtime_dir: &Path, profile: &str) -> Result<Self, EndpointError> {
        Self::unix_socket(runtime_dir, profile)
    }

    /// Derives the platform-native endpoint for a profile.
    ///
    /// # Errors
    ///
    /// Returns [`EndpointError::InvalidProfileName`] when the profile name is
    /// unsafe.
    #[cfg(windows)]
    pub fn native(_runtime_dir: &Path, profile: &str) -> Result<Self, EndpointError> {
        Self::named_pipe(profile)
    }

    /// Derives the platform-native endpoint while separating distinct roots.
    ///
    /// Unix already separates roots because the socket lives inside the runtime
    /// directory. Windows folds the runtime directory into the pipe name.
    ///
    /// # Errors
    ///
    /// Returns [`EndpointError`] when the profile name is unsafe or the runtime
    /// directory is relative.
    #[cfg(unix)]
    pub fn scoped(runtime_dir: &Path, profile: &str) -> Result<Self, EndpointError> {
        Self::unix_socket(runtime_dir, profile)
    }

    /// Derives the platform-native endpoint while separating distinct roots.
    ///
    /// # Errors
    ///
    /// Returns [`EndpointError`] when the profile name is unsafe or the runtime
    /// directory is relative.
    #[cfg(windows)]
    pub fn scoped(runtime_dir: &Path, profile: &str) -> Result<Self, EndpointError> {
        Self::named_pipe_in_root(runtime_dir, profile)
    }

    /// Returns the socket path when this is a Unix endpoint.
    #[must_use]
    pub fn unix_socket_path(&self) -> Option<&Path> {
        match self {
            Self::UnixSocket(path) => Some(path),
            Self::NamedPipe(_) => None,
        }
    }

    /// Returns the pipe name when this is a Windows endpoint.
    #[must_use]
    pub fn named_pipe_name(&self) -> Option<&str> {
        match self {
            Self::NamedPipe(name) => Some(name),
            Self::UnixSocket(_) => None,
        }
    }
}

/// Reports whether a profile name is safe to embed in an endpoint locator.
#[must_use]
pub fn is_valid_profile(profile: &str) -> bool {
    !profile.is_empty()
        && profile.len() <= MAX_PROFILE_NAME_BYTES
        && !profile.starts_with('.')
        && profile
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

/// FNV-1a over bytes.
///
/// A stable, dependency-free digest: the same root directory must always produce
/// the same pipe name across processes and restarts, so a random hash would break
/// exactly the clients it is meant to connect. This is an identifier, not a
/// security primitive.
fn fnv1a32(bytes: &[u8]) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in bytes {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_endpoint_requires_absolute_runtime_directory() {
        let relative = LocalEndpoint::unix_socket(Path::new("runtime"), "default");
        assert_eq!(relative, Err(EndpointError::RelativeRuntimeDirectory));
    }

    #[test]
    fn unix_and_pipe_endpoints_encode_the_profile() {
        let runtime_dir = if cfg!(windows) {
            PathBuf::from(r"C:\jarvis-runtime")
        } else {
            PathBuf::from("/run/jarvis")
        };

        let unix = LocalEndpoint::unix_socket(&runtime_dir, "work");
        assert_eq!(
            unix,
            Ok(LocalEndpoint::UnixSocket(
                runtime_dir.join(UNIX_SOCKET_FILE_NAME)
            ))
        );
        let pipe = LocalEndpoint::named_pipe("work");
        assert_eq!(
            pipe,
            Ok(LocalEndpoint::NamedPipe(format!(
                "{WINDOWS_PIPE_PREFIX}jarvis-work"
            )))
        );
    }

    #[test]
    fn unsafe_profile_names_fail_closed() {
        let oversized = "x".repeat(MAX_PROFILE_NAME_BYTES + 1);
        for profile in [
            "",
            ".",
            "..",
            "a/b",
            r"a\b",
            "with space",
            oversized.as_str(),
        ] {
            assert!(
                !is_valid_profile(profile),
                "profile {profile:?} must be rejected"
            );
            assert_eq!(
                LocalEndpoint::named_pipe(profile),
                Err(EndpointError::InvalidProfileName)
            );
        }
    }

    /// Two installations with the same profile name must not contend for one pipe, so
    /// the name is derived from the runtime directory as well.
    ///
    /// `#[cfg(windows)]` because the behaviour under test is Windows-specific in two
    /// ways: the pipe name is what must differ, and Windows path comparison is
    /// case-insensitive. On unix this test previously ran with `r"C:\one\runtime"`,
    /// which is a RELATIVE path there, so the constructor correctly refused it with
    /// `RelativeRuntimeDirectory` and the assertion failed. The production code was
    /// right and the test was platform-wrong, which is why it surfaced only on the unix
    /// CI runners.
    #[cfg(windows)]
    #[test]
    fn distinct_roots_never_share_a_windows_pipe_name() {
        let first = LocalEndpoint::named_pipe_in_root(Path::new(r"C:\one\runtime"), "default");
        let second = LocalEndpoint::named_pipe_in_root(Path::new(r"C:\two\runtime"), "default");
        assert_ne!(first, second);

        // The same root must always derive the same name, or a client would not
        // find its own daemon after a restart.
        let repeated = LocalEndpoint::named_pipe_in_root(Path::new(r"C:\one\runtime"), "default");
        assert_eq!(first, repeated);

        // Windows path comparison is case-insensitive, so case must not fork it.
        let uppercase = LocalEndpoint::named_pipe_in_root(Path::new(r"C:\ONE\RUNTIME"), "default");
        assert_eq!(first, uppercase);
    }

    #[test]
    fn scoped_endpoints_still_reject_unsafe_inputs() {
        // A relative runtime directory is refused on every platform, so this case is not
        // gated. An absolute root is supplied per platform rather than reusing a Windows
        // path: `C:\runtime` is relative on unix, so the previous fixture asserted the
        // wrong error for the wrong reason there.
        assert_eq!(
            LocalEndpoint::named_pipe_in_root(Path::new("relative"), "default"),
            Err(EndpointError::RelativeRuntimeDirectory)
        );
        let absolute = if cfg!(windows) {
            Path::new(r"C:\runtime")
        } else {
            Path::new("/run/jarvis")
        };
        assert_eq!(
            LocalEndpoint::named_pipe_in_root(absolute, "bad/name"),
            Err(EndpointError::InvalidProfileName)
        );
    }
}
