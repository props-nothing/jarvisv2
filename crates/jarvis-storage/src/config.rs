use std::{
    ffi::OsString,
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use atomic_write_file::AtomicWriteFile;
use jarvis_core::LogLevel;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use toml::Table;

use crate::{
    AppPaths, PathKind,
    paths::{prepare_private_directory, secure_private_file},
};

/// The configuration schema understood by this build.
pub const CURRENT_CONFIG_VERSION: u32 = 1;

const MAX_PROFILE_NAME_BYTES: usize = 64;
const MAX_SHUTDOWN_TIMEOUT_SECONDS: u16 = 300;
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
const CONFIG_FILE_NAME: &str = "config.toml";

/// Profile-specific configuration.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileConfig {
    name: String,
}

impl ProfileConfig {
    /// Returns the stable profile name used in local resource names.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
}

impl Default for ProfileConfig {
    fn default() -> Self {
        Self {
            name: "default".to_owned(),
        }
    }
}

/// Structural logging configuration.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LoggingConfig {
    level: LogLevel,
}

impl LoggingConfig {
    /// Returns the minimum configured log level.
    #[must_use]
    pub const fn level(&self) -> LogLevel {
        self.level
    }
}

/// The default loopback port the HTTP transport binds.
///
/// ADR-0011 makes the HTTP transport a separately-enabled peer to local IPC, bound to
/// loopback by default. The port is fixed rather than ephemeral so a client has something to
/// name, and loopback-only so enabling it does not expose the daemon to the network.
pub const DEFAULT_HTTP_PORT: u16 = 8765;

/// Daemon lifecycle configuration.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DaemonConfig {
    shutdown_timeout_seconds: u16,
    /// Whether the loopback HTTP transport is served at all.
    ///
    /// Off by default. ADR-0011 makes it separately enabled rather than always on, because a
    /// listening port is a larger attack surface than an OS-protected pipe, and a daemon that
    /// opened one unasked would contradict the reason local IPC is preferred.
    #[serde(default)]
    http_enabled: bool,
    /// The loopback port the HTTP transport binds when enabled.
    #[serde(default = "default_http_port")]
    http_port: u16,
}

fn default_http_port() -> u16 {
    DEFAULT_HTTP_PORT
}

impl DaemonConfig {
    /// Returns the graceful-shutdown deadline in seconds.
    #[must_use]
    pub const fn shutdown_timeout_seconds(&self) -> u16 {
        self.shutdown_timeout_seconds
    }

    /// Returns whether the loopback HTTP transport is served.
    #[must_use]
    pub const fn http_enabled(&self) -> bool {
        self.http_enabled
    }

    /// Returns the loopback port the HTTP transport binds.
    #[must_use]
    pub const fn http_port(&self) -> u16 {
        self.http_port
    }
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            shutdown_timeout_seconds: 15,
            http_enabled: false,
            http_port: DEFAULT_HTTP_PORT,
        }
    }
}

/// Effective validated JARVIS configuration schema v1.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    schema_version: u32,
    profile: ProfileConfig,
    logging: LoggingConfig,
    daemon: DaemonConfig,
}

impl Config {
    /// Creates and validates a schema v1 configuration.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] when the profile name or shutdown timeout is invalid.
    pub fn new(
        profile_name: impl Into<String>,
        log_level: LogLevel,
        shutdown_timeout_seconds: u16,
    ) -> Result<Self, ConfigError> {
        let config = Self {
            schema_version: CURRENT_CONFIG_VERSION,
            profile: ProfileConfig {
                name: profile_name.into(),
            },
            logging: LoggingConfig { level: log_level },
            daemon: DaemonConfig {
                shutdown_timeout_seconds,
                ..DaemonConfig::default()
            },
        };
        config.validate()?;
        Ok(config)
    }

    /// Parses, migrates, overrides, and validates a TOML document.
    ///
    /// Environment values use file precedence followed by the explicit
    /// `JARVIS_*` allowlist. Unrelated variables are ignored.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] for malformed or unsupported documents, unknown
    /// keys, invalid values, or unrecognized prefixed environment variables.
    pub fn parse_with_environment<I, K, V>(
        document: &str,
        environment: I,
    ) -> Result<LoadedConfig, ConfigError>
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<OsString>,
        V: Into<OsString>,
    {
        let table = document
            .parse::<Table>()
            .map_err(|_| ConfigError::InvalidDocument)?;
        let version = read_schema_version(&table)?;

        let (mut config, migration) = match version {
            0 => {
                reject_unknown_keys(&table, 0)?;
                let legacy: ConfigV0 =
                    toml::from_str(document).map_err(|_| ConfigError::InvalidDocument)?;
                if legacy.schema_version != 0 {
                    return Err(ConfigError::InvalidSchemaVersion);
                }
                (
                    Self {
                        schema_version: CURRENT_CONFIG_VERSION,
                        profile: ProfileConfig {
                            name: legacy.profile,
                        },
                        logging: LoggingConfig {
                            level: legacy.log_level,
                        },
                        daemon: DaemonConfig {
                            shutdown_timeout_seconds: legacy.shutdown_timeout_seconds,
                            ..DaemonConfig::default()
                        },
                    },
                    Some(ConfigMigration::V0ToV1),
                )
            }
            CURRENT_CONFIG_VERSION => {
                reject_unknown_keys(&table, CURRENT_CONFIG_VERSION)?;
                let config = toml::from_str(document).map_err(|_| ConfigError::InvalidDocument)?;
                (config, None)
            }
            found => {
                return Err(ConfigError::UnsupportedSchemaVersion {
                    found,
                    current: CURRENT_CONFIG_VERSION,
                });
            }
        };

        apply_environment(&mut config, environment)?;
        config.validate()?;
        Ok(LoadedConfig { config, migration })
    }

    /// Returns the configuration schema version.
    #[must_use]
    pub const fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Returns the profile configuration.
    #[must_use]
    pub const fn profile(&self) -> &ProfileConfig {
        &self.profile
    }

    /// Returns the logging configuration.
    #[must_use]
    pub const fn logging(&self) -> &LoggingConfig {
        &self.logging
    }

    /// Returns the daemon lifecycle configuration.
    #[must_use]
    pub const fn daemon(&self) -> &DaemonConfig {
        &self.daemon
    }

    fn validate(&self) -> Result<(), ConfigError> {
        if self.schema_version != CURRENT_CONFIG_VERSION {
            return Err(ConfigError::UnsupportedSchemaVersion {
                found: self.schema_version,
                current: CURRENT_CONFIG_VERSION,
            });
        }
        if !is_valid_profile_name(&self.profile.name) {
            return Err(ConfigError::InvalidProfileName);
        }
        if !(1..=MAX_SHUTDOWN_TIMEOUT_SECONDS).contains(&self.daemon.shutdown_timeout_seconds) {
            return Err(ConfigError::InvalidShutdownTimeout);
        }
        // Port 0 is refused. It asks the operating system to pick an ephemeral port, which
        // would leave a client with no port it could name and make the transport unusable by
        // construction rather than by policy.
        if self.daemon.http_enabled && self.daemon.http_port == 0 {
            return Err(ConfigError::InvalidHttpPort);
        }
        Ok(())
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_CONFIG_VERSION,
            profile: ProfileConfig::default(),
            logging: LoggingConfig::default(),
            daemon: DaemonConfig::default(),
        }
    }
}

/// A migration applied while interpreting a configuration document.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigMigration {
    /// The flat schema v0 document was mapped to nested schema v1.
    V0ToV1,
}

/// Effective configuration plus non-mutating migration evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadedConfig {
    config: Config,
    migration: Option<ConfigMigration>,
}

impl LoadedConfig {
    /// Returns the effective validated configuration.
    #[must_use]
    pub const fn config(&self) -> &Config {
        &self.config
    }

    /// Returns the migration applied in memory, if any.
    #[must_use]
    pub const fn migration(&self) -> Option<ConfigMigration> {
        self.migration
    }

    /// Consumes the load result and returns the effective configuration.
    #[must_use]
    pub fn into_config(self) -> Config {
        self.config
    }
}

/// Loads and atomically persists one configuration document.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigStore {
    path: PathBuf,
}

impl ConfigStore {
    /// Selects `<config>/config.toml` from resolved application paths.
    #[must_use]
    pub fn from_paths(paths: &AppPaths) -> Self {
        Self {
            path: paths.config().join(CONFIG_FILE_NAME),
        }
    }

    /// Creates a store for an explicit absolute configuration path.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Loads the file and applies current process environment overrides.
    ///
    /// A missing file yields validated defaults and does not create or modify
    /// anything on disk.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] for an unsafe endpoint, oversized or unreadable
    /// file, invalid document, migration failure, or invalid environment.
    pub fn load(&self) -> Result<LoadedConfig, ConfigError> {
        self.load_with_environment(std::env::vars_os())
    }

    /// Loads the file with an injected environment for deterministic callers.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] under the same conditions as [`Self::load`].
    pub fn load_with_environment<I, K, V>(
        &self,
        environment: I,
    ) -> Result<LoadedConfig, ConfigError>
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<OsString>,
        V: Into<OsString>,
    {
        self.require_absolute()?;
        match inspect_config_file(&self.path)? {
            ConfigFileState::Missing => {
                let mut config = Config::default();
                apply_environment(&mut config, environment)?;
                config.validate()?;
                Ok(LoadedConfig {
                    config,
                    migration: None,
                })
            }
            ConfigFileState::Present => {
                let document = read_bounded_document(&self.path)?;
                Config::parse_with_environment(&document, environment)
            }
        }
    }

    /// Atomically replaces the file with a validated schema v1 document.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] when validation, serialization, endpoint checks,
    /// private-directory preparation, writing, commit, or final hardening fails.
    pub fn save(&self, config: &Config) -> Result<(), ConfigError> {
        self.require_absolute()?;
        config.validate()?;
        let document = toml::to_string_pretty(config).map_err(|_| ConfigError::SerializeFailed)?;
        let parent = self.path.parent().ok_or(ConfigError::MissingParent)?;
        prepare_private_directory(PathKind::Config, parent)
            .map_err(|_| ConfigError::SecurePathFailed)?;
        reject_unsafe_write_endpoint(&self.path)?;

        let mut staged = stage_document(&self.path, document.as_bytes())?;
        staged.flush().map_err(|error| config_io("flush", &error))?;
        staged
            .commit()
            .map_err(|error| config_io("commit", &error))?;
        secure_private_file(PathKind::Config, &self.path).map_err(|_| ConfigError::SecurePathFailed)
    }

    /// Returns the selected configuration file path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn require_absolute(&self) -> Result<(), ConfigError> {
        if self.path.is_absolute() {
            Ok(())
        } else {
            Err(ConfigError::RelativePath)
        }
    }
}

/// Safe configuration load and validation failures.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum ConfigError {
    /// The TOML syntax or typed shape is invalid.
    #[error("the configuration document is invalid")]
    InvalidDocument,
    /// The document omitted its schema version.
    #[error("the configuration document is missing schema_version")]
    MissingSchemaVersion,
    /// The schema version was not a non-negative integer.
    #[error("the configuration schema_version is invalid")]
    InvalidSchemaVersion,
    /// The document targets a schema newer or otherwise unsupported by this build.
    #[error("configuration schema version {found} is unsupported; this build supports {current}")]
    UnsupportedSchemaVersion {
        /// Version found in the document.
        found: u32,
        /// Version understood by this build.
        current: u32,
    },
    /// A key is not defined by the selected schema version.
    #[error("unknown configuration key: {key}")]
    UnknownKey {
        /// Bounded dotted key path.
        key: String,
    },
    /// The profile name is unsafe for resource naming.
    #[error(
        "profile.name must be 1-64 bytes, start with an ASCII letter or digit, and contain only letters, digits, '.', '_', or '-'"
    )]
    InvalidProfileName,
    /// The graceful shutdown deadline is outside the supported range.
    #[error("daemon.shutdown_timeout_seconds must be between 1 and 300")]
    InvalidShutdownTimeout,
    /// The HTTP transport was enabled with a port that cannot be named.
    ///
    /// Port 0 is refused because it asks the operating system to pick an ephemeral port, which
    /// leaves a client with no port it could name.
    #[error("daemon.http_port must be between 1 and 65535 when http_enabled is true")]
    InvalidHttpPort,
    /// A prefixed environment key is not part of the explicit override contract.
    #[error("unknown configuration environment variable: {key}")]
    UnknownEnvironmentKey {
        /// Bounded environment key.
        key: String,
    },
    /// A recognized override could not be decoded or parsed.
    #[error("invalid value for configuration environment variable {key}")]
    InvalidEnvironmentValue {
        /// Static recognized environment key.
        key: &'static str,
    },
    /// The explicit configuration path was not absolute.
    #[error("the configuration file path must be absolute")]
    RelativePath,
    /// The configuration path did not have a parent directory.
    #[error("the configuration file path has no parent directory")]
    MissingParent,
    /// The configuration path was a symbolic link.
    #[error("the configuration file path is a symbolic link")]
    SymbolicLink,
    /// The configuration endpoint existed but was not a regular file.
    #[error("the configuration path is not a regular file")]
    NotFile,
    /// The configuration file exceeded its bounded size.
    #[error("the configuration file exceeds 1 MiB")]
    DocumentTooLarge,
    /// The configuration bytes were not valid UTF-8.
    #[error("the configuration file is not valid UTF-8")]
    InvalidEncoding,
    /// Typed TOML serialization failed.
    #[error("the configuration could not be serialized")]
    SerializeFailed,
    /// A private directory or file could not be enforced.
    #[error("the configuration path could not be secured")]
    SecurePathFailed,
    /// A bounded filesystem operation failed.
    #[error("configuration {operation} failed with {kind:?}")]
    Io {
        /// Bounded operation name.
        operation: &'static str,
        /// Safe operating-system error category.
        kind: io::ErrorKind,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConfigFileState {
    Missing,
    Present,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigV0 {
    schema_version: u32,
    profile: String,
    log_level: LogLevel,
    shutdown_timeout_seconds: u16,
}

fn read_schema_version(table: &Table) -> Result<u32, ConfigError> {
    let value = table
        .get("schema_version")
        .ok_or(ConfigError::MissingSchemaVersion)?;
    let value = value
        .as_integer()
        .ok_or(ConfigError::InvalidSchemaVersion)?;
    u32::try_from(value).map_err(|_| ConfigError::InvalidSchemaVersion)
}

fn reject_unknown_keys(table: &Table, version: u32) -> Result<(), ConfigError> {
    let mut unknown = Vec::new();
    let root_keys: &[&str] = if version == 0 {
        &[
            "schema_version",
            "profile",
            "log_level",
            "shutdown_timeout_seconds",
        ]
    } else {
        &["schema_version", "profile", "logging", "daemon"]
    };
    collect_unknown_keys(table, root_keys, "", &mut unknown);

    if version == CURRENT_CONFIG_VERSION {
        collect_nested_unknown(table, "profile", &["name"], &mut unknown);
        collect_nested_unknown(table, "logging", &["level"], &mut unknown);
        collect_nested_unknown(
            table,
            "daemon",
            &["shutdown_timeout_seconds", "http_enabled", "http_port"],
            &mut unknown,
        );
    }

    unknown.sort();
    match unknown.into_iter().next() {
        Some(key) => Err(ConfigError::UnknownKey {
            key: bounded_identifier(&key),
        }),
        None => Ok(()),
    }
}

fn collect_nested_unknown(
    table: &Table,
    section: &str,
    allowed: &[&str],
    unknown: &mut Vec<String>,
) {
    if let Some(section_table) = table.get(section).and_then(toml::Value::as_table) {
        collect_unknown_keys(section_table, allowed, section, unknown);
    }
}

fn collect_unknown_keys(table: &Table, allowed: &[&str], prefix: &str, unknown: &mut Vec<String>) {
    for key in table.keys().filter(|key| !allowed.contains(&key.as_str())) {
        if prefix.is_empty() {
            unknown.push(key.clone());
        } else {
            unknown.push(format!("{prefix}.{key}"));
        }
    }
}

fn apply_environment<I, K, V>(config: &mut Config, environment: I) -> Result<(), ConfigError>
where
    I: IntoIterator<Item = (K, V)>,
    K: Into<OsString>,
    V: Into<OsString>,
{
    for (key, value) in environment {
        let key = key.into();
        let Some(key) = key.to_str() else {
            continue;
        };
        if !key.starts_with("JARVIS_") {
            continue;
        }

        let value = value.into();
        match key {
            "JARVIS_PROFILE_NAME" => {
                environment_text(&value, "JARVIS_PROFILE_NAME")?
                    .clone_into(&mut config.profile.name);
            }
            "JARVIS_LOG_LEVEL" => {
                config.logging.level = environment_text(&value, "JARVIS_LOG_LEVEL")?
                    .parse()
                    .map_err(|()| ConfigError::InvalidEnvironmentValue {
                        key: "JARVIS_LOG_LEVEL",
                    })?;
            }
            "JARVIS_SHUTDOWN_TIMEOUT_SECONDS" => {
                config.daemon.shutdown_timeout_seconds =
                    environment_text(&value, "JARVIS_SHUTDOWN_TIMEOUT_SECONDS")?
                        .parse()
                        .map_err(|_| ConfigError::InvalidEnvironmentValue {
                            key: "JARVIS_SHUTDOWN_TIMEOUT_SECONDS",
                        })?;
            }
            "JARVIS_HTTP_ENABLED" => {
                // Parsed as a strict boolean rather than coerced from truthiness. A value such
                // as "yes" or "1" that silently meant `false` would leave an operator believing
                // the transport was enabled when it was not, and the failure would appear as a
                // refused connection rather than as a rejected setting.
                config.daemon.http_enabled = environment_text(&value, "JARVIS_HTTP_ENABLED")?
                    .parse()
                    .map_err(|_| ConfigError::InvalidEnvironmentValue {
                        key: "JARVIS_HTTP_ENABLED",
                    })?;
            }
            "JARVIS_HTTP_PORT" => {
                config.daemon.http_port = environment_text(&value, "JARVIS_HTTP_PORT")?
                    .parse()
                    .map_err(|_| ConfigError::InvalidEnvironmentValue {
                        key: "JARVIS_HTTP_PORT",
                    })?;
            }
            _ => {
                return Err(ConfigError::UnknownEnvironmentKey {
                    key: bounded_identifier(key),
                });
            }
        }
    }
    Ok(())
}

fn environment_text<'a>(value: &'a OsString, key: &'static str) -> Result<&'a str, ConfigError> {
    value
        .to_str()
        .ok_or(ConfigError::InvalidEnvironmentValue { key })
}

fn is_valid_profile_name(value: &str) -> bool {
    let mut bytes = value.bytes();
    value.len() <= MAX_PROFILE_NAME_BYTES
        && bytes
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn bounded_identifier(value: &str) -> String {
    let mut output = String::new();
    for character in value.chars().take(128) {
        output.push(if character.is_control() {
            '?'
        } else {
            character
        });
    }
    if value.chars().count() > 128 {
        output.push_str("...");
    }
    output
}

fn inspect_config_file(path: &Path) -> Result<ConfigFileState, ConfigError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(ConfigError::SymbolicLink),
        Ok(metadata) if !metadata.is_file() => Err(ConfigError::NotFile),
        Ok(metadata) if metadata.len() > MAX_CONFIG_BYTES => Err(ConfigError::DocumentTooLarge),
        Ok(_) => Ok(ConfigFileState::Present),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(ConfigFileState::Missing),
        Err(error) => Err(config_io("inspect", &error)),
    }
}

fn reject_unsafe_write_endpoint(path: &Path) -> Result<(), ConfigError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(ConfigError::SymbolicLink),
        Ok(metadata) if !metadata.is_file() => Err(ConfigError::NotFile),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(config_io("inspect", &error)),
    }
}

fn read_bounded_document(path: &Path) -> Result<String, ConfigError> {
    let file = fs::File::open(path).map_err(|error| config_io("open", &error))?;
    let mut bytes = Vec::new();
    file.take(MAX_CONFIG_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| config_io("read", &error))?;
    if bytes.len() as u64 > MAX_CONFIG_BYTES {
        return Err(ConfigError::DocumentTooLarge);
    }
    String::from_utf8(bytes).map_err(|_| ConfigError::InvalidEncoding)
}

fn stage_document(path: &Path, bytes: &[u8]) -> Result<AtomicWriteFile, ConfigError> {
    let mut file = open_atomic_file(path)?;
    file.write_all(bytes)
        .map_err(|error| config_io("write", &error))?;
    Ok(file)
}

#[cfg(unix)]
fn open_atomic_file(path: &Path) -> Result<AtomicWriteFile, ConfigError> {
    use atomic_write_file::unix::OpenOptionsExt as AtomicOpenOptionsExt;
    use std::os::unix::fs::OpenOptionsExt as StandardOpenOptionsExt;

    let mut options = AtomicWriteFile::options();
    StandardOpenOptionsExt::mode(&mut options, 0o600);
    AtomicOpenOptionsExt::preserve_mode(&mut options, false);
    options
        .open(path)
        .map_err(|error| config_io("open staged file", &error))
}

#[cfg(not(unix))]
fn open_atomic_file(path: &Path) -> Result<AtomicWriteFile, ConfigError> {
    AtomicWriteFile::open(path).map_err(|error| config_io("open staged file", &error))
}

fn config_io(operation: &'static str, error: &io::Error) -> ConfigError {
    ConfigError::Io {
        operation,
        kind: error.kind(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static CONFIG_TEST_ID: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let sequence = CONFIG_TEST_ID.fetch_add(1, Ordering::Relaxed);
            Self(std::env::temp_dir().join(format!(
                "jarvis-storage-config-{}-{sequence}",
                std::process::id()
            )))
        }

        fn store(&self) -> ConfigStore {
            ConfigStore::new(self.0.join("config").join(CONFIG_FILE_NAME))
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _result = fs::remove_dir_all(&self.0);
        }
    }

    const V1_CONFIG: &str = r#"
schema_version = 1

[profile]
name = "home"

[logging]
level = "warn"

[daemon]
shutdown_timeout_seconds = 30
"#;

    #[test]
    fn config_v1_parses_and_environment_has_final_precedence() {
        let loaded = Config::parse_with_environment(
            V1_CONFIG,
            [
                ("UNRELATED", "ignored"),
                ("JARVIS_PROFILE_NAME", "work"),
                ("JARVIS_LOG_LEVEL", "debug"),
                ("JARVIS_SHUTDOWN_TIMEOUT_SECONDS", "45"),
            ],
        )
        .unwrap_or_else(|error| panic!("valid config should load: {error}"));

        assert_eq!(loaded.migration(), None);
        assert_eq!(loaded.config().profile().name(), "work");
        assert_eq!(loaded.config().logging().level(), LogLevel::Debug);
        assert_eq!(loaded.config().daemon().shutdown_timeout_seconds(), 45);
    }

    #[test]
    fn config_v0_migrates_without_mutating_source() {
        let legacy = r#"
schema_version = 0
profile = "legacy"
log_level = "error"
shutdown_timeout_seconds = 20
"#;

        let loaded = Config::parse_with_environment(legacy, Vec::<(String, String)>::new())
            .unwrap_or_else(|error| panic!("legacy config should migrate: {error}"));

        assert_eq!(loaded.migration(), Some(ConfigMigration::V0ToV1));
        assert_eq!(loaded.config().schema_version(), CURRENT_CONFIG_VERSION);
        assert_eq!(loaded.config().profile().name(), "legacy");
        assert_eq!(loaded.config().logging().level(), LogLevel::Error);
        assert!(legacy.contains("schema_version = 0"));
    }

    #[test]
    fn config_unknown_root_and_nested_keys_fail_closed() {
        let root = V1_CONFIG.replacen("schema_version = 1", "schema_version = 1\nextra = true", 1);
        assert_eq!(
            Config::parse_with_environment(&root, Vec::<(String, String)>::new()),
            Err(ConfigError::UnknownKey {
                key: "extra".to_owned()
            })
        );

        let nested = V1_CONFIG.replace("name = \"home\"", "name = \"home\"\ncolour = \"blue\"");
        assert_eq!(
            Config::parse_with_environment(&nested, Vec::<(String, String)>::new()),
            Err(ConfigError::UnknownKey {
                key: "profile.colour".to_owned()
            })
        );
    }

    #[test]
    fn config_newer_schema_and_unknown_environment_fail_closed() {
        let newer = V1_CONFIG.replace("schema_version = 1", "schema_version = 2");
        assert_eq!(
            Config::parse_with_environment(&newer, Vec::<(String, String)>::new()),
            Err(ConfigError::UnsupportedSchemaVersion {
                found: 2,
                current: CURRENT_CONFIG_VERSION
            })
        );

        assert_eq!(
            Config::parse_with_environment(V1_CONFIG, [("JARVIS_LOG_LEVLE", "debug")]),
            Err(ConfigError::UnknownEnvironmentKey {
                key: "JARVIS_LOG_LEVLE".to_owned()
            })
        );
    }

    #[test]
    fn config_invalid_values_do_not_echo_them() {
        let canary = "canary invalid profile";
        let error = Config::parse_with_environment(V1_CONFIG, [("JARVIS_PROFILE_NAME", canary)])
            .err()
            .unwrap_or_else(|| panic!("invalid profile should fail"));

        assert_eq!(error, ConfigError::InvalidProfileName);
        assert!(!error.to_string().contains(canary));

        assert_eq!(
            Config::new("..", LogLevel::Info, 15),
            Err(ConfigError::InvalidProfileName)
        );
        assert_eq!(
            Config::new("valid", LogLevel::Info, 0),
            Err(ConfigError::InvalidShutdownTimeout)
        );
    }

    #[test]
    fn config_malformed_missing_and_negative_versions_are_distinct() {
        assert_eq!(
            Config::parse_with_environment("[", Vec::<(String, String)>::new()),
            Err(ConfigError::InvalidDocument)
        );
        let missing = V1_CONFIG.replacen("schema_version = 1\n", "", 1);
        assert_eq!(
            Config::parse_with_environment(&missing, Vec::<(String, String)>::new()),
            Err(ConfigError::MissingSchemaVersion)
        );
        let negative = V1_CONFIG.replace("schema_version = 1", "schema_version = -1");
        assert_eq!(
            Config::parse_with_environment(&negative, Vec::<(String, String)>::new()),
            Err(ConfigError::InvalidSchemaVersion)
        );
    }

    #[test]
    fn config_non_unicode_recognized_environment_value_fails_closed() {
        let environment = [(OsString::from("JARVIS_LOG_LEVEL"), non_unicode_os_string())];
        assert_eq!(
            Config::parse_with_environment(V1_CONFIG, environment),
            Err(ConfigError::InvalidEnvironmentValue {
                key: "JARVIS_LOG_LEVEL"
            })
        );
    }

    #[test]
    fn config_store_missing_file_uses_defaults_without_writing() {
        let test_directory = TestDirectory::new();
        let store = test_directory.store();

        let loaded = store
            .load_with_environment([("JARVIS_LOG_LEVEL", "warn")])
            .unwrap_or_else(|error| panic!("missing config should use defaults: {error}"));

        assert_eq!(loaded.config().profile().name(), "default");
        assert_eq!(loaded.config().logging().level(), LogLevel::Warn);
        assert!(!store.path().exists());
    }

    #[test]
    fn config_store_atomically_replaces_and_round_trips() {
        let test_directory = TestDirectory::new();
        let store = test_directory.store();
        let first = Config::new("first", LogLevel::Info, 10)
            .unwrap_or_else(|error| panic!("first config should validate: {error}"));
        let second = Config::new("second", LogLevel::Debug, 25)
            .unwrap_or_else(|error| panic!("second config should validate: {error}"));

        store
            .save(&first)
            .unwrap_or_else(|error| panic!("first save should succeed: {error}"));
        store
            .save(&second)
            .unwrap_or_else(|error| panic!("replacement save should succeed: {error}"));
        let loaded = store
            .load_with_environment(Vec::<(String, String)>::new())
            .unwrap_or_else(|error| panic!("saved config should load: {error}"));

        assert_eq!(loaded.config(), &second);
        let persisted = fs::read_to_string(store.path())
            .unwrap_or_else(|error| panic!("saved bytes should be readable: {error}"));
        assert!(persisted.contains("name = \"second\""));
        assert!(!persisted.contains("name = \"first\""));
    }

    #[test]
    fn config_staging_and_discard_preserves_previous_bytes() {
        let test_directory = TestDirectory::new();
        let store = test_directory.store();
        let original = Config::new("original", LogLevel::Info, 15)
            .unwrap_or_else(|error| panic!("original config should validate: {error}"));
        store
            .save(&original)
            .unwrap_or_else(|error| panic!("original save should succeed: {error}"));
        let before = fs::read(store.path())
            .unwrap_or_else(|error| panic!("original bytes should be readable: {error}"));

        let staged = stage_document(store.path(), b"not a complete configuration")
            .unwrap_or_else(|error| panic!("staging should succeed: {error}"));
        staged
            .discard()
            .unwrap_or_else(|error| panic!("discard should succeed: {error}"));
        let after = fs::read(store.path())
            .unwrap_or_else(|error| panic!("original bytes should remain readable: {error}"));

        assert_eq!(after, before);
    }

    #[test]
    fn config_store_rejects_oversized_file_before_parsing() {
        let test_directory = TestDirectory::new();
        let store = test_directory.store();
        fs::create_dir_all(
            store
                .path()
                .parent()
                .unwrap_or_else(|| panic!("test path has parent")),
        )
        .unwrap_or_else(|error| panic!("test config directory should exist: {error}"));
        let max_config_bytes = usize::try_from(MAX_CONFIG_BYTES)
            .unwrap_or_else(|error| panic!("test bound should fit usize: {error}"));
        fs::write(store.path(), vec![b'x'; max_config_bytes + 1])
            .unwrap_or_else(|error| panic!("oversized fixture should be written: {error}"));

        assert_eq!(
            store.load_with_environment(Vec::<(String, String)>::new()),
            Err(ConfigError::DocumentTooLarge)
        );
    }

    #[cfg(windows)]
    #[test]
    fn config_store_atomic_save_removes_windows_everyone_ace() {
        use std::process::Command;

        let test_directory = TestDirectory::new();
        let store = test_directory.store();
        store
            .save(&Config::default())
            .unwrap_or_else(|error| panic!("initial save should succeed: {error}"));

        let grant = Command::new("icacls")
            .arg(store.path())
            .args(["/grant:r", "*S-1-1-0:F", "/q"])
            .output()
            .unwrap_or_else(|error| panic!("Everyone grant should start: {error}"));
        assert!(grant.status.success());
        assert!(windows_sid_lookup_mentions_path(store.path(), "S-1-1-0"));

        store
            .save(&Config::default())
            .unwrap_or_else(|error| panic!("replacement save should succeed: {error}"));
        assert!(!windows_sid_lookup_mentions_path(store.path(), "S-1-1-0"));
    }

    #[cfg(unix)]
    #[test]
    fn config_store_writes_mode_0600_and_rejects_symlink() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let test_directory = TestDirectory::new();
        let store = test_directory.store();
        store
            .save(&Config::default())
            .unwrap_or_else(|error| panic!("save should succeed: {error}"));
        let mode = fs::metadata(store.path())
            .unwrap_or_else(|error| panic!("config metadata should be readable: {error}"))
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);

        let target = test_directory.0.join("target.toml");
        fs::write(&target, V1_CONFIG)
            .unwrap_or_else(|error| panic!("symlink target should be written: {error}"));
        fs::remove_file(store.path())
            .unwrap_or_else(|error| panic!("config should be removed: {error}"));
        symlink(&target, store.path())
            .unwrap_or_else(|error| panic!("config symlink should be created: {error}"));
        assert_eq!(store.load(), Err(ConfigError::SymbolicLink));
        assert_eq!(
            store.save(&Config::default()),
            Err(ConfigError::SymbolicLink)
        );
    }

    #[cfg(unix)]
    fn non_unicode_os_string() -> OsString {
        use std::os::unix::ffi::OsStringExt;

        OsString::from_vec(vec![0xff])
    }

    #[cfg(windows)]
    fn non_unicode_os_string() -> OsString {
        use std::os::windows::ffi::OsStringExt;

        OsString::from_wide(&[0xd800])
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
