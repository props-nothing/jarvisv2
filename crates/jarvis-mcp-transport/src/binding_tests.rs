//! The binding layer, driven by **real requests** through the real SDK service.
//!
//! # What these tests are for
//!
//! `P3-009i` proved the gate decides correctly when handed four values. It could not prove that anything
//! *derives* those values from a request, and that gap is where a binding layer goes wrong in a way no policy
//! test can see: a header read under the wrong name, an absent `Origin` collapsed into an admitted one, a
//! credential accepted without its scheme. Each of those leaves every `RequestGate` test green.
//!
//! So every test here builds an [`http::Request`] and drives [`ServedEndpoint::into_service`] with
//! `tower::ServiceExt::oneshot`, and the runner is a **counter** rather than a scripted answer: the question in
//! most of them is whether the handler ran at all, not what it returned.

use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use http_body_util::BodyExt;
use jarvis_core::{CorrelationId, SystemClock, UtcTimestamp};
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::admission::{
    AdmittedCaller, CallerAdmission, CallerLabel, DEFAULT_REQUESTS_PER_MINUTE, Fingerprint,
};
use crate::binding::{CREDENTIAL_HEADER, ServedEndpoint};
use crate::serve::JarvisMcpServer;
use crate::serving::{MCP_ENDPOINT_PATH, ServingConfig};
use jarvis_mcp::ServerExposure;

/// A credential digest that satisfies `Fingerprint::parse`'s alphabet.
const CALLER_DIGEST: &str = "9f2c41ab77de0355b1c8e0d4a63f29bb8c1740ee5d3a96f2c0b84a1e7d5633aa";

/// A runner that counts how many times it was reached.
struct CountingRunner {
    calls: Mutex<usize>,
}

impl CountingRunner {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(0),
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
impl crate::serve::ServedToolRunner for CountingRunner {
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
        source: jarvis_tools::ToolSource::from_namespace(
            name.rsplit_once('.')
                .map_or(name, |(namespace, _)| namespace),
        ),
        availability: jarvis_tools::Availability::Available,
        sensitivity: jarvis_tools::ToolSensitivity::new(
            jarvis_core::Sensitivity::Internal,
            jarvis_core::Sensitivity::Internal,
        ),
    })
    .unwrap_or_else(|error| panic!("{name}: {error}"))
}

/// A handler over one served tool.
fn handler(runner: Arc<CountingRunner>) -> JarvisMcpServer {
    let definition = definition("jarvis.files.read");
    let (served, exclusions) =
        jarvis_mcp::served_tools([&definition]).unwrap_or_else(|error| panic!("{error}"));
    assert!(exclusions.is_empty(), "{exclusions:?}");
    JarvisMcpServer::new(served, runner)
}

/// The fingerprint value every admitted test caller presents.
fn digest() -> Fingerprint {
    Fingerprint::parse(CALLER_DIGEST).unwrap_or_else(|error| panic!("{error}"))
}

/// An admission policy that admits exactly one credential, with a label for the audit record.
fn admitting(label: &str) -> CallerAdmission {
    let caller = AdmittedCaller::new(digest(), CallerLabel::new(label, "1.0"))
        .unwrap_or_else(|error| panic!("{error}"));
    CallerAdmission::new(vec![caller], DEFAULT_REQUESTS_PER_MINUTE)
        .unwrap_or_else(|error| panic!("{error}"))
}

/// A serving configuration that admits one loopback browser origin, plus `localhost` so a request can carry one.
fn serving_with_origin(origin: &str) -> ServingConfig {
    let exposure = ServerExposure::new([origin]).unwrap_or_else(|error| panic!("{error}"));
    ServingConfig::new(exposure).unwrap_or_else(|error| panic!("{error}"))
}

/// A `tools/list` body carrying the per-request metadata the revision requires.
fn list_tools_body() -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list",
        "params": {
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": crate::serve::served_protocol_version(),
                "io.modelcontextprotocol/clientInfo": { "name": "test-client", "version": "1.0.0" },
                "io.modelcontextprotocol/clientCapabilities": {}
            }
        }
    })
}

/// One request's optional headers, so a test states only what it varies.
struct Headers {
    origin: Option<String>,
    credential: Option<String>,
}

impl Headers {
    const fn none() -> Self {
        Self {
            origin: None,
            credential: None,
        }
    }

    fn origin(origin: &str) -> Self {
        Self {
            origin: Some(origin.to_owned()),
            ..Self::none()
        }
    }

    fn bearer(digest: &str) -> Self {
        Self {
            credential: Some(format!("Bearer {digest}")),
            ..Self::none()
        }
    }

    fn with_origin(mut self, origin: &str) -> Self {
        self.origin = Some(origin.to_owned());
        self
    }
}

/// Posts one `tools/list` through the endpoint and returns the status, the parsed body, and the raw text.
async fn post(
    endpoint: ServedEndpoint,
    headers: &Headers,
) -> (http::StatusCode, Option<Value>, String) {
    let mut builder = http::Request::builder()
        .method("POST")
        .uri(MCP_ENDPOINT_PATH)
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-method", "tools/list")
        .header(
            "mcp-protocol-version",
            crate::serve::served_protocol_version(),
        );
    if let Some(origin) = &headers.origin {
        builder = builder.header("origin", origin);
    }
    if let Some(credential) = &headers.credential {
        builder = builder.header(CREDENTIAL_HEADER, credential);
    }
    let request = builder
        .body(list_tools_body().to_string())
        .unwrap_or_else(|error| panic!("{error}"));

    let response = endpoint
        .into_service::<String>()
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

/// Builds an endpoint over one served tool.
fn endpoint(
    serving: ServingConfig,
    admission: CallerAdmission,
    runner: &Arc<CountingRunner>,
) -> ServedEndpoint {
    ServedEndpoint::new(serving, admission, handler(runner.clone()))
        .unwrap_or_else(|error| panic!("{error}"))
}

/// **A remote caller with no credential is refused, and the handler does not run.**
///
/// The default policy admits nobody remotely, which is the same decision `CallerAdmission::local_only` states,
/// now enforced over a real request. The counter is the assertion that matters: a refusal *after* the handler
/// ran is a report, not a control.
#[tokio::test]
async fn a_remote_caller_with_no_credential_is_refused_before_the_handler_runs() {
    let runner = CountingRunner::new();
    let endpoint = endpoint(
        ServingConfig::loopback_only(),
        CallerAdmission::local_only(),
        &runner,
    );

    let (status, _, raw) = post(endpoint, &Headers::none()).await;

    assert_eq!(
        status,
        http::StatusCode::UNAUTHORIZED,
        "an anonymous remote caller must be refused, got {status}: {raw}"
    );
    assert_eq!(runner.count(), 0, "the handler must not have run");
}

/// **A hostile `Origin` is refused even when the credential would have been admitted.**
///
/// This is `P3-009i`'s order rule seen from the request layer: the origin is checked first, so a request that
/// fails both is an origin refusal. The falsification is the same one — swapping the two blocks in
/// `RequestGate::decide` — and this test would then see `401` instead of `403`.
#[tokio::test]
async fn a_hostile_origin_is_refused_even_with_an_admitted_credential() {
    let runner = CountingRunner::new();
    let endpoint = endpoint(
        serving_with_origin("http://localhost:3000"),
        admitting("vscode"),
        &runner,
    );

    let headers = Headers::bearer(CALLER_DIGEST).with_origin("https://jarvis.example.com");
    let (status, _, raw) = post(endpoint, &headers).await;

    assert_eq!(
        status,
        http::StatusCode::FORBIDDEN,
        "a hostile origin must be refused with 403, got {status}: {raw}"
    );
    assert_eq!(runner.count(), 0, "the handler must not have run");
}

/// **A request that fails both policies is reported as an origin refusal.**
///
/// No credential *and* a hostile origin. Both checks refuse, so the status is decided entirely by the order,
/// which is why this is a separate test from the one above: there, only the origin failed.
#[tokio::test]
async fn a_request_that_fails_both_policies_is_reported_as_an_origin_refusal() {
    let runner = CountingRunner::new();
    let endpoint = endpoint(
        serving_with_origin("http://localhost:3000"),
        admitting("vscode"),
        &runner,
    );

    let headers = Headers::origin("https://jarvis.example.com");
    let (status, _, raw) = post(endpoint, &headers).await;

    assert_eq!(
        status,
        http::StatusCode::FORBIDDEN,
        "the origin verdict must win when both fail, got {status}: {raw}"
    );
}

/// **An admitted remote caller with an allowed origin reaches the handler.**
///
/// The positive control for every refusal above: without it, a layer that refused everything would satisfy them
/// all. It also proves the credential's **shape** was read correctly — a `Bearer` scheme carrying a digest.
#[tokio::test]
async fn an_admitted_remote_caller_reaches_the_handler() {
    let runner = CountingRunner::new();
    let endpoint = endpoint(
        serving_with_origin("http://localhost:3000"),
        admitting("vscode"),
        &runner,
    );

    let headers = Headers::bearer(CALLER_DIGEST).with_origin("http://localhost:3000");
    let (status, body, raw) = post(endpoint, &headers).await;

    assert_eq!(status, http::StatusCode::OK, "raw body: {raw}");
    let body = body.unwrap_or_else(|| panic!("a tools/list answer must be JSON, got: {raw}"));
    assert_eq!(
        body["result"]["tools"][0]["name"], "jarvis.files.read",
        "got: {body}"
    );
}

/// **An `Authorization` value with no `Bearer` scheme is not a credential, so it does not admit.**
///
/// Pins the scheme requirement. Without it, a bare digest would be accepted — a value no real HTTP client
/// sends, and one whose acceptance would widen the set of strings a caller can present as a credential.
#[tokio::test]
async fn a_credential_without_the_bearer_scheme_is_not_recognised() {
    let runner = CountingRunner::new();

    // The correct digest, with no scheme at all — and then with the wrong scheme. Each is a separate request
    // through a freshly built endpoint, because `into_service` consumes the endpoint.
    for value in [CALLER_DIGEST.to_owned(), format!("Basic {CALLER_DIGEST}")] {
        let built = endpoint(ServingConfig::loopback_only(), admitting("vscode"), &runner);
        let headers = Headers {
            credential: Some(value.clone()),
            ..Headers::none()
        };
        let (status, _, raw) = post(built, &headers).await;
        assert_eq!(
            status,
            http::StatusCode::UNAUTHORIZED,
            "{value} must not be accepted as a credential, got {status}: {raw}"
        );
    }
    assert_eq!(runner.count(), 0, "the handler must not have run");
}

/// **The scheme comparison is case-insensitive, so a client spelling `bearer` is not refused for spelling.**
///
/// The counterpart to the test above: the *scheme* is compared case-insensitively (as HTTP requires) while its
/// **presence** is required. Without this, tightening the check to an exact `Bearer ` prefix would look correct.
#[tokio::test]
async fn a_lowercase_bearer_scheme_is_accepted() {
    let runner = CountingRunner::new();
    let endpoint = endpoint(ServingConfig::loopback_only(), admitting("vscode"), &runner);

    let headers = Headers {
        credential: Some(format!("bearer {CALLER_DIGEST}")),
        ..Headers::none()
    };
    let (status, _, raw) = post(endpoint, &headers).await;

    assert_eq!(status, http::StatusCode::OK, "raw body: {raw}");
}

/// **A credential that is not in the allowlist is refused, and the refusal is `401` rather than `403`.**
///
/// `401` because from the caller's side an unknown credential and an absent one are the same situation — present
/// a valid one — and distinguishing them would tell a prober which fingerprints exist.
#[tokio::test]
async fn an_unlisted_credential_is_refused_with_a_challenge_status() {
    let runner = CountingRunner::new();
    let endpoint = endpoint(ServingConfig::loopback_only(), admitting("vscode"), &runner);

    // A well-formed digest that simply is not the admitted one.
    let other = "0000000000000000000000000000000000000000000000000000000000000000";
    let (status, _, raw) = post(endpoint, &Headers::bearer(other)).await;

    assert_eq!(
        status,
        http::StatusCode::UNAUTHORIZED,
        "an unlisted credential must be refused with 401, got {status}: {raw}"
    );
    assert_eq!(runner.count(), 0, "the handler must not have run");
}

/// **The refusal body names the policy class and never the verdict.**
///
/// The verdict is for the log: telling a hostile caller whether its origin was unlisted or malformed would help
/// it enumerate the allowlist. This pins the split, and it fails if `refusal_response` is changed to interpolate
/// `RequestRefusal::reason`, which *does* name the verdict.
#[tokio::test]
async fn the_refusal_body_names_the_policy_class_and_not_the_verdict() {
    let runner = CountingRunner::new();
    let endpoint = endpoint(
        serving_with_origin("http://localhost:3000"),
        admitting("vscode"),
        &runner,
    );

    let headers = Headers::origin("https://jarvis.example.com");
    let (_, body, raw) = post(endpoint, &headers).await;

    let body = body.unwrap_or_else(|| panic!("the refusal must be JSON, got: {raw}"));
    let message = body["error"]["message"]
        .as_str()
        .unwrap_or_else(|| panic!("the refusal must carry a message, got: {body}"));
    assert!(
        message.contains("origin policy"),
        "the message must name the policy class, got: {message}"
    );
    // The verdict names itself in its own `Debug`, which is what `RequestRefusal::reason` prints. Its absence
    // here is the assertion that the wire carries the class only.
    for verdict in ["NotAllowed", "Malformed", "Opaque", "was refused"] {
        assert!(
            !message.contains(verdict),
            "the wire must not name {verdict}, got: {message}"
        );
    }
    assert_eq!(body["error"]["code"], json!(crate::binding::REFUSAL_CODE));
}

/// **An `Origin` too long to be a configured entry is refused rather than treated as absent.**
///
/// The dangerous direction: an absent `Origin` is **admitted** (a non-browser client sends none), so collapsing
/// an oversized value into "absent" would turn a refusal into an admission. This is the falsification target for
/// `MAX_ORIGIN_HEADER_CHARS` — bounding the copy must not bound it into the admitted shape.
#[tokio::test]
async fn an_oversized_origin_is_refused_rather_than_treated_as_absent() {
    let runner = CountingRunner::new();
    let endpoint = endpoint(
        serving_with_origin("http://localhost:3000"),
        admitting("vscode"),
        &runner,
    );

    // Longer than the bound, and built to be unparseable as an origin: it is 4000 characters of padding after a
    // valid-looking scheme, so a truncation to 512 characters is still not a configured entry.
    let oversized = format!("http://localhost:3000/{}", "a".repeat(4000));
    let headers = Headers::bearer(CALLER_DIGEST).with_origin(&oversized);
    let (status, _, raw) = post(endpoint, &headers).await;

    assert_eq!(
        status,
        http::StatusCode::FORBIDDEN,
        "an oversized origin must be refused rather than admitted as absent, got {status}: {raw}"
    );
    assert_eq!(runner.count(), 0, "the handler must not have run");
}

/// **An endpoint over a handler that serves nothing is refused at construction.**
///
/// `ServingConfig::check_servable` is the one place this is consulted, and it is consulted here rather than
/// repeated: a second copy would be a second thing to keep in step. Falsified by removing the call — the
/// endpoint then builds and a client connects to a server that cannot answer anything.
#[tokio::test]
async fn an_endpoint_over_an_empty_served_set_is_refused() {
    let runner = CountingRunner::new();
    let empty = JarvisMcpServer::new(Vec::new(), runner);

    let result = ServedEndpoint::new(ServingConfig::loopback_only(), admitting("vscode"), empty);

    assert!(
        matches!(result, Err(crate::serving::ServiceError::NoToolsAdvertised)),
        "an empty served set must be refused at construction"
    );
}

/// **The gate is reachable from a built endpoint, so a daemon can report the policy it serves under.**
///
/// Without it an operator would compare configuration files against a running process by hand, and a policy
/// that could not be stated is one nobody can confirm was loaded.
#[tokio::test]
async fn the_gate_reports_the_policies_in_force() {
    let runner = CountingRunner::new();
    let endpoint = endpoint(
        serving_with_origin("http://localhost:3000"),
        admitting("vscode"),
        &runner,
    );

    let gate = endpoint.gate();
    assert!(
        gate.serving()
            .origin_check(Some("http://localhost:3000"))
            .permits()
    );
    assert!(
        !gate
            .serving()
            .origin_check(Some("http://localhost:3001"))
            .permits()
    );
    assert!(gate.admission().entry_for(&digest()).is_some());
}

/// **The endpoint's `Debug` names the gate and the path, and not the SDK's internals.**
///
/// A derived `Debug` would print a session manager and a tool-schema cache, which is a dependency's interior in
/// a JARVIS log line. Asserting the *absence* is what pins the hand-written impl.
#[tokio::test]
async fn the_debug_output_omits_the_sdk_interior() {
    let runner = CountingRunner::new();
    let endpoint = endpoint(ServingConfig::loopback_only(), admitting("vscode"), &runner);

    let rendered = format!("{endpoint:?}");

    assert!(rendered.contains("ServedEndpoint"), "got: {rendered}");
    assert!(rendered.contains(MCP_ENDPOINT_PATH), "got: {rendered}");
    // Names from the SDK's service interior that a derived `Debug` would print.
    for internal in ["tool_schemas", "session_manager", "StreamableHttpService"] {
        assert!(
            !rendered.contains(internal),
            "the endpoint's Debug must not print {internal}, got: {rendered}"
        );
    }
}
/// **The service is `Send + Sync + Clone` and its future is `Send + 'static`, which is what mounting needs.**
///
/// Written because mounting it on a real router failed with `<… as Service<…>>::Future cannot be sent between
/// threads safely`, and the error named the **mount site's** generic parameter rather than anything here — an
/// opaque `impl Service<…>` exposes only its declared bounds, and the future is an **associated** type, so the
/// return type said nothing about it.
///
/// The fix was to pin `Service::Future` to a named alias ([`crate::binding::ResponseFuture`]), which makes the
/// bound a contract this crate states. **This test is what holds it**: without it, dropping `Send` from the
/// alias would compile here and fail in the daemon.
#[test]
fn the_service_and_its_future_are_send_and_sync() {
    fn assert_mountable<T>(_: &T)
    where
        T: Send + Sync + Clone + 'static,
    {
    }
    fn assert_future_is_send<F: Send + 'static>(_: &F) {}

    let runner = CountingRunner::new();
    let built = endpoint(ServingConfig::loopback_only(), admitting("vscode"), &runner);
    let service = built.into_service::<String>();
    assert_mountable(&service);

    // The future is named through the alias rather than inferred, so a change to the alias's bounds is a
    // **compile** failure in this file instead of a mount-time failure in a binary. The body never runs; only
    // the type's `Send + 'static` matters, which is why the closure returns the same `Result` shape.
    let future: crate::binding::ResponseFuture = Box::pin(async {
        Ok(http::Response::new(crate::binding::ResponseBody::new(
            http_body_util::Full::new(bytes::Bytes::new()),
        )))
    });
    assert_future_is_send(&future);
}
