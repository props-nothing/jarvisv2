//! The handler, driven through the **real** SDK Streamable HTTP service.
//!
//! # Why this is crate-internal rather than an integration test
//!
//! `P3-009e` tested the refusal rule by calling `JarvisMcpServer::invoke` directly, which proves the rule but
//! **not that the SDK ever routes a request to it**: the trait methods `list_tools` and `call_tool` had never
//! been executed, so a signature or response-shape mismatch would have compiled and been wrong.
//!
//! Proving it needs the SDK's service, and this crate's invariant is that **no provider SDK type appears in its
//! public surface** (`repository-layout.md`, from `AGENTS.md`). An integration test sees only that public
//! surface, so the choice was a public accessor returning an SDK type — which would break the invariant — or
//! moving the proof inside the crate, where the SDK is legitimately in scope. This is that second option, and it
//! is why the file is a `#[cfg(test)]` module rather than `tests/serving.rs`.
//!
//! The distinction the invariant cares about is between **an internal module compiling against the SDK** and
//! **the SDK being part of this crate's contract**. A crate-internal test is the first; a `pub fn` returning
//! `StreamableHttpService` is the second, and it is what `P3-009b` originally did and this corrects.

use std::sync::Arc;

use async_trait::async_trait;
use http_body_util::BodyExt;
use jarvis_core::{CorrelationId, Sensitivity, SystemClock, UtcTimestamp};
use jarvis_mcp::ServerExposure;
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::serve::{JarvisMcpServer, ServedToolRunner, served_protocol_version};
use crate::serving::{MCP_ENDPOINT_PATH, ServingConfig};

/// A runner that answers a fixed confirmation and counts its calls.
struct CountingRunner {
    calls: std::sync::Mutex<usize>,
}

impl CountingRunner {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: std::sync::Mutex::new(0),
        })
    }

    fn count(&self) -> usize {
        *self
            .calls
            .lock()
            .unwrap_or_else(|error| panic!("the counter is usable: {error}"))
    }
}

#[async_trait]
impl ServedToolRunner for CountingRunner {
    async fn run(
        &self,
        _name: &str,
        _arguments: Value,
        _correlation_id: CorrelationId,
    ) -> Result<jarvis_tools::ToolCallResult, jarvis_tools::AdapterError> {
        let mut count = self
            .calls
            .lock()
            .unwrap_or_else(|error| panic!("the counter is usable: {error}"));
        *count += 1;
        let record = jarvis_core::ToolOutcomeRecord::confirmed("jarvis:test")
            .unwrap_or_else(|error| panic!("{error}"));
        Ok(jarvis_tools::ToolCallResult::new(
            record,
            Some(
                jarvis_tools::ProviderEvidence::new("jarvis:test")
                    .unwrap_or_else(|error| panic!("{error}")),
            ),
            Some(jarvis_tools::BoundedOutput::truncating(
                "the file contents".to_owned(),
            )),
            UtcTimestamp::now(&SystemClock),
        ))
    }
}

/// A definition for a tool this project owns, so the served set is non-empty.
fn definition(name: &str) -> jarvis_tools::ToolDefinition {
    jarvis_tools::ToolDefinition::new(jarvis_tools::ToolDefinitionParts {
        id: jarvis_tools::ToolId::new(name).unwrap_or_else(|error| panic!("{name}: {error}")),
        version: "1.0.0".to_owned(),
        title: format!("title for {name}"),
        description: format!("description for {name}"),
        input_schema: jarvis_tools::ToolSchema::parse(
            &json!({
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"],
                "additionalProperties": false
            })
            .to_string(),
        )
        .unwrap_or_else(|error| panic!("{error}")),
        output_schema: jarvis_tools::ToolSchema::parse(
            &json!({
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": { "contents": { "type": "string" } },
                "required": ["contents"],
                "additionalProperties": false
            })
            .to_string(),
        )
        .unwrap_or_else(|error| panic!("{error}")),
        effects: jarvis_tools::EffectSet::single(jarvis_tools::ToolEffect::ReadOnly),
        risk: 0,
        required_scopes: jarvis_tools::ScopeSet::none(),
        approval: jarvis_tools::ApprovalPolicy::Auto,
        timeout_seconds: 10,
        retry: jarvis_tools::RetryDeclaration::none(),
        idempotency: jarvis_tools::Idempotency::Unsupported,
        // Derived, because `ToolDefinition::new` refuses a source that disagrees with the namespace.
        source: jarvis_tools::ToolSource::from_namespace(
            name.rsplit_once('.')
                .map_or(name, |(namespace, _)| namespace),
        ),
        availability: jarvis_tools::Availability::Available,
        sensitivity: jarvis_tools::ToolSensitivity::new(
            Sensitivity::Internal,
            Sensitivity::Internal,
        ),
    })
    .unwrap_or_else(|error| panic!("{name}: {error}"))
}

/// A handler over one served tool.
///
/// Takes the concrete runner rather than `Arc<dyn ServedToolRunner>`, so a call site cannot double-wrap it —
/// `Arc<Arc<CountingRunner>>` does not implement the port, and that is a compile error rather than a silent
/// loss of the counter a test asserts on.
fn handler(runner: Arc<CountingRunner>) -> JarvisMcpServer {
    let definition = definition("jarvis.files.read");
    let (served, exclusions) =
        jarvis_mcp::served_tools([&definition]).unwrap_or_else(|error| panic!("{error}"));
    assert!(exclusions.is_empty(), "{exclusions:?}");
    JarvisMcpServer::new(served, runner)
}

/// The default serving configuration: loopback only, no browser origin admitted.
fn config() -> ServingConfig {
    ServingConfig::loopback_only()
}

/// Posts one JSON-RPC body and returns the status, the parsed JSON when there is one, and the raw text.
///
/// `version` is the `MCP-Protocol-Version` header. It is a parameter rather than a constant because one test
/// asserts the difference between sending it and omitting it.
async fn post(
    config: &ServingConfig,
    server: JarvisMcpServer,
    body: Value,
    version: Option<&str>,
) -> (http::StatusCode, Option<Value>, String) {
    let service = config.sdk_service_for_test(server);

    let mut builder = http::Request::builder()
        .method("POST")
        .uri(MCP_ENDPOINT_PATH)
        .header("host", "localhost")
        .header("content-type", "application/json")
        // **Both media types, because the revision requires the client to accept both.** Sending only
        // `application/json` is answered `406 Not Acceptable`, which is the SDK enforcing that rule — a fact
        // this fixture discovered rather than assumed.
        .header("accept", "application/json, text/event-stream");
    if let Some(version) = version {
        builder = builder.header("mcp-protocol-version", version);
    }
    // The standard request headers the revision requires. `Mcp-Name` is sent only for `tools/call`, because
    // the SDK rejects a `Mcp-Name` that has no body value to match.
    if let Some(method) = body.get("method").and_then(Value::as_str) {
        builder = builder.header("mcp-method", method);
        if method == "tools/call"
            && let Some(name) = body
                .get("params")
                .and_then(|params| params.get("name"))
                .and_then(Value::as_str)
        {
            builder = builder.header("mcp-name", name);
        }
    }
    let request = builder
        .body(body.to_string())
        .unwrap_or_else(|error| panic!("{error}"));

    let response = service
        .oneshot(request)
        .await
        .unwrap_or_else(|error| panic!("the service is infallible: {error:?}"));

    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .unwrap_or_else(|error| panic!("reading the body is infallible: {error:?}"))
        .to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();
    let parsed = serde_json::from_str::<Value>(&text).ok();
    (status, parsed, text)
}

/// A `tools/list` request body, with the per-request metadata the revision requires.
fn list_tools_body() -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list",
        "params": {
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": served_protocol_version(),
                "io.modelcontextprotocol/clientInfo": { "name": "test-client", "version": "1.0.0" },
                "io.modelcontextprotocol/clientCapabilities": {}
            }
        }
    })
}

/// A `tools/call` request body for one tool name.
fn call_tool_body(name: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": name,
            "arguments": { "path": "notes/todo.txt" },
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": served_protocol_version(),
                "io.modelcontextprotocol/clientInfo": { "name": "test-client", "version": "1.0.0" },
                "io.modelcontextprotocol/clientCapabilities": {}
            }
        }
    })
}

/// **`tools/list` reaches the handler through the SDK and answers the served set.**
///
/// This is the test that closes `P3-009b`'s recorded gap: `list_tools` was compiled but never called, so a
/// signature or response mismatch would have gone unnoticed until a client connected.
#[tokio::test]
async fn a_tools_list_request_is_answered_by_the_served_set() {
    let (status, body, raw) = post(
        &config(),
        handler(CountingRunner::new()),
        list_tools_body(),
        Some(served_protocol_version()),
    )
    .await;

    assert_eq!(status, http::StatusCode::OK, "raw body: {raw}");
    let body = body.unwrap_or_else(|| panic!("a tools/list answer must be JSON, got: {raw}"));
    let tools = body["result"]["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("the result must carry a tool array, got: {body}"));
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["name"], "jarvis.files.read");
    // The schema travels as declared, so the client sees the contract the daemon enforces.
    assert_eq!(tools[0]["inputSchema"]["required"], json!(["path"]));
    // And no annotations, which the protocol declares untrusted.
    assert!(tools[0].get("annotations").is_none(), "got: {}", tools[0]);
}

/// **A `tools/call` for a served tool runs and returns the output.**
///
/// The positive control for the refusal below: without it, "the unserved call was refused" would pass even if
/// every call were refused.
#[tokio::test]
async fn a_served_tools_call_runs_and_returns_its_output() {
    let runner = CountingRunner::new();
    let (status, body, raw) = post(
        &config(),
        handler(runner.clone()),
        call_tool_body("jarvis.files.read"),
        Some(served_protocol_version()),
    )
    .await;

    assert_eq!(status, http::StatusCode::OK, "raw body: {raw}");
    let body = body.unwrap_or_else(|| panic!("a tools/call answer must be JSON, got: {raw}"));
    assert_ne!(
        body["result"]["isError"],
        json!(true),
        "the call must not be reported as an error, got: {body}"
    );
    let text = body["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("the output must travel as content, got: {body}"));
    assert!(text.contains("the file contents"), "got: {text}");
    assert_eq!(runner.count(), 1, "the runner must have run exactly once");
}

/// **An unadvertised tool is refused over the real service, and the runner is never reached.**
///
/// The same security property `P3-009e` pinned at the handler, now proven through the SDK's routing — because
/// the property is about what a *client* can do, and a client reaches the handler only this way.
#[tokio::test]
async fn an_unadvertised_tools_call_is_refused_over_the_service() {
    let runner = CountingRunner::new();
    let (status, body, raw) = post(
        &config(),
        handler(runner.clone()),
        call_tool_body("mcp.github.search"),
        Some(served_protocol_version()),
    )
    .await;

    // A refused call is a **result about that call**, not a protocol error, so the status is 200 and the
    // refusal is in the result — which is how the caller can read it at all.
    assert_eq!(status, http::StatusCode::OK, "raw body: {raw}");
    let body = body.unwrap_or_else(|| panic!("a refusal must be JSON, got: {raw}"));
    assert_eq!(
        body["result"]["isError"],
        json!(true),
        "an unadvertised tool must be refused, got: {body}"
    );
    assert_eq!(
        runner.count(),
        0,
        "the runner must not be reached for an unadvertised tool"
    );
}

/// A request whose body carries a per-request protocol version but whose header is absent is refused.
///
/// **The comment here was wrong when first written, and a falsification caught it.** Removing
/// `stateless_protocol_metadata_required(true)` did **not** make this test fail, because the refusal comes from
/// the SDK's own body-versus-header consistency rule — a body `_meta` protocol version requires the matching
/// header whatever that flag says. So this test pins the *header agreement* rule, and
/// `a_request_with_no_protocol_signals_at_all_is_refused` is the one that isolates the flag.
#[tokio::test]
async fn a_request_without_a_protocol_version_header_is_refused() {
    let (status, _, raw) = post(
        &config(),
        handler(CountingRunner::new()),
        list_tools_body(),
        None,
    )
    .await;

    assert_eq!(
        status,
        http::StatusCode::BAD_REQUEST,
        "an absent protocol-version header must be refused, got {status}: {raw}"
    );
    assert!(
        raw.contains("32020"),
        "the refusal must be the header-mismatch error, got: {raw}"
    );
}

/// **A request with no protocol signals at all is refused, and this is the test that isolates
/// `stateless_protocol_metadata_required(true)`.**
///
/// Neither the header nor a body `_meta` version is present, so the SDK's body-versus-header rule has nothing
/// to compare — which leaves the flag as the only thing that can refuse it. Falsified by removing that line
/// from `ServingConfig::sdk`: this request is then routed by the legacy rule, so the refusal changes.
#[tokio::test]
async fn a_request_with_no_protocol_signals_at_all_is_refused() {
    let body = json!({
        "jsonrpc": "2.0",
        "id": 5,
        "method": "tools/list",
        "params": {}
    });
    let (status, _, raw) = post(&config(), handler(CountingRunner::new()), body, None).await;

    assert_eq!(
        status,
        http::StatusCode::BAD_REQUEST,
        "a request with no protocol signals must be refused, got {status}: {raw}"
    );
    assert!(
        raw.contains("32020"),
        "the refusal must be the header-mismatch error, got: {raw}"
    );
}

/// **A hostile `Origin` is refused by JARVIS's policy, and the check is the one that answered.**
///
/// The SDK's own origin validation is disabled in `ServingConfig::sdk`, so this proves the *policy* is the
/// control: the same decision `P3-009a`'s tests pin is the one in force here. The enforcement layer over a
/// request is `P3-009c`'s, so what is asserted is the decision rather than a status code.
#[tokio::test]
async fn a_hostile_origin_is_refused_by_the_policy() {
    let config = config();
    for hostile in [
        "https://jarvis.example.com",
        "http://localhost.evil.test",
        "null",
        "not an origin",
    ] {
        assert!(
            !config.origin_check(Some(hostile)).permits(),
            "{hostile} must be refused"
        );
    }
    // The positive control: an absent Origin is what a local tool sends, and it is admitted.
    assert!(config.origin_check(None).permits());
}

/// **An `Origin` from this host's own page is admitted, so the policy is not simply refusing everything.**
#[tokio::test]
async fn a_configured_loopback_origin_is_admitted() {
    let exposure =
        ServerExposure::new(["http://localhost:3000"]).unwrap_or_else(|error| panic!("{error}"));
    let config = ServingConfig::new(exposure).unwrap_or_else(|error| panic!("{error}"));

    assert!(config.origin_check(Some("http://localhost:3000")).permits());
    assert!(!config.origin_check(Some("http://localhost:3001")).permits());
}

/// **`server/discover` answers with the modern revision, because the handler narrowed it.**
///
/// The SDK's default would advertise `LATEST` = `2025-11-25`, so this is the functional form of `P3-009e`'s
/// override: the value a client actually reads is the one this build implements.
#[tokio::test]
async fn discovering_the_server_reports_the_modern_revision() {
    let body = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "server/discover",
        "params": {
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": served_protocol_version(),
                "io.modelcontextprotocol/clientInfo": { "name": "test-client", "version": "1.0.0" },
                "io.modelcontextprotocol/clientCapabilities": {}
            }
        }
    });
    let (status, body, raw) = post(
        &config(),
        handler(CountingRunner::new()),
        body,
        Some(served_protocol_version()),
    )
    .await;

    assert_eq!(status, http::StatusCode::OK, "raw body: {raw}");
    let body = body.unwrap_or_else(|| panic!("a discover answer must be JSON, got: {raw}"));
    // The supported versions the server advertises, which is where the narrowing is visible to a client.
    let text = body.to_string();
    assert!(
        text.contains(served_protocol_version()),
        "the modern revision must be advertised, got: {body}"
    );
    assert!(
        !text.contains("2025-11-25"),
        "the legacy revision must not be advertised, got: {body}"
    );
}

/// A `tools/call` with no `Mcp-Name` header is refused as a header mismatch rather than reaching the handler,
/// which is the SDK's standard-header validation doing its job.
#[tokio::test]
async fn a_call_without_the_name_header_is_refused_as_a_header_mismatch() {
    // Built by hand rather than through `post`, because that helper sends the `Mcp-Name` header this test is
    // about omitting.
    let service = config().sdk_service_for_test(handler(CountingRunner::new()));
    let request = http::Request::builder()
        .method("POST")
        .uri(MCP_ENDPOINT_PATH)
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", served_protocol_version())
        .header("mcp-method", "tools/call")
        .body(call_tool_body("jarvis.files.read").to_string())
        .unwrap_or_else(|error| panic!("{error}"));

    let response = service
        .oneshot(request)
        .await
        .unwrap_or_else(|error| panic!("the service is infallible: {error:?}"));
    assert_eq!(
        response.status(),
        http::StatusCode::BAD_REQUEST,
        "a missing Mcp-Name header must be refused"
    );
}

/// A `GET` to the endpoint is `405`, because the revision defines no GET stream — POST is the only method.
///
/// `P3-009a` recorded this as a spec obligation; this is it proven against the real service.
#[tokio::test]
async fn a_get_to_the_endpoint_is_method_not_allowed() {
    let service = config().sdk_service_for_test(handler(CountingRunner::new()));
    let request = http::Request::builder()
        .method("GET")
        .uri(MCP_ENDPOINT_PATH)
        .header("host", "localhost")
        .body(String::new())
        .unwrap_or_else(|error| panic!("{error}"));

    let response = service
        .oneshot(request)
        .await
        .unwrap_or_else(|error| panic!("the service is infallible: {error:?}"));
    assert_eq!(response.status(), http::StatusCode::METHOD_NOT_ALLOWED);
}

/// **The request-body bound is enforced.** An oversized body is refused rather than read.
///
/// Falsified by removing `with_max_request_body_bytes` from `ServingConfig::sdk`: the SDK's own default is
/// used, which nothing in this repository stated. The falsification showed `left: 200, right: 413`.
#[tokio::test]
async fn an_oversized_request_body_is_refused() {
    let config = config().with_max_request_body_bytes(256);
    let padding = "x".repeat(1024);
    let body = json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "tools/call",
        "params": {
            "name": "jarvis.files.read",
            "arguments": { "path": padding },
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": served_protocol_version(),
                "io.modelcontextprotocol/clientInfo": { "name": "test-client", "version": "1.0.0" },
                "io.modelcontextprotocol/clientCapabilities": {}
            }
        }
    });

    let service = config.sdk_service_for_test(handler(CountingRunner::new()));
    let request = http::Request::builder()
        .method("POST")
        .uri(MCP_ENDPOINT_PATH)
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", served_protocol_version())
        .header("mcp-method", "tools/call")
        .header("mcp-name", "jarvis.files.read")
        .body(body.to_string())
        .unwrap_or_else(|error| panic!("{error}"));

    let response = service
        .oneshot(request)
        .await
        .unwrap_or_else(|error| panic!("the service is infallible: {error:?}"));
    assert_eq!(
        response.status(),
        http::StatusCode::PAYLOAD_TOO_LARGE,
        "an oversized body must be refused"
    );
}
