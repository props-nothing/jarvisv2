//! `jarvis doctor --live`: the checks that need the outside world, each with the command that fixes it (`P9-016`).
//!
//! The offline `doctor` says whether the installation is sound. This one asks whether it will actually work: does the model
//! server answer and accept the key, do the granted folders still exist, is Docker there for the code tool, is the daemon up
//! and does it have the voice. A key is only ever sent to the provider it belongs to and is never printed.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use jarvis_storage::settings;
use jarvis_storage::{AppPaths, ConfigStore};

/// How bad a finding is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Level {
    Ok,
    Warn,
    Fail,
}

impl Level {
    /// The word printed for it.
    pub const fn word(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warn => "WARN",
            Self::Fail => "FAIL",
        }
    }
}

/// One finding: what was checked, how it went, and what to run if it did not go well.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Finding {
    pub name: &'static str,
    pub level: Level,
    pub detail: String,
    pub fix: Option<String>,
}

fn finding(
    name: &'static str,
    level: Level,
    detail: impl Into<String>,
    fix: Option<&str>,
) -> Finding {
    Finding {
        name,
        level,
        detail: detail.into(),
        fix: fix.map(str::to_owned),
    }
}

/// Whether a model list (`{"data":[{"id":..}]}` or Ollama's `{"models":[{"name":..}]}`) names `model`.
pub fn lists_model(listing: &serde_json::Value, model: &str) -> bool {
    let named = |key: &str, field: &str| {
        listing
            .get(key)
            .and_then(serde_json::Value::as_array)
            .is_some_and(|items| {
                items
                    .iter()
                    .any(|item| item.get(field).and_then(serde_json::Value::as_str) == Some(model))
            })
    };
    named("data", "id") || named("models", "name") || named("models", "model")
}

/// What the status of a model-list request means.
pub fn classify_models_status(status: u16) -> (Level, &'static str, Option<&'static str>) {
    match status {
        200..=299 => (Level::Ok, "the model server answered", None),
        401 | 403 => (
            Level::Fail,
            "the model server rejected the key",
            Some("jarvis keys set model"),
        ),
        404 => (
            Level::Warn,
            "the model server has no model list at that address (it may still work)",
            Some("jarvis config get executor_base_url"),
        ),
        _ => (
            Level::Fail,
            "the model server answered with an error",
            Some("jarvis logs --lines 50"),
        ),
    }
}

async fn model(paths: &AppPaths) -> Finding {
    let (Ok(base), Ok(file), Ok(name)) = (
        settings::get(paths, "executor_base_url"),
        settings::get(paths, "executor_api_key_ref"),
        settings::get(paths, "executor_model_name"),
    ) else {
        return finding(
            "model",
            Level::Fail,
            "no model is configured",
            Some("jarvis init"),
        );
    };
    let Ok(key) = std::fs::read_to_string(&file) else {
        return finding(
            "model",
            Level::Fail,
            "the model key file cannot be read",
            Some("jarvis keys set model"),
        );
    };
    let Ok(client) = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
    else {
        return finding("model", Level::Fail, "no HTTP client could be built", None);
    };
    let Ok(response) = client
        .get(format!("{}/models", base.trim_end_matches('/')))
        .bearer_auth(key.trim())
        .send()
        .await
    else {
        return finding(
            "model",
            Level::Fail,
            format!("could not reach {base}"),
            Some(
                "start the model server (for Ollama: `ollama serve`), or `jarvis config set executor_base_url URL`",
            ),
        );
    };
    let (level, detail, fix) = classify_models_status(response.status().as_u16());
    if level != Level::Ok {
        return finding("model", level, detail, fix);
    }
    let listed = response
        .json::<serde_json::Value>()
        .await
        .is_ok_and(|listing| lists_model(&listing, &name));
    if listed {
        finding(
            "model",
            Level::Ok,
            format!("{base} answered and lists {name}"),
            None,
        )
    } else {
        finding(
            "model",
            Level::Warn,
            format!("{base} answered but does not list {name}"),
            Some("pull or name an available model: `jarvis config set executor_model_name NAME`"),
        )
    }
}

fn folders(paths: &AppPaths) -> Finding {
    let Ok(roots) = settings::get(paths, "tool_workspace_roots") else {
        return finding(
            "folders",
            Level::Ok,
            "none granted, so JARVIS has no file tools (by design)",
            Some("jarvis config set tool_workspace_roots FOLDER"),
        );
    };
    let missing: Vec<&str> = roots
        .split(", ")
        .filter(|root| !Path::new(root).is_dir())
        .collect();
    if missing.is_empty() {
        finding("folders", Level::Ok, format!("{roots} exist"), None)
    } else {
        finding(
            "folders",
            Level::Fail,
            format!("missing: {}", missing.join(", ")),
            Some("jarvis config set tool_workspace_roots FOLDER"),
        )
    }
}

fn run_quiet(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn docker(paths: &AppPaths) -> Option<Finding> {
    let image = settings::get(paths, "code_sandbox_image").ok()?;
    let Some(version) = run_quiet("docker", &["version", "--format", "{{.Server.Version}}"]) else {
        return Some(finding(
            "docker",
            Level::Fail,
            "the code tool is on but Docker is not running",
            Some("start Docker, or turn the code tool off in Settings"),
        ));
    };
    if run_quiet("docker", &["image", "inspect", &image, "--format", "ok"]).is_some() {
        Some(finding(
            "docker",
            Level::Ok,
            format!("Docker {version}; image {image} is present"),
            None,
        ))
    } else {
        let fix = format!("docker pull {image}");
        Some(finding(
            "docker",
            Level::Warn,
            format!("Docker {version} is running but {image} is not pulled (it never is, for you)"),
            Some(&fix),
        ))
    }
}

async fn daemon(paths: &AppPaths, credential: Option<&str>) -> Vec<Finding> {
    let Ok(loaded) = ConfigStore::from_paths(paths).load() else {
        return vec![finding(
            "daemon",
            Level::Fail,
            "the configuration cannot be read",
            Some("jarvis doctor"),
        )];
    };
    let port = loaded.config().daemon().http_port();
    let address = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    if std::net::TcpStream::connect_timeout(&address, Duration::from_millis(400)).is_err() {
        return vec![finding(
            "daemon",
            Level::Warn,
            "it is not running",
            Some("jarvis start"),
        )];
    }
    let mut out = vec![finding(
        "daemon",
        Level::Ok,
        format!("running on 127.0.0.1:{port}"),
        None,
    )];
    let voice_set = settings::key_state(paths, settings::SecretKind::Voice)
        .is_ok_and(|state| state.state == "set");
    if voice_set && let Some(credential) = credential {
        let reply = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(5))
            .build()
            .ok();
        let enabled = match reply {
            Some(client) => client
                .get(format!("http://127.0.0.1:{port}/api/v1/speech"))
                .bearer_auth(credential)
                .send()
                .await
                .ok(),
            None => None,
        };
        let on = match enabled {
            Some(response) => response
                .json::<serde_json::Value>()
                .await
                .ok()
                .and_then(|body| body.get("enabled").and_then(serde_json::Value::as_bool)),
            None => None,
        };
        out.push(match on {
            Some(true) => finding(
                "voice",
                Level::Ok,
                "the running daemon has the neural voice",
                None,
            ),
            _ => finding(
                "voice",
                Level::Warn,
                "a voice key is saved but the running daemon does not have it yet",
                Some("jarvis restart"),
            ),
        });
    }
    out
}

/// Variables in the environment with the `JARVIS_` prefix that JARVIS does not use. They are ignored (another tool may own them),
/// but a mistyped override would otherwise vanish without a word, so they are named here, never with their values.
fn environment() -> Option<Finding> {
    let names = jarvis_storage::unrecognized_environment_keys(std::env::vars_os());
    if names.is_empty() {
        return None;
    }
    Some(finding(
        "environment",
        Level::Warn,
        format!(
            "{} set but not used by JARVIS, so ignored (another program may own {}): if you meant an override, check the spelling",
            names.join(", "),
            if names.len() == 1 { "it" } else { "them" }
        ),
        Some("see docs/development/getting-started.md for the variables JARVIS reads"),
    ))
}

/// Runs every live check.
pub async fn run(paths: &AppPaths, credential: Option<&str>) -> Vec<Finding> {
    let mut out = vec![model(paths).await, folders(paths)];
    out.extend(environment());
    out.extend(docker(paths));
    out.extend(daemon(paths, credential).await);
    out
}

#[cfg(test)]
mod tests;
