//! Which adapter runs which tool: the dispatch table the pipeline resolves against.
//!
//! # Why this is a separate type rather than a field of the pipeline
//!
//! The pipeline's job is to *sequence* the gates — validate, decide, admit, execute, record — and it
//! deliberately decides nothing (`P3-006d`). Which adapter a canonical identifier belongs to is a fact
//! about registration rather than about the sequence, and stating it as its own value makes two properties
//! checkable in one place instead of implied by a field:
//!
//! 1. **Every registered tool has an adapter.** A tool the registry offers that no adapter can run is a
//!    configuration fault, and the honest place to discover it is at startup — not as a call that fails
//!    after policy has already authorized it.
//! 2. **No tool is claimed by two adapters.** Two adapters able to run one identifier means which ran
//!    would depend on the order they were added, which is the order-dependence `McpCatalog` refuses one
//!    level down and would be worse here: the same call would reach different providers on different
//!    builds.
//!
//! # Why it is keyed by identifier rather than by adapter
//!
//! A caller looks up by the tool it is about to run, which is exactly the key the receipt binds. Keying by
//! adapter would mean scanning, and the scan's result would be the first match — an implicit ordering
//! decision, which is the thing this type exists to make impossible.

use std::collections::BTreeMap;
use std::sync::Arc;

use jarvis_tools::{ToolDefinition, ToolExecutor, ToolId};

/// Explains why an adapter set could not be assembled.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DispatchError {
    /// Two adapters claim the same canonical identifier.
    ///
    /// Refused rather than resolved, because a winner chosen by registration order would make the same
    /// call reach different providers on different builds — an ordering nobody declared meaningful.
    Duplicate {
        /// The identifier both adapters claimed.
        tool: String,
    },
    /// A tool the registry offers that no adapter can run.
    ///
    /// A configuration fault: policy would authorize a call nothing can execute, so the failure would
    /// surface after the durable admission rather than at startup.
    Uncovered {
        /// The identifier with no adapter.
        tool: String,
    },
}

impl std::fmt::Display for DispatchError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Duplicate { tool } => write!(
                formatter,
                "two adapters claim the tool {tool}; which ran would depend on registration order"
            ),
            Self::Uncovered { tool } => write!(
                formatter,
                "the tool {tool} is registered but no adapter can run it, so every call to it would be \
                 authorized and then fail"
            ),
        }
    }
}

impl std::error::Error for DispatchError {}

/// The adapters available to a pipeline, keyed by the tool each can run.
///
/// Built from the **definitions** an adapter declares alongside the definitions the registry holds, so the
/// coverage check compares the registry's contract with the adapter's own statement rather than with a
/// second copy of the tool list.
#[derive(Clone)]
pub struct Dispatch {
    adapters: BTreeMap<String, Arc<dyn ToolExecutor>>,
}

impl std::fmt::Debug for Dispatch {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Names the adapters without printing them: an adapter may hold a provider client or a
        // credential-bearing request type, neither of which belongs in a formatted value.
        let ids: Vec<&str> = self
            .adapters
            .values()
            .map(|adapter| adapter.adapter_id())
            .collect();
        formatter
            .debug_struct("Dispatch")
            .field("tools", &self.adapters.len())
            .field("adapters", &ids)
            .finish_non_exhaustive()
    }
}

impl Dispatch {
    /// Builds the table from `(definitions, adapter)` pairs.
    ///
    /// Each pair states the definitions its adapter can run, so the table is derived from what each
    /// adapter claims rather than from a caller's list of identifiers — a caller-supplied list would be a
    /// second statement that can disagree with the adapter.
    ///
    /// # Errors
    ///
    /// Returns [`DispatchError::Duplicate`] when two adapters claim one identifier. Coverage against the
    /// registry is checked by [`Self::verify_covers`], because it needs the registry's own tool list and
    /// this constructor deliberately does not hold one.
    pub fn new(
        sources: impl IntoIterator<Item = (Vec<ToolDefinition>, Arc<dyn ToolExecutor>)>,
    ) -> Result<Self, DispatchError> {
        let mut adapters: BTreeMap<String, Arc<dyn ToolExecutor>> = BTreeMap::new();
        for (definitions, adapter) in sources {
            for definition in definitions {
                let id = definition.id().to_string();
                // Overwrite detection rather than `insert`'s silent last-writer-wins: the second claim is
                // exactly the condition that must be refused.
                if adapters.insert(id.clone(), Arc::clone(&adapter)).is_some() {
                    return Err(DispatchError::Duplicate { tool: id });
                }
            }
        }
        Ok(Self { adapters })
    }

    /// Refuses a table that leaves a registered tool without an adapter.
    ///
    /// Separate from [`Self::new`] because it needs the registry, and the registry is the pipeline's. This
    /// is the check that turns "a call reached the adapter stage and nothing could run it" into a startup
    /// failure — the same reasoning that resolves the executor model and the workspace roots at start.
    ///
    /// # Errors
    ///
    /// Returns [`DispatchError::Uncovered`] for the first registered tool with no adapter, in identifier
    /// order so the report is reproducible.
    pub fn verify_covers(&self, registry_tools: &[ToolId]) -> Result<(), DispatchError> {
        let mut missing: Vec<String> = registry_tools
            .iter()
            .map(ToolId::to_string)
            .filter(|id| !self.adapters.contains_key(id))
            .collect();
        missing.sort();
        match missing.first() {
            Some(tool) => Err(DispatchError::Uncovered { tool: tool.clone() }),
            None => Ok(()),
        }
    }

    /// Returns the adapter that runs a tool.
    ///
    /// `None` for a tool no adapter claims, so a lookup cannot default to another adapter — the same
    /// reasoning `McpCatalog::route` applies to a server.
    #[must_use]
    pub fn adapter_for(&self, id: &ToolId) -> Option<&Arc<dyn ToolExecutor>> {
        self.adapters.get(&id.to_string())
    }

    /// Returns how many tools the table can run.
    ///
    /// `#[cfg(test)]` because nothing in the product asks: the pipeline resolves by identifier and the
    /// coverage check reports what is missing. A count is what a test asserts a table *size* with, and adding
    /// a public accessor nothing calls is how a surface grows a method with no consumer.
    #[cfg(test)]
    #[must_use]
    pub fn len(&self) -> usize {
        self.adapters.len()
    }

    /// Returns whether the table is empty.
    ///
    /// `#[cfg(test)]` for the same reason as [`Self::len`].
    #[cfg(test)]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.adapters.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use async_trait::async_trait;

    use jarvis_tools::{
        AdapterError, ApprovalPolicy, Availability, EffectSet, Idempotency, RetryDeclaration,
        ScopeSet, TOOL_SCHEMA_DIALECT, ToolCallResult, ToolDefinitionParts, ToolEffect,
        ToolSensitivity, ToolSource,
    };
    use serde_json::json;

    /// A named no-op adapter, so two adapters can be distinguished in a test.
    struct Named(&'static str);

    #[async_trait]
    impl ToolExecutor for Named {
        fn adapter_id(&self) -> &'static str {
            self.0
        }

        async fn execute(
            &self,
            _request: &jarvis_tools::ToolExecutionRequest,
        ) -> Result<ToolCallResult, AdapterError> {
            Err(AdapterError::NotImplemented {
                tool: "fixture".to_owned(),
            })
        }
    }

    /// A definition for one identifier, with a locally-scoped namespace so it needs no grants.
    ///
    /// The namespace is `jarvis.` so the declared source is `Native` and agrees with the identifier — a
    /// definition whose declared source disagreed with its namespace is refused by `ToolDefinition::new`, so
    /// the fixture has to be internally consistent rather than merely typed correctly.
    fn definition(id: &str) -> ToolDefinition {
        let tool_id = ToolId::new(id).unwrap_or_else(|error| panic!("{error}"));
        ToolDefinition::new(ToolDefinitionParts {
            id: tool_id,
            version: "1.0.0".to_owned(),
            title: "fixture".to_owned(),
            description: "a fixture tool".to_owned(),
            input_schema: jarvis_tools::ToolSchema::from_value(
                json!({ "$schema": TOOL_SCHEMA_DIALECT, "type": "object" }),
            )
            .unwrap_or_else(|error| panic!("{error}")),
            output_schema: jarvis_tools::ToolSchema::from_value(
                json!({ "$schema": TOOL_SCHEMA_DIALECT }),
            )
            .unwrap_or_else(|error| panic!("{error}")),
            effects: EffectSet::single(ToolEffect::ReadOnly),
            risk: 0,
            required_scopes: ScopeSet::none(),
            approval: ApprovalPolicy::Auto,
            timeout_seconds: 60,
            retry: RetryDeclaration::none(),
            idempotency: Idempotency::Unsupported,
            // Agrees with the `jarvis.` namespace the fixture uses.
            source: ToolSource::Native,
            availability: Availability::Available,
            sensitivity: ToolSensitivity::new(
                jarvis_core::Sensitivity::Internal,
                jarvis_core::Sensitivity::Internal,
            ),
        })
        .unwrap_or_else(|error| panic!("{error}"))
    }

    fn id(text: &str) -> ToolId {
        ToolId::new(text).unwrap_or_else(|error| panic!("{error}"))
    }

    #[test]
    fn a_definition_routes_to_the_adapter_that_declared_it() {
        let dispatch = Dispatch::new([
            (
                vec![definition("jarvis.alpha.one")],
                Arc::new(Named("alpha")) as Arc<dyn ToolExecutor>,
            ),
            (
                vec![definition("jarvis.beta.two")],
                Arc::new(Named("beta")) as Arc<dyn ToolExecutor>,
            ),
        ])
        .unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(dispatch.len(), 2);
        assert_eq!(
            dispatch
                .adapter_for(&id("jarvis.alpha.one"))
                .unwrap_or_else(|| panic!("alpha's tool must route"))
                .adapter_id(),
            "alpha"
        );
        assert_eq!(
            dispatch
                .adapter_for(&id("jarvis.beta.two"))
                .unwrap_or_else(|| panic!("beta's tool must route"))
                .adapter_id(),
            "beta"
        );
        // An unknown identifier routes nowhere, so a lookup cannot default to another adapter.
        assert!(dispatch.adapter_for(&id("jarvis.gamma.three")).is_none());
    }

    /// **Two adapters claiming one identifier is refused.** A winner chosen by registration order would make
    /// the same call reach different providers on different builds, and the order is the argument order of a
    /// constructor — which is not something anyone declared meaningful.
    #[test]
    fn two_adapters_claiming_one_tool_are_refused() {
        let error = Dispatch::new([
            (
                vec![definition("jarvis.alpha.one")],
                Arc::new(Named("alpha")) as Arc<dyn ToolExecutor>,
            ),
            (
                vec![definition("jarvis.alpha.one")],
                Arc::new(Named("beta")) as Arc<dyn ToolExecutor>,
            ),
        ])
        .err()
        .unwrap_or_else(|| panic!("a duplicate claim must be refused"));
        assert_eq!(
            error,
            DispatchError::Duplicate {
                tool: "jarvis.alpha.one".to_owned()
            }
        );
        // The refusal names the tool, because an operator acts on a registration.
        assert!(error.to_string().contains("jarvis.alpha.one"), "{error}");
    }

    /// **A registered tool with no adapter is refused at verification**, which is the check that turns a
    /// guaranteed-runtime failure into a startup failure. Without it the call would be admitted durably and
    /// then fail, leaving a record of an action nothing could perform.
    #[test]
    fn a_registered_tool_with_no_adapter_is_refused() {
        let dispatch = Dispatch::new([(
            vec![definition("jarvis.alpha.one")],
            Arc::new(Named("alpha")) as Arc<dyn ToolExecutor>,
        )])
        .unwrap_or_else(|error| panic!("{error}"));

        // Covered: the registry lists exactly what the adapter declared.
        assert!(dispatch.verify_covers(&[id("jarvis.alpha.one")]).is_ok());

        // Uncovered: the registry holds one more tool than any adapter declared.
        let error = dispatch
            .verify_covers(&[id("jarvis.alpha.one"), id("jarvis.beta.two")])
            .err()
            .unwrap_or_else(|| panic!("an uncovered tool must be refused"));
        assert_eq!(
            error,
            DispatchError::Uncovered {
                tool: "jarvis.beta.two".to_owned()
            }
        );
        assert!(
            error.to_string().contains("no adapter can run it"),
            "{error}"
        );
    }

    /// The coverage check reports the **first** uncovered tool in identifier order, so a configuration with
    /// two gaps reports the same one on every run rather than whichever a hash map happened to yield.
    #[test]
    fn coverage_reports_a_reproducible_first_gap() {
        let dispatch = Dispatch::new([]).unwrap_or_else(|error| panic!("{error}"));
        assert!(dispatch.is_empty());

        let first = dispatch
            .verify_covers(&[id("jarvis.beta.two"), id("jarvis.alpha.one")])
            .err()
            .unwrap_or_else(|| panic!("an empty table covers nothing"));
        assert_eq!(
            first,
            DispatchError::Uncovered {
                tool: "jarvis.alpha.one".to_owned()
            },
            "the first gap is reported in identifier order, not in argument order"
        );
    }

    /// An empty table is legal to *build* — a daemon with no adapters is a valid shape — and it is the
    /// coverage check that refuses it if the registry is non-empty. Asserted so the two responsibilities stay
    /// separate.
    #[test]
    fn an_empty_table_is_built_and_verified_for_coverage() {
        let dispatch = Dispatch::new([]).unwrap_or_else(|error| panic!("{error}"));
        assert!(dispatch.is_empty());
        assert!(dispatch.verify_covers(&[]).is_ok());
        assert!(dispatch.verify_covers(&[id("jarvis.alpha.one")]).is_err());
    }
}
