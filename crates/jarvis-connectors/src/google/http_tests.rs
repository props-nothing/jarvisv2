//! The transport against a **real HTTP server**.
//!
//! # Why the server is hand-written
//!
//! The same reasoning `jarvis-mcp-transport/tests/http.rs` records: a framework would share assumptions with the
//! client under test, so a disagreement with the specification would pass. This server reads and writes HTTP/1.1
//! itself and **records what it received**, so a claim like "no redirect was followed" is observed as *the second
//! server was never contacted* rather than inferred from a return value.
//!
//! # What these tests prove, and what they cannot
//!
//! They prove the transport's own controls — that it presents the credential, that it does not follow a
//! redirect, that it reports a non-2xx as a response, and that the two failure directions are classified
//! correctly. They do **not** prove anything about Google: every response here is written by this file, and the
//! opt-in live smoke test named in the research record remains unwritten.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use jarvis_core::ToolOutcome;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

use super::*;
use crate::google::transport::RetryAfter;

/// One instant, so a seam test does not restate a timestamp.
fn now() -> jarvis_core::UtcTimestamp {
    jarvis_core::UtcTimestamp::from_unix_nanos(1_774_000_000_500_000_000)
        .unwrap_or_else(|_| panic!("a representable instant"))
}

/// One request the server received, as it appeared on the wire.
#[derive(Clone, Debug)]
struct SeenRequest {
    method: String,
    path: String,
    headers: BTreeMap<String, String>,
}

/// How the server answers a request.
#[derive(Clone)]
enum Answer {
    /// A complete response, optionally carrying a `Retry-After`.
    Reply {
        status: u16,
        body: String,
        retry_after: Option<String>,
    },
    /// A `3xx` pointing somewhere else, which the client must not follow.
    Redirect { location: String },
    /// Accept the connection and never answer, so the client's own deadline is what ends the exchange.
    Hang,
}

/// A hand-written HTTP/1.1 server that records every request it receives.
struct TestServer {
    port: u16,
    seen: Arc<Mutex<Vec<SeenRequest>>>,
}

impl TestServer {
    async fn start(answer: Answer) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap_or_else(|error| panic!("bind loopback: {error}"));
        let port = listener
            .local_addr()
            .unwrap_or_else(|error| panic!("local addr: {error}"))
            .port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&seen);

        tokio::spawn(async move {
            loop {
                let Ok((stream, _peer)) = listener.accept().await else {
                    return;
                };
                let answer = answer.clone();
                let recorder = Arc::clone(&recorder);
                tokio::spawn(async move {
                    let _ = serve_connection(stream, &answer, &recorder).await;
                });
            }
        });

        Self { port, seen }
    }

    /// Returns the number of requests received.
    fn request_count(&self) -> usize {
        self.seen
            .lock()
            .map(|guard| guard.len())
            .unwrap_or_default()
    }

    /// Returns the first request received, panicking if there was none.
    fn first(&self) -> SeenRequest {
        self.seen
            .lock()
            .map(|guard| guard.first().cloned())
            .unwrap_or_default()
            .unwrap_or_else(|| panic!("the server must have received a request"))
    }
}

/// Serves requests on one connection until the peer closes it.
async fn serve_connection(
    stream: TcpStream,
    answer: &Answer,
    seen: &Arc<Mutex<Vec<SeenRequest>>>,
) -> std::io::Result<()> {
    // `Hang` accepts and then waits, so the client's deadline is what ends the exchange rather than an error
    // invented by this server.
    if matches!(answer, Answer::Hang) {
        tokio::time::sleep(Duration::from_secs(60)).await;
        return Ok(());
    }

    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);

    loop {
        let mut request_line = String::new();
        if reader.read_line(&mut request_line).await? == 0 {
            return Ok(());
        }
        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or_default().to_owned();
        let path = parts.next().unwrap_or_default().to_owned();

        let mut headers: BTreeMap<String, String> = BTreeMap::new();
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).await? == 0 {
                return Ok(());
            }
            let trimmed = line.trim_end_matches(['\r', '\n']);
            if trimmed.is_empty() {
                break;
            }
            if let Some((name, value)) = trimmed.split_once(':') {
                headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
            }
        }

        if let Ok(mut guard) = seen.lock() {
            guard.push(SeenRequest {
                method: method.clone(),
                path: path.clone(),
                headers: headers.clone(),
            });
        }

        let response = match answer {
            Answer::Reply {
                status,
                body,
                retry_after,
            } => {
                let retry = retry_after
                    .as_ref()
                    .map(|value| format!("Retry-After: {value}\r\n"))
                    .unwrap_or_default();
                format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n{retry}Content-Length: {}\r\n\r\n{body}",
                    body.len()
                )
            }
            Answer::Redirect { location } => {
                format!("HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\n\r\n")
            }
            Answer::Hang => unreachable!("`Hang` returns before this loop"),
        };
        write_half.write_all(response.as_bytes()).await?;
        write_half.flush().await?;
    }
}

/// A token long enough to pass `AccessToken`'s length floor.
fn token() -> AccessToken {
    AccessToken::new("test-token-0123456789abcdefghijkl").unwrap_or_else(|error| panic!("{error}"))
}

/// A request against the given origin, built by the real request layer.
fn request_for_server(server: &TestServer) -> HttpRequest {
    crate::google::request::gmail_messages_list(Some("is:unread"), Some(10), None)
        .unwrap_or_else(|error| panic!("a valid request: {error}"))
        // The request layer binds to the real API base, so the origin is swapped for the loopback server. This
        // is the recorded test seam (`HttpRequest::rebase_to`), and it rewrites the origin alone — the path and
        // the percent-encoded query asserted below are the ones the product builds.
        .rebase_to(&format!("http://127.0.0.1:{}", server.port))
}

#[tokio::test]
async fn a_success_becomes_a_response_and_presents_the_credential() {
    // The positive control for every other test in this file: if this did not work, the refusals below would
    // pass on a transport that never sent anything.
    let server = TestServer::start(Answer::Reply {
        status: 200,
        body: r#"{"messages":[{"id":"m1"}]}"#.to_owned(),
        retry_after: None,
    })
    .await;
    let transport = ReqwestTransport::new().unwrap_or_else(|error| panic!("{error}"));
    let request = request_for_server(&server);

    let response = transport
        .send(HttpMethod::Get, &request, &token())
        .await
        .unwrap_or_else(|error| panic!("a 200 must be a response: {error}"));
    assert_eq!(response.status, 200);
    assert!(response.is_success());
    assert!(response.body.contains("\"m1\""));
    assert_eq!(response.retry_after, None);

    // The server SAW the request, and it carried the credential and the method and a query — so the assertion
    // above is about a real exchange rather than about a value this process made up.
    let seen = server.first();
    assert_eq!(seen.method, "GET");
    assert!(
        seen.headers
            .get("authorization")
            .is_some_and(|value| value.starts_with("Bearer ")),
        "the credential must be presented as a bearer header: {:?}",
        seen.headers.keys()
    );
    assert!(
        seen.path.contains("q=is%3Aunread"),
        "the encoded query must be sent: {}",
        seen.path
    );
    // **The credential is not in the URL**, which the server can observe directly: `HttpRequest` has no field
    // for one, so the path cannot carry it however the transport is written.
    assert!(
        !seen.path.contains("access_token") && !seen.path.contains("token="),
        "a credential must not be reachable through the URL: {}",
        seen.path
    );
}

#[tokio::test]
async fn a_provider_refusal_is_a_response_and_not_a_failure() {
    // A `403` was received and refused. Collapsing it into a `TransportFailure` would lose the status and the
    // machine-readable reason, which is everything the caller's retry decision needs.
    let server = TestServer::start(Answer::Reply {
        status: 403,
        body: r#"{"error":{"code":403,"errors":[{"reason":"rateLimitExceeded"}]}}"#.to_owned(),
        retry_after: None,
    })
    .await;
    let transport = ReqwestTransport::new().unwrap_or_else(|error| panic!("{error}"));

    let response = transport
        .send(HttpMethod::Get, &request_for_server(&server), &token())
        .await
        .unwrap_or_else(|error| panic!("a refusal must be a response, not a failure: {error}"));
    assert_eq!(response.status, 403);
    assert!(!response.is_success());
    assert!(response.body.contains("rateLimitExceeded"));
}

#[tokio::test]
async fn a_stated_retry_delay_is_carried_and_an_unreadable_one_is_absent() {
    let server = TestServer::start(Answer::Reply {
        status: 429,
        body: "{}".to_owned(),
        retry_after: Some("30".to_owned()),
    })
    .await;
    let transport = ReqwestTransport::new().unwrap_or_else(|error| panic!("{error}"));
    let response = transport
        .send(HttpMethod::Get, &request_for_server(&server), &token())
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    // Carried, never interpreted: honouring a delay is a retry decision and `client::classify` owns it.
    assert_eq!(response.retry_after, Some(RetryAfter::Seconds(30)));

    // A value stated in the **other** form RFC 9110 §10.2.3 allows is not `None`. `None` means the header was
    // absent; this means a delay was stated and this client could not read it as seconds. Collapsing the two
    // would read a stated wait as a missing one and retry sooner than the provider asked (`ADR-0076`).
    let odd = TestServer::start(Answer::Reply {
        status: 429,
        body: "{}".to_owned(),
        retry_after: Some("Wed, 21 Oct 2026 07:28:00 GMT".to_owned()),
    })
    .await;
    let response = transport
        .send(HttpMethod::Get, &request_for_server(&odd), &token())
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        response.retry_after,
        Some(RetryAfter::NotSeconds),
        "a stated delay in an unreadable form must not read as an absent one"
    );
    assert!(
        response.retry_after.and_then(RetryAfter::seconds).is_none(),
        "the date form has no seconds this client can use without a clock"
    );
}

#[tokio::test]
async fn a_redirect_is_not_followed_and_the_target_is_never_contacted() {
    // The credential boundary's most important transport-level control. A redirect can move a bearer credential
    // to another origin, so `Policy::none()` is a security statement rather than a preference — and the evidence
    // is that the *second server* never saw a connection.
    let target = TestServer::start(Answer::Reply {
        status: 200,
        body: "{}".to_owned(),
        retry_after: None,
    })
    .await;
    let origin = TestServer::start(Answer::Redirect {
        location: format!("http://127.0.0.1:{}/stolen", target.port),
    })
    .await;
    let transport = ReqwestTransport::new().unwrap_or_else(|error| panic!("{error}"));

    let response = transport
        .send(HttpMethod::Get, &request_for_server(&origin), &token())
        .await
        .unwrap_or_else(|error| {
            panic!("a redirect must be a response carrying its status: {error}")
        });

    // The property first, before any formatting assertion that could mask it: a redirect is reported, not
    // resolved, so the caller's classifier sees the `3xx`.
    assert_eq!(
        response.status, 302,
        "a redirect must arrive as its own status, not as a followed success"
    );
    assert_eq!(
        target.request_count(),
        0,
        "the credential must not be carried to the redirect target"
    );
    assert_eq!(origin.request_count(), 1);
}

#[tokio::test]
async fn a_refused_connection_is_certain_and_a_timeout_is_not() {
    // **The asymmetry this mapping exists for.** A caller that read a timeout as "nothing happened" would retry,
    // and for a non-idempotent effect a retry is a second effect. So the two must classify differently.
    //
    // A refused connection: bind to take a port, then drop the listener so nothing is listening.
    let dead = TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap_or_else(|error| panic!("bind: {error}"));
    let port = dead
        .local_addr()
        .unwrap_or_else(|error| panic!("addr: {error}"))
        .port();
    drop(dead);
    let refused_request = crate::google::request::gmail_messages_list(None, None, None)
        .unwrap_or_else(|error| panic!("{error}"))
        .rebase_to(&format!("http://127.0.0.1:{port}"));
    let transport = ReqwestTransport::new().unwrap_or_else(|error| panic!("{error}"));
    let failure = transport
        .send(HttpMethod::Get, &refused_request, &token())
        .await
        .err()
        .unwrap_or_else(|| panic!("connecting to a closed port must fail"));
    assert_eq!(
        failure,
        TransportFailure::Connect,
        "a refused connection is certain: nothing was written"
    );
    assert!(
        !failure.may_have_reached_the_provider(),
        "nothing was written, so nothing may have happened"
    );

    // A timeout: a real server that accepts and never answers. The short deadline is what ends it.
    let hanging = TestServer::start(Answer::Hang).await;
    let transport =
        ReqwestTransport::with_timeouts(Duration::from_millis(300), Duration::from_millis(200))
            .unwrap_or_else(|error| panic!("{error}"));
    let failure = transport
        .send(HttpMethod::Get, &request_for_server(&hanging), &token())
        .await
        .err()
        .unwrap_or_else(|| panic!("a server that never answers must time out"));
    assert_eq!(failure, TransportFailure::Timeout);
    assert!(
        failure.may_have_reached_the_provider(),
        "a timed-out request may have arrived, so a retry of a non-idempotent effect is unsafe"
    );
}

#[tokio::test]
async fn the_transport_does_not_print_its_internals() {
    // A client's own `Debug` renders its configuration, which may name resolver and TLS settings; a transport's
    // rendering belongs in a diagnostic rather than in a formatted value.
    let transport = ReqwestTransport::new().unwrap_or_else(|error| panic!("{error}"));
    let rendered = format!("{transport:?}");
    assert_eq!(rendered, "ReqwestTransport { .. }");
    assert!(!rendered.contains("Client"), "{rendered}");
}

// ---------------------------------------------------------------------------------------------------------
// The seam: the real adapter driving the real transport over a real socket.
// ---------------------------------------------------------------------------------------------------------

#[tokio::test]
async fn the_real_adapter_drives_the_real_transport_over_a_socket() {
    // **Why this test exists.** Every earlier test put ONE real half against a scripted other half: the adapter
    // ran against [`Scripted`](crate::google::operations::tests), and the transport ran against a hand-written
    // request. So the two were never joined, and a defect *in the seam* — a URL the adapter builds that the
    // transport sends to the wrong place, a header one sets and the other drops — would be invisible to both.
    // This is the shape the whole repository records as where the real defects live.
    let server = TestServer::start(Answer::Reply {
        status: 200,
        body: r#"{"messages":[{"id":"m1"},{"id":"m2"}],"nextPageToken":"more"}"#.to_owned(),
        retry_after: None,
    })
    .await;
    let transport = ReqwestTransport::new().unwrap_or_else(|error| panic!("{error}"));
    let token = token();
    let tool = crate::google::operations::GoogleReadTool::new(&transport, &token, "google-read");

    // The arguments go through the real request builder, and the origin is redirected by the recorded test
    // seam. Everything else about the request — path, encoding, headers — is what the product builds.
    let result = tool
        .run_with_origin(
            "google.gmail_messages_list",
            &serde_json::json!({ "query": "is:unread", "max_results": 10 }),
            now(),
            &format!("http://127.0.0.1:{}", server.port),
        )
        .await
        .unwrap_or_else(|error| panic!("a 200 must produce a result: {error}"));

    assert_eq!(result.outcome(), ToolOutcome::Confirmed);
    let output = result
        .output()
        .unwrap_or_else(|| panic!("a confirmed read carries output"))
        .content();
    // The adapter's own rendering satisfies the schema it declares, and the page token is a separate field —
    // so the seam carries what the tool contract promises rather than the provider's raw resource.
    assert!(
        output.contains("\"message_ids\":[\"m1\",\"m2\"]"),
        "{output}"
    );
    assert!(output.contains("\"next_page_token\":\"more\""), "{output}");

    // The server saw a real, encoded, authenticated request — so the assertions above are about bytes on a
    // socket rather than about a value assembled in this process.
    let seen = server.first();
    assert_eq!(seen.method, "GET");
    // The request target includes the query, so the **path** is compared on its own — the same split the
    // request layer makes between `url()` and `url_with_query()`.
    assert_eq!(
        seen.path.split('?').next(),
        Some("/gmail/v1/users/me/messages"),
        "{}",
        seen.path
    );
    assert!(seen.path.contains("q=is%3Aunread"), "{}", seen.path);
    assert!(seen.path.contains("maxResults=10"), "{}", seen.path);
    assert!(
        seen.headers
            .get("accept")
            .is_some_and(|value| value.contains("application/json")),
        "the declared Accept must reach the wire: {:?}",
        seen.headers.get("accept")
    );
    assert!(
        seen.headers
            .get("authorization")
            .is_some_and(|value| value.starts_with("Bearer ")),
        "the credential must reach the wire as a bearer header"
    );
}

#[tokio::test]
async fn the_real_adapter_reads_a_refusal_from_the_transport_as_a_result() {
    // The seam's other half: a provider refusal crossing a socket must arrive as a `Failed` outcome carrying the
    // machine-readable reason, not as an adapter error. Collapsing it either way would lose the reason code the
    // caller's retry decision needs.
    let server = TestServer::start(Answer::Reply {
        status: 403,
        body: r#"{"error":{"code":403,"errors":[{"reason":"domainPolicy"}]}}"#.to_owned(),
        retry_after: None,
    })
    .await;
    let transport = ReqwestTransport::new().unwrap_or_else(|error| panic!("{error}"));
    let token = token();
    let tool = crate::google::operations::GoogleReadTool::new(&transport, &token, "google-read");

    let result = tool
        .run_with_origin(
            "google.gmail_messages_list",
            &serde_json::json!({}),
            now(),
            &format!("http://127.0.0.1:{}", server.port),
        )
        .await
        .unwrap_or_else(|error| panic!("a refusal is a result, not an adapter error: {error}"));

    assert_eq!(result.outcome(), ToolOutcome::Failed);
    let reason = result.record().reason().unwrap_or_default();
    assert!(
        reason.contains("domainPolicy"),
        "the reason must be the provider's code, not its prose: {reason}"
    );
    assert!(result.output().is_none());
}

#[tokio::test]
async fn a_gateway_error_is_read_as_a_failure_signal_rather_than_as_a_retry() {
    // A real gateway's `502` is a **refusal from a layer in front of the endpoint**, not an unreachable host. It
    // still arrives as a response, so the adapter classifies it — and the seam is what proves the status survives
    // the round trip rather than being turned into an error by the transport.
    let server = TestServer::start(Answer::Reply {
        status: 502,
        body: "<html>Bad Gateway</html>".to_owned(),
        retry_after: None,
    })
    .await;
    let transport = ReqwestTransport::new().unwrap_or_else(|error| panic!("{error}"));
    let token = token();
    let tool = crate::google::operations::GoogleReadTool::new(&transport, &token, "google-read");

    let result = tool
        .run_with_origin(
            "google.gmail_messages_list",
            &serde_json::json!({}),
            now(),
            &format!("http://127.0.0.1:{}", server.port),
        )
        .await
        .unwrap_or_else(|error| panic!("a 502 is a response: {error}"));

    assert_eq!(result.outcome(), ToolOutcome::Failed);
    let reason = result.record().reason().unwrap_or_default();
    // The body was HTML and could not be parsed, so the reason names the *status* — which is a fact even when
    // the body is not. The difference between "we know little" and "we know nothing".
    assert!(reason.contains("502"), "{reason}");
    assert!(
        !reason.contains("<html>"),
        "the body must not be forwarded: {reason}"
    );
}

#[tokio::test]
async fn every_operation_reaches_the_socket_the_product_builds_for_it() {
    // The seam across **all four** declared operations, so a request builder that produced a URL the transport
    // could not send — or a path a reader of the manifest would not expect — fails here rather than in a live
    // call. Each operation gets its own server so the recorded path belongs to one request.
    let cases: [(&str, serde_json::Value, &str); 4] = [
        (
            "google.gmail_messages_list",
            serde_json::json!({ "query": "is:unread" }),
            "/gmail/v1/users/me/messages",
        ),
        (
            "google.gmail_history_list",
            serde_json::json!({ "start_history_id": "12345" }),
            "/gmail/v1/users/me/history",
        ),
        (
            "google.gmail_messages_read",
            serde_json::json!({ "message_id": "m1" }),
            "/gmail/v1/users/me/messages/m1",
        ),
        (
            "google.calendar_events_read",
            serde_json::json!({ "calendar_id": "primary" }),
            "/calendar/v3/calendars/primary/events",
        ),
    ];
    let transport = ReqwestTransport::new().unwrap_or_else(|error| panic!("{error}"));
    let token = token();

    for (tool_name, arguments, expected_path) in cases {
        // A body per operation, so each parses: an events page uses `items`, a history page uses `history`.
        let body = if tool_name.contains("calendar") {
            r#"{"items":[{"id":"e1"}]}"#
        } else if tool_name.contains("history") {
            r#"{"history":[{"messages":[{"id":"m1"}]}],"historyId":"12347"}"#
        } else if tool_name.ends_with("read") {
            r#"{"id":"m1"}"#
        } else {
            r#"{"messages":[{"id":"m1"}]}"#
        };
        let server = TestServer::start(Answer::Reply {
            status: 200,
            body: body.to_owned(),
            retry_after: None,
        })
        .await;
        let adapter =
            crate::google::operations::GoogleReadTool::new(&transport, &token, "google-read");
        let result = adapter
            .run_with_origin(
                tool_name,
                &arguments,
                now(),
                &format!("http://127.0.0.1:{}", server.port),
            )
            .await
            .unwrap_or_else(|error| panic!("{tool_name} must reach a server: {error}"));
        assert_eq!(result.outcome(), ToolOutcome::Confirmed, "{tool_name}");
        assert_eq!(
            server.request_count(),
            1,
            "{tool_name} must send exactly one request"
        );
        let seen = server.first();
        assert_eq!(
            seen.path.split('?').next(),
            Some(expected_path),
            "{tool_name} must address {expected_path}, got {}",
            seen.path
        );
    }
}
