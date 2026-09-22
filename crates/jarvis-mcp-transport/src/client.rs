//! Connecting to an MCP server and reading what it offers.
//!
//! This is the only place in the workspace that speaks the MCP wire. Everything it returns is either
//! a `jarvis_mcp` type or plain `serde_json`, so no SDK type escapes this module — see `AGENTS.md`'s
//! "provider SDK types must not cross JARVIS domain boundaries".
//!
//! # Two decisions worth knowing before reading the code
//!
//! **The lifecycle is named, never defaulted.** The SDK's `serve()` performs the `initialize`
//! handshake, which protocol revision `2026-07-28` removed. `Discover` is used explicitly, and
//! [`crate::revision`] pins the reason. A connection that negotiates a legacy revision is a hard
//! failure rather than a warning, because the two eras differ in ways the rest of this crate does
//! not implement.
//!
//! **A server's `annotations` never reach the translation.** [`rmcp::model::Tool`] carries
//! `annotations`, and `AGENTS.md` plus ADR-0025 require the posture of a tool to come from an
//! operator. The field is *structurally unreachable* here because [`McpToolListing`] has no place to
//! put it, so the translation cannot consult it even by accident — which is a stronger guarantee
//! than a comment saying not to.

use std::borrow::Cow;
use std::time::Duration;

use jarvis_mcp::{McpToolListing, ReportedIdentity, ServerName};
use rmcp::model::{ClientCapabilities, ClientConfig, Implementation, Tool};
use rmcp::service::{ClientLifecycleMode, ClientServiceExt, RunningService, ServiceError};
use rmcp::transport::StreamableHttpClientTransport;
use serde_json::Value;

use crate::error::{ConnectError, ListError};
use crate::revision::{MODERN_REVISION, modern_revision};

/// Identity JARVIS advertises to a server, and the version it advertises it with.
const CLIENT_NAME: &str = "jarvis";
const CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// How long a server has to answer `server/discover` before the connection is abandoned.
///
/// **Required, because the SDK does not bound this in `Discover` mode.** Its own
/// `DEFAULT_AUTO_DISCOVER_TIMEOUT` is applied only in `ClientLifecycleMode::Auto` — the mode this
/// crate deliberately does not use, because `Auto` falls back to the legacy handshake. So a silent
/// peer would leave the client waiting indefinitely, and a daemon starting against a dead or legacy
/// server would hang at startup instead of reporting. Verified in the pinned SDK's
/// `service/client.rs` rather than assumed.
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(15);

/// How a local MCP server is launched.
///
/// A command and its arguments, deliberately not a shell string: `sh -c`/`cmd /c` would make every
/// argument a quoting question and would let a configured server name become a command. The program
/// is spawned directly, so the worst case of a malformed configuration is a failed spawn rather than
/// an executed string.
#[derive(Clone, Debug)]
pub struct StdioCommand {
    program: String,
    args: Vec<String>,
}

impl StdioCommand {
    /// Records a program and its arguments.
    ///
    /// Refuses an empty program, because spawning `""` fails at the OS with a message that does not
    /// say which configured server was at fault.
    ///
    /// # Errors
    ///
    /// Returns [`ConnectError::Unreachable`] when the program name is empty or whitespace.
    pub fn new(program: &str, args: &[&str]) -> Result<Self, ConnectError> {
        let program = program.trim();
        if program.is_empty() {
            return Err(ConnectError::Unreachable(
                "an MCP server command needs a program to run".to_owned(),
            ));
        }
        Ok(Self {
            program: program.to_owned(),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        })
    }

    /// Returns the program this command runs.
    #[must_use]
    pub fn program(&self) -> &str {
        &self.program
    }

    /// Returns the arguments, in order.
    #[must_use]
    pub fn args(&self) -> &[String] {
        &self.args
    }
}

/// What a connected server declared it can do.
///
/// Only the facts this crate acts on. A server also declares `resources` and `prompts`, and they are
/// deliberately **not** modelled: nothing consumes them, and a capability field with no reader gets
/// its meaning invented by its first caller — the rule `P3-005` recorded for the adapter port. When
/// `P3-009` reports why a server is uninteresting to JARVIS those become readable facts and belong
/// here then.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ServerCapabilities {
    /// The server declared a `tools` capability.
    pub tools: bool,
    /// The server declared it will notify when its tool list changes.
    ///
    /// Recorded separately because a server may offer tools without promising to say when they
    /// change, and a client that assumed the notification would cache a stale list forever.
    pub tool_list_changed: bool,
}

impl ServerCapabilities {
    /// Returns whether the server offers the tool listing this crate reads.
    ///
    /// Callers should check this before listing: a server that declared no `tools` capability may
    /// still answer `tools/list` with an empty list, and "offers no tools" and "does not do tools"
    /// are different facts an operator needs to tell apart.
    #[must_use]
    pub fn offers_tools(&self) -> bool {
        self.tools
    }
}

/// A live connection to one MCP server.
///
/// Holds the SDK's running service. The type is public so a caller can keep a connection open across
/// several listings, but no SDK type appears in its public surface.
pub struct McpConnection {
    server: ServerName,
    service: RunningService<rmcp::RoleClient, ClientConfig>,
}

impl std::fmt::Debug for McpConnection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("McpConnection")
            .field("server", &self.server.as_str())
            .finish_non_exhaustive()
    }
}

impl McpConnection {
    /// Returns the operator-chosen name of the server this connection is to.
    #[must_use]
    pub fn server(&self) -> &ServerName {
        &self.server
    }

    /// Returns the protocol revision that was **actually negotiated**.
    ///
    /// An empty string means the SDK reported no revision, which is surfaced rather than defaulted:
    /// a caller recording this cannot invent a revision the peer never agreed to, and `server_info`
    /// being optional under `server/discover` is exactly the kind of absence this must not paper
    /// over.
    #[must_use]
    pub fn negotiated_revision(&self) -> String {
        self.service.peer_info().map_or_else(String::new, |info| {
            info.protocol_version.as_str().to_owned()
        })
    }

    /// Returns what the server reported about itself, when it reported anything.
    ///
    /// This is the input to `P3-008d`'s identity-drift check: the operator chose the local name, so
    /// the server's own claim is evidence and nothing else.
    ///
    /// **`server_info` is optional in the modern lifecycle.** The SDK's own source notes that
    /// `server/discover` responses "are not required to provide it", so a fully conforming modern
    /// server may report no identity at all. That absence is returned as an empty
    /// [`ReportedIdentity`] rather than as an error, because `P3-008d` must be able to compare "no
    /// claim" against a previous claim — a server that *stops* naming itself is a drift worth
    /// seeing, and treating it as a failure would discard exactly that observation.
    #[must_use]
    pub fn reported_identity(&self) -> ReportedIdentity {
        self.service
            .peer_info()
            .and_then(|info| info.server_info.clone())
            .map_or_else(
                || ReportedIdentity::new("", None),
                |reported| ReportedIdentity::new(&reported.name, reported.title.as_deref()),
            )
    }

    /// Returns what the server declared it can do.
    #[must_use]
    pub fn capabilities(&self) -> ServerCapabilities {
        const NONE: ServerCapabilities = ServerCapabilities {
            tools: false,
            tool_list_changed: false,
        };
        self.service
            .peer_info()
            .map_or(NONE, |info| ServerCapabilities {
                tools: info.capabilities.tools.is_some(),
                tool_list_changed: info
                    .capabilities
                    .tools
                    .as_ref()
                    .is_some_and(|tools| tools.list_changed == Some(true)),
            })
    }

    /// Lists every tool the server offers, across all pages.
    ///
    /// Returns `jarvis_mcp` listings whose borrows live as long as `buffer`. Passing the buffer in
    /// rather than owning it means the caller decides how long the schema documents live, which is
    /// what lets a caller translate immediately and drop the wire representation.
    ///
    /// # Errors
    ///
    /// Returns [`ListError::Unavailable`] when the request did not complete and
    /// [`ListError::PeerError`] when the peer answered with a protocol error.
    pub async fn list_tools<'a>(
        &self,
        buffer: &'a mut ToolBuffer,
    ) -> Result<Vec<McpToolListing<'a>>, ListError> {
        let tools = self
            .service
            .list_all_tools()
            .await
            .map_err(|error| classify_list(&error))?;
        // The buffer is rebuilt in one pass so every borrow handed out refers to it, not to the
        // SDK's values. `list_all_tools` has already resolved pagination, so no cursor is left
        // dangling here — the SDK's own `next_cursor` handling is what `P3-007` verified.
        buffer.adopt(tools);
        Ok(buffer.listings())
    }

    /// Closes the connection and waits for the service to stop.
    ///
    /// # Errors
    ///
    /// Returns the SDK's message when cancellation failed, as text: a caller deciding whether to log
    /// a shutdown failure does not need the SDK's error type to do it.
    pub async fn close(self) -> Result<(), String> {
        self.service
            .cancel()
            .await
            .map(|_reason| ())
            .map_err(|error| error.to_string())
    }
}

/// Owns the wire form of a server's tool list so [`McpToolListing`] can borrow from it.
///
/// The SDK hands back `Tool` values owned by the caller. `McpToolListing` borrows its strings and
/// schemas, so something must own them for as long as the listings live. Putting that ownership in a
/// named buffer rather than leaking or cloning makes the lifetime explicit at the call site.
#[derive(Debug, Default)]
pub struct ToolBuffer {
    names: Vec<String>,
    titles: Vec<Option<String>>,
    descriptions: Vec<Option<String>>,
    input_schemas: Vec<Value>,
    output_schemas: Vec<Option<Value>>,
}

impl ToolBuffer {
    /// Creates an empty buffer.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns how many tools the buffer currently holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.names.len()
    }

    /// Returns whether the buffer holds no tools.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// Replaces the contents with a freshly read tool list.
    fn adopt(&mut self, tools: Vec<Tool>) {
        self.names.clear();
        self.titles.clear();
        self.descriptions.clear();
        self.input_schemas.clear();
        self.output_schemas.clear();
        for tool in tools {
            self.names.push(tool.name.into_owned());
            self.titles.push(tool.title);
            self.descriptions
                .push(tool.description.map(Cow::into_owned));
            self.input_schemas
                .push(Value::Object((*tool.input_schema).clone()));
            self.output_schemas.push(
                tool.output_schema
                    .map(|schema| Value::Object((*schema).clone())),
            );
        }
    }

    /// Returns listings that borrow from the buffer.
    ///
    /// Note what is **absent**: `annotations`. The SDK's `Tool` carries `readOnlyHint`,
    /// `destructiveHint`, and `idempotentHint`, and the specification requires clients to treat all
    /// three as untrusted. They are dropped here by construction, so no later change can consult
    /// them without adding a field to `McpToolListing` — which ADR-0025 forbids.
    fn listings(&self) -> Vec<McpToolListing<'_>> {
        (0..self.names.len())
            .map(|index| McpToolListing {
                name: &self.names[index],
                title: self.titles[index].as_deref(),
                description: self.descriptions[index].as_deref(),
                input_schema: &self.input_schemas[index],
                output_schema: self.output_schemas[index].as_ref(),
            })
            .collect()
    }
}

/// Classifies a tool-list failure without retaining the SDK's type.
fn classify_list(error: &ServiceError) -> ListError {
    match error {
        // A protocol error is a fact about the server that a retry will not change, so it is
        // reported separately from a peer that could not be reached.
        ServiceError::McpError(_) => ListError::PeerError(error.to_string()),
        _ => ListError::Unavailable(error.to_string()),
    }
}

/// Builds the client identity JARVIS advertises.
fn client_config() -> ClientConfig {
    // Elicitation, sampling, and roots are deliberately NOT declared. The first two are deprecated
    // by SEP-2577 and their spec-named migrations are already JARVIS's shape (the model provider is
    // called directly, and a human answers through JARVIS's own approval path, not through the
    // server). Declaring a capability JARVIS would answer with a stub tells the server to depend on
    // behaviour that does not exist.
    let capabilities = ClientCapabilities::default();
    let identity = Implementation::new(CLIENT_NAME, CLIENT_VERSION);
    ClientConfig::new(capabilities, identity)
}

/// Connects to a local MCP server over stdio, launching it as a child process.
///
/// # Errors
///
/// Returns [`ConnectError::Unreachable`] when the child cannot be spawned,
/// [`ConnectError::Refused`] when the negotiation fails at the protocol level, and
/// [`ConnectError::LegacyNegotiated`] when the server settles on a revision older than
/// [`MODERN_REVISION`].
pub async fn connect_stdio(
    server: &ServerName,
    command: &StdioCommand,
) -> Result<McpConnection, ConnectError> {
    use rmcp::transport::TokioChildProcess;
    use tokio::process::Command;

    let mut child = Command::new(command.program());
    child.args(command.args());
    let transport = TokioChildProcess::new(child)
        .map_err(|error| ConnectError::Unreachable(error.to_string()))?;
    connect_over(server, transport).await
}

/// Connects to a remote MCP server over Streamable HTTP.
///
/// # Errors
///
/// As [`connect_stdio`], plus [`ConnectError::Refused`] when the endpoint is not one the transport
/// will connect to.
pub async fn connect_http(
    server: &ServerName,
    endpoint: &str,
) -> Result<McpConnection, ConnectError> {
    // No client-level total timeout: an MCP connection is long-lived and a total bound would kill a
    // healthy session. This is the same defect `docs/research/integrations/mcp.md` records for the
    // model client's stream, where a total `timeout` terminated every healthy stream at the
    // deadline.
    let transport = StreamableHttpClientTransport::from_uri(endpoint);
    connect_over(server, transport).await
}

/// Runs the negotiation over any transport the SDK supports.
///
/// Public so a test can drive the real negotiation over an in-process duplex pair. That matters:
/// `P3-007` recorded that `tracing`/`reqwest`-style defects and a *revision* defect are invisible to
/// a fixture that shares the code's assumptions, and the only way to prove `Discover` does not fall
/// back is to send it a peer that answers `server/discover` and nothing else.
///
/// # Errors
///
/// Returns [`ConnectError::Unreachable`] or [`ConnectError::Refused`] when the transport or the
/// negotiation fails, [`ConnectError::DiscoveryTimedOut`] when the peer never answers
/// `server/discover`, and [`ConnectError::LegacyNegotiated`] when the peer settles on a revision
/// other than [`MODERN_REVISION`].
pub async fn connect_over<T, E, A>(
    server: &ServerName,
    transport: T,
) -> Result<McpConnection, ConnectError>
where
    T: rmcp::transport::IntoTransport<rmcp::RoleClient, E, A>,
    E: std::error::Error + Send + Sync + 'static,
{
    let config = client_config();
    // Bounded, because `Discover` has no deadline of its own. See `DISCOVERY_TIMEOUT`.
    let negotiation = config.serve_with_lifecycle(
        transport,
        // `Discover`, not `Auto`: `Auto` falls back to the legacy handshake when the peer does
        // not answer `server/discover` within ten seconds, which would negotiate a different era
        // silently. `Discover` has no fallback path, so a legacy peer fails here and is named.
        ClientLifecycleMode::Discover {
            preferred_versions: vec![modern_revision()],
        },
    );
    let service = tokio::time::timeout(DISCOVERY_TIMEOUT, negotiation)
        .await
        .map_err(|_elapsed| ConnectError::DiscoveryTimedOut {
            seconds: DISCOVERY_TIMEOUT.as_secs(),
        })?
        .map_err(|error| ConnectError::from_sdk(&error))?;

    // The server states which revision it settled on. Checked rather than trusted: `Discover` has no
    // fallback, but the *server* may still answer a revision JARVIS does not implement, and a
    // connection that appeared to work while speaking another era is exactly the failure `P3-007`
    // recorded.
    let negotiated = service.peer_info().map_or_else(String::new, |info| {
        info.protocol_version.as_str().to_owned()
    });
    if negotiated != MODERN_REVISION {
        let _ = service.cancel().await;
        return Err(ConnectError::LegacyNegotiated {
            negotiated,
            wanted: MODERN_REVISION.to_owned(),
        });
    }

    Ok(McpConnection {
        server: server.clone(),
        service,
    })
}

/// A tool listing that owns its data, for a caller that cannot hold a buffer.
///
/// The borrowed [`McpToolListing`] is the right shape for translation, which is immediate. This
/// owned form exists for the one case where a caller must keep the list across an await point and
/// cannot borrow.
#[derive(Clone, Debug, PartialEq)]
pub struct OwnedListing {
    /// The server's own tool name.
    pub name: String,
    /// The optional display title.
    pub title: Option<String>,
    /// The optional model-facing description.
    pub description: Option<String>,
    /// The input schema.
    pub input_schema: Value,
    /// The optional output schema.
    pub output_schema: Option<Value>,
}

impl From<&McpToolListing<'_>> for OwnedListing {
    fn from(listing: &McpToolListing<'_>) -> Self {
        Self {
            name: listing.name.to_owned(),
            title: listing.title.map(str::to_owned),
            description: listing.description.map(str::to_owned),
            input_schema: listing.input_schema.clone(),
            output_schema: listing.output_schema.cloned(),
        }
    }
}

impl OwnedListing {
    /// Borrows this listing in the shape the translator expects.
    #[must_use]
    pub fn as_listing(&self) -> McpToolListing<'_> {
        McpToolListing {
            name: &self.name,
            title: self.title.as_deref(),
            description: self.description.as_deref(),
            input_schema: &self.input_schema,
            output_schema: self.output_schema.as_ref(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn server(name: &str) -> ServerName {
        ServerName::new(name).unwrap_or_else(|error| panic!("fixture server {name}: {error}"))
    }

    /// **The property this whole module is arranged around.** A server's `annotations` claim what its
    /// tools may do — `readOnlyHint`, `destructiveHint`, `idempotentHint` — and the specification
    /// requires clients to treat them as untrusted, while JARVIS policy treats them as exactly the
    /// facts it must not accept from the server. This asserts the translation has no field to put
    /// them in, so the omission is structural rather than a convention someone could forget.
    #[test]
    fn a_tool_listing_carries_no_annotation_field() {
        let buffer = ToolBuffer {
            names: vec!["search".to_owned()],
            titles: vec![None],
            descriptions: vec![None],
            input_schemas: vec![json!({ "type": "object" })],
            output_schemas: vec![None],
        };
        let listings = buffer.listings();
        assert_eq!(listings.len(), 1);
        let serialised = serde_json::to_value(listings[0].name)
            .unwrap_or_else(|error| panic!("fixture name is a string: {error}"));
        assert!(serialised.is_string());
        // The listing exposes exactly five fields; there is no sixth to carry a hint. If someone
        // adds one, this test fails and the reviewer re-reads ADR-0025.
        let listing = &listings[0];
        assert_eq!(listing.name, "search");
        assert!(listing.title.is_none());
        assert!(listing.description.is_none());
        assert!(listing.output_schema.is_none());
    }

    /// An empty program is refused rather than handed to the OS, whose error would name neither the
    /// server nor the configuration that produced it.
    #[test]
    fn a_command_without_a_program_is_refused() {
        let error = StdioCommand::new("   ", &[])
            .err()
            .unwrap_or_else(|| panic!("a blank program must be refused"));
        assert!(error.to_string().contains("needs a program"), "{error}");
    }

    /// The arguments are kept in order and unmodified: a client that reordered or quoted them would
    /// change which command runs.
    #[test]
    fn a_command_preserves_its_arguments_exactly() {
        let command = StdioCommand::new("npx", &["-y", "@modelcontextprotocol/server-everything"])
            .unwrap_or_else(|error| panic!("fixture command: {error}"));
        assert_eq!(command.program(), "npx");
        assert_eq!(
            command.args(),
            ["-y", "@modelcontextprotocol/server-everything"]
        );
        // A program name with surrounding whitespace is trimmed, because a value copied out of a
        // config file routinely carries a trailing newline and an untrimmed value fails to spawn
        // with a message about a missing file.
        let padded = StdioCommand::new("  npx  ", &[])
            .unwrap_or_else(|error| panic!("fixture command: {error}"));
        assert_eq!(padded.program(), "npx");
    }

    /// **Two listings for one server must remain distinguishable**, because `P3-008d` keys the
    /// catalog's collision check on `(server, tool)` and the identity drift on the server name. A
    /// connection that lost its server name would silently make every listing look like it came from
    /// the same place.
    #[test]
    fn a_buffered_listing_stays_borrowed_and_ordered() {
        let buffer = ToolBuffer {
            names: vec!["b".to_owned(), "a".to_owned()],
            titles: vec![Some("B".to_owned()), None],
            descriptions: vec![None, Some("does a".to_owned())],
            input_schemas: vec![json!({"type": "object"}), json!({"type": "object"})],
            output_schemas: vec![Some(json!({"type": "object"})), None],
        };
        let listings = buffer.listings();
        assert_eq!(listings.len(), 2);
        // Order is the wire order, not sorted: the catalog sorts by canonical identifier, and
        // sorting here would hide a server that returns an unstable order, which the specification
        // requires it not to do.
        assert_eq!(listings[0].name, "b");
        assert_eq!(listings[1].name, "a");
        assert_eq!(listings[0].title, Some("B"));
        assert_eq!(listings[1].description, Some("does a"));
        assert!(listings[0].output_schema.is_some());
        assert!(listings[1].output_schema.is_none());

        let owned = OwnedListing::from(&listings[0]);
        let borrowed = owned.as_listing();
        assert_eq!(borrowed.name, "b");
        assert_eq!(borrowed.title, Some("B"));
        assert_eq!(borrowed.input_schema, &json!({"type": "object"}));
        assert_eq!(owned.name, buffer.listings()[0].name);
    }

    /// `adopt` replaces rather than appends. A refresh that appended would double every tool on each
    /// poll, and `P3-008d`'s drift check compares two builds of the same server.
    #[test]
    fn adopting_a_list_replaces_the_previous_one() {
        let mut buffer = ToolBuffer::new();
        assert!(buffer.is_empty());
        buffer.adopt(Vec::new());
        assert!(buffer.is_empty());
    }

    /// The server name a connection carries is the operator's, never the server's own claim — the
    /// rule ADR-0024 exists to hold, asserted at the transport boundary so a future refactor cannot
    /// quietly start using the reported name as the identifier.
    #[test]
    fn a_connection_is_named_by_the_operator_not_the_server() {
        let operator_name = server("acme-mcp");
        assert_eq!(operator_name.as_str(), "acme-mcp");
        // The reported identity is separate evidence, and a server may claim anything.
        let claimed = ReportedIdentity::new("totally-different", Some("Totally Different"));
        assert_ne!(claimed.name, operator_name.as_str());
        assert!(
            !claimed.agrees_with(&ReportedIdentity::new("acme-mcp", None)),
            "a server that renamed itself must not agree with the operator's record"
        );
    }
}
