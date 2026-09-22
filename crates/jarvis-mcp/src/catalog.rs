//! The multi-server catalog: every configured MCP server, and what happens when they collide.
//!
//! # Why this is a separate type from a single listing
//!
//! [`crate::translate_listing`] answers "what did this one server offer". A catalog answers a
//! different question with a different failure mode: **what happens when two servers offer the same
//! name.** Those are not the same problem, and `P3-008b` recorded the gap honestly — a per-listing
//! call builds its own assignment set, so it structurally *cannot* see a collision between servers.
//!
//! This module is the fix. It owns **one** [`NameAssignments`] across every server, so a collision
//! between two servers is detected at exactly the boundary where the information exists.
//!
//! # The ordering decision, which is a security decision
//!
//! The specification requires each server's own `tools/list` to be deterministic, but it says
//! nothing about the order in which a client iterates *servers* — and that order decides which
//! collision loses. Since a collision is refused rather than resolved, the losing server's tool is
//! **absent from discovery entirely**, so an operator who reorders their configuration can silently
//! change which of two colliding tools is reachable.
//!
//! A catalog therefore **refuses to serve at all when two servers collide**, rather than dropping one.
//! Serving a catalog whose contents depend on configuration order would mean the set of tools a model
//! can call is a function of an ordering nobody declared as meaningful. Refusing makes the ambiguity
//! the operator's to resolve, which is the only place it can be resolved correctly — the same
//! reasoning `NameAssignments` applies per tool, applied one level up.
//!
//! # Why the server count is bounded
//!
//! Each server costs at least one tool in the registry (`ToolRegistry::MAX_REGISTERED_TOOLS` is 512),
//! and a catalog that read an unbounded list of servers would be an unbounded amount of work driven
//! by a config file. Bounding the *count* also bounds the number of connections, which is the
//! resource an attacker with config write access would exhaust first.

use std::collections::BTreeSet;
use std::fmt;

use jarvis_tools::{MAX_REGISTERED_TOOLS, ToolDefinition};

use crate::definition::{
    ListingOutcome, McpToolListing, ToolEffectPolicy, TranslatedTool, translate_listing,
};
use crate::server::{NameAssignments, NamingStrategy, ServerName};

/// The most MCP servers one catalog will hold.
///
/// Sixteen, because each server is a process or a network endpoint with its own tool count, and a
/// configuration with more than a handful of MCP servers is a sign the set should be split rather
/// than a case to optimise for. Bounded so a config file cannot make the daemon's startup unbounded.
pub const MAX_MCP_SERVERS: usize = 16;

/// One configured server and its posture.
///
/// The posture is per **server**, not per tool, because that is the granularity an operator can
/// reasonably state: "this server only reads" is a fact about a server, and asking an operator to
/// classify each of a remote server's tools individually would be asking them to maintain a copy of
/// a list the server owns and can change without notice.
#[derive(Clone, Debug)]
pub struct ConfiguredServer {
    /// The operator-chosen local name.
    pub name: ServerName,
    /// The operator's statement about what this server's tools may do.
    pub policy: ToolEffectPolicy,
}

/// Explains why a listing could not be added to the catalog.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CatalogError {
    /// The catalog already holds this server.
    ///
    /// A server added twice would have its tools translated twice into the same identifiers, and the
    /// second translation would look like an idempotent refresh of the first — which is exactly the
    /// case `NameAssignments` deliberately accepts. So it is refused here, where the ambiguity is
    /// visible, rather than silently tolerated.
    DuplicateServer {
        /// The server name that appeared twice.
        server: String,
    },
    /// The catalog holds as many servers as it will.
    TooManyServers {
        /// The bound.
        limit: usize,
    },
}

impl fmt::Display for CatalogError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateServer { server } => write!(
                formatter,
                "the MCP server {server} is configured more than once"
            ),
            Self::TooManyServers { limit } => {
                write!(formatter, "an MCP catalog holds at most {limit} servers")
            }
        }
    }
}

impl std::error::Error for CatalogError {}

/// A tool that could not be offered, and by which server.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogExclusion {
    /// The server that offered it.
    pub server: String,
    /// The server's own tool name.
    pub tool: String,
    /// Why it was excluded, reader-facing.
    pub reason: String,
}

/// One tool the catalog will offer, with the routing information a call needs.
#[derive(Clone, Debug)]
pub struct CatalogEntry {
    /// The canonical definition.
    pub definition: ToolDefinition,
    /// The server to send the call to.
    pub server: ServerName,
    /// The server's own tool name, which is what `tools/call` must carry.
    pub remote: String,
}

/// Every tool a set of configured servers offers.
///
/// Holds **routing**, not authority: `definition` carries the effects, risk, and approval posture,
/// and `server` plus `remote` carry where to send the call. A `CatalogEntry` is therefore everything
/// a caller needs to build a `ToolExecutionRequest` without consulting the catalog again, which keeps
/// the two from disagreeing across an await.
#[derive(Clone, Debug, Default)]
pub struct McpCatalog {
    entries: Vec<CatalogEntry>,
    exclusions: Vec<CatalogExclusion>,
    /// Cross-server collisions, held separately because they are the one condition that makes the
    /// whole catalog unsafe to serve.
    ///
    /// A separate list rather than a flag derived from `exclusions` by inspecting its text: matching
    /// on a `Display` string would make the safety question depend on message wording, so a reworded
    /// error would silently stop the catalog refusing. The category is data, not prose.
    collisions: Vec<CatalogExclusion>,
    truncations: Vec<CatalogExclusion>,
}

impl McpCatalog {
    /// Translates every listing, refusing any tool that collides across servers.
    ///
    /// `listings` is paired with `servers` by the returned entries rather than by position: a server
    /// whose name is unknown is refused, so a caller cannot supply listings in an order that
    /// silently attributes one server's tools to another.
    ///
    /// # Errors
    ///
    /// Returns [`CatalogError`] when a server appears twice in `servers`, or when there are more
    /// servers than [`MAX_MCP_SERVERS`]. Neither is a per-tool condition: both mean the configuration
    /// itself is ambiguous, and serving a subset of an ambiguous configuration would hide that.
    pub fn build(
        servers: &[ConfiguredServer],
        listings: &[(ServerName, Vec<McpToolListing<'_>>)],
        strategy: NamingStrategy,
    ) -> Result<Self, CatalogError> {
        let mut seen = BTreeSet::new();
        for configured in servers {
            if !seen.insert(configured.name.clone()) {
                return Err(CatalogError::DuplicateServer {
                    server: configured.name.to_string(),
                });
            }
        }
        if servers.len() > MAX_MCP_SERVERS {
            return Err(CatalogError::TooManyServers {
                limit: MAX_MCP_SERVERS,
            });
        }

        let mut assignments = NameAssignments::new();
        let mut entries: Vec<CatalogEntry> = Vec::new();
        let mut exclusions = Vec::new();
        let mut collisions = Vec::new();

        for configured in servers {
            let Some((_, tools)) = listings
                .iter()
                .find(|(server, _)| server == &configured.name)
            else {
                // A configured server with no listing contributed no tools. Not an error: a server may
                // be unreachable, or may simply offer nothing. Reported as an exclusion so the absence
                // is visible rather than looking like a server that was never configured.
                exclusions.push(CatalogExclusion {
                    server: configured.name.to_string(),
                    tool: String::new(),
                    reason: "the server offered no tool listing".to_owned(),
                });
                continue;
            };

            let ListingOutcome {
                tools: translated,
                excluded,
            } = translate_listing(&configured.name, tools, strategy, &configured.policy);

            for exclusion in excluded {
                exclusions.push(CatalogExclusion {
                    server: configured.name.to_string(),
                    tool: exclusion.tool,
                    reason: exclusion.reason,
                });
            }
            for tool in translated {
                if let Some(collision) = record(&mut assignments, &configured.name, &tool) {
                    // A cross-server collision is NOT an ordinary exclusion. It is collected
                    // separately so the caller can refuse to serve, because dropping one side would
                    // make the catalog's contents depend on configuration order.
                    collisions.push(CatalogExclusion {
                        server: configured.name.to_string(),
                        tool: tool.remote.clone(),
                        reason: collision,
                    });
                    continue;
                }
                entries.push(CatalogEntry {
                    definition: tool.definition,
                    server: configured.name.clone(),
                    remote: tool.remote,
                });
            }
        }

        // Deterministic output order, so a catalog built from the same configuration twice is equal
        // and a discovery list does not depend on iteration order.
        entries.sort_by(|left, right| left.definition.id().cmp(right.definition.id()));
        let sort_exclusions = |list: &mut Vec<CatalogExclusion>| {
            list.sort_by(|left, right| {
                left.server
                    .cmp(&right.server)
                    .then_with(|| left.tool.cmp(&right.tool))
            });
        };
        sort_exclusions(&mut exclusions);
        sort_exclusions(&mut collisions);

        // The registry bound is enforced here rather than discovered at registration, because a
        // catalog that silently exceeded it would fail later, in a different component, with an
        // error that names the registry rather than the configuration that caused it.
        let mut kept = Vec::with_capacity(entries.len().min(MAX_REGISTERED_TOOLS));
        let mut truncations = Vec::new();
        for entry in entries {
            if kept.len() >= MAX_REGISTERED_TOOLS {
                truncations.push(CatalogExclusion {
                    server: entry.server.to_string(),
                    tool: entry.remote,
                    reason: format!(
                        "the registry holds at most {MAX_REGISTERED_TOOLS} tools, and this one was \
                         beyond the bound"
                    ),
                });
                continue;
            }
            kept.push(entry);
        }

        Ok(Self {
            entries: kept,
            exclusions,
            collisions,
            truncations,
        })
    }

    /// Returns the tools the catalog will offer, in identifier order.
    #[must_use]
    pub fn entries(&self) -> &[CatalogEntry] {
        &self.entries
    }

    /// Returns the tools excluded for a reason specific to them.
    #[must_use]
    pub fn exclusions(&self) -> &[CatalogExclusion] {
        &self.exclusions
    }

    /// Returns the tools dropped because the registry was full.
    ///
    /// Separate from [`Self::exclusions`] because the cause is different and so is the remedy: an
    /// exclusion is about that tool, while a truncation is about the *size* of the configuration. An
    /// operator who fixed a truncation by fixing a single tool would learn nothing.
    #[must_use]
    pub fn truncations(&self) -> &[CatalogExclusion] {
        &self.truncations
    }

    /// Returns whether any tool collided with a tool from another server.
    ///
    /// A caller should refuse to serve when this is true. Read from a dedicated list rather than from
    /// an exclusion's text, so a reworded error cannot silently stop the refusal.
    #[must_use]
    pub fn has_cross_server_collision(&self) -> bool {
        !self.collisions.is_empty()
    }

    /// Returns the cross-server collisions, which are the reason a catalog may not be served.
    #[must_use]
    pub fn collisions(&self) -> &[CatalogExclusion] {
        &self.collisions
    }

    /// Returns the definitions, ready for `ToolRegistry::define_all`.
    #[must_use]
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.entries
            .iter()
            .map(|entry| entry.definition.clone())
            .collect()
    }

    /// Returns the server-supplied name to send for a canonical identifier.
    ///
    /// Returns `None` for a tool the catalog does not hold, so a caller cannot look up routing for a
    /// tool it was never offered and get a default that names some other server.
    #[must_use]
    pub fn route(&self, id: &jarvis_tools::ToolId) -> Option<&CatalogEntry> {
        self.entries
            .iter()
            .find(|entry| entry.definition.id() == id)
    }

    /// Returns how many tools the catalog offers.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns whether the catalog offers nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns every exclusion, including collisions and truncations, for an operator-facing report.
    #[must_use]
    pub fn all_exclusions(&self) -> Vec<&CatalogExclusion> {
        self.exclusions
            .iter()
            .chain(self.collisions.iter())
            .chain(self.truncations.iter())
            .collect()
    }
}

/// Records an assignment, returning the renderable collision reason when one occurred.
fn record(
    assignments: &mut NameAssignments,
    server: &ServerName,
    tool: &TranslatedTool,
) -> Option<String> {
    let canonical = crate::server::CanonicalToolName {
        id: tool.definition.id().clone(),
        remote: tool.remote.clone(),
    };
    assignments
        .assign(server, &canonical)
        .err()
        .map(|collision| collision.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn server(name: &str) -> ServerName {
        ServerName::new(name).unwrap_or_else(|error| panic!("fixture server {name}: {error}"))
    }

    fn schema() -> Value {
        json!({
            "type": "object",
            "properties": { "query": { "type": "string" } },
            "additionalProperties": false
        })
    }

    fn configured(name: &str) -> ConfiguredServer {
        ConfiguredServer {
            name: server(name),
            policy: ToolEffectPolicy::read_only().unwrap_or_else(|error| panic!("{error}")),
        }
    }

    /// Builds a listing pair for one server offering the given tool names.
    fn offers<'a>(
        name: &str,
        tools: &'a [String],
        schema: &'a Value,
    ) -> (ServerName, Vec<McpToolListing<'a>>) {
        let listings = tools
            .iter()
            .map(|tool| McpToolListing {
                name: tool.as_str(),
                title: None,
                description: None,
                input_schema: schema,
                output_schema: None,
            })
            .collect();
        (server(name), listings)
    }

    fn names(tools: &[&str]) -> Vec<String> {
        tools.iter().map(|tool| (*tool).to_owned()).collect()
    }

    #[test]
    fn two_servers_offering_different_names_produce_both_tools() {
        let schema = schema();
        let first = names(&["search"]);
        let second = names(&["list_issues"]);
        let catalog = McpCatalog::build(
            &[configured("alpha"), configured("bravo")],
            &[
                offers("alpha", &first, &schema),
                offers("bravo", &second, &schema),
            ],
            NamingStrategy::Prefixed,
        )
        .unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(catalog.len(), 2);
        assert!(!catalog.has_cross_server_collision());
        let identifiers: Vec<String> = catalog
            .entries()
            .iter()
            .map(|entry| entry.definition.id().to_string())
            .collect();
        assert_eq!(
            identifiers,
            vec!["mcp.alpha.search", "mcp.bravo.list_issues"],
            "entries are in identifier order"
        );
    }

    /// **The defect the per-listing function structurally could not see.** Two servers offer one name
    /// under `Bare`, and the catalog must report it rather than drop one side — because dropping one
    /// side would make the reachable tool set depend on configuration order.
    #[test]
    fn a_cross_server_collision_is_reported_not_resolved() {
        let schema = schema();
        let shared = names(&["search"]);
        let catalog = McpCatalog::build(
            &[configured("alpha"), configured("bravo")],
            &[
                offers("alpha", &shared, &schema),
                offers("bravo", &shared, &schema),
            ],
            NamingStrategy::Bare,
        )
        .unwrap_or_else(|error| panic!("{error}"));

        assert!(
            catalog.has_cross_server_collision(),
            "two servers on one bare name must be reported"
        );
        assert_eq!(
            catalog.len(),
            1,
            "the first is kept; the second is not added"
        );
        // The collision is its own list, so the caller that refuses to serve reads a category rather
        // than a message.
        assert_eq!(catalog.collisions().len(), 1);
        // And the exclusion names both servers, so an operator can decide which to rename.
        let collision = &catalog.collisions()[0];
        assert!(collision.reason.contains("alpha"), "{}", collision.reason);
        assert!(collision.reason.contains("bravo"), "{}", collision.reason);
    }

    /// Reordering the configuration must not change *whether* there is a collision — that is what
    /// makes refusing-to-serve the correct answer rather than an arbitrary winner.
    #[test]
    fn the_collision_detection_does_not_depend_on_server_order() {
        let schema = schema();
        let shared = names(&["search"]);
        for order in [["alpha", "bravo"], ["bravo", "alpha"]] {
            let catalog = McpCatalog::build(
                &[configured(order[0]), configured(order[1])],
                &[
                    offers("alpha", &shared, &schema),
                    offers("bravo", &shared, &schema),
                ],
                NamingStrategy::Bare,
            )
            .unwrap_or_else(|error| panic!("{error}"));
            assert!(
                catalog.has_cross_server_collision(),
                "order {order:?} must still report the collision"
            );
            assert_eq!(catalog.len(), 1, "order {order:?}");
        }
    }

    /// The prefixed strategy is the default precisely because it makes this case go away.
    #[test]
    fn the_prefixed_strategy_keeps_two_servers_apart() {
        let schema = schema();
        let shared = names(&["search"]);
        let catalog = McpCatalog::build(
            &[configured("alpha"), configured("bravo")],
            &[
                offers("alpha", &shared, &schema),
                offers("bravo", &shared, &schema),
            ],
            NamingStrategy::Prefixed,
        )
        .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(catalog.len(), 2);
        assert!(!catalog.has_cross_server_collision());
    }

    /// Routing is what a call needs, and it must be the *server's* name rather than the canonical one.
    #[test]
    fn each_entry_carries_the_server_and_the_remote_name() {
        let schema = schema();
        let tools = names(&["Search"]);
        let catalog = McpCatalog::build(
            &[configured("alpha")],
            &[offers("alpha", &tools, &schema)],
            NamingStrategy::Hashed,
        )
        .unwrap_or_else(|error| panic!("{error}"));

        let entry = catalog
            .entries()
            .first()
            .unwrap_or_else(|| panic!("expected an entry"));
        assert_eq!(entry.server.as_str(), "alpha");
        assert_eq!(
            entry.remote, "Search",
            "the server's own name, not the canonical one"
        );
        assert!(entry.definition.id().name().starts_with('h'));

        // And routing by identifier finds it, carrying both halves.
        let routed = catalog
            .route(entry.definition.id())
            .unwrap_or_else(|| panic!("the entry must be routable by its identifier"));
        assert_eq!(routed.server.as_str(), "alpha");
        assert_eq!(routed.remote, "Search");
    }

    /// A tool the catalog does not hold must not be routable to some other server by default.
    #[test]
    fn an_unknown_identifier_is_not_routed() {
        let schema = schema();
        let tools = names(&["search"]);
        let catalog = McpCatalog::build(
            &[configured("alpha")],
            &[offers("alpha", &tools, &schema)],
            NamingStrategy::Prefixed,
        )
        .unwrap_or_else(|error| panic!("{error}"));
        let absent =
            jarvis_tools::ToolId::new("mcp.alpha.absent").unwrap_or_else(|error| panic!("{error}"));
        assert!(catalog.route(&absent).is_none());
    }

    #[test]
    fn a_server_configured_twice_is_refused() {
        let schema = schema();
        let tools = names(&["search"]);
        let error = McpCatalog::build(
            &[configured("alpha"), configured("alpha")],
            &[offers("alpha", &tools, &schema)],
            NamingStrategy::Prefixed,
        )
        .err()
        .unwrap_or_else(|| panic!("a server configured twice is ambiguous"));
        assert_eq!(
            error,
            CatalogError::DuplicateServer {
                server: "alpha".to_owned()
            }
        );
    }

    #[test]
    fn a_server_with_no_listing_is_visible_rather_than_absent() {
        let schema = schema();
        let tools = names(&["search"]);
        let catalog = McpCatalog::build(
            &[configured("alpha"), configured("silent")],
            &[offers("alpha", &tools, &schema)],
            NamingStrategy::Prefixed,
        )
        .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(catalog.len(), 1);
        // The absence is reported, so it does not look like a server that was never configured.
        assert!(
            catalog
                .exclusions()
                .iter()
                .any(|exclusion| exclusion.server == "silent"
                    && exclusion.reason.contains("no tool listing")),
            "{:?}",
            catalog.exclusions()
        );
    }

    /// A per-tool exclusion from one server must not affect another server's tool.
    #[test]
    fn a_bad_tool_in_one_server_leaves_the_other_alone() {
        let good = schema();
        let bad = json!({
            "type": "object",
            "properties": { "q": { "$ref": "https://example.invalid/x.json" } }
        });
        let catalog = McpCatalog::build(
            &[configured("alpha"), configured("bravo")],
            &[
                (
                    server("alpha"),
                    vec![McpToolListing {
                        name: "broken",
                        title: None,
                        description: None,
                        input_schema: &bad,
                        output_schema: None,
                    }],
                ),
                offers("bravo", &names(&["fine"]), &good),
            ],
            NamingStrategy::Prefixed,
        )
        .unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(catalog.len(), 1);
        assert_eq!(catalog.entries()[0].server.as_str(), "bravo");
        assert_eq!(catalog.exclusions().len(), 1);
        assert_eq!(catalog.exclusions()[0].server, "alpha");
        assert_eq!(catalog.exclusions()[0].tool, "broken");
        assert!(!catalog.has_cross_server_collision());
    }

    /// The registry bound is enforced where the configuration can be blamed, and a truncation is
    /// reported separately from an exclusion because the remedy differs.
    #[test]
    fn exceeding_the_registry_bound_is_reported_as_a_truncation() {
        // One server is enough: the bound is on total tools, and this is the cheapest way to exceed
        // it without building 512 definitions by hand being the point of the test.
        let schema = schema();
        let many: Vec<String> = (0..(MAX_REGISTERED_TOOLS + 3))
            .map(|index| format!("tool_{index}"))
            .collect();
        let catalog = McpCatalog::build(
            &[configured("alpha")],
            &[offers("alpha", &many, &schema)],
            NamingStrategy::Prefixed,
        )
        .unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(catalog.len(), MAX_REGISTERED_TOOLS);
        assert_eq!(catalog.truncations().len(), 3);
        assert!(
            catalog.exclusions().is_empty(),
            "a truncation is not an exclusion"
        );
        // `all_exclusions` is what an operator report renders, so both must appear there.
        assert_eq!(catalog.all_exclusions().len(), 3);
        assert!(
            catalog.truncations()[0].reason.contains("at most"),
            "{}",
            catalog.truncations()[0].reason
        );
    }

    #[test]
    fn too_many_servers_is_refused() {
        let schema = schema();
        let tool = names(&["search"]);
        // Every server offers the SAME tool name, and the bound is checked before any translation, so
        // the refusal is about the server count rather than about a collision.
        let servers: Vec<ConfiguredServer> = (0..=MAX_MCP_SERVERS)
            .map(|index| configured(&format!("s{index}")))
            .collect();
        let listings: Vec<(ServerName, Vec<McpToolListing<'_>>)> = servers
            .iter()
            .map(|configured| {
                (
                    configured.name.clone(),
                    tool.iter()
                        .map(|name| McpToolListing {
                            name: name.as_str(),
                            title: None,
                            description: None,
                            input_schema: &schema,
                            output_schema: None,
                        })
                        .collect::<Vec<_>>(),
                )
            })
            .collect();
        let error = McpCatalog::build(&servers, &listings, NamingStrategy::Prefixed)
            .err()
            .unwrap_or_else(|| panic!("more servers than the bound must be refused"));
        assert_eq!(
            error,
            CatalogError::TooManyServers {
                limit: MAX_MCP_SERVERS
            }
        );
        assert!(error.to_string().contains("at most"), "{error}");
    }

    #[test]
    fn definitions_are_ready_for_the_registry() {
        let schema = schema();
        let tools = names(&["search", "list_issues"]);
        let catalog = McpCatalog::build(
            &[configured("alpha")],
            &[offers("alpha", &tools, &schema)],
            NamingStrategy::Prefixed,
        )
        .unwrap_or_else(|error| panic!("{error}"));
        let definitions = catalog.definitions();
        assert_eq!(definitions.len(), 2);
        // Every one declares MCP as its source, which is what marks it third-party.
        assert!(
            definitions
                .iter()
                .all(|definition| definition.source() == jarvis_tools::ToolSource::Mcp)
        );
        // And they can actually be registered, which is the claim `definitions()` makes.
        let mut registry = jarvis_tools::ToolRegistry::new();
        assert_eq!(registry.define_all(definitions), Ok(()));
    }

    #[test]
    fn an_empty_configuration_produces_an_empty_catalog() {
        let catalog = McpCatalog::build(&[], &[], NamingStrategy::Prefixed)
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(catalog.is_empty());
        assert_eq!(catalog.len(), 0);
        assert!(!catalog.has_cross_server_collision());
    }
}
