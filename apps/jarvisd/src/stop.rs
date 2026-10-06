//! A request to stop the daemon: `POST /api/v1/shutdown`.
//!
//! It wakes the **same** graceful path an operating-system signal does (transports stop accepting, MCP connections
//! close, the database closes, the single-instance lock is released), so `jarvis stop` and a Ctrl-C end the same way.
//! The route sits behind the local credential like the rest of the API; nothing else can ask for it.

use std::time::Duration;

use axum::{Json, http::StatusCode, response::IntoResponse, response::Response};
use serde_json::json;
use tokio::sync::Notify;

static STOP: Notify = Notify::const_new();

/// Completes when a stop has been requested over the API.
pub(crate) async fn requested() {
    STOP.notified().await;
}

/// `POST /api/v1/shutdown`
pub async fn request() -> Response {
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
    use super::*;

    /// A request wakes whoever is waiting for a stop, which is what lets the daemon's shutdown select complete.
    #[tokio::test]
    async fn a_request_wakes_the_waiter() {
        let waiter = tokio::spawn(requested());
        let reply = request().await;
        assert_eq!(reply.status(), StatusCode::ACCEPTED);
        let woke = tokio::time::timeout(Duration::from_secs(3), waiter).await;
        assert!(
            woke.is_ok(),
            "the waiter must be woken within the grace period"
        );
    }
}
