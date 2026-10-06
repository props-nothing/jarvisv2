//! `jarvis config` and `jarvis keys`: change one setting or one key without redoing setup (`P9-014`).
//!
//! Every change is made on the stored document, checked by the **same parser the daemon starts with**, and saved the same
//! way `init` saves it, so a setting cannot be valid here and refused at startup. A key is written to a private file and
//! referenced by path: it is never in the configuration, never printed, never an argument, and never sent anywhere but its
//! provider (`keys test`).

use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use jarvis_storage::{AppPaths, Config, ConfigStore};
use toml::{Table, Value};

use crate::init::{flag_value, strip_verbatim, write_secret};
use crate::output::ExitStatus;

/// How a setting's value is read from the command line.
#[derive(Clone, Copy)]
enum Kind {
    Text,
    File,
    Folders,
    Words,
    Port,
    ToolIds,
}

struct Setting {
    table: &'static str,
    field: &'static str,
    kind: Kind,
    help: &'static str,
}

const SETTINGS: &[Setting] = &[
    Setting {
        table: "daemon",
        field: "executor_model_name",
        kind: Kind::Text,
        help: "the model to talk to",
    },
    Setting {
        table: "daemon",
        field: "executor_base_url",
        kind: Kind::Text,
        help: "the model server address, with its /v1",
    },
    Setting {
        table: "daemon",
        field: "executor_api_key_ref",
        kind: Kind::File,
        help: "file holding the model key (see `jarvis keys set model`)",
    },
    Setting {
        table: "daemon",
        field: "tool_workspace_roots",
        kind: Kind::Folders,
        help: "folders the assistant may read and write (replaces the list)",
    },
    Setting {
        table: "daemon",
        field: "code_sandbox_image",
        kind: Kind::Text,
        help: "container image the code tool runs in",
    },
    Setting {
        table: "daemon",
        field: "code_sandbox_interpreter",
        kind: Kind::Words,
        help: "command inside it, for example: node -e",
    },
    Setting {
        table: "daemon",
        field: "speech_api_key_ref",
        kind: Kind::File,
        help: "file holding the voice key (see `jarvis keys set voice`)",
    },
    Setting {
        table: "daemon",
        field: "speech_voice_id",
        kind: Kind::Text,
        help: "ElevenLabs voice id",
    },
    Setting {
        table: "daemon",
        field: "speech_model",
        kind: Kind::Text,
        help: "ElevenLabs model id",
    },
    Setting {
        table: "daemon",
        field: "http_port",
        kind: Kind::Port,
        help: "the local port",
    },
    Setting {
        table: "daemon",
        field: "mcp_serve_port",
        kind: Kind::Port,
        help: "serve JARVIS tools over MCP on this port",
    },
    Setting {
        table: "policy",
        field: "trust",
        kind: Kind::ToolIds,
        help: "tools allowed to run without asking",
    },
];

/// Finds a setting by `table.field` or, when unambiguous, by its field name alone.
fn find(key: &str) -> Option<&'static Setting> {
    SETTINGS.iter().find(|setting| {
        format!("{}.{}", setting.table, setting.field) == key || setting.field == key
    })
}

fn supported() -> String {
    SETTINGS
        .iter()
        .map(|setting| format!("  {}.{:<26} {}", setting.table, setting.field, setting.help))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The words after the verb, without flags and the values flags take.
fn positionals(arguments: &[String]) -> Vec<String> {
    const WITH_VALUE: [&str; 3] = ["--root", "--from-env", "--file"];
    let mut out = Vec::new();
    let mut skip = false;
    for argument in arguments {
        if skip {
            skip = false;
        } else if WITH_VALUE.contains(&argument.as_str()) {
            skip = true;
        } else if !argument.starts_with("--") {
            out.push(argument.clone());
        }
    }
    out
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

/// Reads the stored document, lets `change` edit it, checks the result with the daemon's own parser and saves it.
fn edit(
    paths: &AppPaths,
    change: impl FnOnce(&mut Table) -> Result<(), String>,
) -> Result<(), String> {
    let store = ConfigStore::from_paths(paths);
    let text = std::fs::read_to_string(store.path())
        .map_err(|_| "there is no configuration yet; run `jarvis init`".to_owned())?;
    let mut table: Table = toml::from_str(&text)
        .map_err(|_| "the stored configuration is not valid TOML".to_owned())?;
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

fn set_value(paths: &AppPaths, setting: &Setting, value: Value) -> Result<(), String> {
    edit(paths, |table| {
        table_mut(table, setting.table)?.insert(setting.field.to_owned(), value);
        Ok(())
    })
}

fn current(paths: &AppPaths) -> Result<Table, String> {
    let store = ConfigStore::from_paths(paths);
    let text = std::fs::read_to_string(store.path())
        .map_err(|_| "there is no configuration yet; run `jarvis init`".to_owned())?;
    toml::from_str(&text).map_err(|_| "the stored configuration is not valid TOML".to_owned())
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

/// A note beside a key file path: whether the file is really there. The key itself is never read here.
fn file_note(setting: &Setting, value: &str) -> &'static str {
    if matches!(setting.kind, Kind::File) {
        if Path::new(value).is_file() {
            "  (file present)"
        } else {
            "  (FILE MISSING)"
        }
    } else {
        ""
    }
}

fn apply_note(paths: &AppPaths) {
    if let Ok(loaded) = ConfigStore::from_paths(paths).load() {
        let port = loaded.config().daemon().http_port();
        let address = std::net::SocketAddr::from(([127, 0, 0, 1], port));
        if std::net::TcpStream::connect_timeout(&address, Duration::from_millis(300)).is_ok() {
            println!("saved. It is running, so apply it with: jarvis restart");
            return;
        }
    }
    println!("saved.");
}

/// `jarvis config show|get|set|unset`
pub fn config(paths: &AppPaths, arguments: &[String]) -> ExitStatus {
    let words = positionals(arguments);
    let json = arguments.iter().any(|argument| argument == "--json");
    let rest = words.get(2..).unwrap_or(&[]);
    let outcome = match words.get(1).map(String::as_str) {
        Some("show") | None => show(paths, json),
        Some("get") => rest.first().map_or_else(
            || Err("usage: jarvis config get KEY".to_owned()),
            |key| get(paths, key),
        ),
        Some("set") => match rest.split_first() {
            Some((key, values)) if !values.is_empty() => set(paths, key, values),
            _ => Err("usage: jarvis config set KEY VALUE...".to_owned()),
        },
        Some("unset") => rest.first().map_or_else(
            || Err("usage: jarvis config unset KEY".to_owned()),
            |key| unset(paths, key),
        ),
        Some(other) => Err(format!(
            "unknown config command {other:?}; use show, get, set or unset"
        )),
    };
    match outcome {
        Ok(()) => ExitStatus::Ok,
        Err(message) => {
            eprintln!("jarvis: {message}");
            ExitStatus::Usage
        }
    }
}

fn unknown(key: &str) -> String {
    format!(
        "{key:?} is not a setting that can be changed here. The settings are:\n{}",
        supported()
    )
}

fn show(paths: &AppPaths, json: bool) -> Result<(), String> {
    let table = current(paths)?;
    if json {
        let mut out = serde_json::Map::new();
        for setting in SETTINGS {
            if let Some(value) = lookup(&table, setting) {
                out.insert(
                    format!("{}.{}", setting.table, setting.field),
                    serde_json::json!(show_value(value)),
                );
            }
        }
        println!("{}", serde_json::Value::Object(out));
        return Ok(());
    }
    for setting in SETTINGS {
        let key = format!("{}.{}", setting.table, setting.field);
        match lookup(&table, setting) {
            Some(value) => {
                let text = show_value(value);
                println!("{key:<34} {text}{}", file_note(setting, &text));
            }
            None => println!("{key:<34} (not set)"),
        }
    }
    Ok(())
}

fn get(paths: &AppPaths, key: &str) -> Result<(), String> {
    let setting = find(key).ok_or_else(|| unknown(key))?;
    let table = current(paths)?;
    let value = lookup(&table, setting).ok_or_else(|| format!("{key} is not set"))?;
    println!("{}", show_value(value));
    Ok(())
}

fn set(paths: &AppPaths, key: &str, values: &[String]) -> Result<(), String> {
    let setting = find(key).ok_or_else(|| unknown(key))?;
    let value = convert(setting.kind, values)?;
    set_value(paths, setting, value)?;
    apply_note(paths);
    Ok(())
}

fn unset(paths: &AppPaths, key: &str) -> Result<(), String> {
    let setting = find(key).ok_or_else(|| unknown(key))?;
    edit(paths, |table| {
        table_mut(table, setting.table)?.remove(setting.field);
        Ok(())
    })?;
    apply_note(paths);
    Ok(())
}

// ---- keys ---------------------------------------------------------------------------------------------------------

/// Which key a `keys` command means.
#[derive(Clone, Copy)]
enum Which {
    Model,
    Voice,
}

impl Which {
    fn parse(word: Option<&String>) -> Result<Self, String> {
        match word.map(String::as_str) {
            Some("model") => Ok(Self::Model),
            Some("voice") => Ok(Self::Voice),
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
    const fn default_env(self) -> &'static str {
        match self {
            Self::Model => "JARVIS_MODEL_API_KEY",
            Self::Voice => "ELEVENLABS_API_KEY",
        }
    }
}

/// Reads a key from an environment variable, a file, or standard input. It is never a command-line value, which would be
/// kept in shell history and visible to other processes.
fn read_secret(arguments: &[String], which: Which) -> Result<String, String> {
    let raw = if let Some(name) = flag_value(arguments, "--from-env") {
        std::env::var(&name).map_err(|_| format!("the environment variable {name} is not set"))?
    } else if let Some(path) = flag_value(arguments, "--file") {
        std::fs::read_to_string(&path).map_err(|_| format!("{path} could not be read"))?
    } else if std::io::stdin().is_terminal() {
        eprint!(
            "Paste the key and press Enter (it is shown as you type; use --from-env {} or --file to avoid that): ",
            which.default_env()
        );
        let _ = std::io::stderr().flush();
        let mut line = String::new();
        std::io::stdin()
            .read_line(&mut line)
            .map_err(|error| error.to_string())?;
        line
    } else {
        let mut all = String::new();
        std::io::stdin()
            .take(1024)
            .read_to_string(&mut all)
            .map_err(|error| error.to_string())?;
        all
    };
    let key = raw.trim().to_owned();
    if key.is_empty() || key.len() > 256 || !key.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err("that is not a usable key (empty, too long, or containing spaces)".to_owned());
    }
    Ok(key)
}

/// `jarvis keys status|set|remove|test`
pub async fn keys(
    paths: &AppPaths,
    arguments: &[String],
    credential: Option<String>,
) -> ExitStatus {
    let words = positionals(arguments);
    let outcome = match words.get(1).map(String::as_str) {
        Some("status") | None => {
            status(paths, arguments.iter().any(|argument| argument == "--json"))
        }
        Some("set") => {
            Which::parse(words.get(2)).and_then(|which| set_key(paths, arguments, which))
        }
        Some("remove") => Which::parse(words.get(2)).and_then(|which| remove_key(paths, which)),
        Some("test") => match Which::parse(words.get(2)) {
            Ok(which) => test_key(paths, which, credential).await,
            Err(message) => Err(message),
        },
        Some(other) => Err(format!(
            "unknown keys command {other:?}; use status, set, remove or test"
        )),
    };
    match outcome {
        Ok(()) => ExitStatus::Ok,
        Err(message) => {
            eprintln!("jarvis: {message}");
            ExitStatus::Usage
        }
    }
}

fn key_state(table: &Table, which: Which) -> (&'static str, Option<String>) {
    let reference = SETTINGS
        .iter()
        .find(|setting| setting.field == which.reference())
        .and_then(|setting| lookup(table, setting))
        .map(show_value);
    let state = match &reference {
        None => "not set",
        Some(path) => match std::fs::metadata(path) {
            Ok(meta) if meta.len() > 0 => "set",
            Ok(_) => "EMPTY FILE",
            Err(_) => "FILE MISSING",
        },
    };
    (state, reference)
}

fn status(paths: &AppPaths, json: bool) -> Result<(), String> {
    let table = current(paths)?;
    let rows = [("model", Which::Model), ("voice", Which::Voice)];
    if json {
        let out: serde_json::Map<String, serde_json::Value> = rows
            .iter()
            .map(|(name, which)| {
                (
                    (*name).to_owned(),
                    serde_json::json!(key_state(&table, *which).0),
                )
            })
            .collect();
        println!("{}", serde_json::Value::Object(out));
        return Ok(());
    }
    for (name, which) in rows {
        let (state, reference) = key_state(&table, which);
        println!("{name:<6} {state:<13} {}", reference.unwrap_or_default());
    }
    Ok(())
}

fn set_key(paths: &AppPaths, arguments: &[String], which: Which) -> Result<(), String> {
    let key = read_secret(arguments, which)?;
    let path = paths.config().join(which.file_name());
    write_secret(&path, &key)
        .map_err(|error| format!("the key file could not be written: {error}"))?;
    let setting = SETTINGS
        .iter()
        .find(|setting| setting.field == which.reference())
        .ok_or_else(|| "internal: unknown key setting".to_owned())?;
    set_value(paths, setting, Value::String(path.display().to_string()))?;
    println!(
        "key written to {} and set; its value is not shown or stored anywhere else",
        path.display()
    );
    apply_note(paths);
    Ok(())
}

fn remove_key(paths: &AppPaths, which: Which) -> Result<(), String> {
    if matches!(which, Which::Model) {
        return Err("the model key cannot be removed, only replaced (the assistant needs a model); use `keys set model`".to_owned());
    }
    edit(paths, |table| {
        let daemon = table_mut(table, "daemon")?;
        for field in ["speech_api_key_ref", "speech_voice_id", "speech_model"] {
            daemon.remove(field);
        }
        Ok(())
    })?;
    let _ = std::fs::remove_file(paths.config().join(which.file_name()));
    apply_note(paths);
    Ok(())
}

async fn test_key(
    paths: &AppPaths,
    which: Which,
    credential: Option<String>,
) -> Result<(), String> {
    let table = current(paths)?;
    match which {
        Which::Model => test_model(&table).await,
        Which::Voice => test_voice(paths, credential).await,
    }
}

async fn test_model(table: &Table) -> Result<(), String> {
    let field = |name: &str| {
        SETTINGS
            .iter()
            .find(|setting| setting.field == name)
            .and_then(|setting| lookup(table, setting))
            .map(show_value)
    };
    let (Some(base), Some(file)) = (field("executor_base_url"), field("executor_api_key_ref"))
    else {
        return Err("no model is configured".to_owned());
    };
    let key = std::fs::read_to_string(&file)
        .map_err(|_| "the model key file could not be read".to_owned())?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|error| error.to_string())?;
    let response = client
        .get(format!("{}/models", base.trim_end_matches('/')))
        .bearer_auth(key.trim())
        .send()
        .await
        .map_err(|_| format!("could not reach {base}"))?;
    match response.status().as_u16() {
        200..=299 => {
            println!("model server answered and accepted the key");
            Ok(())
        }
        401 | 403 => Err("the model server rejected the key".to_owned()),
        other => Err(format!("the model server answered with status {other}")),
    }
}

async fn test_voice(paths: &AppPaths, credential: Option<String>) -> Result<(), String> {
    let loaded = ConfigStore::from_paths(paths)
        .load()
        .map_err(|error| error.to_string())?;
    let port = loaded.config().daemon().http_port();
    let credential = credential.ok_or_else(|| {
        "the daemon has not run yet, so there is no credential; run `jarvis start`".to_owned()
    })?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| error.to_string())?;
    let response = client
        .post(format!("http://127.0.0.1:{port}/api/v1/speech"))
        .bearer_auth(credential)
        .json(&serde_json::json!({ "text": "Systems online." }))
        .send()
        .await
        .map_err(|_| "the daemon is not running; start it with `jarvis start`".to_owned())?;
    match response.status().as_u16() {
        200 => {
            println!("the voice provider accepted the key and produced audio");
            Ok(())
        }
        404 => Err(
            "the running daemon has no voice configured yet; apply it with: jarvis restart"
                .to_owned(),
        ),
        other => Err(format!(
            "the voice test failed with status {other} (see `jarvis logs`)"
        )),
    }
}

#[cfg(test)]
mod tests;
