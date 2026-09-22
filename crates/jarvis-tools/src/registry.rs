//! The capability registry: what may be called, and what a model is told about it.
//!
//! # Three questions the registry answers, and why they are separate
//!
//! 1. **What is registered?** A set of [`ToolDefinition`]s keyed by [`ToolId`], where a second
//!    registration under an existing key is a **refusal**. `docs/architecture/tools-and-connectors.md`
//!    requires that "tool names are namespaced and collisions are errors", and `P3-001`'s
//!    `ToolId` provides the comparable key. The registry is what turns a comparable key into an
//!    actual refusal.
//! 2. **What is callable right now?** Dynamic availability. A declared capability and a currently
//!    working capability are different facts, and `P3-002`'s task is explicitly "dynamic
//!    availability" — so a health check owns the second and the definition owns the first.
//! 3. **What does a model see?** A **bounded, compact** list. The registry holds everything; the
//!    list sent to a model is a selection, and a selection needs a bound or model input size becomes
//!    a function of how many connectors a user installed.
//!
//! # Why `inventory` and `discover` are different methods
//!
//! They serve different audiences and the difference is not cosmetic:
//!
//! - [`ToolRegistry::discover`] is **model-facing**. It lists only what can actually run, because
//!   offering a tool the model cannot call invites a call that will fail — and the model has no way
//!   to know why.
//! - [`ToolRegistry::inventory`] is **operator-facing**. It lists everything with its availability,
//!   because "why is this tool not being offered" is exactly the question an operator needs
//!   answered, and a tool that vanished from the list cannot be diagnosed.
//!
//! Collapsing them into one method with a flag would make it possible to hand an operator view to a
//! model, which leaks unavailable third-party tool names into model input for no reason.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::definition::ToolDefinition;
use crate::effect::ToolEffect;
use crate::identifier::ToolId;
use crate::policy::{Availability, ToolSource};
use crate::risk::Risk;

/// Maximum definitions one registry may hold.
///
/// Bounded because the registry is populated from connector manifests, which are external input, and
/// an unbounded registry is unbounded memory plus an unbounded discovery cost. Generous enough that
/// no hand-authored set reaches it, small enough that a runaway manifest loop is a refusal.
pub const MAX_REGISTERED_TOOLS: usize = 512;

/// Maximum tools in one discovery list.
///
/// Bounded because the list is assembled into a model request. This is the bound that makes model
/// input size independent of how many connectors a user installed, which is the property that
/// matters: a user who adds a hundred connectors must not silently reduce the context available for
/// their actual question.
pub const MAX_DISCOVERY_TOOLS: usize = 64;

/// Maximum description characters in one discovery entry.
///
/// Shorter than [`crate::MAX_TOOL_DESCRIPTION_CHARS`] because this text is repeated per tool: the
/// stored description is written once, and this is what is paid for every tool in every request. What
/// survives the cut is the part a model selects on.
pub const MAX_SUMMARY_DESCRIPTION_CHARS: usize = 160;

/// Explains why a registration was refused.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum RegistrationError {
    /// A tool with this identifier is already registered.
    ///
    /// Carries the identifier so the operator knows what collided, which is the whole point: a
    /// silent overwrite would leave one of the two tools unreachable with no record of which.
    #[error("the tool {id} is already registered")]
    Duplicate {
        /// The identifier that collided.
        id: String,
    },
    /// No tool with this identifier is registered.
    #[error("no tool named {id} is registered")]
    Unknown {
        /// The identifier that was not found.
        id: String,
    },
    /// A replacement carried the same version as the definition it would replace.
    ///
    /// `docs/architecture/tools-and-connectors.md` records `version` as "which behaviour a stored
    /// intent refers to". Changing behaviour without changing the version makes a stored intent
    /// silently mean something else, which is the one thing the field exists to prevent.
    #[error("the tool {id} is already at version {version}; a replacement must change it")]
    VersionUnchanged {
        /// The identifier being replaced.
        id: String,
        /// The version that would not have changed.
        version: String,
    },
    /// The registry is at [`MAX_REGISTERED_TOOLS`].
    #[error("the registry is full at {MAX_REGISTERED_TOOLS} tools")]
    Full,
}

/// Explains why a registry lookup failed.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum RegistryError {
    /// No tool with this identifier is registered.
    #[error("no tool named {id} is registered")]
    NotFound {
        /// The identifier that was not found.
        id: String,
    },
}

/// One registered capability: its declaration plus any runtime availability.
///
/// The two are held separately rather than merged so a health check can set and clear a runtime
/// state without editing the definition — and so clearing it restores the declared value rather than
/// having to remember it.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Entry {
    definition: ToolDefinition,
    runtime: Option<Availability>,
}

impl Entry {
    /// Returns the availability that applies now: the runtime state when one is set, else the
    /// declaration.
    ///
    /// Runtime **replaces** the declaration rather than intersecting with it. A declaration of
    /// `Unavailable` is a statement about the capability ("this build cannot do it"), and no health
    /// check may overturn it; a runtime state is a statement about now. Replacing would let a health
    /// check enable something the build declared impossible, so the declaration wins when it is the
    /// restrictive one.
    fn effective_availability(&self) -> Availability {
        if !self.definition.availability().is_available() {
            return self.definition.availability().clone();
        }
        self.runtime
            .clone()
            .unwrap_or_else(|| self.definition.availability().clone())
    }
}

/// The set of registered capabilities.
///
/// A `BTreeMap` keyed by [`ToolId`] rather than a map keyed by a string: `ToolId`'s ordering is
/// structural, so discovery order is stable without a sort step, and an identifier cannot be
/// registered under a spelling that differs from its own.
#[derive(Clone, Debug, Default)]
pub struct ToolRegistry {
    entries: BTreeMap<ToolId, Entry>,
}

impl ToolRegistry {
    /// Creates an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a definition, refusing a collision.
    ///
    /// # Errors
    ///
    /// Returns [`RegistrationError::Duplicate`] when the identifier is taken and
    /// [`RegistrationError::Full`] at [`MAX_REGISTERED_TOOLS`].
    pub fn define(&mut self, definition: ToolDefinition) -> Result<(), RegistrationError> {
        let id = definition.id().clone();
        if self.entries.contains_key(&id) {
            return Err(RegistrationError::Duplicate { id: id.to_string() });
        }
        if self.entries.len() >= MAX_REGISTERED_TOOLS {
            return Err(RegistrationError::Full);
        }
        self.entries.insert(
            id,
            Entry {
                definition,
                runtime: None,
            },
        );
        Ok(())
    }

    /// Replaces an existing definition, which must declare a **different** version.
    ///
    /// # Errors
    ///
    /// Returns [`RegistrationError::Unknown`] when the identifier is absent, or
    /// [`RegistrationError::VersionUnchanged`] when the version is the same. The second is the
    /// reason this is not just `define`: a behaviour change under an unchanged version makes every
    /// stored intent that named the version mean something it was not written against.
    pub fn replace(&mut self, definition: ToolDefinition) -> Result<(), RegistrationError> {
        let id = definition.id().clone();
        let Some(existing) = self.entries.get(&id) else {
            return Err(RegistrationError::Unknown { id: id.to_string() });
        };
        if existing.definition.version() == definition.version() {
            return Err(RegistrationError::VersionUnchanged {
                id: id.to_string(),
                version: definition.version().to_owned(),
            });
        }
        // A replacement keeps the runtime state: an updated definition is the same capability, so a
        // health verdict about the connector behind it is still about the same thing. Clearing it
        // here would make every upgrade briefly report a tool as available when it is not.
        let runtime = existing.runtime.clone();
        self.entries.insert(
            id,
            Entry {
                definition,
                runtime,
            },
        );
        Ok(())
    }

    /// Registers a whole set of definitions **atomically**: either all of them register or none do.
    ///
    /// # Errors
    ///
    /// Returns [`RegistrationError::Duplicate`] when any identifier collides with an existing tool
    /// **or with another member of the same batch**, and [`RegistrationError::Full`] when the batch
    /// would exceed [`MAX_REGISTERED_TOOLS`].
    ///
    /// Atomicity is the point, not a convenience. A connector manifest is a set of tools that share a
    /// provider account, a scope grant, and a health state, and registering nineteen of its twenty
    /// tools leaves a capability layer that is internally inconsistent with no record of the gap: the
    /// model sees a subset, the operator sees no error, and a later call fails against a tool nothing
    /// registered. Failing the load is recoverable; a partial load is a silent one.
    ///
    /// A collision **within** the batch is checked because [`BTreeMap::insert`] would otherwise make
    /// the last entry win, which is a collision resolved silently — the opposite of
    /// `docs/architecture/tools-and-connectors.md`'s "collisions are errors".
    pub fn define_all(
        &mut self,
        definitions: impl IntoIterator<Item = ToolDefinition>,
    ) -> Result<(), RegistrationError> {
        let incoming: Vec<ToolDefinition> = definitions.into_iter().collect();

        // Collected and checked before anything is inserted, so the checks cannot observe a state the
        // insertion is in the middle of producing.
        let mut batch: BTreeMap<ToolId, ToolDefinition> = BTreeMap::new();
        for definition in incoming {
            let id = definition.id().clone();
            if self.entries.contains_key(&id) {
                return Err(RegistrationError::Duplicate { id: id.to_string() });
            }
            if batch.insert(id.clone(), definition).is_some() {
                return Err(RegistrationError::Duplicate { id: id.to_string() });
            }
        }

        // The bound is checked against the projected total, not the batch size, so a batch that fits
        // on its own but not in the registry is refused rather than partially inserted.
        let projected = self.entries.len().saturating_add(batch.len());
        if projected > MAX_REGISTERED_TOOLS {
            return Err(RegistrationError::Full);
        }

        for (id, definition) in batch {
            self.entries.insert(
                id,
                Entry {
                    definition,
                    runtime: None,
                },
            );
        }
        Ok(())
    }

    /// Removes a definition, returning whether one was present.
    ///
    /// Returns a `bool` rather than an error because removing an absent tool is idempotent in the
    /// situation that matters — a connector being uninstalled twice, or a manifest being reloaded.
    pub fn remove(&mut self, id: &ToolId) -> bool {
        self.entries.remove(id).is_some()
    }

    /// Sets a runtime availability state for a registered tool.
    ///
    /// # Errors
    ///
    /// Returns [`RegistryError::NotFound`] when the identifier is absent. Refusing is deliberate: a
    /// health check that reports on a tool nobody registered is a bug in the check, and accepting it
    /// would silently drop the report.
    pub fn set_availability(
        &mut self,
        id: &ToolId,
        availability: Availability,
    ) -> Result<(), RegistryError> {
        let Some(entry) = self.entries.get_mut(id) else {
            return Err(RegistryError::NotFound { id: id.to_string() });
        };
        entry.runtime = Some(availability);
        Ok(())
    }

    /// Clears a runtime availability state, restoring the declared value.
    ///
    /// # Errors
    ///
    /// Returns [`RegistryError::NotFound`] when the identifier is absent.
    pub fn clear_availability(&mut self, id: &ToolId) -> Result<(), RegistryError> {
        let Some(entry) = self.entries.get_mut(id) else {
            return Err(RegistryError::NotFound { id: id.to_string() });
        };
        entry.runtime = None;
        Ok(())
    }

    /// Returns a definition.
    ///
    /// # Errors
    ///
    /// Returns [`RegistryError::NotFound`] when the identifier is absent.
    pub fn get(&self, id: &ToolId) -> Result<&ToolDefinition, RegistryError> {
        self.entries
            .get(id)
            .map(|entry| &entry.definition)
            .ok_or_else(|| RegistryError::NotFound { id: id.to_string() })
    }

    /// Returns the availability that applies now for a tool.
    ///
    /// # Errors
    ///
    /// Returns [`RegistryError::NotFound`] when the identifier is absent.
    pub fn availability(&self, id: &ToolId) -> Result<Availability, RegistryError> {
        self.entries
            .get(id)
            .map(Entry::effective_availability)
            .ok_or_else(|| RegistryError::NotFound { id: id.to_string() })
    }

    /// Returns whether a tool exists and can currently run.
    #[must_use]
    pub fn is_callable(&self, id: &ToolId) -> bool {
        self.entries
            .get(id)
            .is_some_and(|entry| entry.effective_availability().is_available())
    }

    /// Returns how many tools are registered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns whether no tool is registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns every registered identifier, in stable order.
    pub fn identifiers(&self) -> impl Iterator<Item = &ToolId> + '_ {
        self.entries.keys()
    }

    /// Returns the tools that can run now, bounded for a model.
    ///
    /// Unavailable tools are **absent** rather than flagged, for the reason given in the module
    /// comment: a tool a model cannot call is not a choice, and listing it invites a failing call.
    /// The omissions are reported by count so a caller can tell "there are no more tools" from
    /// "there were more and they did not fit".
    #[must_use]
    pub fn discover(&self) -> DiscoveryReport {
        let mut report = DiscoveryReport {
            tools: Vec::new(),
            omitted: 0,
            available_total: 0,
            registered_total: self.entries.len(),
        };
        for (id, entry) in &self.entries {
            if !entry.effective_availability().is_available() {
                continue;
            }
            report.available_total += 1;
            if report.tools.len() >= MAX_DISCOVERY_TOOLS {
                continue;
            }
            report
                .tools
                .push(ToolSummary::from_definition(id, &entry.definition));
        }
        report.omitted = report.available_total - report.tools.len();
        report
    }

    /// Returns every registered tool with its availability, bounded for an operator.
    ///
    /// Includes unavailable tools, because the reason a tool is not being offered is the question
    /// this view exists to answer.
    #[must_use]
    pub fn inventory(&self) -> Vec<ToolInventoryEntry> {
        self.entries
            .iter()
            .map(|(id, entry)| {
                let availability = entry.effective_availability();
                ToolInventoryEntry {
                    id: id.to_string(),
                    version: entry.definition.version().to_owned(),
                    source: entry.definition.source(),
                    risk: entry.definition.risk(),
                    effects: entry.definition.effects().to_vec(),
                    callable: availability.is_available(),
                    unavailable_reason: availability.reason().map(ToOwned::to_owned),
                }
            })
            .collect()
    }
}

/// One model-facing tool description.
///
/// Carries what a model selects on and nothing it must not act on. `risk`, `effects`, and `source`
/// are included because a model choosing between two tools benefits from knowing one sends mail and
/// another reads a file; `required_scopes`, `approval`, and the schemas are absent because
/// authorization is decided by policy from the registered definition, never from this summary, and
/// repeating them here would create a second place for them to be wrong.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolSummary {
    /// The canonical identifier, which is the name a model calls.
    pub id: String,
    /// The concise title.
    pub title: String,
    /// The description, truncated to [`MAX_SUMMARY_DESCRIPTION_CHARS`].
    pub description: String,
    /// Which class of source the tool came from.
    pub source: ToolSource,
    /// What could happen if it runs.
    pub effects: Vec<ToolEffect>,
    /// How much scrutiny it needs.
    pub risk: Risk,
}

impl ToolSummary {
    /// Builds a summary from a definition, truncating the description on a character boundary.
    fn from_definition(id: &ToolId, definition: &ToolDefinition) -> Self {
        let mut description: String = definition
            .description()
            .chars()
            .take(MAX_SUMMARY_DESCRIPTION_CHARS)
            .collect();
        if definition.description().chars().count() > MAX_SUMMARY_DESCRIPTION_CHARS {
            // A marker rather than a silent cut, so a reader can tell a description was shortened
            // and does not conclude the tool is under-described.
            description.push('…');
        }
        Self {
            id: id.to_string(),
            title: definition.title().to_owned(),
            description,
            source: definition.source(),
            effects: definition.effects().to_vec(),
            risk: definition.risk(),
        }
    }
}

/// The model-facing discovery result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DiscoveryReport {
    /// The tools offered, in stable identifier order.
    pub tools: Vec<ToolSummary>,
    /// How many callable tools were omitted because the list was full.
    pub omitted: usize,
    /// How many callable tools exist, whether or not they fit.
    pub available_total: usize,
    /// How many tools are registered at all, including unavailable ones.
    pub registered_total: usize,
}

impl DiscoveryReport {
    /// Returns whether the list was cut short.
    #[must_use]
    pub const fn is_truncated(&self) -> bool {
        self.omitted > 0
    }

    /// Returns whether there is nothing callable.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

/// One operator-facing inventory entry.
///
/// Carries the availability detail a summary omits, because an operator is asking a different
/// question: not "what may I call" but "what is registered and what is wrong with it".
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolInventoryEntry {
    /// The canonical identifier.
    pub id: String,
    /// The declared version.
    pub version: String,
    /// The source class.
    pub source: ToolSource,
    /// The declared risk.
    pub risk: Risk,
    /// The declared effects.
    pub effects: Vec<ToolEffect>,
    /// Whether the tool can currently run.
    pub callable: bool,
    /// Why it cannot, when it cannot.
    pub unavailable_reason: Option<String>,
}

impl fmt::Display for DiscoveryReport {
    /// Renders the offered identifiers, which is what a human scanning a list needs.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let identifiers = self
            .tools
            .iter()
            .map(|tool| tool.id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        write!(
            formatter,
            "{} of {} callable ({identifiers})",
            self.tools.len(),
            self.available_total
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::definition::ToolDefinitionParts;
    use crate::effect::EffectSet;
    use crate::policy::{ApprovalPolicy, Idempotency, RetryDeclaration, ToolSensitivity};
    use crate::schema::ToolSchema;
    use crate::scope::{Scope, ScopeSet};
    use jarvis_core::Sensitivity;

    const INPUT_SCHEMA: &str = r#"{
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "properties": { "query": { "type": "string" } },
        "required": ["query"],
        "additionalProperties": false
    }"#;

    fn schema() -> ToolSchema {
        ToolSchema::parse(INPUT_SCHEMA).unwrap_or_else(|error| panic!("fixture schema: {error}"))
    }

    fn scope(value: &str) -> Scope {
        Scope::new(value).unwrap_or_else(|error| panic!("{value}: {error}"))
    }

    /// Builds a consistent read-only definition for a given identifier and version.
    fn tool(id: &str, version: &str) -> ToolDefinition {
        let id = ToolId::new(id).unwrap_or_else(|error| panic!("{id}: {error}"));
        let source = id.source();
        ToolDefinition::new(ToolDefinitionParts {
            id,
            version: version.to_owned(),
            title: format!("Title for {version}"),
            description: "Reads one file from the workspace and returns its contents.".to_owned(),
            input_schema: schema(),
            output_schema: schema(),
            effects: EffectSet::single(ToolEffect::ReadOnly),
            risk: 0,
            required_scopes: ScopeSet::single(scope("files.read")),
            approval: ApprovalPolicy::Auto,
            timeout_seconds: 30,
            retry: RetryDeclaration::none(),
            idempotency: Idempotency::Required,
            source,
            availability: Availability::Available,
            sensitivity: ToolSensitivity::new(Sensitivity::Internal, Sensitivity::Internal),
        })
        .unwrap_or_else(|error| panic!("consistent definition: {error}"))
    }

    fn id(value: &str) -> ToolId {
        ToolId::new(value).unwrap_or_else(|error| panic!("{value}: {error}"))
    }

    fn registry_ids(registry: &ToolRegistry) -> Vec<String> {
        registry
            .identifiers()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
    }

    /// A definition registers and is retrievable.
    #[test]
    fn a_definition_registers_and_is_retrievable() {
        let mut registry = ToolRegistry::new();
        assert!(registry.is_empty());
        registry
            .define(tool("jarvis.files.read", "1.0.0"))
            .unwrap_or_else(|error| panic!("first registration: {error}"));
        assert_eq!(registry.len(), 1);

        let found = registry
            .get(&id("jarvis.files.read"))
            .unwrap_or_else(|error| panic!("lookup: {error}"));
        assert_eq!(found.version(), "1.0.0");
    }

    /// **The falsification test: a collision is refused, and the first tool survives.**
    ///
    /// `docs/architecture/tools-and-connectors.md`: "tool names are namespaced and collisions are
    /// errors". The second half of the assertion is the part that matters — a refusal that still
    /// overwrote would satisfy a test that only checked the error, while silently losing a tool.
    #[test]
    fn a_collision_is_refused_and_leaves_the_first_tool_intact() {
        let mut registry = ToolRegistry::new();
        registry
            .define(tool("jarvis.files.read", "1.0.0"))
            .unwrap_or_else(|error| panic!("{error}"));

        let collision = registry.define(tool("jarvis.files.read", "2.0.0"));
        assert_eq!(
            collision,
            Err(RegistrationError::Duplicate {
                id: "jarvis.files.read".to_owned()
            })
        );

        let surviving = registry
            .get(&id("jarvis.files.read"))
            .unwrap_or_else(|error| panic!("the first tool must survive: {error}"));
        assert_eq!(
            surviving.version(),
            "1.0.0",
            "the refused registration must not have replaced the stored definition"
        );
        assert_eq!(registry.len(), 1);
    }

    /// The **same operation name in two namespaces does not collide**, because the namespace is part
    /// of the key.
    ///
    /// The reason the namespace is structural rather than a naming convention: two connectors may
    /// each ship a `search`, and refusing the second would make the registry unusable for the case it
    /// exists to serve.
    #[test]
    fn the_same_name_in_two_namespaces_does_not_collide() {
        let mut registry = ToolRegistry::new();
        for source in ["gmail.search", "drive.search", "mcp.github.search"] {
            registry
                .define(tool(source, "1.0.0"))
                .unwrap_or_else(|error| panic!("{source}: {error}"));
        }
        assert_eq!(registry.len(), 3);
        assert_eq!(
            registry_ids(&registry),
            vec!["drive.search", "gmail.search", "mcp.github.search"],
            "identifiers must be listed in stable order"
        );
    }

    /// A replacement is accepted when the version changes, and refused when it does not.
    #[test]
    fn a_replacement_requires_a_version_change() {
        let mut registry = ToolRegistry::new();
        registry
            .define(tool("jarvis.files.read", "1.0.0"))
            .unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(
            registry.replace(tool("jarvis.files.read", "1.0.0")),
            Err(RegistrationError::VersionUnchanged {
                id: "jarvis.files.read".to_owned(),
                version: "1.0.0".to_owned()
            }),
            "an unchanged version must not accept new behaviour"
        );

        registry
            .replace(tool("jarvis.files.read", "1.1.0"))
            .unwrap_or_else(|error| panic!("a changed version: {error}"));
        assert_eq!(
            registry
                .get(&id("jarvis.files.read"))
                .unwrap_or_else(|error| panic!("{error}"))
                .version(),
            "1.1.0"
        );
        assert_eq!(registry.len(), 1, "a replacement is not a second tool");
    }

    /// **A batch load is atomic: one collision registers nothing at all.**
    ///
    /// The fail-open case a partial connector load would produce: nineteen of twenty tools available,
    /// no error reported, and a call that fails against a tool nothing registered.
    #[test]
    fn a_batch_load_is_atomic() {
        let mut registry = ToolRegistry::new();
        registry
            .define(tool("jarvis.files.read", "1.0.0"))
            .unwrap_or_else(|error| panic!("{error}"));

        // The third entry collides with the pre-existing tool.
        let batch = vec![
            tool("jarvis.files.write", "1.0.0"),
            tool("jarvis.files.list", "1.0.0"),
            tool("jarvis.files.read", "9.0.0"),
        ];
        assert_eq!(
            registry.define_all(batch),
            Err(RegistrationError::Duplicate {
                id: "jarvis.files.read".to_owned()
            })
        );
        assert_eq!(
            registry.len(),
            1,
            "a refused batch must register none of its members, not the prefix that succeeded"
        );
        assert!(
            registry.get(&id("jarvis.files.write")).is_err(),
            "the first member of a refused batch must not be present"
        );
    }

    /// **A collision inside the batch is an error, not last-writer-wins.**
    ///
    /// `BTreeMap::insert` resolves a duplicate by replacing, which is a collision resolved silently.
    #[test]
    fn a_collision_inside_a_batch_is_refused() {
        let mut registry = ToolRegistry::new();
        let batch = vec![
            tool("jarvis.files.read", "1.0.0"),
            tool("jarvis.files.read", "2.0.0"),
        ];
        assert_eq!(
            registry.define_all(batch),
            Err(RegistrationError::Duplicate {
                id: "jarvis.files.read".to_owned()
            })
        );
        assert!(registry.is_empty());
    }

    /// A batch that would overflow the registry is refused whole.
    #[test]
    fn a_batch_that_exceeds_the_bound_is_refused_whole() {
        let mut registry = ToolRegistry::new();
        for index in 0..(MAX_REGISTERED_TOOLS - 1) {
            registry
                .define(tool(&format!("jarvis.tools.op{index:04}"), "1.0.0"))
                .unwrap_or_else(|error| panic!("op{index}: {error}"));
        }
        // One more fits; a batch of two does not.
        let batch = vec![
            tool("jarvis.tools.fits", "1.0.0"),
            tool("jarvis.tools.overflow", "1.0.0"),
        ];
        assert_eq!(registry.define_all(batch), Err(RegistrationError::Full));
        assert!(
            registry.get(&id("jarvis.tools.fits")).is_err(),
            "the member that would have fitted must not be registered"
        );
        assert_eq!(registry.len(), MAX_REGISTERED_TOOLS - 1);
    }

    /// A batch that fits registers every member.
    #[test]
    fn a_fitting_batch_registers_every_member() {
        let mut registry = ToolRegistry::new();
        let batch = vec![
            tool("jarvis.files.read", "1.0.0"),
            tool("jarvis.files.write", "1.0.0"),
            tool("mcp.github.search", "1.0.0"),
        ];
        registry
            .define_all(batch)
            .unwrap_or_else(|error| panic!("a fitting batch: {error}"));
        assert_eq!(registry.len(), 3);
        assert!(registry.is_callable(&id("mcp.github.search")));
    }

    /// Replacing an unregistered tool is refused, so `replace` cannot silently behave as `define`.
    #[test]
    fn replacing_an_unregistered_tool_is_refused() {
        let mut registry = ToolRegistry::new();
        assert_eq!(
            registry.replace(tool("jarvis.files.read", "1.0.0")),
            Err(RegistrationError::Unknown {
                id: "jarvis.files.read".to_owned()
            })
        );
        assert!(
            registry.is_empty(),
            "a refused replace must not add anything"
        );
    }

    /// Removing is idempotent, so unloading a connector twice is not an error.
    #[test]
    fn removal_is_idempotent() {
        let mut registry = ToolRegistry::new();
        registry
            .define(tool("jarvis.files.read", "1.0.0"))
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(registry.remove(&id("jarvis.files.read")));
        assert!(!registry.remove(&id("jarvis.files.read")));
        assert!(registry.is_empty());
    }

    /// **Dynamic availability: a runtime state overrides the declaration, and clearing restores it.**
    #[test]
    fn a_runtime_state_overrides_the_declaration_and_can_be_cleared() {
        let mut registry = ToolRegistry::new();
        let tool_id = id("jarvis.files.read");
        registry
            .define(tool("jarvis.files.read", "1.0.0"))
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(registry.is_callable(&tool_id));

        registry
            .set_availability(
                &tool_id,
                Availability::unavailable("the connector account token expired")
                    .unwrap_or_else(|| panic!("a real reason")),
            )
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(!registry.is_callable(&tool_id));
        assert_eq!(
            registry
                .availability(&tool_id)
                .unwrap_or_else(|error| panic!("{error}"))
                .reason(),
            Some("the connector account token expired")
        );

        registry
            .clear_availability(&tool_id)
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(
            registry.is_callable(&tool_id),
            "clearing must restore the declared availability, not leave it unavailable"
        );
    }

    /// **A runtime state cannot enable what the declaration says the build cannot do.**
    ///
    /// The declaration is a statement about the capability and the runtime state is a statement about
    /// now; let a health check win and an unavailable-in-this-build tool becomes callable because a
    /// probe succeeded.
    #[test]
    fn a_runtime_state_cannot_overturn_a_declared_unavailability() {
        let mut registry = ToolRegistry::new();
        let tool_id = id("jarvis.files.read");
        registry
            .define(declared_unavailable_declaration(&tool_id))
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(
            !registry.is_callable(&tool_id),
            "a declared-unavailable tool must not be callable"
        );

        registry
            .set_availability(&tool_id, Availability::Available)
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(
            !registry.is_callable(&tool_id),
            "a health check must not overturn the build's own declaration"
        );
        assert_eq!(
            registry
                .availability(&tool_id)
                .unwrap_or_else(|error| panic!("{error}"))
                .reason(),
            Some("not built for this platform"),
            "the declaration's reason must be what is reported"
        );

        // Clearing the runtime state changes nothing, so the rule is about the declaration's
        // precedence and not about whether a runtime state is present.
        registry
            .clear_availability(&tool_id)
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(!registry.is_callable(&tool_id));
    }

    /// Builds a definition whose declaration is `Unavailable`.
    fn declared_unavailable_declaration(id: &ToolId) -> ToolDefinition {
        ToolDefinition::new(ToolDefinitionParts {
            id: id.clone(),
            version: "1.0.0".to_owned(),
            title: "Unavailable tool".to_owned(),
            description: "Declared unavailable in this build.".to_owned(),
            input_schema: schema(),
            output_schema: schema(),
            effects: EffectSet::single(ToolEffect::ReadOnly),
            risk: 0,
            required_scopes: ScopeSet::single(scope("files.read")),
            approval: ApprovalPolicy::Auto,
            timeout_seconds: 30,
            retry: RetryDeclaration::none(),
            idempotency: Idempotency::Required,
            source: id.source(),
            availability: Availability::unavailable("not built for this platform")
                .unwrap_or_else(|| panic!("a real reason")),
            sensitivity: ToolSensitivity::new(Sensitivity::Internal, Sensitivity::Internal),
        })
        .unwrap_or_else(|error| panic!("consistent definition: {error}"))
    }

    /// Reporting a runtime state for an unregistered tool is refused rather than dropped.
    #[test]
    fn a_runtime_state_for_an_unknown_tool_is_refused() {
        let mut registry = ToolRegistry::new();
        assert_eq!(
            registry.set_availability(&id("jarvis.files.read"), Availability::Available),
            Err(RegistryError::NotFound {
                id: "jarvis.files.read".to_owned()
            })
        );
        assert_eq!(
            registry.clear_availability(&id("jarvis.files.read")),
            Err(RegistryError::NotFound {
                id: "jarvis.files.read".to_owned()
            })
        );
    }

    /// Looking up an unregistered tool is an error naming the identifier.
    #[test]
    fn an_unknown_lookup_is_an_error() {
        let registry = ToolRegistry::new();
        assert_eq!(
            registry.get(&id("jarvis.files.read")).err(),
            Some(RegistryError::NotFound {
                id: "jarvis.files.read".to_owned()
            })
        );
        assert!(!registry.is_callable(&id("jarvis.files.read")));
    }

    /// **Discovery offers only what can run, and reports the omissions by count.**
    ///
    /// The bound assertion is the important half: a truncated list must be distinguishable from a
    /// complete one, or a caller cannot tell "there are no more tools" from "there were more".
    #[test]
    fn discovery_offers_only_callable_tools_and_reports_omissions() {
        let mut registry = ToolRegistry::new();
        let total = MAX_DISCOVERY_TOOLS + 5;
        for index in 0..total {
            registry
                .define(tool(&format!("jarvis.tools.op{index:03}"), "1.0.0"))
                .unwrap_or_else(|error| panic!("op{index}: {error}"));
        }

        let report = registry.discover();
        assert_eq!(report.available_total, total);
        assert_eq!(report.registered_total, total);
        assert_eq!(report.tools.len(), MAX_DISCOVERY_TOOLS);
        assert_eq!(report.omitted, 5);
        assert!(report.is_truncated());
        assert!(!report.is_empty());

        // Making one tool unavailable shrinks the callable set, and the list no longer truncates.
        registry
            .set_availability(
                &id("jarvis.tools.op000"),
                Availability::unavailable("a probe failed")
                    .unwrap_or_else(|| panic!("a real reason")),
            )
            .unwrap_or_else(|error| panic!("{error}"));
        let with_override = registry.discover();
        assert_eq!(with_override.available_total, total - 1);
        assert_eq!(
            with_override.registered_total, total,
            "an unavailable tool is still registered"
        );
        assert!(
            !with_override
                .tools
                .iter()
                .any(|tool| tool.id == "jarvis.tools.op000"),
            "the now-unavailable tool must not be offered"
        );
        assert_eq!(
            with_override.tools.len(),
            MAX_DISCOVERY_TOOLS,
            "one fewer callable but still more than fit, so the list is still full"
        );
    }

    /// An unavailable tool is absent from discovery and present in the inventory with its reason.
    ///
    /// The two views answering different questions is the design; this asserts the difference rather
    /// than one view's contents.
    #[test]
    fn unavailable_tools_are_absent_from_discovery_and_present_in_the_inventory() {
        let mut registry = ToolRegistry::new();
        let tool_id = id("jarvis.files.read");
        registry
            .define(tool("jarvis.files.read", "1.0.0"))
            .unwrap_or_else(|error| panic!("{error}"));
        registry
            .define(tool("jarvis.files.write", "1.0.0"))
            .unwrap_or_else(|error| panic!("{error}"));
        registry
            .set_availability(
                &tool_id,
                Availability::unavailable("the connector is not configured")
                    .unwrap_or_else(|| panic!("a real reason")),
            )
            .unwrap_or_else(|error| panic!("{error}"));

        let report = registry.discover();
        assert_eq!(report.tools.len(), 1);
        assert_eq!(report.available_total, 1);
        assert_eq!(
            report.registered_total, 2,
            "the unavailable tool is still registered"
        );
        assert!(
            !report
                .tools
                .iter()
                .any(|tool| tool.id == "jarvis.files.read"),
            "an unavailable tool must not be offered to a model"
        );

        let inventory = registry.inventory();
        assert_eq!(inventory.len(), 2);
        let entry = inventory
            .iter()
            .find(|entry| entry.id == "jarvis.files.read")
            .unwrap_or_else(|| panic!("the unavailable tool must be in the inventory"));
        assert!(!entry.callable);
        assert_eq!(
            entry.unavailable_reason.as_deref(),
            Some("the connector is not configured")
        );
    }

    /// A summary carries what a model selects on and omits the authorization fields.
    #[test]
    fn a_summary_carries_selection_fields_and_not_authorization_fields() {
        let mut registry = ToolRegistry::new();
        registry
            .define(tool("mcp.github.search", "1.0.0"))
            .unwrap_or_else(|error| panic!("{error}"));

        let report = registry.discover();
        let summary = report
            .tools
            .first()
            .unwrap_or_else(|| panic!("one tool must be offered"));
        assert_eq!(summary.id, "mcp.github.search");
        assert_eq!(summary.source, ToolSource::Mcp);
        assert_eq!(summary.risk, Risk::Minimal);
        assert_eq!(summary.effects, vec![ToolEffect::ReadOnly]);

        // The serialized summary has exactly the selection fields, so a scope, approval policy, or
        // schema cannot leak into model input through this struct.
        let encoded = serde_json::to_value(summary).unwrap_or_else(|error| panic!("{error}"));
        let mut keys: Vec<&String> = encoded
            .as_object()
            .unwrap_or_else(|| panic!("a summary is an object"))
            .keys()
            .collect();
        keys.sort();
        assert_eq!(
            keys,
            vec!["description", "effects", "id", "risk", "source", "title"]
        );
    }

    /// **A long description is truncated on a character boundary and marked.**
    ///
    /// A byte slice would split a multi-byte character and produce invalid UTF-8 — or, in Rust,
    /// panic. The marker is asserted so a reader can tell a shortened description from a short one.
    #[test]
    fn a_long_description_is_truncated_and_marked() {
        let mut parts = ToolDefinitionParts {
            id: id("jarvis.files.describe"),
            version: "1.0.0".to_owned(),
            title: "Describe".to_owned(),
            description: "é".repeat(MAX_SUMMARY_DESCRIPTION_CHARS + 50),
            input_schema: schema(),
            output_schema: schema(),
            effects: EffectSet::single(ToolEffect::ReadOnly),
            risk: 0,
            required_scopes: ScopeSet::single(scope("files.read")),
            approval: ApprovalPolicy::Auto,
            timeout_seconds: 30,
            retry: RetryDeclaration::none(),
            idempotency: Idempotency::Required,
            source: ToolSource::Native,
            availability: Availability::Available,
            sensitivity: ToolSensitivity::new(Sensitivity::Internal, Sensitivity::Internal),
        };
        parts.source = parts.id.source();
        let definition = ToolDefinition::new(parts).unwrap_or_else(|error| panic!("{error}"));

        let mut registry = ToolRegistry::new();
        registry
            .define(definition)
            .unwrap_or_else(|error| panic!("{error}"));
        let report = registry.discover();
        let summary = report
            .tools
            .first()
            .unwrap_or_else(|| panic!("one tool must be offered"));

        assert!(
            summary.description.ends_with('…'),
            "a truncated description must be marked: {:?}",
            summary.description
        );
        assert_eq!(
            summary.description.chars().count(),
            MAX_SUMMARY_DESCRIPTION_CHARS + 1,
            "the bound plus the marker"
        );
        assert!(
            summary.description.chars().filter(|c| *c == 'é').count()
                == MAX_SUMMARY_DESCRIPTION_CHARS,
            "no multi-byte character may be split or lost"
        );
    }

    /// A short description is not marked, so the marker means something.
    #[test]
    fn a_short_description_is_not_marked() {
        let mut registry = ToolRegistry::new();
        registry
            .define(tool("jarvis.files.read", "1.0.0"))
            .unwrap_or_else(|error| panic!("{error}"));
        let report = registry.discover();
        let summary = report
            .tools
            .first()
            .unwrap_or_else(|| panic!("one tool must be offered"));
        assert!(!summary.description.ends_with('…'));
    }

    /// Discovery order is the identifier order, so two runs agree.
    #[test]
    fn discovery_order_is_stable() {
        let mut registry = ToolRegistry::new();
        for source in ["jarvis.z.last", "jarvis.a.first", "mcp.github.search"] {
            registry
                .define(tool(source, "1.0.0"))
                .unwrap_or_else(|error| panic!("{source}: {error}"));
        }
        let report = registry.discover();
        let listed: Vec<&str> = report.tools.iter().map(|tool| tool.id.as_str()).collect();
        assert_eq!(
            listed,
            vec!["jarvis.a.first", "jarvis.z.last", "mcp.github.search"]
        );
    }

    /// An empty registry discovers nothing and says so without looking truncated.
    #[test]
    fn an_empty_registry_discovers_nothing() {
        let registry = ToolRegistry::new();
        let report = registry.discover();
        assert!(report.is_empty());
        assert!(!report.is_truncated());
        assert_eq!(report.registered_total, 0);
        assert!(registry.inventory().is_empty());
    }

    /// The registry refuses to exceed its bound.
    #[test]
    fn the_registry_is_bounded() {
        let mut registry = ToolRegistry::new();
        for index in 0..MAX_REGISTERED_TOOLS {
            registry
                .define(tool(&format!("jarvis.tools.op{index:04}"), "1.0.0"))
                .unwrap_or_else(|error| panic!("op{index}: {error}"));
        }
        assert_eq!(registry.len(), MAX_REGISTERED_TOOLS);
        assert_eq!(
            registry.define(tool("jarvis.tools.overflow", "1.0.0")),
            Err(RegistrationError::Full)
        );
        assert_eq!(registry.len(), MAX_REGISTERED_TOOLS);
    }

    /// An inventory entry carries the availability detail a summary omits.
    #[test]
    fn an_inventory_entry_carries_the_operator_detail() {
        let mut registry = ToolRegistry::new();
        registry
            .define(tool("mcp.github.search", "2.1.0"))
            .unwrap_or_else(|error| panic!("{error}"));
        let inventory = registry.inventory();
        let entry = inventory
            .first()
            .unwrap_or_else(|| panic!("one entry must be listed"));
        assert_eq!(entry.id, "mcp.github.search");
        assert_eq!(entry.version, "2.1.0");
        assert_eq!(entry.source, ToolSource::Mcp);
        assert!(entry.callable);
        assert_eq!(entry.unavailable_reason, None);
    }
}
