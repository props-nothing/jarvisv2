//! The MCP host role, driven from **configuration text** to a working tool call.
//!
//! # What this closes
//!
//! Nine MCP slices recorded the same limit: "a capability a caller can use, not one an operator can
//! reach". Every piece worked and nothing read a configuration file. These tests start from a TOML
//! document — what an operator would write — and end with a canonical tool definition and a call that
//! runs, which is the whole journey.
//!
//! # Why the child process is the transport under test
//!
//! A stdio server is spawned by the configuration itself, so a passing test proves the *program string an
//! operator wrote* is what gets executed. An in-process fixture would prove the configuration parsed and
//! nothing about the spawn — and the spawn is the part an operator's mistake breaks.
//!
//! The fixture is the same hand-written `fixture-peer` binary the stdio tests use, so no SDK server is
//! involved and a disagreement with the specification would fail rather than pass.
//!
//! # The two properties worth being able to see fail
//!
//! 1. **A server nobody classified is unsafe by default.** The default must be the *severe* posture, so
//!    omitting a posture is a refusal rather than a permission.
//! 2. **A collision refuses the whole host.** Serving a catalog whose contents depended on configuration
//!    order would make the reachable tool set a function of an ordering nobody declared meaningful.

use std::fmt::Write as _;
use std::path::PathBuf;

use jarvis_mcp::{NamingStrategy, ToolEffectPolicy};
use jarvis_mcp_transport::McpHostConfig;
use jarvis_tools::ApprovalPolicy;

/// Builds a tool execution request bound to a receipt **derived** from a real policy decision.
///
/// The receipt cannot be fabricated: `ToolExecutionRequest::new` recomputes the canonical intent hash and
/// compares it against the receipt's, so a hand-built fixture would fail its own binding check. Deriving it
/// from `evaluate` over the same definition and arguments is the only way to build a valid request — which
/// is exactly the property `P3-006b` added, exercised here rather than worked around.
fn request_for(
    definition: &jarvis_tools::ToolDefinition,
    arguments: serde_json::Value,
) -> jarvis_tools::ToolExecutionRequest {
    use jarvis_core::{CorrelationId, SystemClock, UtcTimestamp};
    use jarvis_tools::{
        AuthorizationReceipt, AuthorizationReceiptParts, IdempotencyKey, ToolExecutionRequest,
        ToolExecutionRequestParts,
    };

    let now = UtcTimestamp::now(&SystemClock);
    let expires = UtcTimestamp::from_unix_nanos(now.unix_nanos() + 60_000_000_000)
        .unwrap_or_else(|error| panic!("{error}"));

    // The tool is read-only and requires `mcp.call`, so a credential over a CLI channel with that scope
    // granted is an allowance. Asserted rather than assumed: a held decision could not produce a receipt.
    let decision = jarvis_tools::evaluate(&jarvis_tools::PolicyRequest {
        definition,
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
        "the fixture's definition must be allowed for a request to be buildable: {decision:?}"
    );

    let intent = jarvis_core::CanonicalIntentHash::compute(
        &definition.id().to_string(),
        definition.version(),
        &arguments,
    )
    .unwrap_or_else(|error| panic!("{error}"));

    let receipt = AuthorizationReceipt::new(AuthorizationReceiptParts {
        receipt_id: "receipt-host".to_owned(),
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
        call_id: "call-host".to_owned(),
        tool: definition.id().clone(),
        tool_version: definition.version().to_owned(),
        arguments,
        receipt,
        idempotency_key: IdempotencyKey::parse("0123456789abcdef0123456789abcdef")
            .unwrap_or_else(|error| panic!("{error}")),
        deadline: expires,
        correlation_id: CorrelationId::new(),
    })
    .unwrap_or_else(|error| panic!("the request must bind to its receipt: {error}"))
}

/// Environment variable that turns a missing-fixture skip into a failure.
const REQUIRE_BINARIES_ENV: &str = "ACCEPTANCE_REQUIRE_BINARIES";

/// Locates the `fixture-peer` binary beside the current test executable.
fn fixture_binary() -> Option<PathBuf> {
    let mut directory = std::env::current_exe().ok()?;
    directory.pop();
    if directory.file_name().is_some_and(|name| name == "deps") {
        directory.pop();
    }
    let candidate = directory.join(format!("fixture-peer{}", std::env::consts::EXE_SUFFIX));
    candidate.is_file().then_some(candidate)
}

/// The fixture path, or `None` when it was not built.
///
/// Returns rather than exiting, because a test binary runs every test in one process — an `exit` here
/// would silently skip the rest.
fn fixture_or_skip() -> Option<PathBuf> {
    if let Some(path) = fixture_binary() {
        return Some(path);
    }
    let required = std::env::var(REQUIRE_BINARIES_ENV).is_ok_and(|value| value == "1");
    assert!(
        !required,
        "the fixture-peer binary is missing; run \
         `cargo test -p jarvis-mcp-transport --features fixture-peer`"
    );
    eprintln!("SKIP: fixture-peer was not built; run with --features fixture-peer");
    None
}

/// Builds a host configuration document for one stdio server.
///
/// The posture line is included only when a class is given, so a test can exercise the **default** by
/// omitting it — which is the property that matters most here.
///
/// # Two fixture shapes were wrong before this one, and both are worth knowing
///
/// 1. **TOML forbids re-opening a table after an inline-table value.** Writing `transport = { ... }` and
///    then a `posture = { ... }` line is a syntax error. Each key therefore gets its own `[servers.*]`
///    table.
/// 2. **A Windows path in a basic string is invalid TOML.** `r"C:\Users\..."` contains `\U`, which begins an
///    invalid escape — so on this machine every document was refused as a syntax error. The path is
///    therefore emitted in a **literal** string (`'…'`), where backslashes are literal. That is also why the
///    error message now advises an operator to use single quotes for a Windows path: the first version of
///    the message blamed an unknown key, which was a cause it could not know.
fn document_for(path: &std::path::Path, name: &str, posture: Option<&str>) -> String {
    let program = path
        .to_str()
        .unwrap_or_else(|| panic!("the fixture path must be UTF-8"));
    let mut document = format!(
        "[[servers]]\nname = \"{name}\"\n\
         [servers.transport]\nkind = \"stdio\"\nprogram = '{program}'\n\
         args = [\"--name\", \"fixture-vendor\"]\n"
    );
    if let Some(class) = posture {
        // `writeln!` rather than `push_str(&format!(..))`, which clippy correctly prefers.
        let _ = writeln!(document, "[servers.posture]\nclass = \"{class}\"");
    }
    document
}

/// **The whole journey: configuration text becomes a canonical tool that can be called.** Everything else
/// in this file is a refinement of it.
#[tokio::test]
async fn a_configured_stdio_server_becomes_a_callable_tool() {
    let Some(fixture) = fixture_or_skip() else {
        return;
    };
    let document = document_for(&fixture, "local", Some("read-only"));
    let config = McpHostConfig::parse(&document).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(config.len(), 1);

    let host = config
        .connect(NamingStrategy::Prefixed, &[])
        .await
        .unwrap_or_else(|error| panic!("a configured server must connect: {error}"));

    assert!(host.unreadable().is_empty(), "{:?}", host.unreadable());
    assert_eq!(host.tool_count(), 2);
    let mut ids: Vec<String> = host
        .definitions()
        .iter()
        .map(|definition| definition.id().to_string())
        .collect();
    ids.sort();
    assert_eq!(ids, vec!["mcp.local.fetch", "mcp.local.search"]);

    // **The operator's posture reached the definition.** The configuration said read-only, so the tool is
    // risk 0 and runs without asking — and this is the positive control for the default-posture test below.
    for definition in host.definitions() {
        assert_eq!(definition.risk().level(), 0);
        assert_eq!(definition.approval(), ApprovalPolicy::Auto);
    }

    // And an adapter can be found for a tool the host offers, which is what a dispatch needs.
    let id = host.definitions()[0].id().clone();
    let adapter = host
        .adapter_for(&id)
        .unwrap_or_else(|| panic!("a host must find the adapter for a tool it offers"));
    assert_eq!(adapter.server(), "local");
    // An unknown tool has no adapter, so a dispatch cannot default to another server's.
    let foreign =
        jarvis_tools::ToolId::new("mcp.elsewhere.search").unwrap_or_else(|error| panic!("{error}"));
    assert!(host.adapter_for(&foreign).is_none());

    assert!(host.close().await.is_ok(), "a clean shutdown must succeed");
}

/// **The default posture is severe.** An operator who omits the posture gets a server every call of which
/// is held, so leaving something out fails closed. The assertion is the *opposite* of the test above,
/// which is what makes the pair meaningful.
#[tokio::test]
async fn a_server_with_no_stated_posture_defaults_to_held() {
    let Some(fixture) = fixture_or_skip() else {
        return;
    };
    let document = document_for(&fixture, "unclassified", None);
    let config = McpHostConfig::parse(&document).unwrap_or_else(|error| panic!("{error}"));

    let host = config
        .connect(NamingStrategy::Prefixed, &[])
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    assert_eq!(host.tool_count(), 2);
    for definition in host.definitions() {
        assert_eq!(
            definition.risk().level(),
            3,
            "an unclassified server must get the most scrutiny"
        );
        assert_eq!(definition.approval(), ApprovalPolicy::Ask);
    }
    assert!(host.close().await.is_ok());
}

/// **A collision refuses the whole host**, which is the first caller of
/// `McpCatalog::has_cross_server_collision()`. Two servers offering the same tool under a strategy that
/// drops the namespace cannot be served: which one survived would depend on the order they were written.
#[tokio::test]
async fn a_cross_server_collision_refuses_the_host() {
    let Some(fixture) = fixture_or_skip() else {
        return;
    };
    // Two entries, two names, **one program**. Both offer `search`, and `Bare` drops the namespace, so the
    // identifiers collide — the condition the catalog reports and nothing until now acted on.
    let program = fixture
        .to_str()
        .unwrap_or_else(|| panic!("the fixture path must be UTF-8"));
    let document = format!(
        "[[servers]]\nname = \"alpha\"\n[servers.transport]\nkind = \"stdio\"\nprogram = '{program}'\n\n\
         [[servers]]\nname = \"bravo\"\n[servers.transport]\nkind = \"stdio\"\nprogram = '{program}'\n"
    );
    let config = McpHostConfig::parse(&document).unwrap_or_else(|error| panic!("{error}"));

    let error = config
        .connect(NamingStrategy::Bare, &[])
        .await
        .err()
        .unwrap_or_else(|| panic!("a collision must refuse the host"));
    assert!(
        matches!(error, jarvis_mcp_transport::HostError::Collision { .. }),
        "a collision is a refusal to serve, not a silent drop: {error}"
    );
    // The refusal names the servers, because the remedy is an operator renaming one of them.
    let text = error.to_string();
    assert!(text.contains("alpha"), "{text}");
    assert!(text.contains("bravo"), "{text}");

    // **The positive control**: the same two servers with a strategy that keeps them apart connect fine,
    // so the refusal above is about the collision rather than about the configuration being unusable.
    let separated = config
        .connect(NamingStrategy::Prefixed, &[])
        .await
        .unwrap_or_else(|error| panic!("a namespaced strategy must keep them apart: {error}"));
    assert_eq!(separated.tool_count(), 4);
    assert!(separated.close().await.is_ok());
}

/// A configured program that does not exist does not fail the host; it is reported by name with the
/// transport's own reason, and the other servers' tools survive. One flaky third-party process must not
/// empty the model's tool list.
#[tokio::test]
async fn an_unreachable_server_is_reported_and_the_others_survive() {
    let Some(fixture) = fixture_or_skip() else {
        return;
    };
    let program = fixture
        .to_str()
        .unwrap_or_else(|| panic!("the fixture path must be UTF-8"));
    let document = format!(
        "[[servers]]\nname = \"good\"\n[servers.transport]\nkind = \"stdio\"\nprogram = '{program}'\n\n\
         [[servers]]\nname = \"absent\"\n[servers.transport]\nkind = \"stdio\"\nprogram = \"jarvis-no-such-program-98765\"\n"
    );
    let config = McpHostConfig::parse(&document).unwrap_or_else(|error| panic!("{error}"));

    let host = config
        .connect(NamingStrategy::Prefixed, &[])
        .await
        .unwrap_or_else(|error| panic!("one unreachable server must not fail the host: {error}"));

    // The good server's tools are offered.
    assert_eq!(host.tool_count(), 2);
    // And the bad one is reported with a reason that distinguishes it from an empty list.
    assert_eq!(host.unreadable().len(), 1);
    assert_eq!(host.unreadable()[0].server.as_str(), "absent");
    assert!(
        host.unreadable()[0].reason.contains("could not be reached"),
        "{}",
        host.unreadable()[0].reason
    );
    assert!(host.close().await.is_ok());
}

/// **A tool offered by a configured host actually runs.** This is the last link: configuration → connection
/// → catalog → adapter → a call through the real pipeline. Without it the host could be a well-typed
/// description of something that does not work.
#[tokio::test]
async fn a_tool_from_a_configured_host_runs() {
    let Some(fixture) = fixture_or_skip() else {
        return;
    };
    let document = document_for(&fixture, "local", Some("read-only"));
    let config = McpHostConfig::parse(&document).unwrap_or_else(|error| panic!("{error}"));
    let host = config
        .connect(NamingStrategy::Prefixed, &[])
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    // Build a request exactly as the pipeline would: a receipt derived from a real policy decision over the
    // same arguments, so the binding check `P3-006b` added is satisfied rather than bypassed.
    let definition = host
        .definitions()
        .into_iter()
        .find(|definition| definition.id().name() == "search")
        .unwrap_or_else(|| panic!("the fixture offers a search tool"));
    let request = request_for(&definition, serde_json::json!({ "q": "pumps" }));

    let adapter = host
        .adapter_for(definition.id())
        .unwrap_or_else(|| panic!("the host must route its own tool"));
    let result = jarvis_tools::ToolExecutor::execute(adapter, &request)
        .await
        .unwrap_or_else(|error| panic!("a configured tool must run: {error}"));

    assert_eq!(result.outcome(), jarvis_tools::ToolOutcome::Confirmed);
    assert_eq!(
        result
            .evidence()
            .unwrap_or_else(|| panic!("a confirmation must carry evidence"))
            .as_str(),
        "mcp:local/search"
    );
    // The fixture echoes the tool name back, so this also proves the *server-side* name reached it.
    assert!(
        result
            .output()
            .is_some_and(|output| output.content().contains("search ran")),
        "{:?}",
        result.output().map(jarvis_tools::BoundedOutput::content)
    );

    assert!(host.close().await.is_ok());
}

/// A remote server configured with `https` is accepted; the same server over plaintext is refused when the
/// **file is read**, so an operator learns about it before any connection is attempted.
#[tokio::test]
async fn an_endpoint_refusal_happens_at_parse_time_not_connect_time() {
    let remote = "[[servers]]\nname = \"remote\"\ntransport = { kind = \"http\", endpoint = \"https://mcp.example.com/mcp\" }";
    let config = McpHostConfig::parse(remote).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(config.len(), 1);

    let plaintext = "[[servers]]\nname = \"remote\"\ntransport = { kind = \"http\", endpoint = \"http://mcp.example.com/mcp\" }";
    let error = McpHostConfig::parse(plaintext)
        .err()
        .unwrap_or_else(|| panic!("plaintext remote must be refused when the file is read"));
    assert!(error.to_string().contains("https"), "{error}");
}

/// The posture vocabulary is exactly two classes, and both are reachable. A third value would need a
/// vocabulary nothing consumes; the refusal names the accepted set instead of guessing.
#[test]
fn only_two_posture_classes_are_accepted() {
    for class in ["read-only", "unclassified"] {
        let document = format!(
            "[[servers]]\nname = \"a\"\ntransport = {{ kind = \"stdio\", program = \"p\" }}\nposture = {{ class = \"{class}\" }}"
        );
        assert!(
            McpHostConfig::parse(&document).is_ok(),
            "{class} must be accepted"
        );
    }
    for rejected in ["read_only", "READ-ONLY", "readonly", "auto", "trusted", ""] {
        let document = format!(
            "[[servers]]\nname = \"a\"\ntransport = {{ kind = \"stdio\", program = \"p\" }}\nposture = {{ class = \"{rejected}\" }}"
        );
        assert!(
            McpHostConfig::parse(&document).is_err(),
            "{rejected} must be refused rather than silently becoming a permissive posture"
        );
    }
    // And a policy built from the class is the crate's own, not a second implementation: this asserts the
    // two agree by construction.
    let unclassified = ToolEffectPolicy::unclassified();
    assert_eq!(unclassified.posture().risk, 3);
}
