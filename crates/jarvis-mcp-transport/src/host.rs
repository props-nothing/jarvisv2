//! The MCP host: from live connections to one catalog of canonically-named tools.
//!
//! # The gap this module closes
//!
//! `P3-008a`..`P3-008e` each recorded the same honest limit in slightly different words: **a
//! capability a caller can use, not one an operator can reach**. The pure crate could translate a
//! listing nobody had fetched, and the transport could fetch a listing nobody had translated, and
//! **nothing joined the two**. This is the join, and it is deliberately the last thing built because
//! it is the only place both halves are visible at once — which is exactly the place the previous
//! four slices each found a defect in a *different* seam.
//!
//! # Why the join needs the wire and not just the types
//!
//! [`McpCatalog::build`] takes `ServerListing` values. A caller could in principle construct those by
//! hand, and doing so would test the catalog and prove nothing about whether a *fetched* listing can
//! be turned into one. The two facts a listing must carry come from different places — the tools from
//! `tools/list`, the reported identity from the connection's negotiated peer info — and a join that
//! read either from the wrong one would look correct in every type-level test. So the identity is
//! read from the [`McpConnection`], never invented here.
//!
//! # What this module deliberately does not do
//!
//! It does not open connections. Deciding whether a server is a child process or an HTTP endpoint is
//! an operator's configuration question and a daemon concern (`P3-009`), so this takes connections a
//! caller already established. That keeps the join testable over an in-process duplex pair while the
//! spawn path has its own tests, and it means the daemon cannot accidentally inherit a transport
//! choice made here.

use jarvis_mcp::{
    ConfiguredServer, McpCatalog, ObservedServer, ReportedIdentity, ServerListing, ServerName,
};

use std::sync::Arc;

use crate::client::{McpConnection, OwnedListing, ToolBuffer};
use crate::error::ListError;

/// One server's live connection, together with the operator's classification of it.
///
/// The two travel together because the catalog needs both and they must describe **the same server**:
/// a policy paired with the wrong connection would apply one server's declared posture to another
/// server's tools, which is the `P3-006a` class of defect (two values that must agree, with nothing
/// holding both).
///
/// The name is carried inside [`ConfiguredServer`], so a caller cannot pass a connection under a name
/// that differs from the one the policy was written for.
///
/// # Why the connection is shared
///
/// An `Arc` rather than an owned connection because the **same** connection must serve both the catalog
/// build and the adapter that later runs the tools. Two connections to one server would be two sessions
/// (or two child processes) whose lifecycles diverge, and the shutdown path would close only one of them.
/// [`HostedServer::new`] takes the `Arc` so the sharing is visible at the construction site rather than
/// implied by a clone somewhere downstream.
pub struct HostedServer {
    /// The operator's name and posture for this server.
    pub configured: ConfiguredServer,
    /// The live connection to it, shared with whatever dispatches its calls.
    pub connection: Arc<McpConnection>,
}

impl HostedServer {
    /// Pairs a configured server with its connection.
    ///
    /// Takes the connection as an `Arc` rather than by value, so a caller that also needs it for dispatch
    /// cannot accidentally create a second session by passing a fresh one.
    #[must_use]
    pub const fn new(configured: ConfiguredServer, connection: Arc<McpConnection>) -> Self {
        Self {
            configured,
            connection,
        }
    }
}

impl std::fmt::Debug for HostedServer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HostedServer")
            .field("server", &self.configured.name.as_str())
            .finish_non_exhaustive()
    }
}

/// A configured server whose tools could not be read.
///
/// Separate from a catalog *exclusion*, and the distinction is the reason this type exists. The
/// catalog reports "this server offered no tool listing" for any server that contributed nothing —
/// which is true whether the server is unreachable, refused the request, or genuinely offered an
/// empty list. An operator acting on that message cannot tell "your server is down" from "your server
/// has no tools", and the remedies are different. So the reason is preserved here.
pub struct UnreadableServer {
    /// The operator's name for the server.
    pub server: ServerName,
    /// Why its tools could not be read, reader-facing.
    pub reason: String,
}

impl std::fmt::Debug for UnreadableServer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("UnreadableServer")
            .field("server", &self.server.as_str())
            .field("reason", &self.reason)
            .finish()
    }
}

/// What a host build produced.
pub struct HostBuild {
    /// The aggregated catalog, built from every server that answered.
    pub catalog: McpCatalog,
    /// The servers that could not be read, each with a reason an operator can act on.
    ///
    /// These are also absent from the catalog, so `catalog.exclusions()` will name them too — with
    /// the less specific reason. Both are kept because they answer different questions: the catalog
    /// says which tools a model will not be offered, and this says why a server is missing.
    pub unreadable: Vec<UnreadableServer>,
}

impl std::fmt::Debug for HostBuild {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HostBuild")
            .field("tools", &self.catalog.len())
            .field("unreadable", &self.unreadable.len())
            .finish_non_exhaustive()
    }
}

/// Reads every hosted server and aggregates its tools into one catalog.
///
/// `strategy` decides how a server's tool names become canonical identifiers, and `seen_before` is
/// what each server reported on an earlier build — pass [`McpCatalog::observed`] from the previous
/// build, or an empty slice on the first one.
///
/// A server that cannot be read does **not** fail the build. That is deliberate and matches the
/// specification's own per-tool rule one level up: a single unavailable server must not remove the
/// other servers' tools, or one flaky third-party process would empty the model's tool list. The
/// absence is reported in [`HostBuild::unreadable`] so it is not silent.
///
/// # Errors
///
/// Returns the catalog's own error when the **configuration** is ambiguous — two servers under one
/// name, more servers than the bound, or two listings for one server. Those are refused rather than
/// partially served, because the tool set would otherwise depend on argument order.
pub async fn build_catalog(
    hosted: &[HostedServer],
    strategy: jarvis_mcp::NamingStrategy,
    seen_before: &[ObservedServer],
) -> Result<HostBuild, jarvis_mcp::CatalogError> {
    let mut read: Vec<ReadServer> = Vec::with_capacity(hosted.len());
    let mut unreadable: Vec<UnreadableServer> = Vec::new();

    for server in hosted {
        let name = server.configured.name.clone();
        // A server that declared no `tools` capability is not asked. The specification permits a
        // server to answer `tools/list` with an empty list even when it declared nothing, so asking
        // would work — but "this server does not do tools" and "this server has no tools" are
        // different facts, and only the first is knowable before the request. Reporting it here means
        // the operator is told which one applies.
        if !server.connection.capabilities().offers_tools() {
            unreadable.push(UnreadableServer {
                server: name,
                reason: "the server declared no tools capability".to_owned(),
            });
            continue;
        }
        match read_server(&server.connection).await {
            Ok(entry) => read.push(entry),
            Err(reason) => unreadable.push(UnreadableServer {
                server: name,
                reason,
            }),
        }
    }

    let configured: Vec<ConfiguredServer> = hosted
        .iter()
        .map(|server| server.configured.clone())
        .collect();
    // The owned listings must outlive the borrowed `ServerListing` values, so `read` is bound to a
    // local and the borrows are built from it rather than returned.
    let listings: Vec<ServerListing<'_>> = read
        .iter()
        .map(|entry| ServerListing {
            server: entry.server.clone(),
            reported: entry.reported.clone(),
            tools: entry.tools.iter().map(OwnedListing::as_listing).collect(),
        })
        .collect();

    let catalog = McpCatalog::build(&configured, &listings, strategy, seen_before)?;
    Ok(HostBuild {
        catalog,
        unreadable,
    })
}

/// One server's reading: who it said it was, and what it offered.
struct ReadServer {
    server: ServerName,
    reported: ReportedIdentity,
    tools: Vec<OwnedListing>,
}

/// Reads one connection's identity and tool list.
///
/// The identity is read **before** the tools, and from the connection rather than from the listing
/// result, because it is the only evidence tying the operator's chosen name to the process that
/// answered. Reading them in the other order would not change the result; reading the identity from
/// the tool result would, which is why the two are separate `let`s with a comment rather than one
/// expression.
async fn read_server(connection: &McpConnection) -> Result<ReadServer, String> {
    let reported = connection.reported_identity();
    let mut buffer = ToolBuffer::new();
    let listings = connection
        .list_tools(&mut buffer)
        .await
        .map_err(describe_list_error)?;
    // The borrowed listings are converted immediately: `buffer` is a local, and a caller keeping the
    // borrow would have to keep the buffer too. `OwnedListing` is the shape for that, and converting
    // here means the catalog never holds a borrow that outlives this function.
    let tools = listings.iter().map(OwnedListing::from).collect();
    Ok(ReadServer {
        server: connection.server().clone(),
        reported,
        tools,
    })
}

/// Names a listing failure in terms of what an operator should check.
///
/// The two cases have different remedies — an unreachable peer is worth checking the command or the
/// endpoint, a protocol error means the server answered and disagreed — so they are not collapsed
/// into one sentence.
fn describe_list_error(error: ListError) -> String {
    match error {
        ListError::Unavailable(reason) => {
            format!("the server could not be reached for its tool list: {reason}")
        }
        ListError::PeerError(reason) => {
            format!("the server refused the tool list request: {reason}")
        }
    }
}
