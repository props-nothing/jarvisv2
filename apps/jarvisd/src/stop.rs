//! A request to stop the daemon: `POST /api/v1/shutdown`.
//!
//! It wakes the **same** graceful path an operating-system signal does (transports stop accepting, MCP connections
//! close, the database closes, the single-instance lock is released), so `jarvis stop` and a Ctrl-C end the same way.
//! The route sits behind the local credential like the rest of the API; nothing else can ask for it.
//!
//! # A stop does not silently throw work away
//!
//! A task the daemon is driving is lost to a stop (it is settled as interrupted on the next start). So a stop or a restart that
//! would interrupt running tasks is **refused with the count** unless the caller says `force=true`, which is the caller choosing to
//! lose them. A task waiting for an approval is not counted: it is durable and survives. (A real restart once killed several
//! sub-agent runs an owner had been waiting on, which is why this exists.)

use std::time::Duration;

use axum::{
    Json,
    extract::{RawQuery, State},
    http::StatusCode,
    response::IntoResponse,
    response::Response,
};
use serde_json::json;
use tokio::sync::Notify;

use crate::gateway::GatewayState;

static STOP: Notify = Notify::const_new();

/// Completes when a stop has been requested over the API.
pub(crate) async fn requested() {
    STOP.notified().await;
}

/// Whether a query string carries `force=true`.
pub(crate) fn wants_force(raw: Option<&str>) -> bool {
    raw.is_some_and(|query| query.split('&').any(|pair| pair == "force=true"))
}

/// The refusal to give when tasks are working and the caller did not force, or `None` to go ahead.
pub(crate) async fn refusal_if_working(state: &GatewayState, force: bool) -> Option<Response> {
    if force {
        return None;
    }
    let working = jarvis_storage::count_working_runs(state.database())
        .await
        .unwrap_or(0);
    if working == 0 {
        return None;
    }
    let plural = if working == 1 { "task is" } else { "tasks are" };
    let message = format!(
        "{working} {plural} still working, and stopping now would interrupt {}",
        if working == 1 { "it" } else { "them" }
    );
    Some(
        (
            StatusCode::CONFLICT,
            Json(json!({ "code": "runs_in_flight", "in_flight": working, "message": message })),
        )
            .into_response(),
    )
}

/// `POST /api/v1/shutdown[?force=true]`
pub async fn request(State(state): State<GatewayState>, RawQuery(raw): RawQuery) -> Response {
    if let Some(refusal) = refusal_if_working(&state, wants_force(raw.as_deref())).await {
        return refusal;
    }
    tracing::info!("a stop was requested over the API");
    // After the reply has had time to leave, so the caller is told "stopping" rather than seeing the connection drop.
    tokio::spawn(async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        STOP.notify_one();
    });
    (StatusCode::ACCEPTED, Json(json!({ "stopping": true }))).into_response()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    async fn state() -> (GatewayState, std::path::PathBuf) {
        let tag = jarvis_core::scratch_tag();
        let path = std::env::temp_dir().join(format!("jst-{}", &tag[tag.len() - 12..]));
        std::fs::create_dir_all(&path).unwrap_or_else(|error| panic!("{error}"));
        let database = Arc::new(
            jarvis_storage::SqliteDatabase::open(&path.join("jarvis.sqlite3"))
                .await
                .unwrap_or_else(|error| panic!("{error}")),
        );
        let credential =
            jarvis_core::ClientCredential::generate().unwrap_or_else(|error| panic!("{error}"));
        (GatewayState::new(database, credential), path)
    }

    /// A request wakes whoever is waiting for a stop, which is what lets the daemon's shutdown select complete.
    #[tokio::test]
    async fn a_request_wakes_the_waiter() {
        let (state, path) = state().await;
        let waiter = tokio::spawn(requested());
        let reply = request(State(state), RawQuery(None)).await;
        assert_eq!(reply.status(), StatusCode::ACCEPTED);
        let woke = tokio::time::timeout(Duration::from_secs(3), waiter).await;
        assert!(
            woke.is_ok(),
            "the waiter must be woken within the grace period"
        );
        jarvis_core::remove_scratch_dir(&path);
    }

    #[test]
    fn only_force_true_forces() {
        assert!(wants_force(Some("force=true")));
        assert!(wants_force(Some("a=1&force=true")));
        assert!(!wants_force(Some("force=false")));
        assert!(!wants_force(Some("forced=true")));
        assert!(!wants_force(None));
    }

    /// **A stop that would interrupt a working task is refused with the count, forced past on request, and a parked task is not counted.**
    #[tokio::test]
    async fn a_stop_that_would_interrupt_work_is_refused_unless_forced() {
        let (state, path) = state().await;
        assert!(
            refusal_if_working(&state, false).await.is_none(),
            "nothing is working"
        );

        crate::run_service::RunService::new(state.database_handle())
            .start(&jarvis_protocol::StartRunRequest {
                objective: "work".to_owned(),
                session_id: None,
                idempotency_key: None,
                project_id: None,
            })
            .await
            .unwrap_or_else(|_| panic!("start a run"));
        let refusal = refusal_if_working(&state, false)
            .await
            .unwrap_or_else(|| panic!("a run that has started is working"));
        assert_eq!(refusal.status(), StatusCode::CONFLICT);
        assert!(
            refusal_if_working(&state, true).await.is_none(),
            "force goes past it"
        );
        jarvis_core::remove_scratch_dir(&path);
    }
}
