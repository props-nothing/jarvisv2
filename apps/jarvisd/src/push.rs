//! Telling the owner something while they are away, through ntfy (`ADR-0155`, `docs/research/integrations/ntfy.md`).
//!
//! Off unless `daemon.push_topic` is set. What is sent is deliberately content-free: that an approval is waiting and for which
//! tool, or that a scheduled task finished. Never an answer, a question or an argument, because the topic is the only secret on a
//! server the owner may not control. The model cannot cause a push: this is the daemon speaking to its owner, not a tool.

use std::time::Duration;

use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use jarvis_core::ErrorCode;

use crate::gateway::{GatewayState, error_response};

/// How long one publish may take. A push is a courtesy and must not hold the scheduler up.
const TIMEOUT: Duration = Duration::from_secs(10);

/// Where to push, read from the configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Target {
    server: String,
    topic: String,
}

impl Target {
    /// The configured target, or `None` when push is not set up.
    pub(crate) fn from_config(daemon: &jarvis_storage::DaemonConfig) -> Option<Self> {
        let topic = daemon.push_topic()?;
        Some(Self {
            server: daemon.push_server().trim_end_matches('/').to_owned(),
            topic: topic.to_owned(),
        })
    }

    fn url(&self) -> String {
        format!("{}/{}", self.server, self.topic)
    }
}

/// Publishes `body` with `title` at `priority` (1 to 5).
///
/// # Errors
///
/// Returns the transport error, or a message naming the refusal status. Nothing is retried.
pub(crate) async fn send(
    target: &Target,
    title: &str,
    body: &str,
    priority: u8,
) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .timeout(TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| error.to_string())?;
    let response = client
        .post(target.url())
        .header("Title", ascii(title))
        .header("Priority", priority.clamp(1, 5).to_string())
        .header("Tags", "robot_face")
        .body(body.to_owned())
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(format!("the push server answered {}", response.status()))
    }
}

/// Header-safe text: printable ASCII only, since not every HTTP stack carries UTF-8 headers.
fn ascii(text: &str) -> String {
    text.chars()
        .map(|character| {
            if character.is_ascii_graphic() || character == ' ' {
                character
            } else {
                '?'
            }
        })
        .take(100)
        .collect()
}

fn configured(state: &GatewayState) -> Option<Target> {
    state.settings().and_then(|context| {
        let loaded = jarvis_storage::ConfigStore::from_paths(context.paths())
            .load()
            .ok()?;
        Target::from_config(loaded.config().daemon())
    })
}

/// The authenticated routes, mounted under /api/v1.
pub fn routes() -> axum::Router<GatewayState> {
    axum::Router::new().route("/push/test", axum::routing::post(test))
}

/// Sends one test message, so the owner can tell whether push is set up right.
async fn test(State(state): State<GatewayState>) -> Response {
    let Some(target) = configured(&state) else {
        return error_response(
            StatusCode::CONFLICT,
            ErrorCode::Conflict,
            "push is not set up: set daemon.push_topic first",
        );
    };
    match send(&target, "JARVIS", "This is a test from JARVIS.", 3).await {
        Ok(()) => Json(serde_json::json!({ "sent": true })).into_response(),
        Err(error) => {
            tracing::warn!(%error, "the push test failed");
            error_response(
                StatusCode::BAD_GATEWAY,
                ErrorCode::Internal,
                "the push server did not accept the message",
            )
        }
    }
}

/// Pushes to the owner's phone when push is configured. A failure is logged, never raised.
pub(crate) async fn alert(state: &GatewayState, title: &str, body: &str, priority: u8) {
    let Some(target) = configured(state) else {
        return;
    };
    match send(&target, title, body, priority).await {
        Ok(()) => tracing::info!("pushed a notification to the configured topic"),
        Err(error) => tracing::warn!(%error, "a push notification could not be sent"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn serve_once(status_line: &'static str) -> (u16, tokio::task::JoinHandle<String>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        let port = listener
            .local_addr()
            .unwrap_or_else(|e| panic!("{e}"))
            .port();
        let handle = tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let (mut socket, _) = listener.accept().await.unwrap_or_else(|e| panic!("{e}"));
            let mut seen = Vec::new();
            let mut chunk = [0_u8; 2048];
            loop {
                let read = socket.read(&mut chunk).await.unwrap_or(0);
                seen.extend_from_slice(&chunk[..read]);
                let text = String::from_utf8_lossy(&seen);
                let complete = text
                    .split_once("\r\n\r\n")
                    .is_some_and(|(_, body)| body.contains("waiting"));
                if read == 0 || complete {
                    break;
                }
            }
            let reply =
                format!("{status_line}\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{{}}");
            let _ = socket.write_all(reply.as_bytes()).await;
            String::from_utf8_lossy(&seen).into_owned()
        });
        (port, handle)
    }

    fn target(port: u16) -> Target {
        Target {
            server: format!("http://127.0.0.1:{port}"),
            topic: "jarvis-test-topic".to_owned(),
        }
    }

    #[tokio::test]
    async fn a_push_is_a_plain_post_to_the_topic_with_an_ascii_title_and_no_secrets() {
        let (port, request) = serve_once("HTTP/1.1 200 OK").await;
        let sent = send(
            &target(port),
            "JARVIS \u{2014} needs you",
            "JARVIS is waiting for your approval",
            4,
        )
        .await;
        assert_eq!(sent, Ok(()));
        let text = request
            .await
            .unwrap_or_else(|e| panic!("{e}"))
            .to_ascii_lowercase();
        assert!(text.starts_with("post /jarvis-test-topic "), "{text}");
        assert!(text.contains("title: jarvis ? needs you") && text.contains("priority: 4"));
        assert!(
            !text.contains("authorization"),
            "no credential is ever sent"
        );
        assert!(text.ends_with("jarvis is waiting for your approval"));
    }

    #[tokio::test]
    async fn a_refusing_server_is_reported_and_nothing_is_retried() {
        let (port, request) = serve_once("HTTP/1.1 429 Too Many Requests").await;
        let sent = send(&target(port), "JARVIS", "waiting", 3).await;
        assert!(sent.is_err_and(|error| error.contains("429")));
        assert!(request.await.is_ok());
    }

    #[test]
    fn no_topic_means_no_push() {
        assert_eq!(
            Target::from_config(&jarvis_storage::DaemonConfig::default()),
            None
        );
    }
}
