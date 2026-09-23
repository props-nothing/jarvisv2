//! The host join: live connections become one catalog of canonically-named tools.
//!
//! # What these tests are for
//!
//! `P3-008a`..`P3-008e` proved the pure rules and proved the negotiation, but **nothing joined
//! them** — the catalog had never been built from a listing anybody actually fetched. A type-level
//! test cannot close that: `ServerListing` can be constructed by hand, so a catalog test proves the
//! catalog and says nothing about whether a *fetched* listing becomes one correctly.
//!
//! So every test here goes through a real [`connect_over`] negotiation and a real `tools/list`
//! exchange. The scripted peer is the same hand-written one `wire.rs` uses, for the same reason: a
//! fixture built from the SDK would share the SDK's assumptions.
//!
//! # The two joins worth being able to see fail
//!
//! 1. **The reported identity must come from the connection, not the listing.** The identity is what
//!    `P3-008d` compares to detect a server that changed behind a stable operator name. If the host
//!    invented it or read it from the wrong place, the drift check would compare a value with itself
//!    and never fire — a guard that cannot fail.
//! 2. **One unreadable server must not remove the others.** A third-party process that is down is
//!    ordinary; if it emptied the tool list, an operator would see "no tools" rather than "one server
//!    is down", and the remedy would be unguessable.

mod support;

use std::sync::Arc;

use jarvis_mcp::NamingStrategy;
use jarvis_mcp_transport::{HostedServer, build_catalog, connect_over};
use serde_json::json;
use support::{PeerHandle, ScriptedPeer, announced_safe_tool, discover_result, tool, tools_result};

/// Two duplex halves plus a spawned peer, with the client half ready to hand to `connect_over`.
fn peer_pair(script: ScriptedPeer) -> (tokio::io::DuplexStream, PeerHandle) {
    let (client_side, server_side) = tokio::io::duplex(64 * 1024);
    (client_side, PeerHandle::spawn(server_side, script))
}

fn server(name: &str) -> jarvis_mcp::ServerName {
    jarvis_mcp::ServerName::new(name)
        .unwrap_or_else(|error| panic!("fixture server {name}: {error}"))
}

/// A peer that negotiates modern, reports `reported_name`, and lists the given tools.
fn script_for(reported_name: &str, tools: &[serde_json::Value]) -> ScriptedPeer {
    ScriptedPeer::new()
        .answering(
            "server/discover",
            discover_result(reported_name, Some("Fixture"), true),
        )
        .answering("tools/list", tools_result(tools))
}

/// **The whole join, end to end.** A configured server is connected, its tools are fetched over the
/// wire, translated into canonical definitions, and aggregated — which is the step no previous slice
/// could test.
#[tokio::test]
async fn a_live_connection_becomes_a_catalog_with_canonical_names() {
    let script = script_for(
        "vendor-product",
        &[
            tool("search", Some("Search"), Some("Search the corpus")),
            tool("fetch", None, None),
        ],
    );
    let (client_side, _peer) = peer_pair(script);
    let connection = connect_over(&server("acme"), client_side)
        .await
        .unwrap_or_else(|error| panic!("a modern peer must negotiate: {error}"));

    let hosted = vec![HostedServer {
        configured: jarvis_mcp::ConfiguredServer {
            name: server("acme"),
            policy: jarvis_mcp::ToolEffectPolicy::unclassified(),
        },
        connection: Arc::new(connection),
    }];

    let build = build_catalog(&hosted, NamingStrategy::Prefixed, &[])
        .await
        .unwrap_or_else(|error| panic!("a well-formed configuration must build: {error}"));

    assert!(build.unreadable.is_empty(), "{:?}", build.unreadable);
    assert_eq!(build.catalog.len(), 2);
    // The names are canonical and namespaced, which is what makes the `mcp.` source derivation work.
    let mut ids: Vec<String> = build
        .catalog
        .entries()
        .iter()
        .map(|entry| entry.definition.id().to_string())
        .collect();
    ids.sort();
    assert_eq!(ids, vec!["mcp.acme.fetch", "mcp.acme.search"]);
    // And every tool carries the operator's severe posture, because nobody classified this server.
    for entry in build.catalog.entries() {
        assert_eq!(entry.definition.risk().level(), 3);
    }
}

/// **The identity comes from the connection.** `P3-008d` compares a server's self-report between
/// builds to notice that the process behind an operator's name changed. That check is only meaningful
/// if the value it compares is the server's own claim — a host that defaulted it, or read it from the
/// tool result, would make the drift undetectable while every type still matched.
#[tokio::test]
async fn the_catalog_records_the_servers_own_claim_and_the_operator_name_separately() {
    let script = script_for("vendor-product", &[tool("search", None, None)]);
    let (client_side, _peer) = peer_pair(script);
    let connection = connect_over(&server("acme"), client_side)
        .await
        .unwrap_or_else(|error| panic!("a modern peer must negotiate: {error}"));

    let hosted = vec![HostedServer {
        configured: jarvis_mcp::ConfiguredServer {
            name: server("acme"),
            policy: jarvis_mcp::ToolEffectPolicy::unclassified(),
        },
        connection: Arc::new(connection),
    }];
    let build = build_catalog(&hosted, NamingStrategy::Prefixed, &[])
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    let observed = build.catalog.observed();
    assert_eq!(observed.len(), 1);
    // The operator's name, which the server had no say in.
    assert_eq!(observed[0].server.as_str(), "acme");
    // The server's own claim, which is evidence rather than an identifier.
    assert_eq!(observed[0].reported.name, "vendor-product");
    assert_ne!(
        observed[0].reported.name,
        observed[0].server.as_str(),
        "the two must stay separate or a drift check compares a value with itself"
    );
}

/// **A drift is observable across two host builds**, which is what makes `P3-008d`'s check reachable
/// rather than merely implemented. The first build establishes the baseline; the second, from a peer
/// reporting a different name under the same operator name, records exactly one drift.
#[tokio::test]
async fn a_server_that_changes_its_claim_is_reported_as_a_drift_across_builds() {
    let hosted_with = |reported: &'static str, script: ScriptedPeer| async move {
        let (client_side, _peer) = peer_pair(script);
        let connection = connect_over(&server("acme"), client_side)
            .await
            .unwrap_or_else(|error| panic!("a modern peer must negotiate: {error}"));
        let _ = reported;
        vec![HostedServer {
            configured: jarvis_mcp::ConfiguredServer {
                name: server("acme"),
                policy: jarvis_mcp::ToolEffectPolicy::unclassified(),
            },
            connection: Arc::new(connection),
        }]
    };

    let first = build_catalog(
        &hosted_with(
            "vendor-product",
            script_for("vendor-product", &[tool("search", None, None)]),
        )
        .await,
        NamingStrategy::Prefixed,
        &[],
    )
    .await
    .unwrap_or_else(|error| panic!("{error}"));
    assert!(
        first.catalog.drifts().is_empty(),
        "a first build has no baseline"
    );

    // The same operator name, a different process behind it.
    let second = build_catalog(
        &hosted_with(
            "replacement",
            script_for("replacement", &[tool("search", None, None)]),
        )
        .await,
        NamingStrategy::Prefixed,
        first.catalog.observed(),
    )
    .await
    .unwrap_or_else(|error| panic!("{error}"));

    assert_eq!(second.catalog.drifts().len(), 1);
    let drift = &second.catalog.drifts()[0];
    assert_eq!(drift.server, "acme");
    assert_eq!(drift.before.name, "vendor-product");
    assert_eq!(drift.after.name, "replacement");
}

/// **One unavailable server does not remove the others' tools.** The positive control comes first:
/// the reachable server's tool really is offered, so the assertion is not passing on an empty
/// catalog. Then a second server whose list fails is reported by name with a reason, and the first
/// server's tool survives.
#[tokio::test]
async fn one_unreadable_server_does_not_empty_the_catalog() {
    let good = script_for("good-vendor", &[tool("search", None, None)]);
    let bad = ScriptedPeer::new()
        .answering("server/discover", discover_result("bad-vendor", None, true))
        .failing("tools/list", -32603, "internal error");

    let (good_client, _good_peer) = peer_pair(good);
    let (bad_client, _bad_peer) = peer_pair(bad);
    let good_connection = connect_over(&server("good"), good_client)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let bad_connection = connect_over(&server("bad"), bad_client)
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    let hosted = vec![
        HostedServer {
            configured: jarvis_mcp::ConfiguredServer {
                name: server("good"),
                policy: jarvis_mcp::ToolEffectPolicy::unclassified(),
            },
            connection: Arc::new(good_connection),
        },
        HostedServer {
            configured: jarvis_mcp::ConfiguredServer {
                name: server("bad"),
                policy: jarvis_mcp::ToolEffectPolicy::unclassified(),
            },
            connection: Arc::new(bad_connection),
        },
    ];

    let build = build_catalog(&hosted, NamingStrategy::Prefixed, &[])
        .await
        .unwrap_or_else(|error| panic!("an unreadable server must not fail the build: {error}"));

    // Positive control: the good server's tool is really here.
    assert_eq!(build.catalog.len(), 1);
    assert_eq!(
        build.catalog.entries()[0].definition.id().to_string(),
        "mcp.good.search"
    );
    // And the failure is reported by name, with a reason that distinguishes it from an empty list.
    assert_eq!(build.unreadable.len(), 1);
    assert_eq!(build.unreadable[0].server.as_str(), "bad");
    assert!(
        build.unreadable[0].reason.contains("refused"),
        "a protocol error must be described as a refusal, not as unreachable: {}",
        build.unreadable[0].reason
    );
}

/// A server that declared **no tools capability** is reported as such rather than asked. "This server
/// does not do tools" and "this server has no tools" are different facts, and only the first is
/// knowable before the request — so an operator is told which one applies.
#[tokio::test]
async fn a_server_without_a_tools_capability_is_reported_without_being_asked() {
    // `tools: false` means the discover result declares no tools capability.
    let script =
        ScriptedPeer::new().answering("server/discover", discover_result("no-tools", None, false));
    let (client_side, peer) = peer_pair(script);
    let connection = connect_over(&server("quiet"), client_side)
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    let hosted = vec![HostedServer {
        configured: jarvis_mcp::ConfiguredServer {
            name: server("quiet"),
            policy: jarvis_mcp::ToolEffectPolicy::unclassified(),
        },
        connection: Arc::new(connection),
    }];
    let build = build_catalog(&hosted, NamingStrategy::Prefixed, &[])
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    assert!(build.catalog.is_empty());
    assert_eq!(build.unreadable.len(), 1);
    assert!(
        build.unreadable[0].reason.contains("no tools capability"),
        "{}",
        build.unreadable[0].reason
    );
    // And it was never asked: a request that was not made cannot be a request that failed.
    drop(hosted);
    let seen = peer.finish().await;
    assert!(
        !seen.saw("tools/list"),
        "a server that declared no tools capability must not be asked for its tools: {:?}",
        seen.methods()
    );
}

/// **The server's annotations still get no say, all the way through the join.** This is `ADR-0025`
/// asserted at the outermost seam rather than at the translation: a tool that announces itself
/// read-only and idempotent must arrive with the operator's severe posture, because the host applies
/// the configured policy and nothing else.
#[tokio::test]
async fn a_self_declared_safe_tool_still_arrives_with_the_operators_posture() {
    let script = ScriptedPeer::new()
        .answering("server/discover", discover_result("fixture", None, true))
        .answering(
            "tools/list",
            tools_result(&[announced_safe_tool("send-mail")]),
        );
    let (client_side, _peer) = peer_pair(script);
    let connection = connect_over(&server("acme"), client_side)
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    let hosted = vec![HostedServer {
        configured: jarvis_mcp::ConfiguredServer {
            name: server("acme"),
            policy: jarvis_mcp::ToolEffectPolicy::unclassified(),
        },
        connection: Arc::new(connection),
    }];
    let build = build_catalog(&hosted, NamingStrategy::Prefixed, &[])
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    let entry = &build.catalog.entries()[0];
    assert_eq!(entry.remote, "send-mail");
    assert_eq!(
        entry.definition.risk().level(),
        3,
        "a server's own claim must not lower the risk at any seam"
    );
    assert_eq!(
        entry.definition.approval(),
        jarvis_tools::ApprovalPolicy::Ask,
        "an unclassified server's every call is held"
    );
}

/// A **read-only** classification is honoured through the join, which is the positive control for the
/// severe default above: if every server came out at risk 3, the policy would be ignored and the
/// previous test would pass for the wrong reason.
#[tokio::test]
async fn a_classified_read_only_server_reaches_the_catalog_at_risk_zero() {
    let script = script_for("vendor", &[tool("search", None, None)]);
    let (client_side, _peer) = peer_pair(script);
    let connection = connect_over(&server("acme"), client_side)
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    let hosted = vec![HostedServer {
        configured: jarvis_mcp::ConfiguredServer {
            name: server("acme"),
            policy: jarvis_mcp::ToolEffectPolicy::read_only()
                .unwrap_or_else(|error| panic!("{error}")),
        },
        connection: Arc::new(connection),
    }];
    let build = build_catalog(&hosted, NamingStrategy::Prefixed, &[])
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    let entry = &build.catalog.entries()[0];
    assert_eq!(entry.definition.risk().level(), 0);
    assert_eq!(
        entry.definition.approval(),
        jarvis_tools::ApprovalPolicy::Auto
    );
}

/// An ambiguous configuration is refused by the catalog rather than partially served, and the refusal
/// survives the join: two servers under one operator name is the operator's ambiguity to resolve.
#[tokio::test]
async fn two_servers_under_one_name_are_refused_through_the_join() {
    let (first_client, _first_peer) =
        peer_pair(script_for("vendor", &[tool("search", None, None)]));
    let (second_client, _second_peer) =
        peer_pair(script_for("vendor", &[tool("fetch", None, None)]));
    let first = connect_over(&server("acme"), first_client)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let second = connect_over(&server("acme"), second_client)
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    let hosted = vec![
        HostedServer {
            configured: jarvis_mcp::ConfiguredServer {
                name: server("acme"),
                policy: jarvis_mcp::ToolEffectPolicy::unclassified(),
            },
            connection: Arc::new(first),
        },
        HostedServer {
            configured: jarvis_mcp::ConfiguredServer {
                name: server("acme"),
                policy: jarvis_mcp::ToolEffectPolicy::unclassified(),
            },
            connection: Arc::new(second),
        },
    ];

    let error = build_catalog(&hosted, NamingStrategy::Prefixed, &[])
        .await
        .err()
        .unwrap_or_else(|| panic!("one name configured twice must be refused"));
    assert!(
        error.to_string().contains("acme"),
        "the refusal must name the server: {error}"
    );
}

/// A **cross-server collision** is reported through the join, so the "refuse to serve" signal a
/// daemon needs is reachable from a real configuration. `Bare` drops the namespace, so two servers
/// offering `search` collide — which is the case the whole catalog exists to detect.
#[tokio::test]
async fn a_cross_server_collision_is_visible_from_a_live_configuration() {
    let (alpha_client, _alpha_peer) =
        peer_pair(script_for("alpha-vendor", &[tool("search", None, None)]));
    let (bravo_client, _bravo_peer) =
        peer_pair(script_for("bravo-vendor", &[tool("search", None, None)]));
    let alpha = connect_over(&server("alpha"), alpha_client)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let bravo = connect_over(&server("bravo"), bravo_client)
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    let hosted = vec![
        HostedServer {
            configured: jarvis_mcp::ConfiguredServer {
                name: server("alpha"),
                policy: jarvis_mcp::ToolEffectPolicy::unclassified(),
            },
            connection: Arc::new(alpha),
        },
        HostedServer {
            configured: jarvis_mcp::ConfiguredServer {
                name: server("bravo"),
                policy: jarvis_mcp::ToolEffectPolicy::unclassified(),
            },
            connection: Arc::new(bravo),
        },
    ];

    let build = build_catalog(&hosted, NamingStrategy::Bare, &[])
        .await
        .unwrap_or_else(|error| panic!("a collision is reported, not an error: {error}"));
    assert!(
        build.catalog.has_cross_server_collision(),
        "two servers on one bare name must be visible as a collision"
    );
    // Both servers genuinely answered, so the collision is about them and not about a failed read.
    assert!(build.unreadable.is_empty(), "{:?}", build.unreadable);
    assert_eq!(build.catalog.collisions().len(), 1);
}

/// The catalog's routing carries the **server's own** tool name, which is what a call must send. A
/// join that routed to the canonical identifier would send the server a name it never listed, and the
/// translation would have been applied twice.
#[tokio::test]
async fn routing_carries_the_server_name_a_call_must_send() {
    let script = script_for("vendor", &[tool("list_issues", None, None)]);
    let (client_side, _peer) = peer_pair(script);
    let connection = connect_over(&server("acme"), client_side)
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    let hosted = vec![HostedServer {
        configured: jarvis_mcp::ConfiguredServer {
            name: server("acme"),
            policy: jarvis_mcp::ToolEffectPolicy::unclassified(),
        },
        connection: Arc::new(connection),
    }];
    let build = build_catalog(&hosted, NamingStrategy::Prefixed, &[])
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    let entry = &build.catalog.entries()[0];
    assert_eq!(entry.definition.id().to_string(), "mcp.acme.list_issues");
    assert_eq!(entry.remote, "list_issues");
    assert_eq!(entry.server.as_str(), "acme");
    // And the lookup by identifier gives the same routing, so a call needs no second lookup.
    let routed = build
        .catalog
        .route(entry.definition.id())
        .unwrap_or_else(|| panic!("a held tool must be routable"));
    assert_eq!(routed.remote, entry.remote);
    let _ = json!({});
}
