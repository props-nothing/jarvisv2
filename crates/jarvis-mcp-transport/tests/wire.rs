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
        other => panic!("expected a peer error, got {other:?}"),
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
