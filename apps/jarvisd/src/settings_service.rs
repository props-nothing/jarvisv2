//! The Settings screen's API (`P9-015`): read the settings, change one, set or remove a key, and restart to apply.
//!
//! All of it is `jarvis_storage::settings`, the same code `jarvis config` and `jarvis keys` use, so a change cannot be valid
//! on the screen and refused at startup. Every route sits behind the local credential. **No response contains a key**: a
//! key is accepted write-only and only its state (`set`, `not set`, `FILE MISSING`, `EMPTY FILE`) is ever reported.

use std::path::{Path, PathBuf};

use axum::{
    Json,
    extract::{Path as UrlPath, RawQuery, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use jarvis_core::ErrorCode;
use jarvis_storage::settings::{self, SecretKind};
use jarvis_storage::{AppPaths, ConfigStore};
use serde::Deserialize;
use serde_json::json;

use crate::gateway::{GatewayState, error_response};

/// Where this daemon's profile is, held so the routes read and write the same configuration the daemon started from.
#[derive(Clone)]
pub struct SettingsContext {
    paths: AppPaths,
    root: Option<PathBuf>,
}

impl SettingsContext {
    /// The profile directories the settings are read from.
    #[must_use]
    pub const fn paths(&self) -> &AppPaths {
        &self.paths
    }

    /// Builds the context from the daemon's resolved paths and its `--root`, if any.
    #[must_use]
    pub fn new(paths: AppPaths, root: Option<PathBuf>) -> Self {
        Self { paths, root }
    }
}

/// The body of `PUT /api/v1/settings/{key}`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetBody {
    values: Vec<String>,
}

/// One change in `PUT /api/v1/settings`: set to these values, or remove (`values: null`).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeBody {
    key: String,
    #[serde(default)]
    values: Option<Vec<String>>,
}

/// The body of `PUT /api/v1/settings`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BatchBody {
    changes: Vec<ChangeBody>,
}

/// The body of `PUT /api/v1/settings/tools/{tool}`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PostureBody {
    posture: String,
}

/// The body of `PUT /api/v1/settings/keys/{which}`.
///
/// No `Debug`: a derived one would print the key into any log that formatted the request.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyBody {
    key: String,
}

// The error is the response itself, as every handler here returns it; boxing it would only add an unwrap at each use.
#[allow(clippy::result_large_err)]
fn context(state: &GatewayState) -> Result<&SettingsContext, Response> {
    state.settings().ok_or_else(|| {
        error_response(
            StatusCode::NOT_FOUND,
            ErrorCode::UnavailableCapability,
            "settings are not available on this daemon",
        )
    })
}

/// A refusal with its reason. An API error message is one bounded line, so a longer explanation (the CLI lists every
/// setting) is cut to its first line.
fn refused(message: &str) -> Response {
    let first_line: String = message
        .lines()
        .next()
        .unwrap_or_default()
        .chars()
        .take(300)
        .collect();
    error_response(
        StatusCode::UNPROCESSABLE_ENTITY,
        ErrorCode::Validation,
        &first_line,
    )
}

fn key_json(paths: &AppPaths, kind: SecretKind) -> serde_json::Value {
    settings::key_state(paths, kind).map_or_else(
        |_| json!({ "state": "unknown" }),
        |state| json!({ "state": state.state }),
    )
}

/// `GET /api/v1/settings`
pub async fn list(State(state): State<GatewayState>) -> Response {
    let context = match context(&state) {
        Ok(context) => context,
        Err(response) => return response,
    };
    match settings::list(&context.paths) {
        Ok(views) => {
            let items: Vec<_> = views
                .into_iter()
                .map(|view| {
                    json!({
                        "key": view.key,
                        "kind": view.kind,
                        "help": view.help,
                        "value": view.value,
                        "file_state": view.file_state,
                        "group": view.group,
                        "default": view.default,
                        "unset_means": view.unset_means,
                        "example": view.example,
                    })
                })
                .collect();
            let postures = settings::tool_postures(&context.paths).unwrap_or_default();
            let body = json!({
                "settings": items,
                "postures": postures,
                "keys": {
                    "model": key_json(&context.paths, SecretKind::Model),
                    "voice": key_json(&context.paths, SecretKind::Voice),
            "search": key_json(&context.paths, SecretKind::Search),
            "google": key_json(&context.paths, SecretKind::Google),
                },
            });
            (StatusCode::OK, Json(body)).into_response()
        }
        Err(message) => refused(&message),
    }
}

/// `PUT /api/v1/settings/{key}`
pub async fn set(
    State(state): State<GatewayState>,
    UrlPath(key): UrlPath<String>,
    Json(body): Json<SetBody>,
) -> Response {
    let context = match context(&state) {
        Ok(context) => context,
        Err(response) => return response,
    };
    match settings::set(&context.paths, &key, &body.values) {
        Ok(()) => (
            StatusCode::OK,
            Json(json!({ "saved": true, "restart_to_apply": true })),
        )
            .into_response(),
        Err(message) => refused(&message),
    }
}

/// `PUT /api/v1/settings`: several changes, applied together or not at all.
pub async fn batch(State(state): State<GatewayState>, Json(body): Json<BatchBody>) -> Response {
    let context = match context(&state) {
        Ok(context) => context,
        Err(response) => return response,
    };
    if body.changes.is_empty() || body.changes.len() > 16 {
        return refused("send between 1 and 16 changes");
    }
    let changes: Vec<settings::Change> = body
        .changes
        .into_iter()
        .map(|change| settings::Change {
            key: change.key,
            values: change.values,
        })
        .collect();
    match settings::apply(&context.paths, &changes) {
        Ok(()) => (
            StatusCode::OK,
            Json(json!({ "saved": true, "restart_to_apply": true })),
        )
            .into_response(),
        Err(message) => refused(&message),
    }
}

/// Rebuilds the workspace policy from the saved configuration and hands it to the running pipeline, so a change to a tool's
/// permission (including "off" and "always ask") applies to the next call. Returns whether it did.
///
/// Fails closed in the useful direction: when the saved file cannot be read or composed, the running policy is left exactly as it
/// was and the caller says a restart is needed, so a half-understood file never loosens anything.
fn apply_policy_now(state: &GatewayState, context: &SettingsContext) -> bool {
    let Some(tools) = state.tools() else {
        return false;
    };
    let Ok(loaded) = ConfigStore::from_paths(&context.paths).load() else {
        return false;
    };
    match crate::compose_workspace_policy(loaded.config()) {
        Ok(policy) => {
            tools.replace_workspace_policy(policy);
            true
        }
        Err(_) => false,
    }
}

/// `PUT /api/v1/settings/tools/{tool}`: what the owner has decided about one tool (default, ask, trusted or off).
pub async fn set_posture(
    State(state): State<GatewayState>,
    UrlPath(tool): UrlPath<String>,
    Json(body): Json<PostureBody>,
) -> Response {
    let context = match context(&state) {
        Ok(context) => context,
        Err(response) => return response,
    };
    let posture = match settings::Posture::parse(&body.posture) {
        Ok(posture) => posture,
        Err(message) => return refused(&message),
    };
    // Only a tool this daemon actually has: a typo in a trust list would otherwise be a silent no-op.
    if let Some(tools) = state.tools()
        && !tools
            .policy_inventory()
            .is_ok_and(|entries| entries.iter().any(|entry| entry.id == tool))
    {
        return refused("this daemon has no tool with that id");
    }
    match settings::set_tool_posture(&context.paths, &tool, posture) {
        Ok(()) => {
            tracing::info!(%tool, posture = posture.name(), "a tool's permission was changed from the settings screen");
            let applied = apply_policy_now(&state, context);
            (
                StatusCode::OK,
                Json(json!({ "saved": true, "restart_to_apply": !applied, "applied": applied })),
            )
                .into_response()
        }
        Err(message) => refused(&message),
    }
}

/// `DELETE /api/v1/settings/{key}`
pub async fn unset(State(state): State<GatewayState>, UrlPath(key): UrlPath<String>) -> Response {
    let context = match context(&state) {
        Ok(context) => context,
        Err(response) => return response,
    };
    match settings::unset(&context.paths, &key) {
        Ok(()) => (
            StatusCode::OK,
            Json(json!({ "saved": true, "restart_to_apply": true })),
        )
            .into_response(),
        Err(message) => refused(&message),
    }
}

/// `PUT /api/v1/settings/keys/{which}`: write-only.
pub async fn set_key(
    State(state): State<GatewayState>,
    UrlPath(which): UrlPath<String>,
    Json(body): Json<KeyBody>,
) -> Response {
    let context = match context(&state) {
        Ok(context) => context,
        Err(response) => return response,
    };
    let kind = match SecretKind::parse(&which) {
        Ok(kind) => kind,
        Err(message) => return refused(&message),
    };
    match settings::set_secret(&context.paths, kind, &body.key) {
        Ok(_) => {
            tracing::info!(key = %which, "a key was set from the settings screen");
            let body = json!({ "saved": true, "restart_to_apply": true, "key": key_json(&context.paths, kind) });
            (StatusCode::OK, Json(body)).into_response()
        }
        Err(message) => refused(&message),
    }
}

/// `DELETE /api/v1/settings/keys/{which}`
pub async fn remove_key(
    State(state): State<GatewayState>,
    UrlPath(which): UrlPath<String>,
) -> Response {
    let context = match context(&state) {
        Ok(context) => context,
        Err(response) => return response,
    };
    let kind = match SecretKind::parse(&which) {
        Ok(kind) => kind,
        Err(message) => return refused(&message),
    };
    match settings::remove_secret(&context.paths, kind) {
        Ok(()) => (
            StatusCode::OK,
            Json(json!({ "saved": true, "restart_to_apply": true })),
        )
            .into_response(),
        Err(message) => refused(&message),
    }
}

/// `POST /api/v1/restart`: starts `jarvis restart` (the `jarvis` program beside this one) and returns at once; that command
/// stops this daemon gracefully and starts a new one with the saved settings.
pub async fn restart(State(state): State<GatewayState>, RawQuery(raw): RawQuery) -> Response {
    let context = match context(&state) {
        Ok(context) => context,
        Err(response) => return response,
    };
    let force = crate::stop::wants_force(raw.as_deref());
    if let Some(refusal) = crate::stop::refusal_if_working(&state, force).await {
        return refusal;
    }
    let Some(cli) = std::env::current_exe()
        .ok()
        .and_then(|exe| sibling_cli(&exe))
    else {
        return error_response(
            StatusCode::NOT_IMPLEMENTED,
            ErrorCode::Unsupported,
            "the `jarvis` program was not found beside this one; run `jarvis restart` in a terminal",
        );
    };
    match spawn_restart(&cli, context.root.as_deref(), force) {
        Ok(()) => (StatusCode::ACCEPTED, Json(json!({ "restarting": true }))).into_response(),
        Err(message) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::Internal,
            &message,
        ),
    }
}

fn sibling_cli(daemon: &Path) -> Option<PathBuf> {
    let name = if cfg!(windows) {
        "jarvis.exe"
    } else {
        "jarvis"
    };
    let candidate = daemon.parent()?.join(name);
    candidate.is_file().then_some(candidate)
}

/// Starts `jarvis restart [--root DIR]` detached, sharing no handle with this process.
fn spawn_restart(cli: &Path, root: Option<&Path>, force: bool) -> Result<(), String> {
    #[cfg(windows)]
    {
        let quote = |text: &str| format!("'{}'", text.replace('\'', "''"));
        let mut arguments = vec![quote("restart")];
        if force {
            arguments.push(quote("--force"));
        }
        if let Some(root) = root {
            arguments.push(quote("--root"));
            arguments.push(quote(&root.display().to_string()));
        }
        let script = format!(
            "Start-Process -FilePath {} -ArgumentList @({}) -WindowStyle Hidden",
            quote(&cli.display().to_string()),
            arguments.join(",")
        );
        std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
    #[cfg(not(windows))]
    {
        let mut command = std::process::Command::new(cli);
        command.arg("restart");
        if force {
            command.arg("--force");
        }
        if let Some(root) = root {
            command.arg("--root").arg(root);
        }
        command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}
