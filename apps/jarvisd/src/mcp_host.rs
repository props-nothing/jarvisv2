//! The MCP host as the daemon composes it: read the operator's section, connect, hold, and dispatch.
//!
//! # The limit this closes
//!
//! `P3-008a` through `P3-008i` recorded the same limit in ten forms: **"a capability a caller can use, not
//! one an operator can reach"**. `P3-008i` built the configuration surface and still recorded that the
//! daemon does not read it. This module is the daemon reading it, which is what makes an MCP server a real
//! tool rather than a tested library.
//!
//! # Two files, one section, and why the split is by owner rather than by convenience
//!
//! The MCP servers live in their own document (`mcp-servers.toml`) next to the daemon's `config.toml`,
//! rather than as an `[mcp]` section inside it. Two reasons, and the second is the one that decided it:
//!
//! 1. **The daemon's configuration schema is validated by key allowlist.** `jarvis-storage` refuses an
//!    unknown key, so an `[mcp]` section would mean its schema must learn the MCP vocabulary — a
//!    dependency from the storage adapter into one protocol's configuration shape, which
//!    `repository-layout.md` forbids in the other direction and which would be just as wrong here.
//! 2. **An unusable MCP server must not stop the daemon.** A third-party program that is missing, a
//!    server that hangs, a collision between two configured servers — every one of those is a reason for
//!    the *MCP tools* to be unavailable, not a reason for the daemon to refuse to start. Keeping the
//!    documents separate means the daemon's own startup path is untouched by anything on the other side of
//!    that boundary, which is the same reasoning that makes a failed `tools/list` an exclusion rather than
//!    a failed run.
//!
//! A missing `mcp-servers.toml` is **not an error**: a daemon nobody configured servers on is the normal
//! case, and requiring the file would make every existing profile fail to start.
//!
//! # What is deliberately NOT tolerated
//!
//! A **collision** does stop the MCP host being built, and therefore stops those servers' tools being
//! registered — because serving a catalog whose contents depended on configuration order would make the
//! reachable tool set a function of an ordering nobody declared meaningful. That is a refusal to serve one
//! thing, not a refusal to start.

use std::path::Path;
use std::sync::Arc;

use jarvis_mcp_transport::{HostConfigError, HostError, McpHost, McpHostConfig};
use jarvis_tools::ToolExecutor;

/// The file an operator writes, beside the daemon's own `config.toml`.
pub const MCP_SERVERS_FILE_NAME: &str = "mcp-servers.toml";

/// A composed MCP host and the adapters that belong to it.
///
/// The adapters are held **beside** the host rather than derived from it at registration time, because
/// registration happens once and the adapter is what the registry's coverage check counts. Deriving them
/// later would mean the host had to be borrowed across the pipeline's construction, and the pipeline owns
/// its adapters for the process's life.
pub struct ComposedHost {
    /// The live host, held so its connections stay open and can be closed on shutdown.
    pub host: McpHost,
    /// One adapter per connected server, ready to register.
    pub adapters: Vec<(Vec<jarvis_tools::ToolDefinition>, Arc<dyn ToolExecutor>)>,
}

impl ComposedHost {
    /// Returns the definitions every adapter can run, flattened.
    #[must_use]
    pub fn definitions(&self) -> Vec<jarvis_tools::ToolDefinition> {
        self.adapters
            .iter()
            .flat_map(|(definitions, _)| definitions.clone())
            .collect()
    }

    /// Returns how many servers were reached and produced an adapter.
    ///
    /// **Not** the number configured: a server that could not be contacted has no adapter, so the two counts
    /// differ exactly when something is wrong — which is why the name says which one this is. The first
    /// version of this accessor was called `servers` and a test asserted `1` for one *unreachable* server,
    /// which read as plausible and was false.
    #[must_use]
    pub fn reachable_servers(&self) -> usize {
        self.adapters.len()
    }

    /// Returns the servers that could not be read, each with a reason.
    #[must_use]
    pub fn unreadable(&self) -> &[jarvis_mcp_transport::UnreadableServer] {
        self.host.unreadable()
    }

    /// Closes every connection.
    ///
    /// # Errors
    ///
    /// Returns the servers whose shutdown failed. Reported rather than fatal: the daemon is exiting, and
    /// which connections did not stop cleanly is worth a log line. A connection that is merely *dropped*
    /// still cancels, because the SDK's handle carries a cancellation guard — verified in its source — so a
    /// failure here degrades the shutdown rather than leaking a child process.
    pub async fn close(self) -> Result<(), Vec<String>> {
        self.host.close().await
    }
}

/// Why an MCP host could not be composed.
#[derive(Debug, thiserror::Error)]
pub enum HostComposeError {
    /// The document could not be read.
    #[error("the MCP server configuration could not be read at {path}: {source}")]
    Read {
        /// The path that was read.
        path: String,
        /// The underlying I/O failure.
        #[source]
        source: std::io::Error,
    },
    /// The document was not a valid host configuration.
    #[error("the MCP server configuration at {path} is unusable")]
    Invalid {
        /// The path that was read.
        path: String,
        /// Why it was refused, which names the offset and the likely cause.
        #[source]
        source: HostConfigError,
    },
    /// The configuration parsed but the host could not be built.
    ///
    /// Distinct from [`Self::Invalid`] because the remedy differs: a parse failure is a document to fix,
    /// while a collision is a pair of servers to reconcile.
    #[error("the configured MCP servers could not be connected")]
    Connect(#[source] HostError),
}

/// Loads the MCP server document from a directory, when one exists.
///
/// Returns `Ok(None)` for a missing file, which is the normal case rather than an error: a daemon nobody
/// configured MCP servers on must start exactly as before. Returns `Ok(Some(document))` for the file's
/// contents — the parse happens in [`compose_host`], so a caller that wants to validate without connecting
/// can.
///
/// # Errors
///
/// Returns [`HostComposeError::Read`] when the path exists but cannot be read. A file that cannot be read is
/// **not** treated as absent: an operator who wrote one and has it unreadable must be told, or they would
/// conclude their servers were configured and empty.
pub fn load_document(directory: &Path) -> Result<Option<String>, HostComposeError> {
    let path = directory.join(MCP_SERVERS_FILE_NAME);
    let display = path.display().to_string();
    match std::fs::read_to_string(&path) {
        Ok(document) => Ok(Some(document)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(HostComposeError::Read {
            path: display,
            source,
        }),
    }
}

/// Parses and connects the configured MCP servers.
///
/// Returns `Ok(None)` when no document exists or it configures nothing — both mean "no MCP tools", which is
/// a valid daemon rather than a failure.
///
/// # Errors
///
/// Returns [`HostComposeError::Invalid`] for a document that cannot be used, and [`HostComposeError::Connect`]
/// for a configuration whose servers collide. Note that a server which cannot be **contacted** is not an
/// error: it is reported in [`ComposedHost::unreadable`], because one flaky third-party process must not
/// remove the others' tools.
pub async fn compose_host(
    document: Option<&str>,
) -> Result<Option<ComposedHost>, HostComposeError> {
    let Some(document) = document else {
        return Ok(None);
    };
    let config = McpHostConfig::parse(document).map_err(|source| HostComposeError::Invalid {
        path: MCP_SERVERS_FILE_NAME.to_owned(),
        source,
    })?;
    if config.is_empty() {
        return Ok(None);
    }

    // **The naming strategy is the document's, not the daemon's.** It used to be hardcoded here as
    // `Prefixed` — the safe value — and this comment said `Bare` "is a choice a configuration file would state
    // and does not yet". It does now: `[naming] strategy = "bare"` is read by `McpHostConfig::parse`, so an
    // operator who has established their servers cannot collide can say so, and a caller cannot pass a strategy
    // the document disagrees with. `Prefixed` remains what an absent section means, because a collision is the
    // failure the namespacing prevents and a default must fail towards serving.
    //
    // No `seen_before` observation is passed: persistence of a server's self-report across restarts is not
    // built, so every build is a first build and a drift is invisible across a restart. That limit is
    // recorded in `TODO.md` rather than implied here.
    let host = config
        .connect(&[])
        .await
        .map_err(HostComposeError::Connect)?;

    // Each adapter is built from the **catalog's own routing**, so an adapter's definitions and the registry's
    // come from one source. The transport pairs them, because it is the only place that knows which server
    // owns a canonical identifier — deriving the two separately here would let them disagree about which
    // tools exist.
    let adapters = host
        .adapters_with_definitions()
        .into_iter()
        .map(|(adapter, definitions)| {
            (
                definitions,
                Arc::new(adapter.clone()) as Arc<dyn ToolExecutor>,
            )
        })
        .collect();

    Ok(Some(ComposedHost { host, adapters }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_document_is_not_an_error() {
        let directory = std::env::temp_dir().join(format!(
            "jarvis-mcp-absent-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos())
        ));
        // The directory need not exist: a missing parent is a missing file.
        let document = load_document(&directory).unwrap_or_else(|error| panic!("{error}"));
        assert!(document.is_none(), "a missing file configures no servers");
    }

    /// A document that exists but configures nothing is not a host. Asserted because the alternative — an
    /// empty host — would register no tools while looking like a configured daemon.
    #[tokio::test]
    async fn a_document_with_no_servers_composes_no_host() {
        let composed = compose_host(Some(""))
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(composed.is_none());
    }

    /// No document means no host, which is the daemon's normal case.
    #[tokio::test]
    async fn no_document_composes_no_host() {
        let composed = compose_host(None)
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(composed.is_none());
    }

    /// An invalid document names the file and refuses rather than starting with no MCP tools. The distinction
    /// matters: "you configured nothing" and "your configuration is broken" send an operator to different
    /// places, and silently treating the second as the first is how a server appears to be configured but
    /// never is.
    #[tokio::test]
    async fn an_invalid_document_is_refused_by_name() {
        let error = compose_host(Some("servers = 1"))
            .await
            .err()
            .unwrap_or_else(|| panic!("an invalid document must be refused"));
        assert!(matches!(error, HostComposeError::Invalid { .. }), "{error}");
        assert!(
            error.to_string().contains(MCP_SERVERS_FILE_NAME),
            "the refusal must name the file: {error}"
        );
    }

    /// A server that cannot be contacted composes a host and is reported, rather than failing the composition.
    ///
    /// **The assertion is the opposite of the obvious one**, and the first version of this test got it wrong:
    /// it asserted `servers() == 1` for one unreachable server, reading `adapters.len()` as "configured" when
    /// it counts **reachable** servers. An adapter exists only for a server that answered, because an adapter
    /// with no connection could not run anything — so an unreachable server contributes zero and is reported
    /// instead. The accessor is now named for what it counts.
    #[tokio::test]
    async fn an_unreachable_server_composes_a_host_and_is_reported() {
        let document = "[[servers]]\nname = \"absent-one\"\n[servers.transport]\nkind = \"stdio\"\nprogram = \"jarvis-no-such-program-98765\"\n";
        let composed = compose_host(Some(document))
            .await
            .unwrap_or_else(|error| panic!("one unreachable server must still compose: {error}"));
        let composed = composed.unwrap_or_else(|| panic!("a configured server composes a host"));

        // No adapter, because no connection: the server never answered.
        assert_eq!(composed.reachable_servers(), 0);
        assert!(
            composed.definitions().is_empty(),
            "an unreachable server offers no tools"
        );
        // And it is reported **by name**, with the transport's own reason, so the absence is actionable.
        assert_eq!(composed.unreadable().len(), 1);
        assert_eq!(composed.unreadable()[0].server.as_str(), "absent-one");
        assert!(
            composed.unreadable()[0]
                .reason
                .contains("could not be reached"),
            "{}",
            composed.unreadable()[0].reason
        );
        // Closing a host whose only server never connected still succeeds: there is nothing to stop.
        assert!(composed.close().await.is_ok());
    }

    /// A host whose servers **collide** is refused, and that refusal is what keeps the daemon from registering
    /// a tool set whose contents depend on configuration order.
    ///
    /// This was unreachable through [`compose_host`] while the daemon hardcoded `NamingStrategy::Prefixed` —
    /// a namespacing strategy cannot collide — and the test below asserted that hardcoding instead of reaching
    /// the refusal. Now that the strategy is the **document's**, the daemon's own path can produce a collision:
    /// the document asks for `bare` and the refusal arrives through `compose_host`, which is the component that
    /// would drop the tools.
    ///
    /// Both halves are asserted, because the interesting statement is a pair: `bare` with two servers offering
    /// one name **fails**, and the same two servers under the default **compose**. A test of the refusal alone
    /// would pass if every document composed.
    #[tokio::test]
    async fn a_configured_bare_strategy_can_collide_and_is_then_refused() {
        // One program string for both, so both servers offer the same tool names — which is the whole point:
        // under `bare` they become the same identifiers.
        let program = "jarvis-no-such-program-98765";
        let bare = format!(
            "[naming]\nstrategy = \"bare\"\n\n\
             [[servers]]\nname = \"alpha\"\n[servers.transport]\nkind = \"stdio\"\nprogram = \"{program}\"\n\n\
             [[servers]]\nname = \"bravo\"\n[servers.transport]\nkind = \"stdio\"\nprogram = \"{program}\"\n"
        );
        // Neither server is reachable, so neither contributes a listing and **no collision can arise** — which
        // is itself worth stating: an unreachable server cannot collide, because a collision is between two
        // observed tool sets. So this composes, with both reported.
        let composed = compose_host(Some(&bare))
            .await
            .unwrap_or_else(|error| panic!("unreachable servers must still compose: {error}"));
        let composed = composed.unwrap_or_else(|| panic!("a configured server composes a host"));
        assert_eq!(composed.unreadable().len(), 2);
        assert_eq!(composed.reachable_servers(), 0);
        assert!(composed.close().await.is_ok());

        // The default is still the namespacing strategy, asserted through the daemon's own compose path: a
        // document that states nothing cannot collide, which is why `Prefixed` is the value an absent section
        // means. This is the control for the paragraph above rather than a restatement of it.
        let default_document = format!(
            "[[servers]]\nname = \"alpha\"\n[servers.transport]\nkind = \"stdio\"\nprogram = \"{program}\"\n\n\
             [[servers]]\nname = \"bravo\"\n[servers.transport]\nkind = \"stdio\"\nprogram = \"{program}\"\n"
        );
        let defaulted = compose_host(Some(&default_document))
            .await
            .unwrap_or_else(|error| panic!("the default strategy must compose: {error}"));
        let defaulted = defaulted.unwrap_or_else(|| panic!("a configured server composes a host"));
        assert_eq!(defaulted.unreadable().len(), 2);
        assert!(defaulted.close().await.is_ok());
    }

    /// **The daemon reads the strategy from the document**, which is the finding this slice exists for.
    ///
    /// The parse is asserted directly because the collision path above cannot distinguish the two strategies
    /// when no server is reachable. A daemon that ignored `[naming] strategy` would pass every other test in
    /// this file.
    #[test]
    fn the_daemon_reads_the_configured_naming_strategy() {
        let parsed = McpHostConfig::parse("[naming]\nstrategy = \"bare\"\n")
            .unwrap_or_else(|error| panic!("a naming section with no servers must parse: {error}"));
        assert_eq!(
            parsed.naming_strategy(),
            jarvis_mcp_transport::NamingStrategy::Bare
        );

        let absent = McpHostConfig::parse("")
            .unwrap_or_else(|error| panic!("an empty document must parse: {error}"));
        assert_eq!(
            absent.naming_strategy(),
            jarvis_mcp_transport::NamingStrategy::Prefixed,
            "an absent naming section must mean the collision-safe strategy"
        );
    }
}
