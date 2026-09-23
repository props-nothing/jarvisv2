//! What JARVIS is willing to serve to a remote MCP client, and why a transitive tool is not offered.
//!
//! # Why this is separate from [`crate::McpCatalog`]
//!
//! [`crate::McpCatalog`] answers "what can this client reach", and its rule is that **a server does not
//! name itself** — an operator chooses the local name and the server's own claim is evidence rather than
//! identity (ADR-0024). This module answers the **opposite direction's** question, "what may a stranger
//! reach", and the rules are different in three ways that make sharing a type wrong:
//!
//! 1. **The naming argument runs the other way.** Inbound, the operator is the *client's* operator, whom
//!    we do not control; the only name we can be sure they can use is the **canonical JARVIS identifier**,
//!    which is already namespace-qualified (`jarvis.files.read`, `mcp.github.search`). So exposure
//!    transmits the identifier verbatim rather than inventing a translation, and a tool whose identifier
//!    is not already safe to transmit is **excluded, not renamed** — silently renaming would give a remote
//!    caller a name an operator cannot find in their own configuration.
//! 2. **The posture argument does too.** Outbound, a posture is what an *operator* declared about a
//!    third party (ADR-0025). Inbound, that posture is irrelevant: what matters is whether the
//!    capability's source is code **this project wrote**. A tool reached from someone else's MCP server
//!    is a capability we do not control, and re-exposing it would let our own exposure policy, tool
//!    list, and audit trail describe a chain of trust we are not actually holding.
//! 3. **Availability is a different question.** Outbound, a server being unreachable excludes its tools.
//!    Inbound, the same tool may be *known* and *unavailable* — a connector whose account token expired —
//!    and advertising it would offer a remote caller a tool that fails on every call, which reads as a
//!    broken tool rather than an absent capability. This is the reasoning `compose_tool_pipeline` already
//!    applies when it returns no pipeline over zero roots.
//!
//! # Why transitive tools are refused rather than nested
//!
//! The refusal is the module's most consequential rule, so its reason is worth stating plainly. Our
//! exposure decisions — the origin allowlist, the loopback bind, the per-client allowlist `P3-009c` adds —
//! describe **this daemon**. If `mcp.github.search` is offered, a caller reaching it has driven a call
//! through our policy and then through a third party's tool that our policy never classified, in a
//! context we did not choose. The `ToolEffectPolicy` that governs it was written for *our* use of that
//! server, on *our* machine, under this user's grants. Re-transmitting it makes that policy a statement
//! about a caller it was never about, and the risk level it carries is the one our operator guessed about
//! someone else's code.
//!
//! Nested exposure is deliberately **not** built here. A federation feature is a design problem with its
//! own questions — whether the third party consented, whose credentials execute the call, which audit
//! record owns it — and building it as a side effect of "expose my tools" is how a trust boundary gets
//! widened without a decision.

use std::collections::BTreeSet;

use jarvis_tools::{ToolDefinition, ToolId, ToolSource};

/// The most tools one daemon will serve.
///
/// The exposed list reaches a remote client's context, so it is bounded for the same reason
/// [`crate::MAX_MCP_SERVERS`] bounds a configured server list: a machine with a large registry must not
/// silently enlarge what every remote caller is shown. Thirty-two is far above a useful
/// single-user-assistant surface and far below a registry that has grown by accident.
pub const MAX_EXPOSED_TOOLS: usize = 32;

/// Why one tool was not offered to a remote client.
///
/// Reported **per tool**, never as a failure of the whole list, because the specification requires that a
/// single unusable entry not remove the others — the rule `crate::check_tool_schema` already follows for
/// a malformed definition. Here the consequence is stronger than a missing convenience: a `Transitive`
/// refusal is a **trust decision**, so a caller must be able to see that a tool was withheld and why,
/// rather than inferring it from absence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExposureExclusion {
    /// The tool comes from code this project did not write.
    Transitive {
        /// The identifier, so the refusal names a specific capability rather than a count.
        tool: String,
        /// The source class, so the reason can distinguish a nested MCP server from a runtime.
        source: ToolSource,
    },
    /// The tool is known but not currently usable.
    Unavailable {
        /// The identifier.
        tool: String,
        /// The adapter's own bounded reason.
        reason: String,
    },
    /// The identifier is not safe to transmit as an MCP tool name.
    UnrepresentableName {
        /// The identifier that was refused.
        tool: String,
        /// Why it cannot be transmitted.
        reason: String,
    },
    /// The exposed list was full.
    ///
    /// Separate from the per-tool reasons because its remedy is different: an operator fixing one tool's
    /// availability would learn nothing from this, and this is about the **size** of the surface. The same
    /// distinction `McpCatalog` draws between an exclusion and a truncation.
    BeyondLimit {
        /// How many tools were dropped once the list was full.
        dropped: usize,
    },
}

impl ExposureExclusion {
    /// Returns the identifier this refusal is about, when it is about one.
    ///
    /// `None` for [`Self::BeyondLimit`], which is about the list rather than a tool. Returning `None`
    /// rather than an empty string keeps "no tool" distinguishable from "a tool whose name is empty".
    #[must_use]
    pub fn tool(&self) -> Option<&str> {
        match self {
            Self::Transitive { tool, .. }
            | Self::Unavailable { tool, .. }
            | Self::UnrepresentableName { tool, .. } => Some(tool),
            Self::BeyondLimit { .. } => None,
        }
    }

    /// Returns a bounded, operator-facing reason.
    #[must_use]
    pub fn reason(&self) -> String {
        match self {
            Self::Transitive { tool, source } => format!(
                "{tool} comes from {} code, so it is not re-exposed; a remote caller would reach a \
                 capability this daemon's policy never classified",
                source.as_str()
            ),
            Self::Unavailable { tool, reason } => {
                format!("{tool} is known but currently unusable: {reason}")
            }
            Self::UnrepresentableName { tool, reason } => {
                format!("{tool} cannot be transmitted as an MCP tool name: {reason}")
            }
            Self::BeyondLimit { dropped } => format!(
                "{dropped} tools were not offered because the exposed surface is limited to \
                 {MAX_EXPOSED_TOOLS}"
            ),
        }
    }
}

/// One tool JARVIS will serve, as the protocol's tool-list shape needs it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServedTool {
    name: String,
    title: String,
    description: String,
    input_schema: serde_json::Value,
}

impl ServedTool {
    /// Returns the name a remote client calls.
    ///
    /// The **canonical JARVIS identifier**, transmitted verbatim. Not a translation: a rename would give a
    /// remote caller a name the daemon's own operator cannot find in their configuration, and the
    /// identifier is already namespace-qualified so it needs no disambiguation.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the display title.
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Returns the model-facing description.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    /// Returns the input schema, as the tool declared it.
    #[must_use]
    pub const fn input_schema(&self) -> &serde_json::Value {
        &self.input_schema
    }
}

/// Builds the tool list JARVIS serves, and reports every tool it withheld.
///
/// # Errors
///
/// Returns [`ExposureError::NoTools`] when the result would be empty, because an MCP server that
/// advertises nothing is not a useful server and starting one would be a silent no-op rather than a
/// reported configuration fault. Callers that want to serve nothing should not bind at all.
pub fn served_tools<'a>(
    definitions: impl IntoIterator<Item = &'a ToolDefinition>,
) -> Result<(Vec<ServedTool>, Vec<ExposureExclusion>), ExposureError> {
    let mut served: Vec<ServedTool> = Vec::new();
    let mut exclusions: Vec<ExposureExclusion> = Vec::new();
    let mut names: BTreeSet<String> = BTreeSet::new();
    let mut dropped: usize = 0;

    // Sorted by identifier so two builds of the same registry produce the same list in the same order.
    // The specification requires the tool set not to vary per connection and says the order **SHOULD** be
    // deterministic; a `HashMap` iteration order would satisfy the first and not the second, and the
    // difference would show up as a client's tools appearing to change between calls.
    let mut ordered: Vec<&ToolDefinition> = definitions.into_iter().collect();
    ordered.sort_by_key(|definition| definition.id().to_string());

    for definition in ordered {
        let id = definition.id().to_string();

        // Checked in order of severity, so the reason a tool is absent is the most consequential one
        // that applies. A transitive tool that is also unavailable is reported as transitive, because
        // that is the reason no change to its availability would fix.
        if definition.source().is_third_party() {
            exclusions.push(ExposureExclusion::Transitive {
                tool: id,
                source: definition.source(),
            });
            continue;
        }

        if let Some(reason) = definition.availability().reason() {
            exclusions.push(ExposureExclusion::Unavailable {
                tool: id,
                reason: reason.to_owned(),
            });
            continue;
        }

        if let Err(reason) = transmittable_name(&id) {
            exclusions.push(ExposureExclusion::UnrepresentableName {
                tool: id,
                reason: reason.to_owned(),
            });
            continue;
        }

        // **The bound is checked only after every eligibility check has passed**, so what it truncates
        // is a genuinely servable tool rather than one that would have been excluded for a better
        // reason. That ordering is what makes `dropped` mean "withheld for size" rather than "withheld".
        if served.len() >= MAX_EXPOSED_TOOLS {
            dropped += 1;
            continue;
        }

        // A duplicate is not possible through `ToolRegistry`, which refuses a second registration under
        // one key. Checked anyway rather than assumed, because this function accepts any iterator and has
        // no way to know what produced it — and a duplicated name in a tool list is exactly the ambiguity
        // `jarvis-mcp` exists to prevent.
        if !names.insert(id.clone()) {
            exclusions.push(ExposureExclusion::UnrepresentableName {
                tool: id,
                reason: "the identifier appeared twice".to_owned(),
            });
            continue;
        }

        served.push(ServedTool {
            name: id,
            title: definition.title().to_owned(),
            description: definition.description().to_owned(),
            input_schema: definition.input_schema().document().clone(),
        });
    }

    // Counted across the single pass rather than recomputed afterwards, so the number cannot disagree
    // with the list it describes.
    if dropped > 0 {
        exclusions.push(ExposureExclusion::BeyondLimit { dropped });
    }

    if served.is_empty() {
        return Err(ExposureError::NoTools);
    }
    Ok((served, exclusions))
}

/// Returns whether an identifier is safe to send as an MCP tool name.
///
/// The protocol **SHOULD**-constrains a tool name to 1–128 characters of `A-Za-z0-9_.-`, and it is
/// **case-sensitive**. The check here is deliberately **stricter** than that alphabet: it admits exactly
/// what the canonical-identifier rules produce — lowercase, digits, `_`, `.`, and `-` — so it refuses
/// uppercase as well. A name this project cannot produce has no business being transmitted, and the
/// stricter form means a future identifier vocabulary cannot become transmittable by accident.
///
/// The bound is checked rather than assumed, because a name over 128 characters would be silently
/// truncated by some client, and a truncation is how two distinct tools become one name at the far end.
///
/// **The `:` case is the one that matters.** `ToolId`'s name segment legally contains `:`, and the
/// protocol's tool-name alphabet does not — so a canonical identifier like `jarvis.files:read` is a real,
/// constructible tool that **cannot be transmitted**. That is exactly why this returns an error rather
/// than a rewritten name: the tool is excluded and reported, not silently renamed.
fn transmittable_name(id: &str) -> Result<(), &'static str> {
    if id.is_empty() {
        return Err("it is empty");
    }
    // 128 is the protocol's cap; the byte length is what a header or a body would carry.
    if id.len() > 128 {
        return Err("it exceeds the protocol's 128-character tool-name limit");
    }
    if !id.chars().all(|character| {
        character.is_ascii_lowercase()
            || character.is_ascii_digit()
            || matches!(character, '_' | '.' | '-')
    }) {
        return Err(
            "it contains a character outside the identifier alphabet, which the protocol's tool-name \
             alphabet may not carry either",
        );
    }
    // A leading or trailing separator is not a name this project produces, and it would let a client's
    // display logic render an odd name that re-parses differently.
    if id.starts_with(['.', '_', '-']) || id.ends_with(['.', '_', '-']) {
        return Err("it begins or ends with a separator");
    }
    Ok(())
}

/// Explains why the served surface could not be built.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExposureError {
    /// Every tool was excluded, so there is nothing to serve.
    NoTools,
}

impl std::fmt::Display for ExposureError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoTools => write!(
                formatter,
                "no tool can be served: every registered tool was excluded, so binding an MCP endpoint \
                 would advertise nothing"
            ),
        }
    }
}

impl std::error::Error for ExposureError {}

/// A JARVIS tool identifier parsed for transmission.
///
/// Exists so a caller can check one identifier without building a whole list. The list builder uses the
/// same function, so a tool offered by [`served_tools`] and a tool approved by this constructor can never
/// disagree about what is transmittable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServableName(String);

impl ServableName {
    /// Validates one identifier for transmission.
    ///
    /// # Errors
    ///
    /// Returns the reason it cannot be transmitted.
    pub fn new(id: &ToolId) -> Result<Self, &'static str> {
        let value = id.to_string();
        transmittable_name(&value)?;
        Ok(Self(value))
    }

    /// Returns the name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use jarvis_core::Sensitivity;
    use jarvis_tools::{
        ApprovalPolicy, Availability, EffectSet, Idempotency, RetryDeclaration, ScopeSet,
        ToolDefinitionParts, ToolEffect, ToolSchema, ToolSensitivity,
    };

    /// The schema every fixture declares, so the served schema can be compared to a known value.
    const INPUT_SCHEMA: &str = r#"{
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "properties": { "path": { "type": "string" } },
        "required": ["path"],
        "additionalProperties": false
    }"#;

    /// The output schema every fixture declares. Present so the fixture is a complete definition rather
    /// than one relying on a constructor default.
    const OUTPUT_SCHEMA: &str = r#"{
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "properties": { "contents": { "type": "string" } },
        "required": ["contents"],
        "additionalProperties": false
    }"#;

    fn schema(document: &str) -> ToolSchema {
        ToolSchema::parse(document).unwrap_or_else(|error| panic!("{document}: {error}"))
    }

    /// Builds a definition with only the fields a test varies.
    ///
    /// The source is **derived from the identifier** by `ToolDefinition::new`, so a test cannot declare a
    /// mismatch — which is the point of that constructor, and means `source` here is chosen only by
    /// picking an identifier namespace.
    fn definition(id: &str, availability: Availability) -> ToolDefinition {
        let effects = EffectSet::single(ToolEffect::ReadOnly);
        ToolDefinition::new(ToolDefinitionParts {
            id: ToolId::new(id).unwrap_or_else(|error| panic!("{id}: {error}")),
            version: "1.0.0".to_owned(),
            title: format!("title for {id}"),
            description: format!("description for {id}"),
            input_schema: schema(INPUT_SCHEMA),
            output_schema: schema(OUTPUT_SCHEMA),
            effects,
            risk: 0,
            required_scopes: ScopeSet::none(),
            approval: ApprovalPolicy::Auto,
            timeout_seconds: 10,
            retry: RetryDeclaration::none(),
            idempotency: Idempotency::Unsupported,
            source: jarvis_tools::ToolSource::from_namespace(
                id.rsplit_once('.').map_or(id, |(namespace, _)| namespace),
            ),
            availability,
            sensitivity: ToolSensitivity::new(Sensitivity::Internal, Sensitivity::Internal),
        })
        .unwrap_or_else(|error| panic!("{id}: {error}"))
    }

    fn native(id: &str) -> ToolDefinition {
        definition(id, Availability::Available)
    }

    /// **The module's central rule.** A tool reached from a third party is never re-exposed.
    ///
    /// Falsified by removing the `is_third_party` check: `mcp.github.search` then appears in the served
    /// list, and a remote caller would reach a capability whose risk level our operator guessed about code
    /// this project did not write — under an origin policy and audit trail that describe our daemon rather
    /// than that chain.
    #[test]
    fn a_third_party_tool_is_excluded_and_named() {
        let (served, exclusions) = served_tools([
            &native("jarvis.files.read"),
            &native("mcp.github.search"),
            &native("runtime.claude.run"),
            &native("extension.notes.append"),
        ])
        .unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(served.len(), 1);
        assert_eq!(served[0].name(), "jarvis.files.read");

        // Named, individually, with the source class — so a caller can see *that* a tool was withheld and
        // what kind of thing it was.
        let refused: Vec<&str> = exclusions
            .iter()
            .filter_map(ExposureExclusion::tool)
            .collect();
        assert_eq!(
            refused,
            vec![
                "extension.notes.append",
                "mcp.github.search",
                "runtime.claude.run"
            ],
            "every third-party tool must be named, in identifier order"
        );
        for exclusion in &exclusions {
            if let ExposureExclusion::Transitive { source, .. } = exclusion {
                assert!(source.is_third_party());
            }
        }
    }

    /// A connector tool is **native-adjacent** rather than third-party: JARVIS wrote the adapter, even
    /// though it calls a provider API. So it is servable.
    ///
    /// This is the boundary's subtle case, and it is the reason the rule is `is_third_party()` rather than
    /// "not `Native`". A connector's risk was declared by our own operator against an adapter this project
    /// maintains, which is the same standing a `jarvis.*` tool has.
    #[test]
    fn a_connector_tool_is_servable_because_this_project_wrote_the_adapter() {
        let (served, exclusions) = served_tools([&native("gmail.messages.send")])
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(served.len(), 1);
        assert_eq!(served[0].name(), "gmail.messages.send");
        assert!(exclusions.is_empty(), "{exclusions:?}");
    }

    /// A known-but-unavailable tool is not advertised, because advertising it would offer a remote caller a
    /// tool that fails on every call.
    ///
    /// Falsified by dropping the availability check: the tool appears, and the absence a caller sees is
    /// "the call failed" rather than "the capability is not offered".
    #[test]
    fn an_unavailable_tool_is_excluded_with_its_reason() {
        let unavailable = definition(
            "gmail.messages.send",
            Availability::unavailable("the account is not connected")
                .unwrap_or_else(|| panic!("a bounded reason is accepted")),
        );
        let (served, exclusions) = served_tools([&native("jarvis.files.read"), &unavailable])
            .unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(served.len(), 1);
        assert_eq!(exclusions.len(), 1);
        match &exclusions[0] {
            ExposureExclusion::Unavailable { tool, reason } => {
                assert_eq!(tool, "gmail.messages.send");
                assert_eq!(reason, "the account is not connected");
            }
            other => panic!("expected an availability exclusion, got {other:?}"),
        }
    }

    /// The most consequential reason wins, so the report names the thing no availability change would fix.
    ///
    /// A transitive tool that is also unavailable is reported as **transitive**. Falsified by checking
    /// availability first: the reason becomes "the account is not connected", and an operator would go and
    /// reconnect an account — fixing nothing, because the tool would still not be exposed.
    #[test]
    fn a_transitive_tool_that_is_also_unavailable_is_reported_as_transitive() {
        let both = definition(
            "mcp.github.search",
            Availability::unavailable("the token expired")
                .unwrap_or_else(|| panic!("a bounded reason is accepted")),
        );
        let (_, exclusions) = served_tools([&native("jarvis.files.read"), &both])
            .unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(exclusions.len(), 1);
        assert!(
            matches!(exclusions[0], ExposureExclusion::Transitive { .. }),
            "the reason must be the one that cannot be fixed by availability, got {:?}",
            exclusions[0]
        );
    }

    /// The list is ordered by identifier, so a client's tool set does not appear to move between calls.
    ///
    /// Falsified by returning the iteration order: with an unordered input the output order varies, and the
    /// specification's requirement that the set not vary per connection becomes satisfied only by accident.
    #[test]
    fn the_served_list_is_in_identifier_order_whatever_the_input_order() {
        let (served, _) = served_tools([
            &native("jarvis.z.last"),
            &native("jarvis.a.first"),
            &native("jarvis.m.middle"),
        ])
        .unwrap_or_else(|error| panic!("{error}"));

        let names: Vec<&str> = served.iter().map(ServedTool::name).collect();
        assert_eq!(
            names,
            vec!["jarvis.a.first", "jarvis.m.middle", "jarvis.z.last"]
        );
    }

    /// An empty surface is an error rather than an empty list, so a caller cannot bind an endpoint that
    /// advertises nothing and believe it is serving.
    #[test]
    fn a_surface_with_no_servable_tool_is_an_error() {
        assert_eq!(
            served_tools([&native("mcp.github.search")]).err(),
            Some(ExposureError::NoTools)
        );
    }

    /// The bound is enforced and reported as a **list** problem rather than blamed on a tool.
    ///
    /// Falsified by removing the bound: every tool is served, and the exposed surface grows with a
    /// registry nobody meant to publish.
    #[test]
    fn tools_beyond_the_limit_are_withheld_and_counted_once() {
        let owned: Vec<ToolDefinition> = (0..MAX_EXPOSED_TOOLS + 3)
            .map(|index| native(&format!("jarvis.tool{index:03}")))
            .collect();
        let references: Vec<&ToolDefinition> = owned.iter().collect();
        let (served, exclusions) =
            served_tools(references).unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(served.len(), MAX_EXPOSED_TOOLS);
        let truncations: Vec<&ExposureExclusion> = exclusions
            .iter()
            .filter(|exclusion| matches!(exclusion, ExposureExclusion::BeyondLimit { .. }))
            .collect();
        assert_eq!(truncations.len(), 1, "the truncation is reported once");
        match truncations[0] {
            ExposureExclusion::BeyondLimit { dropped } => assert_eq!(*dropped, 3),
            other => panic!("expected a truncation, got {other:?}"),
        }
        // And a truncation names no tool, because it is about the list.
        assert_eq!(truncations[0].tool(), None);
    }

    /// An identifier that is not transmittable is excluded rather than renamed.
    ///
    /// Renaming would give a remote caller a name the daemon's own operator cannot find in their
    /// configuration, which is the failure a translation is supposed to avoid.
    #[test]
    fn an_untransmittable_identifier_is_excluded_rather_than_renamed() {
        assert!(transmittable_name("jarvis.files.read").is_ok());
        assert!(transmittable_name(&"a".repeat(128)).is_ok());

        // Uppercase is legal in the protocol's alphabet and outside this project's, so it is refused:
        // the check enforces the canonical vocabulary rather than merely the wire's.
        assert!(transmittable_name("Jarvis.Files").is_err());
        assert!(transmittable_name("jarvis.files read").is_err());
        assert!(transmittable_name(".leading").is_err());
        assert!(transmittable_name("trailing.").is_err());
        assert!(transmittable_name("").is_err());
        assert!(transmittable_name(&"a".repeat(129)).is_err());
    }

    /// **A constructible tool that cannot be transmitted is excluded, not renamed** — the case the
    /// colon makes real.
    ///
    /// `ToolId`'s name segment legally contains `:`, so `jarvis.files:read` is a tool this project can
    /// register and the protocol's tool-name alphabet cannot carry. Falsified by rewriting the name
    /// instead of refusing: the tool appears under a name an operator cannot find in their own
    /// configuration, and a remote caller would call something the daemon never declared.
    #[test]
    fn a_tool_whose_name_the_protocol_cannot_carry_is_excluded_and_named() {
        let colon = native("jarvis.files:read");
        let (served, exclusions) = served_tools([&native("jarvis.files.read"), &colon])
            .unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(served.len(), 1, "only the transmittable tool is served");
        assert_eq!(served[0].name(), "jarvis.files.read");
        match exclusions.as_slice() {
            [ExposureExclusion::UnrepresentableName { tool, reason }] => {
                assert_eq!(tool, "jarvis.files:read");
                assert!(!reason.is_empty(), "the refusal must state a reason");
            }
            other => panic!("expected one unrepresentable-name exclusion, got {other:?}"),
        }
    }

    /// The schema travels as the tool declared it, so a remote caller sees the contract the daemon
    /// enforces rather than a summary of it.
    #[test]
    fn the_input_schema_is_transmitted_unchanged() {
        let definition = native("jarvis.files.read");
        let (served, _) = served_tools([&definition]).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            served[0].input_schema(),
            definition.input_schema().document(),
            "the served schema must be the declared one"
        );
    }

    /// A single refusal never removes the others, which is the specification's explicit requirement.
    #[test]
    fn one_refusal_leaves_every_other_tool_servable() {
        let unavailable = definition(
            "gmail.messages.send",
            Availability::unavailable("the account is not connected")
                .unwrap_or_else(|| panic!("a bounded reason is accepted")),
        );
        let (served, exclusions) = served_tools([
            &native("jarvis.a"),
            &unavailable,
            &native("mcp.github.search"),
            &native("jarvis.b"),
        ])
        .unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(served.len(), 2);
        assert_eq!(exclusions.len(), 2);
    }
}
