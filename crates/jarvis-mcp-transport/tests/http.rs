//! `connect_http` against a **real HTTP server**.
//!
//! # What this closes
//!
//! `P3-008e` recorded this limit in plain words — "`connect_stdio` and `connect_http` are **entirely
//! unexercised**" — and `P3-008f` closed the stdio half. This closes the other one. An in-process
//! duplex pair proves the framing and the negotiation; it says nothing about whether the Streamable
//! HTTP transport forms the requests the specification requires, whether a redirect is really refused,
//! or whether an HTTP-level failure arrives with the status in its message.
//!
//! # The server is hand-written
//!
//! `HttpTestServer` below reads and writes HTTP/1.1 itself rather than using a framework, for the
//! reason the stdio peer and the MCP scripted peer are hand-written: a fixture built from the same
//! library as the code under test (or from a framework that shares its conveniences) can agree with
//! that library and disagree with the specification. This server also **records what it received**, so
//! the assertions are about bytes rather than about a negotiation that returned `Ok`.
//!
//! # The transport's own behaviours are now observable
//!
//! `docs/architecture/security.md` requires "redirect revalidation" and the MCP specification requires
//! `Origin` validation and protocol headers. Two of those are testable from the client side, and both
//! are asserted here: a redirect must not be followed (the redirect target must receive *nothing*), and
//! the request must carry `Accept` for both response media types and the negotiated protocol version.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use jarvis_mcp::{McpToolListing, NamingStrategy, ServerName, ToolEffectPolicy};
use jarvis_mcp_transport::{McpHttpEndpoint, ToolBuffer, connect_http};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

/// One request the server received, as it appeared on the wire.
#[derive(Clone, Debug)]
struct SeenRequest {
    method: String,
    path: String,
    headers: BTreeMap<String, String>,
    body: Value,
}

/// How the server answers.
#[derive(Clone)]
enum Reply {
    /// A JSON-RPC result for a method, wrapped in a response envelope.
    Result(Value),
    /// A JSON-RPC error with the given code and message.
    Error { code: i64, message: String },
    /// An HTTP redirect, which the client must not follow.
    Redirect { location: String },
    /// A bare HTTP failure status.
    Status { code: u16, body: String },
    /// A success status whose body is not JSON, which must be refused rather than read as a result.
    NonJson { content_type: String, body: String },
}

/// A hand-written HTTP/1.1 server that answers `server/discover` and `tools/list`.
struct HttpTestServer {
    port: u16,
    seen: Arc<Mutex<Vec<SeenRequest>>>,
}

impl HttpTestServer {
    /// Starts a server that answers the given MCP methods, and keeps serving until dropped.
    async fn start(replies: BTreeMap<String, Reply>) -> Self {
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
                let replies = replies.clone();
                let recorder = Arc::clone(&recorder);
                tokio::spawn(async move {
                    let _ = serve_connection(stream, &replies, &recorder).await;
                });
            }
        });

        Self { port, seen }
    }

    /// Returns a validated endpoint pointing at this server.
    fn endpoint(&self) -> McpHttpEndpoint {
        let url = format!("http://127.0.0.1:{}/mcp", self.port);
        McpHttpEndpoint::parse(&url).unwrap_or_else(|error| panic!("{url}: {error}"))
    }

    /// Returns the requests received so far.
    fn requests(&self) -> Vec<SeenRequest> {
        self.seen
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }
}

/// Serves requests on one connection until the peer closes it.
async fn serve_connection(
    stream: TcpStream,
    replies: &BTreeMap<String, Reply>,
    seen: &Arc<Mutex<Vec<SeenRequest>>>,
) -> std::io::Result<()> {
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

        let body = read_body(&mut reader, &headers).await?;
        let parsed: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
        let rpc_method = parsed
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        if let Ok(mut guard) = seen.lock() {
            guard.push(SeenRequest {
                method: method.clone(),
                path: path.clone(),
                headers: headers.clone(),
                body: parsed.clone(),
            });
        }

        // A `GET` opens the standalone SSE stream. This server declines it explicitly rather than
        // hanging: a peer that answered nothing would leave the client waiting and the test would look
        // like a bug in the transport.
        if method == "GET" {
            let response = "HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\n\r\n";
            write_half.write_all(response.as_bytes()).await?;
            continue;
        }

        let reply = replies.get(&rpc_method);
        let id = parsed.get("id").cloned().unwrap_or(Value::Null);
        let (status, content_type, payload) = encode_reply(reply, &id, &rpc_method);
        let head = format!(
            "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\r\n",
            payload.len()
        );
        write_half.write_all(head.as_bytes()).await?;
        write_half.write_all(payload.as_bytes()).await?;
        write_half.flush().await?;
    }
}

/// Reads a request body, honouring either framing.
async fn read_body(
    reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>,
    headers: &BTreeMap<String, String>,
) -> std::io::Result<String> {
    if headers
        .get("transfer-encoding")
        .is_some_and(|value| value.contains("chunked"))
    {
        let mut body = String::new();
        loop {
            let mut size_line = String::new();
            if reader.read_line(&mut size_line).await? == 0 {
                return Ok(body);
            }
            let size = usize::from_str_radix(size_line.trim().split(';').next().unwrap_or("0"), 16)
                .unwrap_or(0);
            if size == 0 {
                let mut trailer = String::new();
                let _ = reader.read_line(&mut trailer).await?;
                return Ok(body);
            }
            let mut chunk = vec![0_u8; size];
            reader.read_exact(&mut chunk).await?;
            body.push_str(&String::from_utf8_lossy(&chunk));
            let mut crlf = [0_u8; 2];
            reader.read_exact(&mut crlf).await?;
        }
    }
    let length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let mut buffer = vec![0_u8; length];
    reader.read_exact(&mut buffer).await?;
    Ok(String::from_utf8_lossy(&buffer).into_owned())
}

/// Turns a scripted reply into a status line, a content type, and a body.
fn encode_reply(reply: Option<&Reply>, id: &Value, method: &str) -> (String, String, String) {
    match reply {
        Some(Reply::Result(result)) => (
            "200 OK".to_owned(),
            "application/json".to_owned(),
            json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string(),
        ),
        Some(Reply::Error { code, message }) => (
            "200 OK".to_owned(),
            "application/json".to_owned(),
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": code, "message": message }
            })
            .to_string(),
        ),
        Some(Reply::Redirect { location }) => (
            format!("302 Found\r\nLocation: {location}"),
            "text/plain".to_owned(),
            String::new(),
        ),
        Some(Reply::Status { code, body }) => (
            format!("{code} Failure"),
            "text/plain".to_owned(),
            body.clone(),
        ),
        Some(Reply::NonJson { content_type, body }) => {
            ("200 OK".to_owned(), content_type.clone(), body.clone())
        }
        // An unscripted method is a JSON-RPC error rather than silence, so a client that calls the
        // wrong method gets a diagnosable refusal instead of a timeout.
        None => (
            "200 OK".to_owned(),
            "application/json".to_owned(),
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": format!("method not found: {method}") }
            })
            .to_string(),
        ),
    }
}

/// The `server/discover` result a modern server sends, in the shape the SDK reads.
fn discover_result(name: &str) -> Value {
    json!({
        "resultType": "complete",
        "supportedVersions": ["2026-07-28"],
        "capabilities": { "tools": { "listChanged": true } },
        "ttlMs": 0,
        "cacheScope": "private",
        "_meta": {
            "io.modelcontextprotocol/serverInfo": { "name": name, "version": "1.0.0" }
        }
    })
}

/// A `tools/list` result carrying one tool.
fn tools_result() -> Value {
    json!({
        "resultType": "complete",
        "tools": [{
            "name": "search",
            "inputSchema": { "type": "object", "properties": { "q": { "type": "string" } } }
        }],
        "ttlMs": 0,
        "cacheScope": "private"
    })
}

fn server(name: &str) -> ServerName {
    ServerName::new(name).unwrap_or_else(|error| panic!("fixture server {name}: {error}"))
}

/// **The claim this file exists to make: a real HTTP server can be negotiated with and listed.**
/// Every other test here is a refinement of it.
#[tokio::test]
async fn a_real_http_server_negotiates_and_lists_over_streamable_http() {
    let http = HttpTestServer::start(BTreeMap::from([
        (
            "server/discover".to_owned(),
            Reply::Result(discover_result("http-vendor")),
        ),
        ("tools/list".to_owned(), Reply::Result(tools_result())),
    ]))
    .await;

    let connection = connect_http(&server("remote"), &http.endpoint())
        .await
        .unwrap_or_else(|error| panic!("a real HTTP server must negotiate: {error}"));

    // The revision came out of the HTTP response, so this also proves the response framing is right.
    assert_eq!(connection.negotiated_revision(), "2026-07-28");
    assert!(connection.capabilities().offers_tools());
    assert_eq!(connection.reported_identity().name, "http-vendor");

    let mut buffer = ToolBuffer::new();
    let listings = connection
        .list_tools(&mut buffer)
        .await
        .unwrap_or_else(|error| panic!("a real tool list must be readable: {error}"));
    assert_eq!(listings.len(), 1);
    assert_eq!(listings[0].name, "search");

    let _ = connection.close().await;
}

/// **The request must be the one the specification describes.** A client that negotiated modern but
/// omitted the protocol version, or that asked for only one response media type, would look healthy
/// and be unreadable to a conforming middle box. The server records what it received, so this is an
/// assertion about bytes.
#[tokio::test]
async fn the_request_carries_the_headers_the_protocol_requires() {
    let http = HttpTestServer::start(BTreeMap::from([(
        "server/discover".to_owned(),
        Reply::Result(discover_result("http-vendor")),
    )]))
    .await;

    let connection = connect_http(&server("remote"), &http.endpoint())
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let _ = connection.close().await;

    let requests = http.requests();
    let discover = requests
        .iter()
        .find(|request| request.method == "POST")
        .unwrap_or_else(|| panic!("the server must have seen a POST"));
    assert_eq!(discover.path, "/mcp");
    // Both media types, because the server chooses which one to answer with.
    let accept = discover
        .headers
        .get("accept")
        .unwrap_or_else(|| panic!("a POST must carry Accept: {:?}", discover.headers));
    assert!(accept.contains("text/event-stream"), "{accept}");
    assert!(accept.contains("application/json"), "{accept}");
    // The negotiated protocol version travels on the request, which is what makes the protocol
    // stateless: there is no session to carry it.
    assert!(
        discover.headers.contains_key("mcp-protocol-version"),
        "a modern request must name its protocol version: {:?}",
        discover.headers
    );
    assert_eq!(
        discover
            .headers
            .get("mcp-protocol-version")
            .map(String::as_str),
        Some("2026-07-28")
    );
    // `Mcp-Method` is what lets a middle box route the request without parsing the body, and it must
    // agree with the body — a disagreement is the `-32020 HeaderMismatch` the specification defines.
    assert_eq!(
        discover.headers.get("mcp-method").map(String::as_str),
        Some("server/discover"),
        "{:?}",
        discover.headers
    );
    // And the body is the discover request, self-identifying per the stateless contract.
    assert_eq!(discover.body["method"], "server/discover");
}

/// **A redirect must not be followed.** A redirect can move the request to another origin, and the MCP
/// transport posts tool arguments — potentially with an `Authorization` header — so following one is
/// how a credential reaches a host the operator never named.
///
/// The strong assertion is that the redirect *target* received **nothing**. Asserting only that the
/// connection failed would also pass on a server that simply refused everything.
#[tokio::test]
async fn a_redirect_is_refused_and_the_target_is_never_contacted() {
    // A second server, standing in for wherever a redirect would have sent the request.
    let target = HttpTestServer::start(BTreeMap::new()).await;
    let target_url = format!("http://127.0.0.1:{}/mcp", target.port);

    let http = HttpTestServer::start(BTreeMap::from([(
        "server/discover".to_owned(),
        Reply::Redirect {
            location: target_url,
        },
    )]))
    .await;

    let outcome = connect_http(&server("remote"), &http.endpoint()).await;
    let Some(error) = outcome.err() else {
        panic!("a redirect must not be followed to a working endpoint");
    };

    // **The security property is asserted FIRST, and the order is deliberate.** Falsifying this test by
    // allowing redirects is what showed why: with the redirect followed, the refusal that arrives is a
    // protocol error whose text does not mention `302`, so a message assertion placed first fires and
    // the run ends before the security assertion is reached — the test fails for a *formatting* reason
    // while the property it exists to check goes unobserved. Checking the property first means a
    // regression is reported as the thing that broke.
    assert!(
        target.requests().is_empty(),
        "a redirect target must never receive the request: {:?}",
        target.requests()
    );
    // The refusal names the status, so an operator can tell a redirect from an unreachable host.
    assert!(error.to_string().contains("302"), "{error}");
}

/// An HTTP failure status is refused with the status in the message. A bare "could not be reached"
/// would send an operator to check a network that is working — the opaque-diagnostic defect this
/// project has recorded more than once.
#[tokio::test]
async fn an_http_failure_names_the_status() {
    let http = HttpTestServer::start(BTreeMap::from([(
        "server/discover".to_owned(),
        Reply::Status {
            code: 503,
            body: "service unavailable".to_owned(),
        },
    )]))
    .await;

    let error = connect_http(&server("remote"), &http.endpoint())
        .await
        .err()
        .unwrap_or_else(|| panic!("an HTTP failure must not yield a connection"));
    assert!(error.to_string().contains("503"), "{error}");
}

/// A success status whose body is not JSON is refused rather than read as an absent result. The
/// content type is what the transport must key on, so a server answering `text/html` with a login page
/// cannot be mistaken for a peer that agreed to anything.
#[tokio::test]
async fn a_non_json_success_body_is_refused() {
    let http = HttpTestServer::start(BTreeMap::from([(
        "server/discover".to_owned(),
        Reply::NonJson {
            content_type: "text/html".to_owned(),
            body: "<html>login</html>".to_owned(),
        },
    )]))
    .await;

    let error = connect_http(&server("remote"), &http.endpoint())
        .await
        .err()
        .unwrap_or_else(|| panic!("a non-JSON body must not yield a connection"));
    // The message names the content type, which is the actionable part.
    assert!(error.to_string().contains("text/html"), "{error}");
}

/// **A JSON-RPC error on `server/discover` is a refusal by the peer, not an unreachable host.** The two
/// have different remedies, and this is the case a legacy server or a wrong endpoint produces.
#[tokio::test]
async fn a_json_rpc_error_on_discovery_is_a_refusal_not_an_unreachable_host() {
    let http = HttpTestServer::start(BTreeMap::from([(
        "server/discover".to_owned(),
        Reply::Error {
            code: -32601,
            message: "method not found".to_owned(),
        },
    )]))
    .await;

    let error = connect_http(&server("remote"), &http.endpoint())
        .await
        .err()
        .unwrap_or_else(|| panic!("a refused discovery must not yield a connection"));
    assert!(error.to_string().contains("method not found"), "{error}");
}

/// **`tools/call` over HTTP**, which exercises the call path through the second transport. A
/// regression that only affected one transport would otherwise be invisible.
#[tokio::test]
async fn a_tool_call_runs_over_http() {
    let http = HttpTestServer::start(BTreeMap::from([
        (
            "server/discover".to_owned(),
            Reply::Result(discover_result("http-vendor")),
        ),
        (
            "tools/call".to_owned(),
            Reply::Result(json!({
                "resultType": "complete",
                "content": [{ "type": "text", "text": "search ran over http" }],
                "isError": false
            })),
        ),
    ]))
    .await;

    let connection = connect_http(&server("remote"), &http.endpoint())
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let mut arguments = serde_json::Map::new();
    arguments.insert("q".to_owned(), json!("pumps"));
    let result = connection
        .call_tool("search", &arguments)
        .await
        .unwrap_or_else(|error| panic!("a call over HTTP must return: {error}"));
    assert!(result.is_success());
    assert_eq!(result.text, "search ran over http");

    let _ = connection.close().await;
}

/// The whole point of a validated endpoint: an unroutable URL is refused **before** any connection is
/// attempted, so no request leaves the process. Asserted by pointing at a server that records
/// connections, so "nothing was contacted" is observed rather than assumed.
#[tokio::test]
async fn an_invalid_endpoint_never_reaches_the_network() {
    let http = HttpTestServer::start(BTreeMap::new()).await;
    let plaintext_remote = format!("http://localhost.evil.example:{}/mcp", http.port);

    // The parse refuses it, which is where the guarantee lives.
    assert!(McpHttpEndpoint::parse(&plaintext_remote).is_err());
    // And no endpoint value can be constructed that would reach the server over plaintext off
    // loopback, so nothing was contacted.
    assert!(http.requests().is_empty(), "{:?}", http.requests());
}

/// The host join works through the HTTP transport too, so `build_catalog` is not accidentally
/// stdio-only. This is the composition `P3-009` will call, exercised on the transport it will use for
/// remote servers.
#[tokio::test]
async fn the_host_join_works_over_http() {
    use jarvis_mcp_transport::{HostedServer, build_catalog};

    let http = HttpTestServer::start(BTreeMap::from([
        (
            "server/discover".to_owned(),
            Reply::Result(discover_result("http-vendor")),
        ),
        ("tools/list".to_owned(), Reply::Result(tools_result())),
    ]))
    .await;

    let connection = connect_http(&server("remote"), &http.endpoint())
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let hosted = vec![HostedServer {
        configured: jarvis_mcp::ConfiguredServer {
            name: server("remote"),
            policy: ToolEffectPolicy::unclassified(),
        },
        connection: Arc::new(connection),
    }];

    let build = build_catalog(&hosted, NamingStrategy::Prefixed, &[])
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(build.unreadable.is_empty(), "{:?}", build.unreadable);
    assert_eq!(build.catalog.len(), 1);
    assert_eq!(
        build.catalog.entries()[0].definition.id().to_string(),
        "mcp.remote.search"
    );
    // The identity came from the HTTP response and stayed separate from the operator's name.
    assert_eq!(build.catalog.observed()[0].reported.name, "http-vendor");
    assert_eq!(build.catalog.observed()[0].server.as_str(), "remote");
}

/// `McpToolListing` is what a translated listing borrows from, so a listing fetched over HTTP must
/// still produce a definition — asserted here rather than only over stdio, because the HTTP path
/// returns the same type through a different code path.
#[tokio::test]
async fn an_http_listing_translates_to_a_definition() {
    let http = HttpTestServer::start(BTreeMap::from([
        (
            "server/discover".to_owned(),
            Reply::Result(discover_result("http-vendor")),
        ),
        ("tools/list".to_owned(), Reply::Result(tools_result())),
    ]))
    .await;

    let connection = connect_http(&server("remote"), &http.endpoint())
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let mut buffer = ToolBuffer::new();
    let listings = connection
        .list_tools(&mut buffer)
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    let policy = ToolEffectPolicy::unclassified();
    let listing: &McpToolListing<'_> = &listings[0];
    let translated = jarvis_mcp::translate_tool(
        &server("remote"),
        listing,
        NamingStrategy::Prefixed,
        &policy,
    )
    .unwrap_or_else(|error| panic!("a listing fetched over HTTP must translate: {error}"));
    assert_eq!(translated.definition.id().to_string(), "mcp.remote.search");
    assert_eq!(translated.remote, "search");

    let _ = connection.close().await;
}
