//! The MCP tool adapter: a call through the real pipeline, against real servers.
//!
//! # What these tests are for
//!
//! `P3-008f`/`P3-008g` proved `tools/call` worked and recorded that **nothing drove it from a run**.
//! This adapter is that driver, and the tests here exercise it end to end: a policy-authorized
//! `ToolExecutionRequest`, a real connection, and a real `ToolCallResult` at the other end.
//!
//! # The mapping is what is being tested, not the transport
//!
//! The transport is already proven. What is new here is the **conversion** from what a server said to
//! an outcome JARVIS can store, and that conversion is where a mistake becomes expensive:
//!
//! - Treating a transport failure as "nothing happened" invites a retry that duplicates an effect.
//! - Treating a tool's own refusal as a transport failure loses the reason the operator needs.
//! - Treating an undecodable answer as a refusal claims nothing happened when the server actually ran.
//!
//! So each test names the outcome it expects and asserts the *specific* vocabulary, never merely that
//! something was returned.

mod support;

use std::sync::Arc;

use jarvis_core::{CorrelationId, SystemClock, UtcTimestamp};
use jarvis_mcp::{NamingStrategy, ToolEffectPolicy};
use jarvis_mcp_transport::{McpToolAdapter, connect_over};
use jarvis_tools::{
    AdapterError, AuthorizationReceipt, AuthorizationReceiptParts, IdempotencyKey,
    ToolExecutionRequest, ToolExecutionRequestParts, ToolExecutor, ToolId, ToolOutcome,
};
use serde_json::{Value, json};
use support::{PeerHandle, ScriptedPeer, discover_result, tool, tools_result};

/// Two duplex halves plus a spawned peer, with the client half ready to hand to `connect_over`.
fn peer_pair(script: ScriptedPeer) -> (tokio::io::DuplexStream, PeerHandle) {
    let (client_side, server_side) = tokio::io::duplex(64 * 1024);
    (client_side, PeerHandle::spawn(server_side, script))
}

fn server(name: &str) -> jarvis_mcp::ServerName {
    jarvis_mcp::ServerName::new(name)
        .unwrap_or_else(|error| panic!("fixture server {name}: {error}"))
}

/// A scripted peer that lists one tool named `search` and answers `tools/call` with `reply`.
fn peer_for(reply: Value) -> ScriptedPeer {
    ScriptedPeer::new()
        .answering(
            "server/discover",
            discover_result("fixture-vendor", None, true),
        )
        .answering("tools/list", tools_result(&[tool("search", None, None)]))
        .answering("tools/call", reply)
}

/// Connects, builds the catalog, and returns an adapter for the fixture server.
async fn adapter_for(script: ScriptedPeer) -> McpToolAdapter {
    let (client_side, _peer) = peer_pair(script);
    let connection = Arc::new(
        connect_over(&server("fixture"), client_side)
            .await
            .unwrap_or_else(|error| panic!("a modern peer must negotiate: {error}")),
    );

    // The **host join**, so the adapter's routing comes from a real catalog rather than being fabricated.
    // That matters: a hand-written route table would not prove that the identifier the receipt binds is
    // the one the adapter routes, which is the property the adapter's `new` is arranged around.
    let mut buffer = jarvis_mcp_transport::ToolBuffer::new();
    let listings = connection
        .list_tools(&mut buffer)
        .await
        .unwrap_or_else(|error| panic!("the fixture must list its tools: {error}"));
    let translated = jarvis_mcp::translate_listing(
        &server("fixture"),
        &listings,
        NamingStrategy::Prefixed,
        &ToolEffectPolicy::unclassified(),
    );
    assert!(translated.excluded.is_empty(), "{:?}", translated.excluded);
    assert_eq!(translated.tools.len(), 1);

    // `CatalogEntry` is needed for the adapter's routing, and it is built here from the translated
    // definition plus the server's own name — exactly what `McpCatalog` carries.
    let entry = jarvis_mcp::CatalogEntry {
        definition: translated.tools[0].definition.clone(),
        server: server("fixture"),
        remote: translated.tools[0].remote.clone(),
    };
    McpToolAdapter::new(&server("fixture"), connection, &[entry])
}

/// The one definition the fixture registers, built from the real translation of the fixture's listing.
///
/// Built through `translate_tool` with a **read-only** policy so `evaluate` returns an allowance and a
/// receipt can be derived from that decision. The posture is not the subject of these tests — the adapter
/// never consults it — but it must be one that authorizes, or the fixture could not build a valid request.
fn definition_for() -> jarvis_tools::ToolDefinition {
    let schema = json!({
        "type": "object",
        "properties": { "q": { "type": "string" } }
    });
    let listing = jarvis_mcp::McpToolListing {
        name: "search",
        title: None,
        description: None,
        input_schema: &schema,
        output_schema: None,
    };
    let translated = jarvis_mcp::translate_tool(
        &server("fixture"),
        &listing,
        NamingStrategy::Prefixed,
        &ToolEffectPolicy::read_only().unwrap_or_else(|error| panic!("{error}")),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    translated.definition
}

/// A timestamp `seconds` after another, via the fallible constructor the type actually has.
fn after(base: UtcTimestamp, seconds: i128) -> UtcTimestamp {
    UtcTimestamp::from_unix_nanos(base.unix_nanos() + seconds * 1_000_000_000)
        .unwrap_or_else(|error| panic!("the fixture offset must be representable: {error}"))
}

/// Builds a request whose receipt is **derived** from a real policy decision over `arguments`.
///
/// Every field comes from the same two inputs — the definition and the arguments — so a fixture cannot
/// describe a tool the receipt does not cover. That is what makes the adapter tests meaningful: the
/// binding check `P3-006b` added would refuse a fabricated fixture, so a fixture that builds at all has
/// passed it.
fn request_with(arguments: Value) -> ToolExecutionRequest {
    let definition = definition_for();
    let now = UtcTimestamp::now(&SystemClock);
    let expires = after(now, 60);

    // `evaluate` over a read-only definition with its scope granted and a credential claimed returns an
    // allowance, which is the only decision a receipt can be derived from without an approval.
    //
    // Two fixture details were wrong on the first attempt and both are recorded here because each one
    // silently turned every test into a held call: the scope must be granted (`mcp.call`), and the
    // claimed strength must be at least `Credential` — even a minimal-risk tool requires
    // `ChannelEvidence`, so an `Absent` claim is an `InsufficientAuthentication` hold.
    let decision = jarvis_tools::evaluate(&jarvis_tools::PolicyRequest {
        definition: &definition,
        actor: jarvis_tools::ActorAuthority::active(jarvis_tools::ScopeSet::single(
            jarvis_tools::Scope::new(jarvis_mcp::DEFAULT_MCP_SCOPE)
                .unwrap_or_else(|error| panic!("{error}")),
        )),
        workspace: &jarvis_tools::WorkspacePolicy::default(),
        channel: jarvis_core::SessionChannel::Cli,
        claimed_strength: jarvis_tools::AuthenticationStrength::Credential,
        available: true,
        target: jarvis_tools::TargetAssessment::none(),
    });
    assert!(
        !decision.is_denied() && decision.is_allowed(),
        "the fixture's read-only definition must be allowed: {decision:?}"
    );

    let intent = jarvis_core::CanonicalIntentHash::compute(
        &definition.id().to_string(),
        definition.version(),
        &arguments,
    )
    .unwrap_or_else(|error| panic!("the fixture arguments must be hashable: {error}"));

    let receipt = AuthorizationReceipt::new(AuthorizationReceiptParts {
        receipt_id: "receipt-fixture".to_owned(),
        tool: definition.id().clone(),
        tool_version: definition.version().to_owned(),
        arguments: arguments.clone(),
        intent_hash: intent,
        policy_version: "policy-1".to_owned(),
        decision,
        approval: None,
        correlation_id: CorrelationId::new(),
        issued_at: now,
    })
    .unwrap_or_else(|error| panic!("the derived receipt must be valid: {error}"));

    ToolExecutionRequest::new(ToolExecutionRequestParts {
        call_id: "call-fixture".to_owned(),
        tool: definition.id().clone(),
        tool_version: definition.version().to_owned(),
        arguments,
        receipt,
        idempotency_key: IdempotencyKey::parse("0123456789abcdef0123456789abcdef")
            .unwrap_or_else(|error| panic!("{error}")),
        deadline: expires,
        correlation_id: CorrelationId::new(),
    })
    .unwrap_or_else(|error| panic!("the fixture request must bind to its receipt: {error}"))
}

/// **A successful call becomes `Confirmed`, with the output kept separate from the evidence.** The split
/// is what `tools-and-connectors.md` requires, and it is asserted here rather than assumed: a reader must
/// be able to tell the locator from the content.
#[tokio::test]
async fn a_successful_call_is_confirmed_with_separate_evidence_and_output() {
    let adapter = adapter_for(peer_for(json!({
        "resultType": "complete",
        "content": [{ "type": "text", "text": "found three documents" }],
        "isError": false
    })))
    .await;

    let result = adapter
        .execute(&request_with(json!({ "q": "pumps" })))
        .await
        .unwrap_or_else(|error| panic!("a successful call must return: {error}"));

    assert_eq!(result.outcome(), ToolOutcome::Confirmed);
    assert!(result.is_terminal());
    // The evidence is a locator, not the text.
    let evidence = result
        .evidence()
        .unwrap_or_else(|| panic!("a confirmed outcome must carry evidence"));
    assert_eq!(evidence.as_str(), "mcp:fixture/search");
    // And the output is the server's text, not the locator.
    let output = result
        .output()
        .unwrap_or_else(|| panic!("a successful call with text must carry output"));
    assert_eq!(output.content(), "found three documents");
    assert!(!output.is_truncated());
}

/// **A tool that reports failure is `Failed`, not an adapter error.** The tool ran and said it could not
/// do the thing: the request was not refused, so the pipeline must record an outcome rather than a
/// transport failure — and the server's own reason must survive as the reason.
#[tokio::test]
async fn a_tool_that_refuses_is_failed_with_the_servers_reason() {
    let adapter = adapter_for(peer_for(json!({
        "resultType": "complete",
        "content": [{ "type": "text", "text": "no such document" }],
        "isError": true
    })))
    .await;

    let result = adapter
        .execute(&request_with(json!({ "q": "pumps" })))
        .await
        .unwrap_or_else(|error| {
            panic!("a tool refusal is an outcome, not an adapter error: {error}")
        });

    assert_eq!(result.outcome(), ToolOutcome::Failed);
    let reason = result
        .record()
        .reason()
        .unwrap_or_else(|| panic!("a failed outcome must carry a reason"));
    assert!(reason.contains("no such document"), "{reason}");
    assert!(reason.starts_with("mcp:search: "), "{reason}");
    // A failed call has nothing to output: reporting content would make a refusal look like a result.
    assert!(result.output().is_none(), "{:?}", result.output());
}

/// **A JSON-RPC error is `ProviderRefused`, not `AmbiguousAfterReaching`.** The peer answered and refused
/// the request, so nothing happened and the reason is known. The distinction decides whether a retry is
/// safe, which is why it is asserted by vocabulary rather than by "an error occurred".
#[tokio::test]
async fn a_protocol_error_is_a_refusal_not_an_ambiguity() {
    let adapter =
        adapter_for(peer_for(json!(null)).failing("tools/call", -32602, "unknown tool")).await;

    let error = adapter
        .execute(&request_with(json!({ "q": "pumps" })))
        .await
        .err()
        .unwrap_or_else(|| panic!("a refused request must not yield a result"));
    assert!(
        matches!(error, AdapterError::ProviderRefused { .. }),
        "a peer's refusal is a known non-effect, not an ambiguity: {error}"
    );
    assert!(error.to_string().contains("unknown tool"), "{error}");
}

/// **An undecodable answer is `AmbiguousAfterReaching`.** The peer answered with a shape this client
/// cannot read, so it *ran something* and the outcome cannot be established. Classifying this as a
/// refusal would claim nothing happened and invite a retry that duplicates an effect.
#[tokio::test]
async fn an_undecodable_answer_is_ambiguous() {
    let adapter = adapter_for(peer_for(
        json!({ "resultType": "task", "unexpected": true }),
    ))
    .await;

    let error = adapter
        .execute(&request_with(json!({ "q": "pumps" })))
        .await
        .err()
        .unwrap_or_else(|| panic!("an undecodable result must not yield an outcome"));
    assert!(
        matches!(error, AdapterError::AmbiguousAfterReaching { .. }),
        "the server answered, so the effect is unknown rather than disproven: {error}"
    );
}

/// **MRTR and Tasks are refused, and neither claims an effect.** JARVIS declares no input handler and
/// polls no task, so a server demanding either reached its own boundary without doing the work.
#[tokio::test]
async fn a_demand_for_client_input_is_refused_by_the_adapter() {
    let adapter = adapter_for(peer_for(json!({
        "resultType": "input_required",
        "requestState": "opaque-handle"
    })))
    .await;

    let error = adapter
        .execute(&request_with(json!({ "q": "pumps" })))
        .await
        .err()
        .unwrap_or_else(|| panic!("an unfillable input round must be refused"));
    assert!(
        matches!(error, AdapterError::ProviderRefused { .. }),
        "a mode JARVIS cannot serve is a refusal, not an ambiguity: {error}"
    );
    assert!(error.to_string().contains("input round"), "{error}");
}

/// A task answer is likewise refused, and the message says which mode was reached for.
#[tokio::test]
async fn a_task_answer_is_refused_by_the_adapter() {
    let adapter = adapter_for(peer_for(json!({
        "resultType": "task",
        "taskId": "task-1",
        "status": "working",
        "createdAt": "2026-01-01T00:00:00Z",
        "lastUpdatedAt": "2026-01-01T00:00:00Z",
        "ttlMs": null
    })))
    .await;

    let error = adapter
        .execute(&request_with(json!({ "q": "pumps" })))
        .await
        .err()
        .unwrap_or_else(|| panic!("a task must not read as a completed call"));
    assert!(
        matches!(error, AdapterError::ProviderRefused { .. }),
        "{error}"
    );
    assert!(error.to_string().contains("task"), "{error}");
}

/// **An unrouted identifier is `NotImplemented`.** This is the registry-mistake case: a tool the
/// pipeline offered that this adapter has no route for. It must be distinguishable from a provider
/// failure, because the remedy is a configuration change rather than a retry.
#[tokio::test]
async fn an_unrouted_tool_is_not_implemented() {
    // The positive control first: an adapter that DOES hold the route succeeds, so the refusal below is
    // not passing on an adapter that is broken for some other reason.
    let routed = adapter_for(peer_for(json!({
        "resultType": "complete",
        "content": [{ "type": "text", "text": "ran" }],
        "isError": false
    })))
    .await;
    assert!(
        routed
            .execute(&request_with(json!({ "q": "pumps" })))
            .await
            .is_ok()
    );

    // The same request against an adapter built with no routing at all.
    let (client_side, _peer) = peer_pair(peer_for(json!({
        "resultType": "complete",
        "content": [],
        "isError": false
    })));
    let connection = Arc::new(
        connect_over(&server("fixture"), client_side)
            .await
            .unwrap_or_else(|error| panic!("{error}")),
    );
    let routeless = McpToolAdapter::new(&server("fixture"), connection, &[]);
    assert_eq!(routeless.routed_tools(), 0);
    let error = routeless
        .execute(&request_with(json!({ "q": "pumps" })))
        .await
        .err()
        .unwrap_or_else(|| panic!("a tool with no route must be refused"));
    assert!(
        matches!(error, AdapterError::NotImplemented { .. }),
        "an unrouted tool is a configuration mistake: {error}"
    );
    // And the refusal names the tool, so an operator can act on it.
    assert!(error.to_string().contains("mcp.fixture.search"), "{error}");
}

/// **The adapter routes only its own server's tools.** An entry for a different server must be skipped,
/// or a call for server A could run on server B under a name B recognizes — a call executing with the
/// wrong server's posture, which is the whole point of classifying servers separately.
#[tokio::test]
async fn entries_for_another_server_are_not_routed() {
    let (client_side, _peer) = peer_pair(peer_for(json!({
        "resultType": "complete",
        "content": [],
        "isError": false
    })));
    let connection = Arc::new(
        connect_over(&server("fixture"), client_side)
            .await
            .unwrap_or_else(|error| panic!("{error}")),
    );

    let other = jarvis_mcp::CatalogEntry {
        definition: definition_for(),
        server: server("elsewhere"),
        remote: "search".to_owned(),
    };
    let adapter = McpToolAdapter::new(&server("fixture"), connection, &[other]);
    assert_eq!(
        adapter.routed_tools(),
        0,
        "another server's entry must not be routable through this adapter"
    );
}

/// **A request that went out with no answer back is `AmbiguousAfterReaching`, and this mapping was
/// missing a test until a falsification run showed it.** Changing the mapping to `RefusedBeforeReaching`
/// left the whole suite green, which meant nothing pinned the one decision that stops a non-idempotent
/// effect being repeated: calling this a refusal claims nothing happened, and the caller then retries a
/// tool that may have sent a message.
#[tokio::test]
async fn a_dead_peer_after_sending_is_ambiguous_not_a_refusal() {
    let adapter = adapter_for(peer_for(json!(null)).hanging_up("tools/call")).await;

    let error = adapter
        .execute(&request_with(json!({ "q": "pumps" })))
        .await
        .err()
        .unwrap_or_else(|| panic!("a dead peer must not yield an outcome"));
    assert!(
        matches!(error, AdapterError::AmbiguousAfterReaching { .. }),
        "the request was sent, so the effect may have happened: {error}"
    );
}

/// A call already past its deadline is refused **before** the server is asked, so a late call cannot
/// start work. `RefusedBeforeReaching` rather than `ProviderRefused`: nothing left this process.
#[tokio::test]
async fn a_past_deadline_call_is_refused_before_reaching_the_server() {
    // The peer is scripted to *succeed*, so a call that reached it would return Ok. That is what makes
    // this test about the deadline rather than about the server.
    let adapter = adapter_for(peer_for(json!({
        "resultType": "complete",
        "content": [{ "type": "text", "text": "should not run" }],
        "isError": false
    })))
    .await;

    // The deadline is moved into the past after the request is built. `ToolExecutionRequest::new`
    // validates the receipt's own interval, so a receipt that was already expired could not be built at
    // all; the deadline field is what this test moves.
    let mut request = request_with(json!({ "q": "pumps" }));
    let past = after(UtcTimestamp::now(&SystemClock), -1);
    request = with_deadline(&request, past);

    let error = adapter
        .execute(&request)
        .await
        .err()
        .unwrap_or_else(|| panic!("a late call must be refused"));
    assert!(
        matches!(error, AdapterError::RefusedBeforeReaching { .. }),
        "nothing happened, so this is a refusal rather than a provider answer: {error}"
    );
    assert!(error.to_string().contains("deadline"), "{error}");
}

/// Rebuilds a request with a different deadline, keeping every other field as it was.
///
/// A helper rather than a field assignment because `ToolExecutionRequest`'s fields are private: the only
/// way to change one is to build a new request through the same constructor, which also re-runs the
/// binding checks. That is the point — a fixture cannot produce a request the real path would refuse.
fn with_deadline(request: &ToolExecutionRequest, deadline: UtcTimestamp) -> ToolExecutionRequest {
    ToolExecutionRequest::new(ToolExecutionRequestParts {
        call_id: request.call_id().to_owned(),
        tool: request.tool().clone(),
        tool_version: request.tool_version().to_owned(),
        arguments: request.arguments().clone(),
        receipt: request.receipt().clone(),
        idempotency_key: request.idempotency_key().clone(),
        deadline,
        correlation_id: request.correlation_id(),
    })
    .unwrap_or_else(|error| panic!("the rebuilt request must stay valid: {error}"))
}

/// The adapter identifies itself by a stable name, which is what a log line and an audit record cite.
#[tokio::test]
async fn the_adapter_names_itself() {
    let adapter = adapter_for(peer_for(json!({
        "resultType": "complete",
        "content": [],
        "isError": false
    })))
    .await;
    assert_eq!(adapter.adapter_id(), "mcp-tool");
    assert_eq!(adapter.server(), "fixture");
    assert_eq!(adapter.routed_tools(), 1);
}

/// A success with **no content at all** is still `Confirmed` with no output — not `Failed` and not an
/// empty output. The call is the evidence; an empty output would be a *value*, making "returned nothing"
/// indistinguishable from "returned an empty string".
#[tokio::test]
async fn a_contentless_success_is_confirmed_without_output() {
    let adapter = adapter_for(peer_for(json!({
        "resultType": "complete",
        "content": [],
        "isError": false
    })))
    .await;

    let result = adapter
        .execute(&request_with(json!({ "q": "pumps" })))
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(result.outcome(), ToolOutcome::Confirmed);
    assert!(
        result.output().is_none(),
        "an absent output must stay absent rather than becoming an empty one"
    );
    assert!(
        result.evidence().is_some(),
        "the call itself is the evidence"
    );
}

/// `structured_content` is preferred over the text blocks, because it is the protocol's own
/// machine-readable form. The assertion is that the output parses as JSON rather than as a JSON string —
/// the shape a caller would otherwise have to un-escape.
#[tokio::test]
async fn structured_content_is_preferred_over_text() {
    let adapter = adapter_for(peer_for(json!({
        "resultType": "complete",
        "content": [{ "type": "text", "text": "{\"count\":3}" }],
        "structuredContent": { "count": 3 },
        "isError": false
    })))
    .await;

    let result = adapter
        .execute(&request_with(json!({ "q": "pumps" })))
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let output = result
        .output()
        .unwrap_or_else(|| panic!("a structured result must carry output"));
    let parsed: Value = serde_json::from_str(output.content())
        .unwrap_or_else(|error| panic!("the output must be JSON, not a JSON string: {error}"));
    assert_eq!(parsed["count"], 3);
}

/// The receipt binding is enforced *by the request*, so a request whose arguments do not match its
/// receipt cannot even be built. Asserted here so the adapter tests cannot pass while silently bypassing
/// authorization — the positive control for every test above.
#[tokio::test]
async fn a_request_whose_arguments_are_not_authorized_cannot_be_built() {
    // Build a valid request for one argument set, then rebuild it with different arguments. The receipt
    // is carried over unchanged, so the digest no longer covers the arguments and the constructor must
    // refuse it — which is the binding `P3-006b` added, exercised rather than assumed.
    let authorized = request_with(json!({ "q": "pumps" }));
    let error = ToolExecutionRequest::new(ToolExecutionRequestParts {
        call_id: authorized.call_id().to_owned(),
        tool: authorized.tool().clone(),
        tool_version: authorized.tool_version().to_owned(),
        arguments: json!({ "q": "something else" }),
        receipt: authorized.receipt().clone(),
        idempotency_key: authorized.idempotency_key().clone(),
        deadline: authorized.deadline(),
        correlation_id: authorized.correlation_id(),
    })
    .err()
    .unwrap_or_else(|| panic!("arguments outside the receipt must be refused"));
    // The specific variant, not merely `is_err`: the binding check is what must fire.
    assert!(
        matches!(
            error,
            jarvis_tools::ExecutionRequestError::ArgumentsNotAuthorized
        ),
        "{error}"
    );
}

/// A helper the fixture needs: the canonical identifier of the tool the adapter routes.
///
/// Kept as a function so the catalog entry, the receipt, and the request all describe **one** tool. Three
/// separately-written copies would let a fixture pass while describing three different tools, which is
/// exactly the "two values that must agree" defect this project keeps finding.
fn definition_identity() -> ToolId {
    definition_for().id().clone()
}

/// A smoke assertion that the fixture's identity helper agrees with the definition it derives from.
#[test]
fn the_fixture_identity_helper_agrees_with_its_definition() {
    assert_eq!(definition_identity(), *definition_for().id());
    assert_eq!(definition_identity().to_string(), "mcp.fixture.search");
}
