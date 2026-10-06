//! `jarvis config` and `jarvis keys`: change one setting or one key without redoing setup (`P9-014`).
//!
//! The logic lives in `jarvis_storage::settings`, shared with the console's Settings screen, so a change cannot be valid in
//! one surface and refused in another. This file only reads the command line and prints. A key is read from an environment
//! variable, a file or standard input, never from an argument (it would stay in shell history).

use std::io::{IsTerminal, Read, Write};
use std::time::Duration;

use jarvis_storage::settings::{self, SecretKind};
use jarvis_storage::{AppPaths, ConfigStore};

use crate::init::flag_value;
use crate::output::ExitStatus;

/// The words after the command name, without flags and the values flags take.
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

fn finish(outcome: Result<(), String>) -> ExitStatus {
    match outcome {
        Ok(()) => ExitStatus::Ok,
        Err(message) => {
            eprintln!("jarvis: {message}");
            ExitStatus::Usage
        }
    }
}

/// `jarvis config show|get|set|unset`
pub fn config(paths: &AppPaths, arguments: &[String]) -> ExitStatus {
    let words = positionals(arguments);
    let json = arguments.iter().any(|argument| argument == "--json");
    let rest = words.get(2..).unwrap_or(&[]);
    finish(match words.get(1).map(String::as_str) {
        Some("show") | None => show(paths, json),
        Some("get") => rest.first().map_or_else(
            || Err("usage: jarvis config get KEY".to_owned()),
            |key| settings::get(paths, key).map(|value| println!("{value}")),
        ),
        Some("set") => match rest.split_first() {
            Some((key, values)) if key == "code_sandbox" && values.len() >= 2 => {
                set_sandbox(paths, Some(values))
            }
            Some((key, values)) if !values.is_empty() => {
                settings::set(paths, key, values).map(|()| apply_note(paths))
            }
            _ => Err("usage: jarvis config set KEY VALUE...   (code sandbox: jarvis config set code_sandbox IMAGE COMMAND...)".to_owned()),
        },
        Some("unset") => match rest.first().map(String::as_str) {
            Some("code_sandbox") => set_sandbox(paths, None),
            Some(key) => settings::unset(paths, key).map(|()| apply_note(paths)),
            None => Err("usage: jarvis config unset KEY".to_owned()),
        },
        Some(other) => Err(format!(
            "unknown config command {other:?}; use show, get, set or unset"
        )),
    })
}

/// The code sandbox is an image and the command that runs a snippet in it; one without the other is refused, so they are set
/// and removed together: `jarvis config set code_sandbox node:22-alpine node -e`, `jarvis config unset code_sandbox`.
fn set_sandbox(paths: &AppPaths, values: Option<&[String]>) -> Result<(), String> {
    let changes = [
        settings::Change {
            key: "code_sandbox_image".to_owned(),
            values: values.map(|v| vec![v[0].clone()]),
        },
        settings::Change {
            key: "code_sandbox_interpreter".to_owned(),
            values: values.map(|v| v[1..].to_vec()),
        },
    ];
    settings::apply(paths, &changes).map(|()| apply_note(paths))
}

fn show(paths: &AppPaths, json: bool) -> Result<(), String> {
    let views = settings::list(paths)?;
    if json {
        let out: serde_json::Map<String, serde_json::Value> = views
            .into_iter()
            .filter_map(|view| view.value.map(|value| (view.key, serde_json::json!(value))))
            .collect();
        println!("{}", serde_json::Value::Object(out));
        return Ok(());
    }
    for view in views {
        let note = match view.file_state {
            Some("present") => "  (file present)",
            Some(_) => "  (FILE MISSING)",
            None => "",
        };
        match view.value {
            Some(value) => println!("{:<34} {value}{note}", view.key),
            None => println!("{:<34} (not set)", view.key),
        }
    }
    Ok(())
}

fn kind_of(word: Option<&String>) -> Result<SecretKind, String> {
    word.map_or_else(
        || Err("name the key: model or voice".to_owned()),
        |word| SecretKind::parse(word),
    )
}

/// Reads a key from an environment variable, a file, or standard input. It is never a command-line value, which would be
/// kept in shell history and visible to other processes.
fn read_secret(arguments: &[String], kind: SecretKind) -> Result<String, String> {
    let raw = if let Some(name) = flag_value(arguments, "--from-env") {
        std::env::var(&name).map_err(|_| format!("the environment variable {name} is not set"))?
    } else if let Some(path) = flag_value(arguments, "--file") {
        std::fs::read_to_string(&path).map_err(|_| format!("{path} could not be read"))?
    } else if std::io::stdin().is_terminal() {
        eprint!(
            "Paste the key and press Enter (it is shown as you type; use --from-env {} or --file to avoid that): ",
            kind.default_env()
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
    settings::validate_secret(&raw)
}

/// `jarvis keys status|set|remove|test`
pub async fn keys(
    paths: &AppPaths,
    arguments: &[String],
    credential: Option<String>,
) -> ExitStatus {
    let words = positionals(arguments);
    finish(match words.get(1).map(String::as_str) {
        Some("status") | None => {
            status(paths, arguments.iter().any(|argument| argument == "--json"))
        }
        Some("set") => kind_of(words.get(2)).and_then(|kind| {
            let key = read_secret(arguments, kind)?;
            let path = settings::set_secret(paths, kind, &key)?;
            println!(
                "key written to {} and set; its value is not shown or stored anywhere else",
                path.display()
            );
            apply_note(paths);
            Ok(())
        }),
        Some("remove") => kind_of(words.get(2))
            .and_then(|kind| settings::remove_secret(paths, kind).map(|()| apply_note(paths))),
        Some("test") => match kind_of(words.get(2)) {
            Ok(SecretKind::Model) => test_model(paths).await,
            Ok(SecretKind::Voice) => test_voice(paths, credential).await,
            Err(message) => Err(message),
        },
        Some(other) => Err(format!(
            "unknown keys command {other:?}; use status, set, remove or test"
        )),
    })
}

fn status(paths: &AppPaths, json: bool) -> Result<(), String> {
    let model = settings::key_state(paths, SecretKind::Model)?;
    let voice = settings::key_state(paths, SecretKind::Voice)?;
    if json {
        println!(
            "{}",
            serde_json::json!({ "model": model.state, "voice": voice.state })
        );
        return Ok(());
    }
    for (name, state) in [("model", model), ("voice", voice)] {
        println!(
            "{name:<6} {:<13} {}",
            state.state,
            state.path.unwrap_or_default()
        );
    }
    Ok(())
}

async fn test_model(paths: &AppPaths) -> Result<(), String> {
    let base = settings::get(paths, "executor_base_url")
        .map_err(|_| "no model is configured".to_owned())?;
    let file = settings::get(paths, "executor_api_key_ref")
        .map_err(|_| "no model is configured".to_owned())?;
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
