//! Canonical tool contracts: identifiers, schemas, effects, risk, scopes, and outcomes.
//!
//! # Why this is its own crate
//!
//! `docs/architecture/repository-layout.md` gives `jarvis-tools` the "canonical registry, schema
//! checks, execution pipeline, MCP translation, sandbox adapters". `P3-001` is the first part: the
//! vocabulary every later tool slice is written against. It lives in an adapter crate rather than in
//! `jarvis-core` because validating a JSON Schema needs a JSON Schema implementation, and
//! `jarvis-core` must stay free of third-party dependencies beyond the ones it already justifies.
//!
//! # The shape of the contract
//!
//! A tool definition is **metadata**, and every field answers a question something downstream must
//! ask:
//!
//! | Field | The question it answers |
//! | --- | --- |
//! | `id` | what may be called, and what may not collide with it |
//! | `version` | which behaviour a stored intent refers to |
//! | `input_schema` / `output_schema` | what a caller may send, and what a caller may receive |
//! | `effects` | what could happen if it runs |
//! | `risk` | how much scrutiny it needs before it runs |
//! | `scopes` | what an actor must have been granted |
//! | `approval` | whether a human decides first |
//! | `timeout` / `retry` | how long it may take and whether repeating is safe |
//! | `idempotency` | whether repeating it is even meaningful |
//! | `availability` | whether it can run right now |
//! | `sensitivity` | what classification its input and output carry |
//!
//! [`ToolRegistry`] holds these and answers two different questions with two different views:
//! [`ToolRegistry::discover`] is model-facing and offers only what can run, while
//! [`ToolRegistry::inventory`] is operator-facing and lists everything with its availability. They
//! are separate methods rather than one with a flag, so an operator view cannot be handed to a model.
//!
//! # What this module deliberately does not do
//!
//! It does not decide anything. Effects, risk, and approval are **declared** here; whether a given
//! actor in a given workspace may run a tool is `P3-003`'s deterministic policy. Keeping the
//! declaration separate from the decision is what makes the policy engine auditable: it reads a
//! contract rather than reconstructing intent from a description string.

mod definition;
mod documents;
mod effect;
mod identifier;
mod outcome;
mod policy;
mod registry;
mod risk;
mod schema;
mod scope;

pub use definition::{
    MAX_TOOL_DESCRIPTION_CHARS, MAX_TOOL_TITLE_CHARS, ToolDefinition, ToolDefinitionError,
    ToolDefinitionParts,
};
pub use documents::{DocumentError, DocumentSet, MAX_SUPPLIED_DOCUMENTS};
pub use effect::{EffectSet, ToolEffect};
pub use identifier::{
    MAX_TOOL_ID_CHARS, MAX_TOOL_ID_SEGMENT_CHARS, MAX_TOOL_VERSION_CHARS, ToolId, ToolIdError,
};
pub use outcome::{MAX_OUTCOME_DETAIL_CHARS, ToolOutcome, ToolOutcomeError, ToolOutcomeRecord};
pub use policy::{
    ApprovalPolicy, Availability, Idempotency, MAX_AVAILABILITY_REASON_CHARS,
    MAX_TOOL_BACKOFF_SECONDS, MAX_TOOL_RETRY_ATTEMPTS, MAX_TOOL_TIMEOUT_SECONDS, RetryDeclaration,
    RetryPolicy, RetryPolicyError, ToolSensitivity, ToolSource,
};
pub use registry::{
    DiscoveryReport, MAX_DISCOVERY_TOOLS, MAX_REGISTERED_TOOLS, MAX_SUMMARY_DESCRIPTION_CHARS,
    RegistrationError, RegistryError, ToolInventoryEntry, ToolRegistry, ToolSummary,
};
pub use risk::{MAX_RISK_LEVEL, Risk, RiskError};
pub use schema::{
    MAX_REPORTED_VIOLATIONS, MAX_TOOL_SCHEMA_BYTES, SchemaError, SchemaViolation,
    TOOL_SCHEMA_DIALECT, ToolSchema, ValidationReport,
};
pub use scope::{
    MAX_SCOPE_CHARS, MAX_SCOPE_SEGMENT_CHARS, Scope, ScopeError, ScopeSet, WILDCARD_ACTION,
};
