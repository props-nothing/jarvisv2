//! The digest: what JARVIS did over the last stretch of time (`ADR-0155`).
//!
//! Read-only, over rows that already exist. It exists so that coming back after a day takes one look rather than opening every
//! conversation: how many runs finished, failed or are waiting for an answer, what each project got done and decided, and what
//! it cost in tokens.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use jarvis_core::{ErrorCode, SystemClock, UtcTimestamp};
use jarvis_protocol::{DigestHighlight, DigestProblem, DigestProject, DigestReply};

use crate::gateway::{GatewayState, error_response};

/// The window when none is named.
const DEFAULT_HOURS: u32 = 24;

/// The widest window: a month.
const MAX_HOURS: u32 = 24 * 31;

/// The authenticated routes, mounted under /api/v1.
pub fn routes() -> axum::Router<GatewayState> {
    use axum::routing::get;
    axum::Router::new()
        .route("/digest", get(default_window))
        .route("/digest/{hours}", get(window))
}

async fn default_window(State(state): State<GatewayState>) -> Response {
    build(&state, DEFAULT_HOURS).await
}

async fn window(State(state): State<GatewayState>, Path(hours): Path<u32>) -> Response {
    if hours == 0 || hours > MAX_HOURS {
        return error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::Validation,
            "the window is between 1 hour and 31 days",
        );
    }
    build(&state, hours).await
}

async fn build(state: &GatewayState, hours: u32) -> Response {
    let unavailable = || {
        error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::Internal,
            "the local database is not available",
        )
    };
    let Ok(identity) = jarvis_storage::load_local_identity(state.database()).await else {
        return unavailable();
    };
    let now = UtcTimestamp::now(&SystemClock).unix_nanos();
    let Ok(since) = UtcTimestamp::from_unix_nanos(now - i128::from(hours) * 3_600_000_000_000)
    else {
        return unavailable();
    };
    match jarvis_storage::digest_since(state.database(), identity.workspace_id(), since).await {
        Ok(stored) => Json(reply(hours, stored)).into_response(),
        Err(error) => {
            tracing::warn!(%error, "a digest could not be built");
            unavailable()
        }
    }
}

fn reply(hours: u32, stored: jarvis_storage::StoredDigest) -> DigestReply {
    let totals = stored.totals;
    DigestReply {
        hours,
        runs: totals.runs,
        succeeded: totals.succeeded,
        failed: totals.failed,
        cancelled: totals.cancelled,
        waiting: totals.waiting,
        input_tokens: totals.input_tokens,
        output_tokens: totals.output_tokens,
        projects: stored
            .projects
            .into_iter()
            .map(|project| DigestProject {
                name: project.name,
                runs: project.runs,
                highlights: project
                    .highlights
                    .into_iter()
                    .map(|note| DigestHighlight {
                        kind: note.kind.as_str().to_owned(),
                        text: note.body,
                        created_at: note.created_at,
                    })
                    .collect(),
            })
            .collect(),
        problems: stored
            .problems
            .into_iter()
            .map(|run| DigestProblem {
                objective: crate::schedule_service::shown_objective(&run.objective),
                error_code: run.error_code,
            })
            .collect(),
    }
}
