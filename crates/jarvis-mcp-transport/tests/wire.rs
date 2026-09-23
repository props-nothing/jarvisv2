//! The MCP wire negotiation, driven against a scripted peer.
//!
//! # What these tests are for
//!
//! `P3-008a`..`P3-008d` built and tested the translation rules as pure functions. This is the first
//! slice where a real MCP connection exists, and the honest-limit every one of those entries carried
//! was "nothing exercised against a real MCP server". These tests do not remove that limit — a
//! scripted peer is not a real server — but they remove a different one: **the negotiation itself was
//! unverified**, and the negotiation is where `P3-007` found the trap.
//!
//! The peer writes the wire format itself (see `support`), so a passing test here says the client
//! agrees with the *documented* framing, not merely with the SDK.
//!
//! # The two failures worth being able to see
//!
//! 1. **A silent fall back to the legacy handshake.** `initialize` was removed in `2026-07-28`. A
//!    client that sends it gets a legacy session that *works*, so nothing fails — and the rest of
//!    this crate does not implement the legacy era's behaviour. The first test here asserts
//!    `initialize` is never sent.
//! 2. **A server's self-description becoming an identifier.** ADR-0024. The connection carries the
//!    operator's name and the server's claim as two separate things, and a test asserts they do not
//!    merge.

mod support;

use jarvis_mcp::ServerName;
use jarvis_mcp_transport::{ConnectError, connect_over};
use serde_json::json;
use support::{PeerHandle, ScriptedPeer, announced_safe_tool, discover_result, tool, tools_result};

/// Two duplex halves plus a spawned peer, with the client half ready to hand to `connect_over`.
fn peer_pair(script: ScriptedPeer) -> (tokio::io::DuplexStream, PeerHandle) {
    let (client_side, server_side) = tokio::io::duplex(64 * 1024);
    (client_side, PeerHandle::spawn(server_side, script))
}

fn server(name: &str) -> ServerName {
    ServerName::new(name).unwrap_or_else(|error| panic!("fixture server {name}: {error}"))
}

/// **The defect `P3-007` recorded, tested as a behaviour.** Protocol revision `2026-07-28` removed
/// the `initialize` handshake and replaced it with `server/discover`. A client that performs the
/// handshake negotiates the legacy era and does not fail, so the only way to know which era is on the
/// wire is to look at the wire.
///
/// The peer answers `server/discover` and **nothing else**. If the client sent `initialize` the
/// negotiation would stall and this test would time out rather than pass — so this asserts both that
/// discovery happened and that the handshake did not.
#[tokio::test]
async fn a_modern_connection_discovers_and_never_initializes() {
    let script = ScriptedPeer::new().answering(
        "server/discover",
        discover_result("fixture", Some("Fixture"), true),
    );
    let (client_side, peer) = peer_pair(script);

    let connection = connect_over(&server("fixture"), client_side)
        .await
        .unwrap_or_else(|error| panic!("a modern peer must negotiate: {error}"));

    assert_eq!(connection.negotiated_revision(), "2026-07-28");
    assert!(connection.capabilities().offers_tools());
    assert!(
        connection.capabilities().tool_list_changed,
        "the peer declared list_changed, and a client that ignored it would cache a stale list"
    );

    drop(connection);
    let seen = peer.finish().await;
    assert!(seen.saw("server/discover"), "{:?}", seen.methods());
    assert!(
        !seen.saw("initialize"),
        "the initialize handshake was removed in 2026-07-28; sending it selects the legacy era: {:?}",
        seen.methods()
    );
    // `notifications/initialized` is the legacy handshake's second half. It must not appear either,
    // and it is a notification so the peer would not have replied to it — checking the recorded
    // methods is the only way to see it.
    assert!(
        !seen.saw("notifications/initialized"),
        "{:?}",
        seen.methods()
    );
}

/// A peer that only knows the legacy handshake must produce a **failure**, not a working connection.
/// `Auto` would fall back here; `Discover` must not.
///
/// The peer answers nothing at all, which is exactly what a legacy server looks like: it is listening
/// for an `initialize` request that a modern client never sends, so it never replies. That is why the
/// failure must be a **bounded timeout** — without one, this exact scenario is a daemon hanging at
/// startup against a server that will never answer.
///
/// The test asserts a refusal, and the deadline is enforced by the client rather than by
/// `tokio::time::timeout` here, so the guarantee under test is the client's own.
#[tokio::test]
async fn a_peer_that_does_not_answer_discovery_produces_no_connection() {
    let (client_side, _peer) = peer_pair(ScriptedPeer::new());

    let started = std::time::Instant::now();
    let outcome = connect_over(&server("legacy"), client_side).await;
    let elapsed = started.elapsed();

    let error = outcome
        .err()
        .unwrap_or_else(|| panic!("a peer that answers nothing must not yield a connection"));
    assert!(
        matches!(error, ConnectError::DiscoveryTimedOut { .. }),
        "a silent peer must time out rather than fail some other way: {error}"
    );
    // The refusal must name the deadline, and the remedy it suggests is the protocol era rather than
    // the network — a legacy server is the likeliest cause of silence here.
    assert!(error.to_string().contains("server/discover"), "{error}");
    assert!(error.to_string().contains("legacy"), "{error}");
    // And it must be bounded: a client that waited forever would hang a daemon, which is the defect
    // this deadline exists to prevent.
    assert!(
        elapsed < std::time::Duration::from_secs(60),
        "the deadline must bound the wait, took {elapsed:?}"
    );
}

/// A server that offers a revision older than the modern one is refused **by name**, and the message
/// says what was asked for. `Discover` has no fallback, but the server still chooses what it reports,
/// so this is the case where a client could otherwise accept a revision it cannot speak.
#[tokio::test]
async fn a_server_that_reports_only_a_legacy_revision_is_refused() {
    let legacy = json!({
        "resultType": "complete",
        "supportedVersions": ["2025-11-25"],
        "capabilities": { "tools": {} },
        "ttlMs": 0,
        "cacheScope": "private"
    });
    let (client_side, _peer) = peer_pair(ScriptedPeer::new().answering("server/discover", legacy));

    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        connect_over(&server("old"), client_side),
    )
    .await
    .unwrap_or_else(|_| panic!("the negotiation must fail rather than wait forever"));

    let error = outcome
        .err()
        .unwrap_or_else(|| panic!("a legacy-only server must not yield a connection"));
    // The failure must be actionable: it names the revision the server offered, so an operator can
    // tell a protocol mismatch apart from a network fault.
    assert!(
        error.to_string().contains("2025-11-25"),
        "the refusal should name what the server offered: {error}"
    );
}

/// **A server's tool list translates, end to end, from bytes.** This is the claim `P3-008a` could not
/// make: the rules were tested, but never against a listing that arrived over a wire.
#[tokio::test]
async fn a_tool_list_arrives_over_the_wire_and_translates() {
    let script = ScriptedPeer::new()
        .answering(
            "server/discover",
            discover_result("fixture", Some("Fixture"), true),
        )
        .answering(
            "tools/list",
            tools_result(&[
                tool("search", Some("Search"), Some("Search the corpus")),
                tool("fetch", None, None),
            ]),
        );
    let (client_side, peer) = peer_pair(script);

    let connection = connect_over(&server("fixture"), client_side)
        .await
        .unwrap_or_else(|error| panic!("a modern peer must negotiate: {error}"));

    let mut buffer = jarvis_mcp_transport::ToolBuffer::new();
    let listings = connection
        .list_tools(&mut buffer)
        .await
        .unwrap_or_else(|error| panic!("a well-formed tool list must be readable: {error}"));

    assert_eq!(listings.len(), 2);
    assert_eq!(listings[0].name, "search");
    assert_eq!(listings[0].title, Some("Search"));
    assert_eq!(listings[0].description, Some("Search the corpus"));
    assert!(listings[0].input_schema.is_object());
    assert!(listings[0].output_schema.is_none());
    // A tool with no title or description yields `None`, not an invented placeholder. The
    // description the model sees must be absent rather than plausible.
    assert_eq!(listings[1].name, "fetch");
    assert!(listings[1].title.is_none());
    assert!(listings[1].description.is_none());

    drop(connection);
    let seen = peer.finish().await;
    assert!(seen.saw("tools/list"), "{:?}", seen.methods());
}

/// **ADR-0025 enforced against a real listing.** A server that claims its tool is read-only and
/// idempotent must not have that claim reach the translation. The listing type has no field for
/// annotations, so this asserts the omission survives contact with a payload that carries them —
/// which is stronger than asserting the field is absent from a struct.
#[tokio::test]
async fn a_server_that_claims_its_tool_is_safe_gets_no_say() {
    let script = ScriptedPeer::new()
        .answering("server/discover", discover_result("fixture", None, true))
        .answering(
            "tools/list",
            tools_result(&[announced_safe_tool("send-mail")]),
        );
    let (client_side, _peer) = peer_pair(script);

    let connection = connect_over(&server("fixture"), client_side)
        .await
        .unwrap_or_else(|error| panic!("a modern peer must negotiate: {error}"));
    let mut buffer = jarvis_mcp_transport::ToolBuffer::new();
    let listings = connection
        .list_tools(&mut buffer)
        .await
        .unwrap_or_else(|error| {
            panic!("the listing carries annotations but is still readable: {error}")
        });

    assert_eq!(listings.len(), 1);
    assert_eq!(listings[0].name, "send-mail");
    // The only thing that survived is the name and the schema. Every hint the server asserted is
    // gone, and there is nowhere for it to be: `McpToolListing` has five fields and none of them is
    // an annotation. If this test ever fails to compile because that changed, the reviewer must read
    // ADR-0025 before adding the field.
    assert!(listings[0].title.is_none());
    assert!(listings[0].description.is_none());
    assert!(listings[0].input_schema.is_object());
}

/// The operator's name for a server and the server's claim about itself are separate facts.
///
/// `P3-008d` compares the claim across connections to detect a server that changed under a stable
/// operator name, so the two must be readable independently. A transport that merged them would
/// make that check compare a value with itself.
#[tokio::test]
async fn the_operator_name_and_the_reported_identity_stay_separate() {
    let script = ScriptedPeer::new().answering(
        "server/discover",
        discover_result("vendor-product", Some("Vendor Product"), true),
    );
    let (client_side, _peer) = peer_pair(script);

    let connection = connect_over(&server("acme-mcp"), client_side)
        .await
        .unwrap_or_else(|error| panic!("a modern peer must negotiate: {error}"));

    // The operator chose this, and the server had no say in it.
    assert_eq!(connection.server().as_str(), "acme-mcp");
    // The server asserted this, and it is evidence rather than an identifier.
    let reported = connection.reported_identity();
    assert_eq!(reported.name, "vendor-product");
    assert_eq!(reported.title.as_deref(), Some("Vendor Product"));
    assert_ne!(
        reported.name,
        connection.server().as_str(),
        "the two must not merge, or a drift check would compare a value with itself"
    );
}

/// A modern `server/discover` response is **not required to carry an identity** — the SDK's own source
/// notes this — so a conforming server may report none. The connection must still succeed, and the
/// absence must be visible as an empty identity rather than as an error or an invented name.
///
/// This matters for `P3-008d`: a server that *stops* naming itself is a drift, and treating the
/// absence as a failure would discard the observation.
#[tokio::test]
async fn a_server_that_reports_no_identity_still_connects() {
    let anonymous = json!({
        "resultType": "complete",
        "supportedVersions": ["2026-07-28"],
        "capabilities": { "tools": {} },
        "ttlMs": 0,
        "cacheScope": "private"
    });
    let (client_side, _peer) =
        peer_pair(ScriptedPeer::new().answering("server/discover", anonymous));

    let connection = connect_over(&server("quiet"), client_side)
        .await
        .unwrap_or_else(|error| {
            panic!("a server that omits its identity is still conforming: {error}")
        });

    let reported = connection.reported_identity();
    assert_eq!(reported.name, "");
    assert!(reported.title.is_none());
    // And the revision is still known, so the absence is specific to the identity rather than a
    // sign the whole peer info was missing.
    assert_eq!(connection.negotiated_revision(), "2026-07-28");
}

/// **The stateless contract, on the wire.** Revision `2026-07-28` removed the `initialize` handshake,
/// so the per-connection facts the handshake used to establish — the protocol version, the client's
/// identity, its capabilities — must instead travel on **every request**. A client that negotiated
/// modern but omitted that metadata would look healthy and be unreadable to a conforming server.
///
/// The peer records what it received, so this is an assertion about bytes rather than about a
/// negotiation that returned `Ok`.
#[tokio::test]
async fn a_discovery_request_carries_its_own_protocol_metadata() {
    let script =
        ScriptedPeer::new().answering("server/discover", discover_result("fixture", None, true));
    let (client_side, peer) = peer_pair(script);

    let connection = connect_over(&server("fixture"), client_side)
        .await
        .unwrap_or_else(|error| panic!("a modern peer must negotiate: {error}"));
    drop(connection);

    let seen = peer.finish().await;
    let discover = seen
        .seen()
        .iter()
        .find(|request| request.method == "server/discover")
        .unwrap_or_else(|| panic!("the peer must have seen a discover request"));
    // The request must name the revision it is asking for and identify the client. The exact nesting
    // is the SDK's, so this asserts the *facts* are present rather than a particular shape — a shape
    // assertion would break on an SDK patch that changed nothing observable.
    let rendered = discover.params.to_string();
    assert!(
        rendered.contains("2026-07-28"),
        "a modern request must carry the revision it asks for: {rendered}"
    );
    assert!(
        rendered.contains("jarvis"),
        "a modern request must identify the client: {rendered}"
    );
}

/// A peer that answers `tools/list` with a protocol error must be reported as a **peer** error, not
/// as an unreachable server. The two have different remedies — one is a fact about the server, the
/// other is worth retrying — so collapsing them sends an operator to the wrong one.
#[tokio::test]
async fn a_protocol_error_on_the_tool_list_is_not_reported_as_unreachable() {
    let script = ScriptedPeer::new()
        .answering("server/discover", discover_result("fixture", None, true))
        .failing("tools/list", -32601, "method not found");
    let (client_side, _peer) = peer_pair(script);

    let connection = connect_over(&server("fixture"), client_side)
        .await
        .unwrap_or_else(|error| {
            panic!("the negotiation succeeds; the list is what fails: {error}")
        });
    let mut buffer = jarvis_mcp_transport::ToolBuffer::new();
    let error = connection
        .list_tools(&mut buffer)
        .await
        .err()
        .unwrap_or_else(|| panic!("a JSON-RPC error must not read as an empty tool list"));

    match error {
        jarvis_mcp_transport::ListError::PeerError(message) => {
            assert!(message.contains("method not found"), "{message}");
        }
        jarvis_mcp_transport::ListError::Unavailable(message) => {
            panic!("a protocol error must not read as an unreachable peer: {message}")
        }
    }
}

/// The refusal vocabulary must not leak an SDK type: a caller that matched on the SDK's variants
/// would have a reason to keep the SDK in its dependency list, which is exactly the coupling the
/// crate split exists to prevent.
///
/// This is a compile-time property asserted as a test so it appears in the suite: `ConnectError` and
/// `ListError` are constructed here from plain values only.
#[test]
fn the_error_types_are_constructible_without_the_sdk() {
    let unreachable = ConnectError::Unreachable("no child process".to_owned());
    let refused = ConnectError::Refused("protocol disagreement".to_owned());
    let legacy = ConnectError::LegacyNegotiated {
        negotiated: "2025-11-25".to_owned(),
        wanted: "2026-07-28".to_owned(),
    };
    let list_error = jarvis_mcp_transport::ListError::Unavailable("timed out".to_owned());

    assert!(unreachable.to_string().contains("could not be reached"));
    assert!(refused.to_string().contains("protocol level"));
    assert!(legacy.to_string().contains("2025-11-25"));
    assert!(legacy.to_string().contains("2026-07-28"));
    assert!(list_error.to_string().contains("timed out"));
}

/// **`tools/call` over the wire.** The transport can now run a tool, which `P3-008e` recorded as
/// absent. The name sent must be the **server's own** name, not the canonical identifier: sending the
/// canonical id would be the translation applied twice, and the peer here records what it received so
/// the assertion is about bytes rather than about a call that returned `Ok`.
#[tokio::test]
async fn a_tool_call_sends_the_servers_own_name_and_reads_the_result() {
    let script = ScriptedPeer::new()
        .answering("server/discover", discover_result("fixture", None, true))
        .answering(
            "tools/call",
            json!({
                "resultType": "complete",
                "content": [{ "type": "text", "text": "search ran" }],
                "isError": false
            }),
        );
    let (client_side, peer) = peer_pair(script);
    let connection = connect_over(&server("fixture"), client_side)
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    let mut arguments = serde_json::Map::new();
    arguments.insert("q".to_owned(), json!("pumps"));
    let result = connection
        .call_tool("search", &arguments)
        .await
        .unwrap_or_else(|error| panic!("a well-formed call must return: {error}"));

    assert!(result.is_success());
    assert_eq!(result.text, "search ran");
    assert!(result.structured.is_none());

    drop(connection);
    let seen = peer.finish().await;
    let call = seen
        .seen()
        .iter()
        .find(|request| request.method == "tools/call")
        .unwrap_or_else(|| panic!("the peer must have seen a tools/call request"));
    assert_eq!(call.params["name"], "search");
    assert_eq!(call.params["arguments"]["q"], "pumps");
}

/// **A tool that reports failure is not a transport error.** A `CallToolResult` with `isError` set
/// means the tool *ran* and could not do what was asked — an outcome, not a broken connection.
/// Collapsing it into a `CallError` would lose the distinction `ToolOutcome` exists to make, and would
/// make a tool's own honest refusal look like an unreachable server.
#[tokio::test]
async fn a_tool_that_reports_failure_is_a_result_not_an_error() {
    let script = ScriptedPeer::new()
        .answering("server/discover", discover_result("fixture", None, true))
        .answering(
            "tools/call",
            json!({
                "resultType": "complete",
                "content": [{ "type": "text", "text": "no such document" }],
                "isError": true
            }),
        );
    let (client_side, _peer) = peer_pair(script);
    let connection = connect_over(&server("fixture"), client_side)
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    let result = connection
        .call_tool("search", &serde_json::Map::new())
        .await
        .unwrap_or_else(|error| panic!("a tool failure is a result, not an error: {error}"));
    assert!(!result.is_success());
    // The tool's own explanation is preserved, which is what a caller records as the failure reason.
    assert_eq!(result.text, "no such document");
}

/// A JSON-RPC error on `tools/call` is a **peer** error, not an unreachable one — the same distinction
/// `tools/list` already makes, asserted for the call path so the two cannot drift apart.
#[tokio::test]
async fn a_protocol_error_on_a_tool_call_is_not_reported_as_unreachable() {
    let script = ScriptedPeer::new()
        .answering("server/discover", discover_result("fixture", None, true))
        .failing("tools/call", -32602, "unknown tool");
    let (client_side, _peer) = peer_pair(script);
    let connection = connect_over(&server("fixture"), client_side)
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    let error = connection
        .call_tool("missing", &serde_json::Map::new())
        .await
        .err()
        .unwrap_or_else(|| panic!("a JSON-RPC error must not read as a successful call"));
    assert!(
        matches!(error, jarvis_mcp_transport::CallError::PeerError(_)),
        "{error}"
    );
    assert!(error.to_string().contains("unknown tool"), "{error}");
}

/// **An MRTR round is refused by name, not left to fail deeper in.** Revision `2026-07-28` lets a
/// server answer a call with `input_required` and expect the client to retry with answers. JARVIS
/// declares no input handler — the only human answer comes through JARVIS's own approval path — so a
/// round cannot be fulfilled. This uses the single-round request deliberately, which turns the mode
/// into a **named refusal** instead of an SDK error about a missing handler.
#[tokio::test]
async fn a_server_that_demands_client_input_is_refused_by_name() {
    // `InputRequiredResult` requires `resultType: "input_required"` **and at least one of**
    // `inputRequests` or `requestState` — its custom deserializer enforces both, precisely so the
    // variant cannot greedily match an unrelated object in the untagged union. `requestState` alone
    // is a valid round: the specification allows a server to ask the client to retry with an opaque
    // state handle, which is what a load-shedding server does, and it needs no client input handler
    // to *describe*.
    let script = ScriptedPeer::new()
        .answering("server/discover", discover_result("fixture", None, true))
        .answering(
            "tools/call",
            json!({ "resultType": "input_required", "requestState": "opaque-handle" }),
        );
    let (client_side, _peer) = peer_pair(script);
    let connection = connect_over(&server("fixture"), client_side)
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    let error = connection
        .call_tool("send_mail", &serde_json::Map::new())
        .await
        .err()
        .unwrap_or_else(|| panic!("an MRTR round cannot be fulfilled and must be refused"));
    assert!(
        matches!(error, jarvis_mcp_transport::CallError::InputRequired),
        "a demand for input must be refused by name, not as an opaque failure: {error}"
    );
    // The refusal points at the mechanism JARVIS actually has, so an operator is not sent hunting a
    // setting that does not exist.
    assert!(error.to_string().contains("approval"), "{error}");
}

/// **A task answer is refused by name.** `2026-07-28` moved Tasks to an extension and this crate
/// declares no tasks capability, so a server returning one is speaking a mode JARVIS never asked for.
/// Treating it as an empty success would tell the caller that work finished which had not started.
#[tokio::test]
async fn a_task_answer_is_refused_rather_than_read_as_success() {
    // `CreateTaskResult` **flattens** the seed task rather than nesting it under a `task` key — a fact
    // that is easy to get wrong, and the first version of this fixture did (it nested, so the untagged
    // union matched nothing and the refusal arrived as `Undecodable` instead of `Task`). `taskId`,
    // `status`, `createdAt`, and `lastUpdatedAt` are required; the rest are optional.
    let script = ScriptedPeer::new()
        .answering("server/discover", discover_result("fixture", None, true))
        .answering(
            "tools/call",
            json!({
                "resultType": "task",
                "taskId": "task-1",
                "status": "working",
                "createdAt": "2026-01-01T00:00:00Z",
                "lastUpdatedAt": "2026-01-01T00:00:00Z",
                "ttlMs": null
            }),
        );
    let (client_side, _peer) = peer_pair(script);
    let connection = connect_over(&server("fixture"), client_side)
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    let error = connection
        .call_tool("long_job", &serde_json::Map::new())
        .await
        .err()
        .unwrap_or_else(|| panic!("a task must not read as a completed call"));
    assert!(
        matches!(error, jarvis_mcp_transport::CallError::Task),
        "a task answer must be refused by name: {error}"
    );
    assert!(error.to_string().contains("task"), "{error}");
}

/// **A result whose shape contradicts its `resultType` is reported as undecodable, not as a network
/// fault.** The protocol's result union is deserialized as an **untagged** enum, so a mismatch is not
/// reported as "a malformed `input_required`" — it becomes the SDK's generic `UnexpectedResponse`.
/// Classifying that as "the call did not complete" would describe an unreachable peer and send an
/// operator to check a connection that is working, so it has its own variant.
#[tokio::test]
async fn a_result_whose_shape_contradicts_its_type_is_reported_as_undecodable() {
    // Declares `task` but carries none of the task fields, so no variant of the union can match.
    let script = ScriptedPeer::new()
        .answering("server/discover", discover_result("fixture", None, true))
        .answering(
            "tools/call",
            json!({ "resultType": "task", "unexpected": true }),
        );
    let (client_side, _peer) = peer_pair(script);
    let connection = connect_over(&server("fixture"), client_side)
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    let error = connection
        .call_tool("search", &serde_json::Map::new())
        .await
        .err()
        .unwrap_or_else(|| panic!("an undecodable result must not read as a success"));
    assert!(
        matches!(error, jarvis_mcp_transport::CallError::Undecodable(_)),
        "an answer that could not be decoded is a protocol disagreement, not an unreachable peer: \
         {error}"
    );
}

/// `CallError` must be constructible from plain values, for the same reason the other two are: a
/// caller that matched on an SDK variant would have a reason to keep the SDK in its dependency list.
#[test]
fn the_call_error_is_constructible_without_the_sdk() {
    let unavailable = jarvis_mcp_transport::CallError::Unavailable("peer left".to_owned());
    let peer = jarvis_mcp_transport::CallError::PeerError("unknown tool".to_owned());
    let undecodable =
        jarvis_mcp_transport::CallError::Undecodable("unexpected response".to_owned());
    let input = jarvis_mcp_transport::CallError::InputRequired;
    let task = jarvis_mcp_transport::CallError::Task;

    assert!(unavailable.to_string().contains("did not complete"));
    assert!(peer.to_string().contains("unknown tool"));
    assert!(undecodable.to_string().contains("untagged"));
    assert!(input.to_string().contains("MRTR"));
    assert!(task.to_string().contains("task"));
}
