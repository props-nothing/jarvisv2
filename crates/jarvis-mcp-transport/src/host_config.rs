//! The MCP host: which servers an operator configured, and how to reach them.
//!
//! # The limit this closes, and it is the last one
//!
//! `P3-008a` through `P3-008h` recorded the same sentence in nine different forms: **"a capability a
//! caller can use, not one an operator can reach"**. The pure crate could translate a listing nobody
//! fetched; the transport could fetch one nobody translated; the adapter could run a call nothing
//! dispatched. Every piece worked and **nothing read a configuration file**, so the whole MCP
//! integration was reachable only from a test.
//!
//! This is the configuration surface. It is deliberately the last thing built, because it is the only
//! place where all three of those halves are visible at once — and it is also the first caller of
//! `McpCatalog::has_cross_server_collision()`, which has existed since `P3-008c` with a documented
//! instruction ("a caller should refuse to serve when this is true") and no caller to do it.
//!
//! # What an operator can and cannot say here
//!
//! A server entry carries a **name**, a **transport**, and a **posture**, and the posture vocabulary is
//! deliberately two values: `read-only` and `unclassified`.
//!
//! That narrowness is the decision, not an omission. A full [`ToolEffectPolicy`] needs effects, risk,
//! scopes, retry, idempotency, and sensitivity — and a configuration file that let an operator write all
//! six would be a vocabulary whose entries nothing consumes yet, which is how a naming scheme with no
//! users gets invented. More importantly, the *safe* posture already exists and is severe: a server
//! nobody classified gets [`ToolEffectPolicy::unclassified`], so **the failure of leaving something out
//! is a refusal rather than a permission**. The one thing an operator can usefully narrow today is "this
//! server only reads", because that is the common case and the difference is stark. Anything else is
//! refused by name, listing what is accepted — an unknown posture must not silently become the permissive
//! one.
//!
//! # No credential appears in this document
//!
//! A server entry names a program or a URL and nothing else. A bearer token for a remote endpoint would
//! be a secret in a plaintext configuration file that is then a substring of every log line mentioning
//! the endpoint — the mistake `ApiKey::new` refuses for provider keys. [`McpHttpEndpoint`] refuses a URL
//! with embedded userinfo for the same reason, so the two rules meet: the configuration cannot hold a
//! credential even accidentally. A remote server needing authentication is a slice of its own, and it
//! belongs in the credential store rather than here.

use std::collections::BTreeSet;
use std::sync::Arc;

use jarvis_mcp::{
    CatalogError, ConfiguredServer, MAX_MCP_SERVERS, McpCatalog, NamingStrategy, ServerName,
    ServerNameError, ToolEffectPolicy,
};
use serde::Deserialize;

use crate::adapter::McpToolAdapter;
use crate::client::{McpConnection, StdioCommand};
use crate::endpoint::{EndpointError, McpHttpEndpoint};
use crate::error::ConnectError;
use crate::host::{HostBuild, HostedServer, UnreadableServer, build_catalog};

/// The most bytes a host configuration document may occupy.
///
/// The same bound the daemon's own configuration uses, for the same reason: the file is read at startup,
/// and an unbounded read driven by a configuration file is work an attacker with write access to that
/// file would not have to justify.
pub const MAX_HOST_CONFIG_BYTES: usize = 1024 * 1024;

/// The document's top-level shape.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HostDocument {
    /// The servers the operator configured.
    #[serde(default)]
    servers: Vec<ServerDocument>,
}

/// One server entry.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ServerDocument {
    /// The operator-chosen local name.
    name: String,
    /// How to reach it.
    transport: TransportDocument,
    /// What its tools may do. Absent means nobody has classified it.
    #[serde(default)]
    posture: PostureDocument,
}

/// How a server is reached.
///
/// # The shape here is load-bearing, and the first version was wrong
///
/// `deny_unknown_fields` **cannot** be combined with an internally-tagged enum: the tag field is consumed
/// before the variant is deserialized, so the variant sees `kind` as an unrecognized key and fails with
/// "unknown field `kind`". Every document was refused, and the refusal was indistinguishable from a
/// genuinely unknown key because both map to [`HostConfigError::InvalidDocument`].
///
/// The fix is per-variant structs, each carrying its own `deny_unknown_fields` — which is where the
/// attribute actually works, because by then the tag has been removed. An unknown key inside a transport is
/// still refused, and so is an unknown transport kind; the two are simply refused by different layers.
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum TransportDocument {
    /// A child process speaking the stdio wire format.
    Stdio(StdioDocument),
    /// A remote endpoint speaking Streamable HTTP.
    Http(HttpDocument),
}

/// A stdio server: a program and its arguments.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StdioDocument {
    /// The program to run, spawned directly — never through a shell.
    program: String,
    /// Its arguments.
    #[serde(default)]
    args: Vec<String>,
}

/// A remote server: one endpoint URL.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HttpDocument {
    /// The endpoint URL, validated by [`McpHttpEndpoint`].
    endpoint: String,
}

/// The posture an operator stated.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PostureDocument {
    /// The posture class. Absent means `unclassified`.
    #[serde(default)]
    class: PostureClass,
}

/// The postures a configuration file can express.
///
/// Two values, and the refusal for anything else names them. See the module doc for why this is narrow
/// rather than a gap.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
enum PostureClass {
    /// Nobody has classified this server: outward-reaching, risk 3, every call held.
    #[default]
    Unclassified,
    /// The operator has established that this server only reads.
    ReadOnly,
}

impl PostureClass {
    /// Returns the policy this class denotes.
    ///
    /// # Errors
    ///
    /// Returns [`HostConfigError::UnusableConstant`] when a fixed value this crate supplies is rejected by
    /// its own constructor, which would be an authoring error rather than a configuration one. Reported so
    /// a future change to a constant produces a diagnosable refusal instead of a panic in a daemon.
    fn policy(self) -> Result<ToolEffectPolicy, HostConfigError> {
        match self {
            Self::Unclassified => Ok(ToolEffectPolicy::unclassified()),
            Self::ReadOnly => ToolEffectPolicy::read_only()
                .map_err(|error| HostConfigError::UnusableConstant(error.to_string())),
        }
    }
}

/// How to reach one configured server.
#[derive(Debug)]
pub enum ServerTransport {
    /// A child process, already validated.
    Stdio(StdioCommand),
    /// A remote endpoint, already validated.
    Http(McpHttpEndpoint),
}

/// A validated host configuration: what to connect to, and what each server's tools may do.
///
/// Holding both halves in one value is the point — an operator's posture and the coordinates of the
/// server it describes must travel together, or a policy could be applied to a server it was not written
/// about.
#[derive(Debug)]
pub struct McpHostConfig {
    servers: Vec<ConfiguredServer>,
    transports: Vec<ServerTransport>,
}

impl McpHostConfig {
    /// Parses a host configuration document.
    ///
    /// # Errors
    ///
    /// Returns [`HostConfigError`] for a malformed or oversized document, an unknown key at any level, an
    /// invalid server name, a program or endpoint that cannot be used, a server configured twice, or more
    /// servers than [`MAX_MCP_SERVERS`]. Every one of those is a configuration fault, so all of them are
    /// reported when the configuration is read rather than when a server is contacted.
    pub fn parse(document: &str) -> Result<Self, HostConfigError> {
        if document.len() > MAX_HOST_CONFIG_BYTES {
            return Err(HostConfigError::DocumentTooLarge);
        }
        let parsed: HostDocument = toml::from_str(document).map_err(|error| {
            // The span is a byte range; only its start is kept, and only as a character offset, because a
            // byte offset into a UTF-8 document is not what an editor shows. See the variant's doc for why
            // the parser's message is not forwarded.
            HostConfigError::InvalidDocument {
                at: error.span().map(|span| {
                    document
                        .get(..span.start)
                        .map_or(span.start, |prefix| prefix.chars().count())
                }),
            }
        })?;

        if parsed.servers.len() > MAX_MCP_SERVERS {
            return Err(HostConfigError::TooManyServers {
                limit: MAX_MCP_SERVERS,
            });
        }

        let mut servers = Vec::with_capacity(parsed.servers.len());
        let mut transports = Vec::with_capacity(parsed.servers.len());
        let mut seen: BTreeSet<ServerName> = BTreeSet::new();
        for entry in parsed.servers {
            let name = ServerName::new(&entry.name).map_err(HostConfigError::ServerName)?;
            // Refused here rather than left to the catalog, so the refusal names the **configuration file**
            // and the line an operator must edit. `McpCatalog::build` refuses the same condition, and both
            // checks existing is deliberate: this one is actionable, that one is the invariant.
            if !seen.insert(name.clone()) {
                return Err(HostConfigError::DuplicateServer {
                    server: name.to_string(),
                });
            }
            let transport = ServerTransport::from_document(&name, entry.transport)?;
            servers.push(ConfiguredServer {
                name,
                policy: entry.posture.class.policy()?,
            });
            transports.push(transport);
        }

        Ok(Self {
            servers,
            transports,
        })
    }

    /// Parses an empty configuration, which configures no servers.
    #[must_use]
    pub fn none() -> Self {
        Self {
            servers: Vec::new(),
            transports: Vec::new(),
        }
    }

    /// Returns the servers an operator configured.
    #[must_use]
    pub fn servers(&self) -> &[ConfiguredServer] {
        &self.servers
    }

    /// Returns how many servers are configured.
    #[must_use]
    pub fn len(&self) -> usize {
        self.servers.len()
    }

    /// Returns whether no servers are configured.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.servers.is_empty()
    }

    /// Connects to every configured server and builds one catalog from their tools.
    ///
    /// `strategy` decides how tool names become canonical identifiers; `seen_before` is what each server
    /// reported on an earlier build, so a changed self-description is visible ([`McpCatalog::observed`]).
    ///
    /// A server that cannot be contacted does **not** fail the build — one flaky third-party process must
    /// not empty the model's tool list — and is reported in [`McpHost::unreadable`]. A server that
    /// collides with another **is** a build failure, because a tool set that depended on configuration
    /// order is not something to serve.
    ///
    /// # Errors
    ///
    /// Returns [`HostError::Catalog`] when the configuration is ambiguous, and [`HostError::NoServers`]
    /// when nothing was configured — the second is an error rather than an empty host because a caller
    /// that asked for a host and received one that can do nothing would look healthy while offering no
    /// tools at all.
    pub async fn connect(
        &self,
        strategy: NamingStrategy,
        seen_before: &[jarvis_mcp::ObservedServer],
    ) -> Result<McpHost, HostError> {
        if self.servers.is_empty() {
            return Err(HostError::NoServers);
        }

        // The connections are held in one place and shared, so the host and every adapter refer to the
        // **same** connection. A second connection per adapter would open a second session (or a second
        // child process) and make the host's shutdown close only one of them.
        let mut hosted = Vec::with_capacity(self.servers.len());
        let mut unreachable = Vec::new();
        let mut opened: Vec<(ConfiguredServer, Arc<McpConnection>)> =
            Vec::with_capacity(self.servers.len());
        for (configured, transport) in self.servers.iter().zip(&self.transports) {
            match transport.connect(&configured.name).await {
                Ok(connection) => opened.push((configured.clone(), connection)),
                Err(reason) => unreachable.push(UnreadableServer {
                    server: configured.name.clone(),
                    reason,
                }),
            }
        }
        for (configured, connection) in &opened {
            hosted.push(HostedServer::new(
                configured.clone(),
                Arc::clone(connection),
            ));
        }

        // A server that could not be **contacted** contributes no listing, and `build_catalog` reports that
        // as "offered no tool listing" — which is true but less useful than the transport's own reason. Both
        // are kept: this one says why a server is missing, the catalog's says which tools a model will not
        // be offered.
        let HostBuild {
            catalog,
            unreadable,
        } = build_catalog(&hosted, strategy, seen_before)
            .await
            .map_err(HostError::Catalog)?;

        let mut unreadable = unreadable;
        unreadable.extend(unreachable);

        // **The first caller of the collision check.** Refused rather than partially served, because serving a
        // catalog whose contents depended on configuration order would make the reachable tool set a function
        // of an ordering nobody declared meaningful — the reasoning `McpCatalog` records, applied by the one
        // component that can act on it.
        //
        // **The catalog's own reason text is carried, and that is a finding rather than laziness.** Each
        // collision entry names only the server that *lost* the assignment, so an error built from those names
        // read `["bravo", "bravo"]` — naming one server twice while the remedy ("rename one of them") needs
        // **both**. The reason text is `NameCollision`'s, which names both sides, so it is what makes the
        // refusal actionable. A message that does not identify what must change is the opaque-diagnostic defect
        // in the one place an operator has nothing else to go on.
        if catalog.has_cross_server_collision() {
            let collisions: Vec<String> = catalog
                .collisions()
                .iter()
                .map(|collision| collision.reason.clone())
                .collect();
            return Err(HostError::Collision { collisions });
        }

        // The adapters are built from the catalog's own routing, so an adapter cannot name a tool the
        // catalog did not offer — the catalog is the only thing that knows a canonical identifier's
        // server-side name. Each adapter shares the connection the host holds.
        let adapters = hosted
            .iter()
            .map(|server| {
                McpToolAdapter::new(
                    &server.configured.name,
                    Arc::clone(&server.connection),
                    catalog.entries(),
                )
            })
            .collect();

        Ok(McpHost {
            catalog,
            unreadable,
            adapters,
            connections: hosted,
        })
    }
}

impl ServerTransport {
    /// Validates a transport from its document form.
    fn from_document(
        server: &ServerName,
        document: TransportDocument,
    ) -> Result<Self, HostConfigError> {
        match document {
            TransportDocument::Stdio(StdioDocument { program, args }) => {
                let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
                let command = StdioCommand::new(&program, &borrowed).map_err(|error| {
                    HostConfigError::Transport {
                        server: server.to_string(),
                        reason: error.to_string(),
                    }
                })?;
                Ok(Self::Stdio(command))
            }
            TransportDocument::Http(HttpDocument { endpoint }) => {
                let endpoint = McpHttpEndpoint::parse(&endpoint).map_err(|error| {
                    HostConfigError::Transport {
                        server: server.to_string(),
                        reason: describe_endpoint_error(&error),
                    }
                })?;
                Ok(Self::Http(endpoint))
            }
        }
    }

    /// Connects to the server this transport describes.
    async fn connect(&self, server: &ServerName) -> Result<std::sync::Arc<McpConnection>, String> {
        let connection = match self {
            Self::Stdio(command) => crate::connect_stdio(server, command).await,
            Self::Http(endpoint) => crate::connect_http(server, endpoint).await,
        };
        connection.map(std::sync::Arc::new).map_err(|error| {
            // The transport's own message is the actionable part, and it already distinguishes an
            // unreachable peer from a refusal from a legacy negotiation.
            let _: &ConnectError = &error;
            error.to_string()
        })
    }
}

/// A connected host: one catalog and the adapters that can run its tools.
///
/// Everything a caller needs to offer MCP tools, in one value — which is what makes it possible for the
/// daemon to hold *either* a configured host *or* nothing, rather than a set of pieces that can disagree.
pub struct McpHost {
    catalog: McpCatalog,
    unreadable: Vec<UnreadableServer>,
    adapters: Vec<McpToolAdapter>,
    connections: Vec<HostedServer>,
}

impl std::fmt::Debug for McpHost {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("McpHost")
            .field("tools", &self.catalog.len())
            .field("servers", &self.connections.len())
            .field("unreadable", &self.unreadable.len())
            .finish_non_exhaustive()
    }
}

impl McpHost {
    /// Returns the aggregated catalog.
    #[must_use]
    pub const fn catalog(&self) -> &McpCatalog {
        &self.catalog
    }

    /// Returns the servers that could not be read, each with a reason.
    #[must_use]
    pub fn unreadable(&self) -> &[UnreadableServer] {
        &self.unreadable
    }

    /// Returns the adapters, one per connected server.
    #[must_use]
    pub fn adapters(&self) -> &[McpToolAdapter] {
        &self.adapters
    }

    /// Returns how many tools the host offers.
    #[must_use]
    pub fn tool_count(&self) -> usize {
        self.catalog.len()
    }

    /// Returns the definitions to register, ready for `ToolRegistry::define_all`.
    #[must_use]
    pub fn definitions(&self) -> Vec<jarvis_tools::ToolDefinition> {
        self.catalog.definitions()
    }

    /// Finds the adapter that can run a tool, by canonical identifier.
    ///
    /// Returns `None` for a tool the host does not offer, so a dispatch cannot default to another server's
    /// adapter — the same reasoning `McpCatalog::route` applies to routing.
    #[must_use]
    pub fn adapter_for(&self, id: &jarvis_tools::ToolId) -> Option<&McpToolAdapter> {
        let entry = self.catalog.route(id)?;
        self.adapters
            .iter()
            .find(|adapter| adapter.server() == entry.server.as_str())
    }

    /// Closes every connection, returning each server that failed to stop.
    ///
    /// Takes `self` so a closed host cannot be used afterwards — a host holding dead connections while
    /// still reporting a tool count would look healthy and fail every call.
    ///
    /// The adapters are dropped **first**, because each holds a shared handle to a connection and a
    /// connection can only be closed by its last owner. Dropping them releases those handles, which is
    /// what makes each connection uniquely owned here. A connection that is somehow still shared is
    /// reported rather than silently left open, because "I could not stop this" is a fact worth a log
    /// line at shutdown.
    ///
    /// # Errors
    ///
    /// Returns the servers whose shutdown failed, as text. A shutdown failure is reported rather than
    /// fatal: the process is exiting, and which connections did not stop cleanly is worth recording.
    pub async fn close(self) -> Result<(), Vec<String>> {
        let Self {
            adapters,
            connections,
            ..
        } = self;
        drop(adapters);

        let mut failures = Vec::new();
        for server in connections {
            let name = server.configured.name.to_string();
            match Arc::try_unwrap(server.connection) {
                Ok(connection) => {
                    if let Err(reason) = connection.close().await {
                        failures.push(format!("{name}: {reason}"));
                    }
                }
                // Still shared, which should not happen once the adapters are gone. Reported rather than
                // claimed as a successful stop: the connection's own drop guard will cancel it, but saying
                // "closed" about a connection this function did not close would be a false statement.
                Err(_) => failures.push(format!(
                    "{name}: the connection is still shared and was not explicitly closed"
                )),
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures)
        }
    }
}

/// Why a host configuration was refused.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HostConfigError {
    /// The document could not be read as a host configuration.
    ///
    /// # Why this carries a position and not the parser's message
    ///
    /// A syntax error, a misspelled key, and a wrongly-typed value all arrive here, and the first version of
    /// this message asserted "an unknown key is refused rather than ignored" — a **cause it cannot know**.
    /// That was found by a fixture whose Windows path used a TOML basic string, where `\U` begins an invalid
    /// escape: a syntax error was reported as a key mistake, which is the opaque-diagnostic defect this
    /// project keeps removing, in the one place an operator has nothing else to go on.
    ///
    /// The parser's own text is **not** forwarded. It can echo the document, and a configuration file holds
    /// paths and hostnames that do not belong in a log line. A **character offset** is bounded, actionable,
    /// and reveals nothing: it tells an operator where to look without restating what they wrote.
    InvalidDocument {
        /// The character offset at which the document stopped being understood, when the parser reports one.
        at: Option<usize>,
    },
    /// The document exceeded [`MAX_HOST_CONFIG_BYTES`].
    DocumentTooLarge,
    /// A server name was not a usable local name.
    ServerName(ServerNameError),
    /// A server was configured more than once.
    ///
    /// Refused because two entries under one name are two claims about one server, and which won would
    /// depend on the order they were written in.
    DuplicateServer {
        /// The name that appeared twice.
        server: String,
    },
    /// More servers were configured than the catalog will hold.
    TooManyServers {
        /// The bound.
        limit: usize,
    },
    /// A server's transport could not be used.
    Transport {
        /// The server the transport belongs to.
        server: String,
        /// Why it was refused, reader-facing.
        reason: String,
    },
    /// A fixed value this crate supplies was rejected by its own constructor.
    UnusableConstant(String),
}

impl std::fmt::Display for HostConfigError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidDocument { at } => match at {
                Some(offset) => write!(
                    formatter,
                    "the MCP server configuration is not a valid document (it stops being \
                     understandable at character {offset}); a misspelled key, a wrong value type, and an \
                     invalid escape in a basic string all look like this, so check the keys this build \
                     defines and remember that a Windows path needs single quotes rather than double"
                ),
                None => write!(
                    formatter,
                    "the MCP server configuration is not a valid document; check the keys this build \
                     defines, and remember that a Windows path needs single quotes rather than double"
                ),
            },
            Self::DocumentTooLarge => write!(
                formatter,
                "the MCP server configuration exceeds {MAX_HOST_CONFIG_BYTES} bytes"
            ),
            Self::ServerName(source) => {
                write!(formatter, "an MCP server name is unusable: {source}")
            }
            Self::DuplicateServer { server } => write!(
                formatter,
                "the MCP server {server} is configured more than once"
            ),
            Self::TooManyServers { limit } => {
                write!(formatter, "at most {limit} MCP servers may be configured")
            }
            Self::Transport { server, reason } => {
                write!(
                    formatter,
                    "the MCP server {server} has an unusable transport: {reason}"
                )
            }
            Self::UnusableConstant(reason) => {
                write!(formatter, "a fixed MCP policy value was refused: {reason}")
            }
        }
    }
}

impl std::error::Error for HostConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ServerName(source) => Some(source),
            _ => None,
        }
    }
}

/// Why a host could not be built.
#[derive(Debug, thiserror::Error)]
pub enum HostError {
    /// Nothing was configured.
    ///
    /// An error rather than an empty host: a caller that asked for a host and received one offering no
    /// tools would look healthy while being useless, and the failure would appear later as "a tool is
    /// missing" rather than as "nothing was configured".
    #[error("no MCP servers are configured, so there is no host to build")]
    NoServers,
    /// The configuration was ambiguous.
    #[error(transparent)]
    Catalog(#[from] CatalogError),
    /// Two configured servers offer tools that become one canonical identifier.
    ///
    /// Refused rather than partially served, because which server's tool survived would depend on the
    /// order the servers were written in — an ordering nobody declared meaningful.
    #[error(
        "configured MCP servers offer tools that become the same identifier: {joined}; rename one of the \
         named servers or choose a naming strategy that keeps them apart",
        joined = collisions.join("; ")
    )]
    Collision {
        /// One description per collision, each naming **both** servers involved.
        ///
        /// The catalog's own `NameCollision` text rather than a server name, because a collision entry records
        /// only the losing side and a refusal that names one server twice does not say what to change.
        collisions: Vec<String>,
    },
}

/// Names an endpoint refusal in terms an operator can act on.
///
/// The endpoint error already distinguishes a plaintext remote endpoint from a fragment from embedded
/// credentials; this only carries it as text so the configuration error names one actionable reason.
fn describe_endpoint_error(error: &EndpointError) -> String {
    error.to_string()
}

/// Extracts the host configuration section from a daemon configuration document, when present.
///
/// The daemon's own configuration is a different schema with its own validation, and this is deliberately
/// **not** a second parser for it: the caller passes the already-extracted `[mcp]` section text. That keeps
/// one schema owner (`jarvis-storage`) and one host-schema owner (here), with no overlap to disagree.
///
/// # Errors
///
/// Returns [`HostConfigError`] under the same conditions as [`McpHostConfig::parse`].
pub fn parse_host_section(section: &str) -> Result<McpHostConfig, HostConfigError> {
    McpHostConfig::parse(section)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_document_configures_no_servers() {
        let config = McpHostConfig::parse("").unwrap_or_else(|error| panic!("{error}"));
        assert!(config.is_empty());
        assert_eq!(config.len(), 0);
    }

    /// A stdio server is parsed, and its posture defaults to the **severe** one. The default matters more
    /// than the parse: an operator who omits the posture must get a server every call of which is held, not
    /// one presumed harmless.
    #[test]
    fn a_stdio_server_defaults_to_the_unclassified_posture() {
        let document = r#"
            [[servers]]
            name = "local-tools"
            transport = { kind = "stdio", program = "mcp-server", args = ["--port", "0"] }
        "#;
        let config = McpHostConfig::parse(document).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(config.len(), 1);
        let server = &config.servers[0];
        assert_eq!(server.name.as_str(), "local-tools");
        // The default posture is severe: risk 3 and every call asked about.
        assert_eq!(server.policy.posture().risk, 3);
        assert_eq!(
            server.policy.posture().approval,
            jarvis_tools::ApprovalPolicy::Ask
        );
        // The program and arguments survived in order.
        let ServerTransport::Stdio(command) = &config.transports[0] else {
            panic!("a stdio entry must produce a stdio transport");
        };
        assert_eq!(command.program(), "mcp-server");
        assert_eq!(command.args(), ["--port", "0"]);
    }

    /// **The one posture an operator can state** produces risk 0 and `Auto`, which is the positive control
    /// for the default above: if both produced the same policy, the class would be ignored.
    #[test]
    fn a_read_only_posture_is_honoured() {
        let document = r#"
            [[servers]]
            name = "reader"
            transport = { kind = "stdio", program = "mcp-read" }
            posture = { class = "read-only" }
        "#;
        let config = McpHostConfig::parse(document).unwrap_or_else(|error| panic!("{error}"));
        let server = &config.servers[0];
        assert_eq!(server.policy.posture().risk, 0);
        assert_eq!(
            server.policy.posture().approval,
            jarvis_tools::ApprovalPolicy::Auto
        );
    }

    /// **An unknown posture is refused, not ignored.** A typo in a posture must not silently leave a server
    /// at the permissive value an operator was trying to set — the failure direction is what makes this
    /// worth a test.
    #[test]
    fn an_unknown_posture_is_refused() {
        let document = r#"
            [[servers]]
            name = "reader"
            transport = { kind = "stdio", program = "mcp-read" }
            posture = { class = "read_only" }
        "#;
        // `read_only` is snake case; the vocabulary is kebab-case, so this is a typo and must be refused.
        let error = McpHostConfig::parse(document)
            .err()
            .unwrap_or_else(|| panic!("an unknown posture must be refused"));
        assert!(
            matches!(error, HostConfigError::InvalidDocument { .. }),
            "{error}"
        );
        // The refusal says where the document stopped being understood, and warns about the most likely
        // cause rather than asserting one — a syntax error and an unknown key are indistinguishable here.
        let text = error.to_string();
        assert!(text.contains("character 1"), "{text}");
        assert!(text.contains("single quotes"), "{text}");
    }

    /// An unknown key at **any** level is refused. The daemon's own configuration has this property, and a
    /// host configuration that ignored a misspelled `endpont` would start and then fail every call.
    #[test]
    fn an_unknown_key_is_refused_at_every_level() {
        for document in [
            // Top level.
            "server = 1",
            // Server level.
            "[[servers]]\nname = \"a\"\ntransport = { kind = \"stdio\", program = \"p\" }\nposturex = 1",
            // Transport level.
            "[[servers]]\nname = \"a\"\ntransport = { kind = \"stdio\", program = \"p\", cwd = \"/\" }",
            // Posture level.
            "[[servers]]\nname = \"a\"\ntransport = { kind = \"stdio\", program = \"p\" }\nposture = { klass = \"read-only\" }",
            // Transport kind, which must be one of the two shapes.
            "[[servers]]\nname = \"a\"\ntransport = { kind = \"ws\", endpoint = \"wss://x\" }",
        ] {
            let error = McpHostConfig::parse(document)
                .err()
                .unwrap_or_else(|| panic!("{document} must be refused"));
            // The variant and the operator-facing shape, not an exact offset: the offset is diagnostic
            // detail whose value depends on the parser, while "this document is not valid" is the fact.
            assert!(
                matches!(error, HostConfigError::InvalidDocument { .. }),
                "{document}: {error}"
            );
            assert!(
                error.to_string().contains("not a valid document"),
                "{document}"
            );
        }
    }

    #[test]
    fn a_server_configured_twice_is_refused_by_name() {
        let document = r#"
            [[servers]]
            name = "twice"
            transport = { kind = "stdio", program = "a" }

            [[servers]]
            name = "twice"
            transport = { kind = "stdio", program = "b" }
        "#;
        let error = McpHostConfig::parse(document)
            .err()
            .unwrap_or_else(|| panic!("one name twice must be refused"));
        assert_eq!(
            error,
            HostConfigError::DuplicateServer {
                server: "twice".to_owned()
            }
        );
    }

    #[test]
    fn an_unusable_name_is_refused() {
        // Dots are refused, because `mcp.a.b` would be ambiguous between server `a.b` and server `a`.
        let document =
            "[[servers]]\nname = \"a.b\"\ntransport = { kind = \"stdio\", program = \"p\" }";
        assert!(matches!(
            McpHostConfig::parse(document),
            Err(HostConfigError::ServerName(_))
        ));
    }

    #[test]
    fn an_empty_program_is_refused() {
        let document =
            "[[servers]]\nname = \"a\"\ntransport = { kind = \"stdio\", program = \"  \" }";
        let error = McpHostConfig::parse(document)
            .err()
            .unwrap_or_else(|| panic!("an empty program must be refused"));
        assert!(
            matches!(error, HostConfigError::Transport { .. }),
            "{error}"
        );
    }

    /// The endpoint validation is the transport's, applied at configuration time — so a plaintext remote
    /// endpoint is refused when the file is read rather than when the server is contacted.
    #[test]
    fn a_plaintext_remote_endpoint_is_refused_at_parse_time() {
        let document = "[[servers]]\nname = \"a\"\ntransport = { kind = \"http\", endpoint = \"http://remote.example/mcp\" }";
        let error = McpHostConfig::parse(document)
            .err()
            .unwrap_or_else(|| panic!("plaintext remote must be refused"));
        assert!(
            matches!(error, HostConfigError::Transport { .. }),
            "{error}"
        );
        assert!(error.to_string().contains("https"), "{error}");

        // A loopback endpoint is accepted, which is the positive control.
        let loopback = "[[servers]]\nname = \"a\"\ntransport = { kind = \"http\", endpoint = \"http://127.0.0.1:8000/mcp\" }";
        assert!(McpHostConfig::parse(loopback).is_ok());
    }

    /// An endpoint carrying a credential in its userinfo is refused, so a configuration file cannot hold a
    /// secret even by accident.
    #[test]
    fn an_endpoint_with_embedded_credentials_is_refused() {
        let document = "[[servers]]\nname = \"a\"\ntransport = { kind = \"http\", endpoint = \"https://user:secret@remote.example/mcp\" }";
        let error = McpHostConfig::parse(document)
            .err()
            .unwrap_or_else(|| panic!("embedded credentials must be refused"));
        assert!(error.to_string().contains("separately"), "{error}");
    }

    #[test]
    fn too_many_servers_are_refused() {
        use std::fmt::Write as _;

        let mut document = String::new();
        for index in 0..=MAX_MCP_SERVERS {
            // `writeln!` rather than `push_str(&format!(..))`, which clippy correctly prefers: the
            // intermediate allocation is pure cost for a test fixture.
            let _ = writeln!(
                document,
                "[[servers]]\nname = \"s{index}\"\ntransport = {{ kind = \"stdio\", program = \"p\" }}"
            );
        }
        let error = McpHostConfig::parse(&document)
            .err()
            .unwrap_or_else(|| panic!("more than the bound must be refused"));
        assert_eq!(
            error,
            HostConfigError::TooManyServers {
                limit: MAX_MCP_SERVERS
            }
        );
    }

    #[test]
    fn an_oversized_document_is_refused() {
        let document = "x".repeat(MAX_HOST_CONFIG_BYTES + 1);
        let error = McpHostConfig::parse(&document)
            .err()
            .unwrap_or_else(|| panic!("an oversized document must be refused"));
        assert_eq!(error, HostConfigError::DocumentTooLarge);
    }

    /// The host section helper is the same parser, so a caller extracting `[mcp]` from the daemon's
    /// document gets identical validation rather than a second, weaker path.
    #[test]
    fn the_host_section_helper_uses_the_same_validation() {
        let document =
            "[[servers]]\nname = \"a\"\ntransport = { kind = \"stdio\", program = \"p\" }";
        let direct = McpHostConfig::parse(document).unwrap_or_else(|error| panic!("{error}"));
        let via_helper = parse_host_section(document).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(direct.len(), via_helper.len());

        // And a refusal is identical too, which is what makes the helper safe to use.
        let bad = "server = 1";
        assert_eq!(
            McpHostConfig::parse(bad).err(),
            parse_host_section(bad).err()
        );
    }

    /// Configuring nothing and asking for a host is an error rather than an empty host.
    #[tokio::test]
    async fn connecting_with_no_servers_is_an_error() {
        let config = McpHostConfig::none();
        let error = config
            .connect(NamingStrategy::Prefixed, &[])
            .await
            .err()
            .unwrap_or_else(|| panic!("no servers must not yield a host"));
        assert!(matches!(error, HostError::NoServers), "{error}");
    }

    /// A configured server that cannot be contacted does not fail the build, and the reason is preserved
    /// separately from the catalog's generic "offered no listing".
    #[tokio::test]
    async fn an_unreachable_server_does_not_fail_the_build() {
        let document = "[[servers]]\nname = \"absent\"\ntransport = { kind = \"stdio\", program = \"jarvis-no-such-program-98765\" }";
        let config = McpHostConfig::parse(document).unwrap_or_else(|error| panic!("{error}"));
        let host = config
            .connect(NamingStrategy::Prefixed, &[])
            .await
            .unwrap_or_else(|error| {
                panic!("an unreachable server must not fail the build: {error}")
            });

        // The server is reported, by name, with the transport's own reason.
        assert_eq!(host.unreadable().len(), 1);
        assert_eq!(host.unreadable()[0].server.as_str(), "absent");
        assert!(
            host.unreadable()[0].reason.contains("could not be reached"),
            "{}",
            host.unreadable()[0].reason
        );
        // And it offers no tools, because the only configured server could not be reached.
        assert_eq!(host.tool_count(), 0);
        assert!(host.catalog().is_empty());
        // Closing a host with a failed connection still succeeds: there is nothing to stop.
        assert!(host.close().await.is_ok());
    }
}
