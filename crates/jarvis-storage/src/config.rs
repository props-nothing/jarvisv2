use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use atomic_write_file::AtomicWriteFile;
use jarvis_core::{ApprovalPolicy, LogLevel, Risk};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use toml::Table;

use crate::{
    AppPaths, PathKind,
    paths::{prepare_private_directory, secure_private_file},
};

/// The configuration schema understood by this build.
pub const CURRENT_CONFIG_VERSION: u32 = 1;

/// The `executor_model` value that selects the live OpenAI-compatible provider.
///
/// Exposed so the daemon's composition site and this validation agree on one spelling rather than two
/// copies of a literal that must match — the defect class this repository keeps finding, where two values
/// that have to agree have nothing holding both. The deterministic `scripted` implementation is the other
/// value `executor_model` accepts, and it is named where it is built.
pub const LIVE_PROVIDER_MODEL_NAME: &str = "openai-compatible";

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
///
/// Not `Copy`, because `executor_model` holds a name and a `String` cannot be copied. `Clone`
/// remains, so a caller that needs an owned copy still gets one.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
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
    /// Which model the native run executor drives, or `None` for no executor.
    ///
    /// # Why this is a name and not a URL plus a key
    ///
    /// A provider endpoint carries a credential in its path or query, so putting it here would
    /// store a secret in a plaintext configuration file and make it a substring of every log line
    /// that mentions the endpoint. This field names a **built-in** model implementation; provider
    /// coordinates belong in the credential store when an adapter can reach one (`P2-003`
    /// deliberately shipped no live path). Until then the only selectable value is `scripted`,
    /// which is a deterministic local model and makes no claim to be a language model.
    ///
    /// `None` means runs are accepted and recorded but not executed, which is exactly what
    /// `P2-007` shipped. The daemon refuses an unrecognized value rather than ignoring it, so a
    /// typo cannot silently leave every run unexecuted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    executor_model: Option<String>,
    /// The model identifier a **live** provider is asked for.
    ///
    /// Distinct from [`Self::executor_model`], which selects a built-in **implementation**. This is the
    /// name the provider knows the model by (`gpt-oss:20b`, `llama3.2`, `qwen2.5`), and a self-hosted
    /// server serves whatever its operator pulled. Required when the selected implementation is a live
    /// provider, refused when it is not — a setting with no consumer is the shape this repository removes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    executor_model_name: Option<String>,
    /// The base URL a **live** provider is called at, including its version path (`https://host/v1`).
    ///
    /// # Why no credential may appear here
    ///
    /// A key placed in a URL is a substring of every log line, access log, and referrer it passes
    /// through — the mistake `jarvis-models`'s `BaseUrl` is built to reject. So the URL carries only the
    /// endpoint and the key is read from [`Self::executor_api_key_ref`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    executor_base_url: Option<String>,
    /// A **file** whose contents are the provider API key, read once at daemon startup.
    ///
    /// # Why a path and not the key
    ///
    /// The key is deliberately **not** in this document. A configuration file is plaintext, is routinely
    /// read by tooling and pasted into support bundles, and a credential in it is a credential in every
    /// copy of it. A path names a file the operator controls with its own permissions, and the key never
    /// enters the configuration, a log line, or an error message — the daemon reads it into the adapter's
    /// own redacting [`jarvis_models::openai::ApiKey`] and nowhere else. The path must be absolute, so it
    /// resolves the same wherever the daemon is started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    executor_api_key_ref: Option<PathBuf>,
    /// The absolute directories the read-only filesystem tool is confined to.
    ///
    /// # Why this is configuration and not derived
    ///
    /// `docs/architecture/tools-and-connectors.md` requires "explicit granted roots" for the
    /// filesystem adapter, and `docs/adr/0020-filesystem-confinement-is-a-handle.md` makes an empty or
    /// unusable grant a hard error rather than an empty workspace. A grant must therefore come from an
    /// operator decision, never from the daemon's working directory or a default like the profile root
    /// — a default would grant read access to a directory nobody chose, and one that the daemon's own
    /// state lives in.
    ///
    /// Empty means **no filesystem tool is registered at all**. That is deliberately not "a tool that
    /// reads nothing": a registered tool with no roots would be offered to a model and then fail every
    /// call, whereas an absent tool is simply not something the model can ask for.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tool_workspace_roots: Vec<PathBuf>,
    /// Whether JARVIS serves its own tools over MCP, and on which loopback port.
    ///
    /// # Why this is separate from `http_enabled`
    ///
    /// `http_enabled` serves the **JARVIS REST API**, which authenticates with the profile credential and
    /// speaks this project's own protocol. Serving MCP means accepting a *third-party protocol's* requests
    /// from clients that are not JARVIS, which is a different trust boundary with a different policy in
    /// front of it (`P3-009a`/`P3-009g`/`P3-009i`). Making one switch enable both would mean an operator
    /// who wanted the REST API also opened an MCP endpoint, which is the kind of coupling this project keeps
    /// finding one level down.
    ///
    /// It is also a separate **port** rather than a path on the REST listener for the same reason plus two
    /// mechanical ones: the MCP transport takes over the whole request (`axum`'s `fallback_service`, not a
    /// route, because the SDK's service dispatches on its own headers and methods), so a second listener is
    /// simpler than a nested router; and two listeners mean a mistake in one cannot expose the other.
    ///
    /// `None` means the endpoint is **not served at all**, which is the default: a daemon that opened an
    /// inbound protocol port unasked would contradict the reason this project prefers OS-native local IPC.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mcp_serve_port: Option<u16>,
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

    /// Returns the configured executor model name, when one is set.
    ///
    /// This selects the built-in **implementation** (`scripted`, `openai-compatible`), not the provider's
    /// model identifier — see [`Self::executor_model_name`].
    #[must_use]
    pub fn executor_model(&self) -> Option<&str> {
        self.executor_model.as_deref()
    }

    /// Returns the provider model identifier a live provider is asked for, when set.
    #[must_use]
    pub fn executor_model_name(&self) -> Option<&str> {
        self.executor_model_name.as_deref()
    }

    /// Returns the base URL a live provider is called at, when set.
    #[must_use]
    pub fn executor_base_url(&self) -> Option<&str> {
        self.executor_base_url.as_deref()
    }

    /// Returns the file whose contents are the provider API key, when set.
    #[must_use]
    pub fn executor_api_key_ref(&self) -> Option<&Path> {
        self.executor_api_key_ref.as_deref()
    }

    /// Returns whether every live-provider setting is present.
    ///
    /// All three are required together: a live implementation needs a model identifier, an endpoint, and a
    /// credential. Partial configuration is refused by [`Config::validate`] rather than discovered when the
    /// first run is started.
    #[must_use]
    pub fn has_complete_provider(&self) -> bool {
        self.executor_model_name.is_some()
            && self.executor_base_url.is_some()
            && self.executor_api_key_ref.is_some()
    }

    /// Returns the absolute directories the read-only filesystem tool is confined to.
    #[must_use]
    pub fn tool_workspace_roots(&self) -> &[PathBuf] {
        &self.tool_workspace_roots
    }

    /// Returns the loopback port JARVIS serves its own tools over MCP on, when enabled.
    #[must_use]
    pub const fn mcp_serve_port(&self) -> Option<u16> {
        self.mcp_serve_port
    }
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            shutdown_timeout_seconds: 15,
            http_enabled: false,
            http_port: DEFAULT_HTTP_PORT,
            // Off by default: a daemon that executes runs without being asked to would spend a
            // model budget nobody enabled.
            executor_model: None,
            executor_model_name: None,
            executor_base_url: None,
            executor_api_key_ref: None,
            // Empty: no filesystem tool is registered until an operator grants roots.
            tool_workspace_roots: Vec::new(),
            // Not served: an inbound MCP endpoint is opt-in, deliberately.
            mcp_serve_port: None,
        }
    }
}

/// The workspace's tool-authorization policy, as an operator writes it.
///
/// # Why this is a document section and not a database row
///
/// `ADR-0122` decides it: the configuration document is the **seed**, and the daemon's workspace
/// policy is built from it at startup. A control-plane UI that edits policy at runtime writes a row,
/// which is a different slice; what this section provides is the declarative form, because it is the
/// form an operator can review, diff, and keep under version control — and because a policy that only
/// exists in a database is a security posture nobody can read before the daemon starts.
///
/// # Why the risk fields are typed and the tool lists are not
///
/// `max_risk` and `approval_threshold` are [`Risk`] values, which is `jarvis-core`'s vocabulary and
/// therefore reachable from this adapter crate — a bad value is refused by `serde` with the field
/// named. The tool identifiers stay `String` here and are parsed where the tool registry is, because
/// `ToolId` belongs to `jarvis-tools` and `docs/architecture/repository-layout.md` forbids an adapter
/// depending on another adapter. A `ToolId::new` failure therefore surfaces at the composition root
/// with the identifier named, which is the same place an unknown *tool* is noticed.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyConfig {
    /// The highest risk the workspace permits at all. Absent means the workspace default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_risk: Option<Risk>,
    /// The risk at which an approval is always required. Absent means the workspace default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    approval_threshold: Option<Risk>,
    /// Tools the workspace refuses outright.
    ///
    /// A denial outranks every grant and every approval, so this is the one list whose entries take
    /// effect before any other setting is consulted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    deny: Vec<String>,
    /// Per-tool approval overrides, as `id = "policy"` entries.
    ///
    /// A table rather than a list of `{tool, approval}` records, because the operator's question is
    /// "what has been said about this tool" and a table answers it in one lookup. A **map** in TOML
    /// also cannot contain the same key twice, which is the duplicate-entry defect a list would allow.
    ///
    /// The values are [`ApprovalPolicy`], so a typo is refused by `serde` rather than silently
    /// becoming a default. The overrides can only **tighten**: see
    /// [`jarvis_tools::WorkspacePolicy::requiring`].
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    approval: BTreeMap<String, ApprovalPolicy>,
}

impl PolicyConfig {
    /// Returns the configured ceiling, when one was written.
    #[must_use]
    pub const fn max_risk(&self) -> Option<Risk> {
        self.max_risk
    }

    /// Returns the configured approval threshold, when one was written.
    #[must_use]
    pub const fn approval_threshold(&self) -> Option<Risk> {
        self.approval_threshold
    }

    /// Returns the denied tool identifiers, in stable order.
    #[must_use]
    pub fn deny(&self) -> &[String] {
        &self.deny
    }

    /// Returns the per-tool approval overrides, in stable identifier order.
    #[must_use]
    pub const fn approval(&self) -> &BTreeMap<String, ApprovalPolicy> {
        &self.approval
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
    /// The workspace tool-authorization policy.
    ///
    /// `default` so a document written before this section existed — or one that simply does not
    /// configure policy — parses and gets the workspace defaults. That direction is the safe one:
    /// an absent section means "no operator opinion", which leaves every tool's own declaration in
    /// force and denies nothing, whereas requiring the section would make an upgrade refuse to start.
    #[serde(default)]
    policy: PolicyConfig,
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
            policy: PolicyConfig::default(),
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
                        policy: PolicyConfig::default(),
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

    /// Returns the workspace tool-authorization policy.
    #[must_use]
    pub const fn policy(&self) -> &PolicyConfig {
        &self.policy
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
        // The same rule for the MCP endpoint, and it matters more here: an MCP **client** is configured with
        // a URL, so an ephemeral port would leave a third-party client unable to reach a server the operator
        // believes they enabled. `Some(0)` is refused rather than normalised.
        if self.daemon.mcp_serve_port == Some(0) {
            return Err(ConfigError::InvalidMcpServePort);
        }
        // An MCP endpoint with nothing to serve is refused at startup rather than left to answer every
        // request with an empty catalogue. `served_tools` already refuses an empty surface, so this is the
        // **configuration-time** form of the same decision: an operator who enabled the endpoint without
        // granting roots or configuring a server would otherwise get a daemon that reports itself ready
        // with a port that cannot answer anything.
        if self.daemon.mcp_serve_port.is_some() && self.daemon.tool_workspace_roots.is_empty() {
            return Err(ConfigError::McpServeWithoutTools);
        }
        // The live-provider settings are grouped: they are meaningful together and each is meaningless
        // alone. Three cases are refused rather than tolerated, because each would otherwise be a daemon
        // that started and then failed — or worse, quietly drove nothing:
        //
        // - **a live implementation with no provider coordinates** cannot call anything;
        // - **provider coordinates with no live implementation** are settings with no consumer (the shape
        //   that reads as configured while doing nothing), which is why they are refused rather than
        //   ignored;
        // - **a base URL with no key file**, or the reverse, is half a credential — the provider would
        //   reject every call at runtime rather than at startup.
        let live = self.daemon.executor_model.as_deref() == Some(LIVE_PROVIDER_MODEL_NAME);
        let has_coordinates = self.daemon.executor_model_name.is_some()
            || self.daemon.executor_base_url.is_some()
            || self.daemon.executor_api_key_ref.is_some();
        if live && !self.daemon.has_complete_provider() {
            return Err(ConfigError::IncompleteModelProvider);
        }
        if !live && has_coordinates {
            return Err(ConfigError::ModelProviderWithoutImplementation);
        }
        // The path must be absolute so it resolves the same wherever the daemon is started, the same rule
        // the configuration path itself follows.
        if let Some(path) = &self.daemon.executor_api_key_ref
            && !path.is_absolute()
        {
            return Err(ConfigError::RelativeModelApiKeyRef);
        }
        self.validate_policy()?;
        Ok(())
    }

    /// Validates the workspace policy section.
    ///
    /// # What is refused, and what deliberately is not
    ///
    /// Two things are refused here because they are **self-contradictory documents**, not because the
    /// values are unsafe:
    ///
    /// - a threshold above the ceiling, which would require approval for every risk already refused —
    ///   the same contradiction [`jarvis_tools::WorkspacePolicy::new`] refuses, checked here as well so
    ///   an operator gets a configuration error naming the file rather than a daemon that will not start
    ///   for a reason expressed in another crate's vocabulary;
    /// - a blank tool identifier, which names nothing and would silently apply to nothing.
    ///
    /// A tool identifier that names no **registered** tool is deliberately **not** refused. The registry
    /// is not reachable from this crate, and more importantly an MCP server's tools are discovered at
    /// startup: an operator writing a policy for a server that is temporarily down would otherwise be
    /// unable to start the daemon. An override for a tool that does not exist is inert, and the
    /// composition root reports the mismatch rather than this layer guessing.
    fn validate_policy(&self) -> Result<(), ConfigError> {
        // The two fields are independently optional, so the effective pair is each field's value or its
        // default. The defaults cannot contradict each other (`High` ceiling, `Moderate` threshold), so
        // only a written pair can.
        let max_risk = self.policy.max_risk.unwrap_or(Risk::High);
        let threshold = self.policy.approval_threshold.unwrap_or(Risk::Moderate);
        if threshold > max_risk {
            return Err(ConfigError::ApprovalThresholdAboveCeiling {
                approval_threshold: threshold,
                max_risk,
            });
        }
        for identifier in self.policy.approval.keys() {
            if identifier.trim().is_empty() {
                return Err(ConfigError::BlankPolicyTool { key: "approval" });
            }
        }
        for identifier in &self.policy.deny {
            if identifier.trim().is_empty() {
                return Err(ConfigError::BlankPolicyTool { key: "deny" });
            }
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
            policy: PolicyConfig::default(),
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
    /// The MCP endpoint was enabled with a port that cannot be named.
    ///
    /// Refused for the same reason as [`Self::InvalidHttpPort`], and it matters more here because an MCP
    /// **client** is configured with a URL: an ephemeral port would leave a third-party client unable to
    /// reach a server the operator believes they enabled.
    #[error("daemon.mcp_serve_port must be between 1 and 65535 when set")]
    InvalidMcpServePort,
    /// The MCP endpoint was enabled with no tool that could be served.
    ///
    /// Refused at startup rather than left to answer every request with an empty catalogue. `served_tools`
    /// refuses an empty surface already, so this is the configuration-time form of the same decision: an
    /// operator who enabled the endpoint without granting roots would otherwise get a daemon that reports
    /// itself ready with a port that can answer nothing.
    #[error(
        "daemon.mcp_serve_port requires a tool to serve: set daemon.tool_workspace_roots or configure \
         an MCP server"
    )]
    McpServeWithoutTools,
    /// A live provider implementation was selected without its coordinates.
    ///
    /// Refused at startup rather than left to fail on the first run: an operator who selected the live
    /// provider but supplied no endpoint, model, or key would otherwise get a daemon that reports itself
    /// ready and then fails every run.
    #[error(
        "daemon.executor_model = openai-compatible requires daemon.executor_model_name, \
         daemon.executor_base_url, and daemon.executor_api_key_ref"
    )]
    IncompleteModelProvider,
    /// Provider coordinates were supplied without a live implementation to consume them.
    ///
    /// Refused because a setting with no consumer reads as configured while doing nothing — the shape this
    /// repository removes wherever it finds it. It also catches the common mistake of setting the provider
    /// fields and leaving `executor_model` unset, which would otherwise leave every run unexecuted.
    #[error(
        "daemon.executor_model_name, daemon.executor_base_url, and daemon.executor_api_key_ref require \
         daemon.executor_model = openai-compatible"
    )]
    ModelProviderWithoutImplementation,
    /// The API key file path was not absolute.
    ///
    /// Refused for the same reason the configuration path is: a relative path resolves against the
    /// process's working directory, so the daemon would read a different file depending on where it was
    /// started — or none at all.
    #[error("daemon.executor_api_key_ref must be an absolute path")]
    RelativeModelApiKeyRef,
    /// The policy approval threshold was above the ceiling the same document sets.
    ///
    /// A self-contradictory document: every risk that could be approved is already refused, so the
    /// threshold can never take effect. Refused here as well as in `jarvis-tools` so the error names the
    /// configuration file an operator edited rather than a crate they have not read.
    #[error(
        "policy.approval_threshold ({approval_threshold}) is above policy.max_risk ({max_risk}), \
         which would require approval for every risk already refused"
    )]
    ApprovalThresholdAboveCeiling {
        /// The threshold the document declared.
        approval_threshold: Risk,
        /// The ceiling the document declared.
        max_risk: Risk,
    },
    /// A policy entry named no tool.
    ///
    /// Refused rather than skipped: a blank identifier applies to nothing while reading as a configured
    /// restriction, and the likely cause is a templating mistake an operator needs to see.
    #[error("policy.{key} contains a blank tool identifier")]
    BlankPolicyTool {
        /// Which policy list held the blank entry.
        key: &'static str,
    },
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
        &["schema_version", "profile", "logging", "daemon", "policy"]
    };
    collect_unknown_keys(table, root_keys, "", &mut unknown);

    if version == CURRENT_CONFIG_VERSION {
        collect_nested_unknown(table, "profile", &["name"], &mut unknown);
        collect_nested_unknown(table, "logging", &["level"], &mut unknown);
        collect_nested_unknown(
            table,
            "policy",
            &["max_risk", "approval_threshold", "deny", "approval"],
            &mut unknown,
        );
        collect_nested_unknown(
            table,
            "daemon",
            &[
                "shutdown_timeout_seconds",
                "http_enabled",
                "http_port",
                "executor_model",
                "executor_model_name",
                "executor_base_url",
                "executor_api_key_ref",
                "tool_workspace_roots",
                "mcp_serve_port",
            ],
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
            "JARVIS_EXECUTOR_MODEL" => {
                let name = environment_text(&value, "JARVIS_EXECUTOR_MODEL")?;
                // An empty value is refused rather than treated as unset: `JARVIS_EXECUTOR_MODEL=`
                // reads as "enable the executor" and would otherwise silently disable it.
                if name.trim().is_empty() {
                    return Err(ConfigError::InvalidEnvironmentValue {
                        key: "JARVIS_EXECUTOR_MODEL",
                    });
                }
                config.daemon.executor_model = Some(name.to_owned());
            }
            "JARVIS_MCP_SERVE_PORT" => {
                // An empty value is refused rather than treated as "not served", for the same reason
                // `JARVIS_EXECUTOR_MODEL=` is: it reads as "enable this" and would otherwise silently
                // disable it, leaving an operator to debug a client connection instead of a setting.
                let text = environment_text(&value, "JARVIS_MCP_SERVE_PORT")?;
                if text.trim().is_empty() {
                    return Err(ConfigError::InvalidEnvironmentValue {
                        key: "JARVIS_MCP_SERVE_PORT",
                    });
                }
                config.daemon.mcp_serve_port =
                    Some(
                        text.parse()
                            .map_err(|_| ConfigError::InvalidEnvironmentValue {
                                key: "JARVIS_MCP_SERVE_PORT",
                            })?,
                    );
            }
            _ => {
                // The live-provider variables are handled by a helper, so this function stays about the
                // handful of core lifecycle settings rather than growing a match arm per provider field.
                // A `false` return means the key is not one this build recognizes, which fails closed.
                if !apply_provider_environment(&mut config.daemon, key, &value)? {
                    return Err(ConfigError::UnknownEnvironmentKey {
                        key: bounded_identifier(key),
                    });
                }
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

/// Applies the live-provider environment variables, returning whether the key was one of them.
///
/// Extracted so [`apply_environment`] does not grow an arm per provider field: the three variables are a
/// group (they configure one thing) and handling them together keeps that grouping visible. Each is refused
/// when empty rather than treated as unset, for the reason stated on `JARVIS_EXECUTOR_MODEL`: an empty value
/// reads as "set this" and would otherwise silently leave the provider half-configured.
fn apply_provider_environment(
    daemon: &mut DaemonConfig,
    key: &str,
    value: &OsString,
) -> Result<bool, ConfigError> {
    match key {
        "JARVIS_EXECUTOR_MODEL_NAME" => {
            let name = environment_text(value, "JARVIS_EXECUTOR_MODEL_NAME")?;
            if name.trim().is_empty() {
                return Err(ConfigError::InvalidEnvironmentValue {
                    key: "JARVIS_EXECUTOR_MODEL_NAME",
                });
            }
            daemon.executor_model_name = Some(name.to_owned());
        }
        "JARVIS_EXECUTOR_BASE_URL" => {
            let url = environment_text(value, "JARVIS_EXECUTOR_BASE_URL")?;
            if url.trim().is_empty() {
                return Err(ConfigError::InvalidEnvironmentValue {
                    key: "JARVIS_EXECUTOR_BASE_URL",
                });
            }
            // The raw text is stored rather than a trimmed copy: a URL with surrounding whitespace would be
            // a different endpoint, and `BaseUrl::new` trims and validates it at construction, so storing
            // the raw value keeps the configuration and the adapter from disagreeing about what was set.
            daemon.executor_base_url = Some(url.to_owned());
        }
        "JARVIS_EXECUTOR_API_KEY_REF" => {
            let path = environment_text(value, "JARVIS_EXECUTOR_API_KEY_REF")?;
            if path.trim().is_empty() {
                return Err(ConfigError::InvalidEnvironmentValue {
                    key: "JARVIS_EXECUTOR_API_KEY_REF",
                });
            }
            daemon.executor_api_key_ref = Some(PathBuf::from(path));
        }
        _ => return Ok(false),
    }
    Ok(true)
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

    use super::*;

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!(
                "jarvis-storage-config-{}",
                jarvis_core::scratch_tag()
            )))
        }

        fn store(&self) -> ConfigStore {
            ConfigStore::new(self.0.join("config").join(CONFIG_FILE_NAME))
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            jarvis_core::remove_scratch_dir(&self.0);
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

    /// A config that selects the live provider reads all four settings, from the document and the
    /// environment, with the environment winning.
    #[test]
    fn the_live_provider_model_is_configurable_from_document_and_environment() {
        // The key path must be absolute **on the platform running the test**, because validation refuses a
        // relative one and a Unix path is relative on Windows — the platform-fixture trap this workspace
        // has recorded before. Forward slashes are used on both, because a Windows path with backslashes
        // inside a TOML **basic** string is an escape sequence and would make the document invalid rather
        // than test the path rule; `C:/...` is still an absolute Windows path.
        let key_path = if cfg!(windows) {
            "C:/jarvis/model.key"
        } else {
            "/etc/jarvis/model.key"
        };
        let document = format!(
            r#"
schema_version = 1

[profile]
name = "home"

[logging]
level = "warn"

[daemon]
shutdown_timeout_seconds = 30
executor_model = "openai-compatible"
executor_model_name = "gpt-oss:20b"
executor_base_url = "http://127.0.0.1:11434/v1"
executor_api_key_ref = "{key_path}"
"#
        );
        let loaded =
            Config::parse_with_environment(&document, [("JARVIS_EXECUTOR_MODEL_NAME", "llama3.2")])
                .unwrap_or_else(|error| {
                    panic!("a complete live-provider config must load: {error}")
                });

        let daemon = loaded.config().daemon();
        assert_eq!(
            daemon.executor_model(),
            Some(LIVE_PROVIDER_MODEL_NAME),
            "the implementation selector must be read"
        );
        assert_eq!(
            daemon.executor_model_name(),
            Some("llama3.2"),
            "the environment must override the document's provider model"
        );
        assert_eq!(
            daemon.executor_base_url(),
            Some("http://127.0.0.1:11434/v1"),
            "the base URL must round-trip"
        );
        assert_eq!(
            daemon.executor_api_key_ref(),
            Some(Path::new(key_path)),
            "the API key **file** path must round-trip, and it must be a path not the key"
        );
        assert!(
            daemon.has_complete_provider(),
            "all three provider coordinates must be present"
        );
    }

    /// **The live implementation without its coordinates is refused at startup.**
    #[test]
    fn a_live_provider_without_coordinates_is_refused() {
        let document = r#"
schema_version = 1

[profile]
name = "home"

[logging]
level = "warn"

[daemon]
shutdown_timeout_seconds = 30
executor_model = "openai-compatible"
"#;
        let error = Config::parse_with_environment(document, Vec::<(String, String)>::new())
            .err()
            .unwrap_or_else(|| {
                panic!("a live provider with no endpoint, model, or key must be refused")
            });
        assert_eq!(error, ConfigError::IncompleteModelProvider);
    }

    /// **Provider coordinates with no live implementation are refused rather than ignored.**
    ///
    /// A setting with no consumer reads as configured while doing nothing, which is exactly the mistake an
    /// operator makes when they set the provider fields and forget `executor_model`.
    #[test]
    fn provider_coordinates_without_the_implementation_are_refused() {
        let document = r#"
schema_version = 1

[profile]
name = "home"

[logging]
level = "warn"

[daemon]
shutdown_timeout_seconds = 30
executor_model_name = "gpt-oss:20b"
executor_base_url = "http://127.0.0.1:11434/v1"
executor_api_key_ref = "/etc/jarvis/model.key"
"#;
        let error = Config::parse_with_environment(document, Vec::<(String, String)>::new())
            .err()
            .unwrap_or_else(|| {
                panic!("provider coordinates with no implementation must be refused")
            });
        assert_eq!(error, ConfigError::ModelProviderWithoutImplementation);
    }

    /// The API key file path must be absolute, for the same reason the configuration path must be.
    #[test]
    fn a_relative_api_key_path_is_refused() {
        let document = r#"
schema_version = 1

[profile]
name = "home"

[logging]
level = "warn"

[daemon]
shutdown_timeout_seconds = 30
executor_model = "openai-compatible"
executor_model_name = "gpt-oss:20b"
executor_base_url = "http://127.0.0.1:11434/v1"
executor_api_key_ref = "model.key"
"#;
        let error = Config::parse_with_environment(document, Vec::<(String, String)>::new())
            .err()
            .unwrap_or_else(|| panic!("a relative key path must be refused"));
        assert_eq!(error, ConfigError::RelativeModelApiKeyRef);
    }

    /// **A policy section is read, and the absent section is the workspace default rather than an error.**
    ///
    /// The two halves are one claim: an operator can configure policy, and a document that does not
    /// still starts. The second half matters because it is what an upgrade looks like — a config
    /// written before this section existed must not refuse to load.
    #[test]
    fn a_policy_section_is_read_and_defaults_when_absent() {
        let configured = r#"
schema_version = 1

[profile]
name = "home"

[logging]
level = "warn"

[daemon]
shutdown_timeout_seconds = 30

[policy]
max_risk = "high"
approval_threshold = "low"
deny = ["jarvis.mail.send"]

[policy.approval]
"jarvis.files.read" = "ask"
"mcp.github.create_issue" = "deny"
"#;
        let loaded = Config::parse_with_environment(configured, Vec::<(String, String)>::new())
            .unwrap_or_else(|error| panic!("a policy section should load: {error}"));
        let policy = loaded.config().policy();
        assert_eq!(policy.max_risk(), Some(Risk::High));
        assert_eq!(policy.approval_threshold(), Some(Risk::Low));
        assert_eq!(policy.deny(), ["jarvis.mail.send"]);
        assert_eq!(
            policy.approval().get("jarvis.files.read"),
            Some(&ApprovalPolicy::Ask)
        );
        assert_eq!(
            policy.approval().get("mcp.github.create_issue"),
            Some(&ApprovalPolicy::Deny)
        );

        // The absent section is the default, not an error.
        let bare = Config::parse_with_environment(V1_CONFIG, Vec::<(String, String)>::new())
            .unwrap_or_else(|error| panic!("a document with no policy section must load: {error}"));
        assert_eq!(bare.config().policy(), &PolicyConfig::default());
        assert_eq!(bare.config().policy().max_risk(), None);
    }

    /// **An unknown approval policy name is refused rather than defaulted.**
    ///
    /// The direction that matters: the likely mistake is a misspelled *tightening*, and a default
    /// would silently leave the tool's own policy in force while the operator believes the opposite.
    #[test]
    fn an_unknown_approval_policy_name_is_refused() {
        let document = r#"
schema_version = 1

[profile]
name = "home"

[logging]
level = "warn"

[daemon]
shutdown_timeout_seconds = 30

[policy.approval]
"jarvis.files.read" = "always"
"#;
        let error = Config::parse_with_environment(document, Vec::<(String, String)>::new())
            .err()
            .unwrap_or_else(|| panic!("an unknown approval policy must be refused"));
        assert_eq!(error, ConfigError::InvalidDocument);
    }

    /// **A threshold above the ceiling is refused, because the document contradicts itself.**
    ///
    /// Every risk that could be approved is already refused, so the threshold can never take effect.
    /// Asserted with the two field names in the message, so an operator is told which values clash.
    #[test]
    fn a_policy_threshold_above_the_ceiling_is_refused() {
        let document = r#"
schema_version = 1

[profile]
name = "home"

[logging]
level = "warn"

[daemon]
shutdown_timeout_seconds = 30

[policy]
max_risk = "low"
approval_threshold = "high"
"#;
        let error = Config::parse_with_environment(document, Vec::<(String, String)>::new())
            .err()
            .unwrap_or_else(|| panic!("a contradictory policy must be refused"));
        assert_eq!(
            error,
            ConfigError::ApprovalThresholdAboveCeiling {
                approval_threshold: Risk::High,
                max_risk: Risk::Low,
            }
        );
    }

    /// **A blank tool identifier is refused rather than skipped.**
    ///
    /// A blank entry applies to nothing while reading as a configured restriction, and the likely cause
    /// is a templating mistake the operator needs to see.
    #[test]
    fn a_blank_policy_tool_identifier_is_refused() {
        let document = r#"
schema_version = 1

[profile]
name = "home"

[logging]
level = "warn"

[daemon]
shutdown_timeout_seconds = 30

[policy]
deny = ["  "]
"#;
        let error = Config::parse_with_environment(document, Vec::<(String, String)>::new())
            .err()
            .unwrap_or_else(|| panic!("a blank tool identifier must be refused"));
        assert_eq!(error, ConfigError::BlankPolicyTool { key: "deny" });
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
