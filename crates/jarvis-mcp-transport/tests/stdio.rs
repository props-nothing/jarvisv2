//! `connect_stdio` against a **real child process**.
//!
//! # What this closes
//!
//! `P3-008e` recorded this limit exactly: "`connect_stdio` and `connect_http` are **entirely
//! unexercised** — every test drives `connect_over` over an in-process duplex pair, so the
//! child-process spawn and the HTTP transport have no test at all, and `StdioCommand` being spawned is
//! the one path where a real OS error could appear."
//!
//! An in-process duplex pair cannot close that. It proves the framing and the negotiation; it says
//! nothing about whether the transport's spawn works, whether the child's stdio is wired the way the
//! framing assumes, or whether a program that does not exist surfaces as an actionable refusal rather
//! than a hang. Those are the failures a user hits first.
//!
//! # The fixture is a real program
//!
//! `src/bin/fixture_peer.rs` is a hand-written MCP server, enabled by the `fixture-peer` feature and
//! spawned through the same OS path an operator's configured server would take. It is not built by
//! `cargo build --workspace`, because the feature is off by default.
//!
//! # Why the tests skip rather than fail when the fixture is missing
//!
//! `cargo test --workspace` does not enable `fixture-peer`, so this binary is absent on a plain
//! workspace run. Failing then would make the default suite red for a reason unrelated to the code,
//! so the tests skip with a message naming the feature. CI enables the feature and sets
//! `ACCEPTANCE_REQUIRE_BINARIES=1`, which turns the skip into a failure — the same pattern the
//! phase gates use, and for the same reason.

use std::path::PathBuf;

use jarvis_mcp::ServerName;
use jarvis_mcp_transport::{StdioCommand, ToolBuffer, connect_stdio};

/// Environment variable that turns a missing-fixture skip into a failure.
///
/// Deliberately not prefixed `JARVIS_`: the daemon treats every unknown `JARVIS_*` variable as a
/// configuration error, so a harness variable there would stop the daemon from starting.
const REQUIRE_BINARIES_ENV: &str = "ACCEPTANCE_REQUIRE_BINARIES";

/// Locates the `fixture-peer` binary beside the current test executable.
///
/// Cargo places integration-test binaries in `target/<profile>/deps`, so the crate's binaries sit one
/// directory up in `target/<profile>`. Returns `None` when the fixture was not built.
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
/// Returns rather than exiting, because **a test binary runs all of its tests in one process**: an
/// `exit(0)` here would stop the suite after the first test and report the rest as absent, which is
/// indistinguishable from them passing. Each test therefore returns early on `None`.
///
/// When `ACCEPTANCE_REQUIRE_BINARIES=1` the absence is a failure instead, which is what lets CI prove
/// this file ran rather than skipped.
fn fixture_binary_or_skip() -> Option<PathBuf> {
    if let Some(path) = fixture_binary() {
        return Some(path);
    }
    let required = std::env::var(REQUIRE_BINARIES_ENV).is_ok_and(|value| value == "1");
    assert!(
        !required,
        "the fixture-peer binary is missing; run \
         `cargo test -p jarvis-mcp-transport --features fixture-peer` \
         (or `cargo build --workspace --features fixture-peer`)"
    );
    eprintln!(
        "SKIP: fixture-peer was not built; run with --features fixture-peer to exercise connect_stdio"
    );
    None
}

/// Builds a `StdioCommand` for the fixture with the given arguments.
fn fixture_command(arguments: &[&str]) -> StdioCommand {
    let fixture = fixture_binary_or_skip()
        .unwrap_or_else(|| panic!("a caller must check for the fixture before building a command"));
    StdioCommand::new(
        fixture
            .to_str()
            .unwrap_or_else(|| panic!("the fixture path must be UTF-8")),
        arguments,
    )
    .unwrap_or_else(|error| panic!("{error}"))
}

fn server(name: &str) -> ServerName {
    ServerName::new(name).unwrap_or_else(|error| panic!("fixture server {name}: {error}"))
}

/// **The claim this file exists to make: a real child process can be spawned, negotiated with, and
/// listed.** Everything else here is a refinement of it.
#[tokio::test]
async fn a_real_child_process_negotiates_and_lists_its_tools() {
    let Some(_) = fixture_binary_or_skip() else {
        return;
    };
    let command = fixture_command(&["--name", "fixture-vendor"]);

    let connection = connect_stdio(&server("fixture"), &command)
        .await
        .unwrap_or_else(|error| panic!("a real child process must negotiate: {error}"));

    assert_eq!(connection.negotiated_revision(), "2026-07-28");
    assert!(connection.capabilities().offers_tools());
    // The identity came out of the child's own `_meta`, so this also proves the fixture's framing and
    // the transport's reading of it agree.
    assert_eq!(connection.reported_identity().name, "fixture-vendor");

    let mut buffer = ToolBuffer::new();
    let listings = connection
        .list_tools(&mut buffer)
        .await
        .unwrap_or_else(|error| panic!("a real child's tool list must be readable: {error}"));
    assert_eq!(listings.len(), 2);
    assert_eq!(listings[0].name, "search");
    assert_eq!(listings[1].name, "fetch");

    let _ = connection.close().await;
}

/// **A program that does not exist is an actionable refusal, and it is bounded.** The failure mode
/// this guards is a hang: a transport that waited on a child which never spawned would block a daemon
/// at startup with no message. The fixture path is deliberately one that cannot exist.
#[tokio::test]
async fn a_missing_program_is_refused_rather_than_hung() {
    // A path that cannot be a real program on any platform this runs on.
    let missing = StdioCommand::new(
        "jarvis-fixture-peer-that-does-not-exist-12345",
        &["--name", "absent"],
    )
    .unwrap_or_else(|error| panic!("a non-empty program name is a valid command: {error}"));

    let started = std::time::Instant::now();
    let outcome = connect_stdio(&server("absent"), &missing).await;
    let elapsed = started.elapsed();

    let error = outcome
        .err()
        .unwrap_or_else(|| panic!("a program that cannot be spawned must not yield a connection"));
    assert!(
        matches!(error, jarvis_mcp_transport::ConnectError::Unreachable(_)),
        "a spawn failure is a transport fault, not a protocol refusal: {error}"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(30),
        "the refusal must be bounded, took {elapsed:?}"
    );
}

/// An empty program is refused before a spawn is attempted, so a configuration mistake names the
/// mistake rather than surfacing as an opaque OS error.
#[test]
fn an_empty_program_is_refused_by_name() {
    let error = StdioCommand::new("   ", &[])
        .err()
        .unwrap_or_else(|| panic!("an empty program must be refused"));
    assert!(
        error.to_string().contains("program"),
        "the refusal should say what is missing: {error}"
    );
}

/// The fixture's identity and tool list are its arguments, so one binary stands in for several
/// servers. This asserts that the arguments really reach the child — a transport that dropped the
/// arguments would still connect, and every assertion above would pass with the defaults.
#[tokio::test]
async fn the_servers_arguments_reach_the_child_process() {
    let Some(_) = fixture_binary_or_skip() else {
        return;
    };
    let command = fixture_command(&["--name", "renamed-vendor", "--tool", "only_tool"]);

    let connection = connect_stdio(&server("fixture"), &command)
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    // A different reported name than the default, which is the evidence the argument arrived.
    assert_eq!(connection.reported_identity().name, "renamed-vendor");
    let mut buffer = ToolBuffer::new();
    let listings = connection
        .list_tools(&mut buffer)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(listings.len(), 1);
    assert_eq!(listings[0].name, "only_tool");

    let _ = connection.close().await;
}

/// **`tools/call` over a real child process.** The transport can now run a tool, which `P3-008e`
/// explicitly recorded as absent ("No `tools/call`: this slice lists tools and stops").
#[tokio::test]
async fn a_tool_call_runs_against_a_real_child_process() {
    let Some(_) = fixture_binary_or_skip() else {
        return;
    };
    let command = fixture_command(&["--name", "fixture-vendor"]);
    let connection = connect_stdio(&server("fixture"), &command)
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    let mut arguments = serde_json::Map::new();
    arguments.insert("q".to_owned(), serde_json::json!("pumps"));
    let result = connection
        .call_tool("search", &arguments)
        .await
        .unwrap_or_else(|error| panic!("a real tool call must return: {error}"));

    assert!(result.is_success());
    assert_eq!(result.text, "search ran");

    let _ = connection.close().await;
}

/// The server's own tool name is what a call must carry, and a call to a second tool proves the name
/// is per-call rather than constant: a transport that sent the last listed name would satisfy a
/// one-tool assertion.
#[tokio::test]
async fn each_call_carries_the_name_it_was_given() {
    let Some(_) = fixture_binary_or_skip() else {
        return;
    };
    let command = fixture_command(&["--name", "fixture-vendor"]);
    let connection = connect_stdio(&server("fixture"), &command)
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    let arguments = serde_json::Map::new();
    let first = connection
        .call_tool("search", &arguments)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let second = connection
        .call_tool("fetch", &arguments)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(first.text, "search ran");
    assert_eq!(second.text, "fetch ran");
    assert_ne!(
        first.text, second.text,
        "the name must reach the server per call, not be cached"
    );

    let _ = connection.close().await;
}
