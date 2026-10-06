//! The settings a person may change after setup, and the keys behind them: one code path for `jarvis config`,
//! `jarvis keys` and the console's Settings screen (`P9-014`, `P9-015`).
//!
//! Every change is made on the stored document, checked by the **same parser the daemon starts with**, and saved the
//! way `init` saves it, so a setting cannot be valid in one surface and refused at startup. A key is written to a
//! private file and the configuration holds only its path; no function here returns, logs or errors with a key's value.

use std::path::{Path, PathBuf};

use toml::{Table, Value};

use crate::{AppPaths, Config, ConfigStore, PathKind, secure_private_file};

/// How a setting's value is read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    Text,
    File,
    Folders,
    Words,
    Port,
    ToolIds,
}

impl Kind {
    const fn name(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::File => "file",
            Self::Folders => "folders",
            Self::Words => "words",
            Self::Port => "port",
            Self::ToolIds => "tools",
        }
    }
}

struct Setting {
    table: &'static str,
    field: &'static str,
    kind: Kind,
    help: &'static str,
    /// Which tab of the settings screen it lives on.
    group: &'static str,
    /// The value in force when it is not set, when there is one.
    default: Option<&'static str>,
    /// What it means that it is not set: why it is off by default.
    unset_means: &'static str,
    /// A sample value, shown as a hint.
    example: &'static str,
}

const SETTINGS: &[Setting] = &[
    Setting {
        table: "daemon",
        field: "executor_model_name",
        kind: Kind::Text,
        help: "the model to talk to",
        group: "brain",
        default: None,
        unset_means: "No model, so the assistant cannot answer. Setup always sets one.",
        example: "glm-5.3:cloud",
    },
    Setting {
        table: "daemon",
        field: "executor_base_url",
        kind: Kind::Text,
        help: "the model server address, with its /v1",
        group: "brain",
        default: None,
        unset_means: "No model server. Setup always sets one.",
        example: "http://localhost:11434/v1",
    },
    Setting {
        table: "daemon",
        field: "executor_api_key_ref",
        kind: Kind::File,
        help: "file holding the model key (set the key itself with `jarvis keys set model`)",
        group: "brain",
        default: None,
        unset_means: "No key file. Setup writes one (a placeholder for local Ollama).",
        example: "",
    },
    Setting {
        table: "daemon",
        field: "tool_workspace_roots",
        kind: Kind::Folders,
        help: "folders the assistant may read and write (replaces the list)",
        group: "files",
        default: None,
        unset_means: "Off by design: with no folder granted the assistant has no file tools at all, so nothing outside a folder you chose is ever reachable.",
        example: "C:/Users/me/notes; C:/Users/me/projects",
    },
    Setting {
        table: "daemon",
        field: "code_sandbox_image",
        kind: Kind::Text,
        help: "container image the code tool runs in",
        group: "files",
        default: None,
        unset_means: "Off by design: running code needs Docker and an image you pulled yourself (it is never pulled for you), so there is no code tool until you name one.",
        example: "node:22-alpine",
    },
    Setting {
        table: "daemon",
        field: "code_sandbox_interpreter",
        kind: Kind::Words,
        help: "command inside it, for example: node -e",
        group: "files",
        default: None,
        unset_means: "Needed together with the image above; off while there is no image.",
        example: "node -e",
    },
    Setting {
        table: "daemon",
        field: "speech_api_key_ref",
        kind: Kind::File,
        help: "file holding the voice key (set the key itself with `jarvis keys set voice`)",
        group: "voice",
        default: None,
        unset_means: "Opt-in: without an ElevenLabs key the console speaks with your browser's own voice.",
        example: "",
    },
    Setting {
        table: "daemon",
        field: "speech_voice_id",
        kind: Kind::Text,
        help: "ElevenLabs voice id",
        group: "voice",
        default: Some("JBFqnCBsd6RMkjVDRZzb"),
        unset_means: "Uses the built-in default voice (George). Only used once a voice key is set.",
        example: "JBFqnCBsd6RMkjVDRZzb",
    },
    Setting {
        table: "daemon",
        field: "speech_model",
        kind: Kind::Text,
        help: "ElevenLabs model id",
        group: "voice",
        default: Some("eleven_v4_turbo"),
        unset_means: "Uses the built-in default model. Only used once a voice key is set.",
        example: "eleven_flash_v2_5",
    },
    Setting {
        table: "policy",
        field: "trust",
        kind: Kind::ToolIds,
        help: "tools allowed to run without asking",
        group: "permissions",
        default: None,
        unset_means: "Nothing is trusted by default: a tool that the policy holds always asks you first.",
        example: "jarvis.files.edit",
    },
    Setting {
        table: "daemon",
        field: "http_port",
        kind: Kind::Port,
        help: "the local port the console and CLI use",
        group: "advanced",
        default: Some("8765"),
        unset_means: "Uses the default port.",
        example: "8765",
    },
    Setting {
        table: "daemon",
        field: "mcp_serve_port",
        kind: Kind::Port,
        help: "serve JARVIS tools to other programs over MCP on this port",
        group: "advanced",
        default: None,
        unset_means: "Off by design: JARVIS opens no inbound MCP port unless you ask it to.",
        example: "8766",
    },
];
/// One setting as shown to a person.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SettingView {
    /// `table.field`, the name `jarvis config` uses.
    pub key: String,
    /// What shape of value it takes: `text`, `file`, `folders`, `words`, `port` or `tools`.
    pub kind: &'static str,
    /// A one-line description.
    pub help: &'static str,
    /// The current value (a path for a key file, never a key), if set.
    pub value: Option<String>,
    /// For a file setting: `present` or `missing`.
    pub file_state: Option<&'static str>,
    /// The tab it lives on: `brain`, `voice`, `files`, `permissions` or `advanced`.
    pub group: &'static str,
    /// The value in force when it is not set, if there is one.
    pub default: Option<&'static str>,
    /// What it means that it is not set.
    pub unset_means: &'static str,
    /// A sample value.
    pub example: &'static str,
}

fn find(key: &str) -> Option<&'static Setting> {
    SETTINGS.iter().find(|setting| {
        format!("{}.{}", setting.table, setting.field) == key || setting.field == key
    })
}

/// The supported settings, one per line, for an error message.
#[must_use]
pub fn supported() -> String {
    SETTINGS
        .iter()
        .map(|setting| format!("  {}.{:<26} {}", setting.table, setting.field, setting.help))
        .collect::<Vec<_>>()
        .join("\n")
}

fn unknown(key: &str) -> String {
    format!(
        "{key:?} is not a setting that can be changed here. The settings are:\n{}",
        supported()
    )
}

fn strip_verbatim(path: PathBuf) -> PathBuf {
    let text = path.display().to_string();
    text.strip_prefix(r"\\?\").map_or(path, PathBuf::from)
}

fn convert(kind: Kind, values: &[String]) -> Result<Value, String> {
    let one = || match values {
        [only] if !only.trim().is_empty() => Ok(only.trim().to_owned()),
        _ => Err("this setting takes exactly one value".to_owned()),
    };
    match kind {
        Kind::Text => Ok(Value::String(one()?)),
        Kind::File => {
            let path = PathBuf::from(one()?);
            if !path.is_absolute() || !path.is_file() {
                return Err(format!(
                    "{} is not the absolute path of an existing file",
                    path.display()
                ));
            }
            Ok(Value::String(path.display().to_string()))
        }
        Kind::Folders => {
            let mut roots = Vec::new();
            for value in values {
                match std::fs::canonicalize(value) {
                    Ok(resolved) if resolved.is_dir() => {
                        roots.push(Value::String(
                            strip_verbatim(resolved).display().to_string(),
                        ));
                    }
                    _ => return Err(format!("{value} is not a folder that exists")),
                }
            }
            Ok(Value::Array(roots))
        }
        Kind::Words => {
            let words: Vec<String> = if values.len() == 1 {
                values[0].split_whitespace().map(str::to_owned).collect()
            } else {
                values.to_vec()
            };
            if words.is_empty() {
                return Err("give the command to run, for example: node -e".to_owned());
            }
            Ok(Value::Array(words.into_iter().map(Value::String).collect()))
        }
        Kind::Port => one()?
            .parse::<u16>()
            .ok()
            .filter(|port| *port > 0)
            .map(|port| Value::Integer(i64::from(port)))
            .ok_or_else(|| "a port is a number from 1 to 65535".to_owned()),
        Kind::ToolIds => {
            if values.iter().any(|value| value.trim().is_empty()) {
                return Err("tool ids must not be blank".to_owned());
            }
            Ok(Value::Array(
                values.iter().cloned().map(Value::String).collect(),
            ))
        }
    }
}

fn read_table(store: &ConfigStore) -> Result<Table, String> {
    let text = std::fs::read_to_string(store.path())
        .map_err(|_| "there is no configuration yet; run `jarvis init`".to_owned())?;
    toml::from_str(&text).map_err(|_| "the stored configuration is not valid TOML".to_owned())
}

/// Reads the stored document, lets `change` edit it, checks the result with the daemon's own parser and saves it. A
/// refused change leaves the stored document untouched.
fn edit(
    paths: &AppPaths,
    change: impl FnOnce(&mut Table) -> Result<(), String>,
) -> Result<(), String> {
    let store = ConfigStore::from_paths(paths);
    let mut table = read_table(&store)?;
    change(&mut table)?;
    let document = toml::to_string(&table).map_err(|error| error.to_string())?;
    let config: Config =
        Config::parse_with_environment(&document, std::iter::empty::<(&str, &str)>())
            .map_err(|error| format!("that would make the configuration invalid: {error}"))?
            .into_config();
    store
        .save(&config)
        .map_err(|error| format!("the configuration could not be saved: {error}"))
}

fn table_mut<'a>(table: &'a mut Table, name: &str) -> Result<&'a mut Table, String> {
    table
        .entry(name.to_owned())
        .or_insert_with(|| Value::Table(Table::new()))
        .as_table_mut()
        .ok_or_else(|| format!("[{name}] in the configuration is not a table"))
}

fn lookup<'a>(table: &'a Table, setting: &Setting) -> Option<&'a Value> {
    table.get(setting.table)?.as_table()?.get(setting.field)
}

fn show_value(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(items) => items.iter().map(show_value).collect::<Vec<_>>().join(", "),
        other => other.to_string(),
    }
}

/// Every supported setting with its current value.
///
/// # Errors
///
/// Returns a message when there is no configuration yet or it cannot be read.
pub fn list(paths: &AppPaths) -> Result<Vec<SettingView>, String> {
    let table = read_table(&ConfigStore::from_paths(paths))?;
    Ok(SETTINGS
        .iter()
        .map(|setting| {
            let value = lookup(&table, setting).map(show_value);
            let file_state = (setting.kind == Kind::File)
                .then(|| {
                    value.as_deref().map(|path| {
                        if Path::new(path).is_file() {
                            "present"
                        } else {
                            "missing"
                        }
                    })
                })
                .flatten();
            SettingView {
                key: format!("{}.{}", setting.table, setting.field),
                kind: setting.kind.name(),
                help: setting.help,
                value,
                file_state,
                group: setting.group,
                default: setting.default,
                unset_means: setting.unset_means,
                example: setting.example,
            }
        })
        .collect())
}

/// One setting's value.
///
/// # Errors
///
/// Returns a message for an unknown or unset setting.
pub fn get(paths: &AppPaths, key: &str) -> Result<String, String> {
    let setting = find(key).ok_or_else(|| unknown(key))?;
    let table = read_table(&ConfigStore::from_paths(paths))?;
    lookup(&table, setting)
        .map(|value| match (setting.kind, value) {
            // A command reads as a command (`node -e`), not as a list.
            (Kind::Words, Value::Array(words)) => {
                words.iter().map(show_value).collect::<Vec<_>>().join(" ")
            }
            _ => show_value(value),
        })
        .ok_or_else(|| format!("{key} is not set"))
}

/// Sets one setting. The value is converted for its kind, the whole document is re-validated by the daemon's parser, and
/// nothing is written if that fails.
///
/// # Errors
///
/// Returns a message saying why the change was refused.
pub fn set(paths: &AppPaths, key: &str, values: &[String]) -> Result<(), String> {
    let setting = find(key).ok_or_else(|| unknown(key))?;
    let value = convert(setting.kind, values)?;
    edit(paths, |table| {
        table_mut(table, setting.table)?.insert(setting.field.to_owned(), value);
        Ok(())
    })
}

/// Removes one setting.
///
/// # Errors
///
/// Returns a message for an unknown setting or when removing it would leave the configuration invalid.
pub fn unset(paths: &AppPaths, key: &str) -> Result<(), String> {
    let setting = find(key).ok_or_else(|| unknown(key))?;
    edit(paths, |table| {
        table_mut(table, setting.table)?.remove(setting.field);
        Ok(())
    })
}

/// One change in a batch: set a setting to these values, or (`None`) remove it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Change {
    /// The setting, as `table.field` or its field name.
    pub key: String,
    /// The new values, or `None` to remove the setting.
    pub values: Option<Vec<String>>,
}

/// Applies several changes **together**: every value is converted, all are applied to the stored document, and the result is
/// checked once by the daemon's parser, so settings that only make sense as a pair (a code image and its interpreter, a
/// voice key and its voice) can be changed in one step. If any part is refused nothing is written.
///
/// # Errors
///
/// Returns a message saying which change was refused or why the result would be invalid.
pub fn apply(paths: &AppPaths, changes: &[Change]) -> Result<(), String> {
    let mut prepared = Vec::new();
    for change in changes {
        let setting = find(&change.key).ok_or_else(|| unknown(&change.key))?;
        let value = match &change.values {
            Some(values) => Some(convert(setting.kind, values)?),
            None => None,
        };
        prepared.push((setting, value));
    }
    edit(paths, |table| {
        for (setting, value) in prepared {
            let section = table_mut(table, setting.table)?;
            match value {
                Some(value) => {
                    section.insert(setting.field.to_owned(), value);
                }
                None => {
                    section.remove(setting.field);
                }
            }
        }
        Ok(())
    })
}

/// What the owner has decided about one tool, beyond what the tool declares for itself.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Posture {
    /// No decision: the tool's own declaration and the risk threshold apply.
    Default,
    /// Always ask before this tool runs (`policy.approval`).
    Ask,
    /// Run without asking (`policy.trust`); never waives the external-communication rule, the ceiling or a denial.
    Trusted,
    /// Never run (`policy.deny`).
    Off,
}

impl Posture {
    /// Parses `default`, `ask`, `trusted` or `off`.
    ///
    /// # Errors
    ///
    /// Returns a message for anything else.
    pub fn parse(word: &str) -> Result<Self, String> {
        match word {
            "default" => Ok(Self::Default),
            "ask" => Ok(Self::Ask),
            "trusted" => Ok(Self::Trusted),
            "off" => Ok(Self::Off),
            _ => Err("a posture is default, ask, trusted or off".to_owned()),
        }
    }

    /// The word for this posture.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Ask => "ask",
            Self::Trusted => "trusted",
            Self::Off => "off",
        }
    }
}

fn valid_tool_id(tool: &str) -> bool {
    !tool.is_empty()
        && tool.len() <= 128
        && tool
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// The tools the owner has given a posture, as `tool -> ask | trusted | off`. A tool with no entry has the default.
///
/// # Errors
///
/// Returns a message when there is no configuration yet.
pub fn tool_postures(
    paths: &AppPaths,
) -> Result<std::collections::BTreeMap<String, &'static str>, String> {
    let table = read_table(&ConfigStore::from_paths(paths))?;
    let mut out = std::collections::BTreeMap::new();
    let policy = table.get("policy").and_then(Value::as_table);
    for (field, name) in [("trust", "trusted"), ("deny", "off")] {
        for item in policy
            .and_then(|policy| policy.get(field))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(tool) = item.as_str() {
                out.insert(tool.to_owned(), name);
            }
        }
    }
    for tool in policy
        .and_then(|policy| policy.get("approval"))
        .and_then(Value::as_table)
        .into_iter()
        .flat_map(|map| map.keys())
    {
        out.insert(tool.clone(), "ask");
    }
    Ok(out)
}

/// Sets what the owner has decided about one tool. A tool has exactly one posture, so choosing one removes the others.
///
/// # Errors
///
/// Returns a message for an unusable tool id or when the result would be an invalid configuration.
pub fn set_tool_posture(paths: &AppPaths, tool: &str, posture: Posture) -> Result<(), String> {
    if !valid_tool_id(tool) {
        return Err("a tool id is letters, digits, dots, dashes and underscores".to_owned());
    }
    edit(paths, |table| {
        let policy = table_mut(table, "policy")?;
        for field in ["trust", "deny"] {
            if let Some(items) = policy.get_mut(field).and_then(Value::as_array_mut) {
                items.retain(|item| item.as_str() != Some(tool));
            }
        }
        if let Some(map) = policy.get_mut("approval").and_then(Value::as_table_mut) {
            map.remove(tool);
        }
        match posture {
            Posture::Default => {}
            Posture::Ask => {
                table_mut(policy, "approval")?
                    .insert(tool.to_owned(), Value::String("ask".to_owned()));
            }
            Posture::Trusted | Posture::Off => {
                let field = if posture == Posture::Trusted {
                    "trust"
                } else {
                    "deny"
                };
                let list = policy
                    .entry(field.to_owned())
                    .or_insert_with(|| Value::Array(Vec::new()));
                list.as_array_mut()
                    .ok_or_else(|| format!("policy.{field} in the configuration is not a list"))?
                    .push(Value::String(tool.to_owned()));
            }
        }
        // Empty lists and tables are dropped so the file stays as small as it was before the change.
        policy.retain(|_, value| match value {
            Value::Array(items) => !items.is_empty(),
            Value::Table(map) => !map.is_empty(),
            _ => true,
        });
        Ok(())
    })
}

/// Whether a tool is listed as trusted in a stored document (used by tests).
#[cfg(test)]
fn is_trusted(paths: &AppPaths, tool: &str) -> bool {
    tool_postures(paths).is_ok_and(|postures| postures.get(tool) == Some(&"trusted"))
}

/// Which key a keys operation means.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SecretKind {
    /// The model provider's key.
    Model,
    /// The speech provider's key.
    Voice,
}

impl SecretKind {
    /// Parses `model` or `voice`.
    ///
    /// # Errors
    ///
    /// Returns a message for anything else.
    pub fn parse(word: &str) -> Result<Self, String> {
        match word {
            "model" => Ok(Self::Model),
            "voice" => Ok(Self::Voice),
            _ => Err("name the key: model or voice".to_owned()),
        }
    }

    const fn reference(self) -> &'static str {
        match self {
            Self::Model => "executor_api_key_ref",
            Self::Voice => "speech_api_key_ref",
        }
    }

    const fn file_name(self) -> &'static str {
        match self {
            Self::Model => "model.key",
            Self::Voice => "speech.key",
        }
    }

    /// The environment variable the CLI suggests for this key.
    #[must_use]
    pub const fn default_env(self) -> &'static str {
        match self {
            Self::Model => "JARVIS_MODEL_API_KEY",
            Self::Voice => "ELEVENLABS_API_KEY",
        }
    }
}

/// Whether a key is configured, without reading it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeyState {
    /// `not set`, `set`, `EMPTY FILE` or `FILE MISSING`.
    pub state: &'static str,
    /// The key file's path, if configured.
    pub path: Option<String>,
}

/// The state of one key. Only the file's existence and size are looked at, never its contents.
///
/// # Errors
///
/// Returns a message when there is no configuration yet.
pub fn key_state(paths: &AppPaths, kind: SecretKind) -> Result<KeyState, String> {
    let table = read_table(&ConfigStore::from_paths(paths))?;
    let path = SETTINGS
        .iter()
        .find(|setting| setting.field == kind.reference())
        .and_then(|setting| lookup(&table, setting))
        .map(show_value);
    let state = match &path {
        None => "not set",
        Some(path) => match std::fs::metadata(path) {
            Ok(meta) if meta.len() > 0 => "set",
            Ok(_) => "EMPTY FILE",
            Err(_) => "FILE MISSING",
        },
    };
    Ok(KeyState { state, path })
}

/// Checks that text is usable as a key and returns it trimmed. The refusal never repeats the text.
///
/// # Errors
///
/// Returns a message for an empty, oversized or space-containing key.
pub fn validate_secret(raw: &str) -> Result<String, String> {
    let key = raw.trim().to_owned();
    if key.is_empty() || key.len() > 256 || !key.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err("that is not a usable key (empty, too long, or containing spaces)".to_owned());
    }
    Ok(key)
}

/// Writes a key to a private file in the configuration directory and points the configuration at it.
///
/// # Errors
///
/// Returns a message when the key is unusable, the file cannot be written, or the configuration would become invalid.
pub fn set_secret(paths: &AppPaths, kind: SecretKind, raw: &str) -> Result<PathBuf, String> {
    let key = validate_secret(raw)?;
    let path = paths.config().join(kind.file_name());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("the key file could not be written: {error}"))?;
    }
    std::fs::write(&path, key)
        .map_err(|error| format!("the key file could not be written: {error}"))?;
    secure_private_file(PathKind::Config, &path)
        .map_err(|error| format!("the key file could not be made private: {error}"))?;
    let reference = path.display().to_string();
    set(paths, kind.reference(), &[reference])?;
    Ok(path)
}

/// Removes the voice key and the settings that depend on it. The model key cannot be removed, only replaced.
///
/// # Errors
///
/// Returns a message for the model key, or when the configuration would become invalid.
pub fn remove_secret(paths: &AppPaths, kind: SecretKind) -> Result<(), String> {
    if kind == SecretKind::Model {
        return Err(
            "the model key cannot be removed, only replaced (the assistant needs a model)"
                .to_owned(),
        );
    }
    edit(paths, |table| {
        let daemon = table_mut(table, "daemon")?;
        for field in ["speech_api_key_ref", "speech_voice_id", "speech_model"] {
            daemon.remove(field);
        }
        Ok(())
    })?;
    let _ = std::fs::remove_file(paths.config().join(kind.file_name()));
    Ok(())
}

#[cfg(test)]
mod tests;
