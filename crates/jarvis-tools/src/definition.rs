//! The canonical tool definition: every declared field of a capability, validated as a whole.
//!
//! # Why the aggregate validates cross-field relationships
//!
//! Each field on its own is already checked by its own type — a [`Risk`] cannot exceed the level a
//! number names, a [`RetryPolicy`] cannot promise blind retries for an ambiguous effect. What none of
//! those types can see is the relationship *between* fields, and the relationships are where the
//! dangerous declarations live:
//!
//! - a tool declaring risk below what its effects require would be run with the posture of a weaker
//!   capability than it has (enforced by [`Risk::declared_for`], called from here);
//! - a tool declaring blind retries for a mutating non-idempotent effect would repeat an effect that
//!   may already have happened (enforced by [`RetryPolicy::blind`], called from here);
//! - a tool declaring `Available` while its description admits it needs a connector account nobody
//!   configured is a declaration that will fail at the call instead of at discovery.
//!
//! The third is not enforceable mechanically, and is **not** claimed to be. It is listed because the
//! gap is real and a reader should see it stated rather than assume the constructor covers it.
//!
//! # Construction is the only door
//!
//! The fields are private and there is no `Default`. A definition therefore only exists if it went
//! through [`ToolDefinition::new`], which is the property `P3-002`'s registry relies on: a registered
//! tool is one whose declaration was consistent when it was registered.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::effect::EffectSet;
use crate::identifier::ToolId;
use crate::outcome::{ToolOutcome, ToolOutcomeError, ToolOutcomeRecord};
use crate::policy::{
    ApprovalPolicy, Availability, Idempotency, MAX_TOOL_TIMEOUT_SECONDS, RetryDeclaration,
    RetryPolicy, ToolSensitivity, ToolSource,
};
use crate::risk::{Risk, RiskError};
use crate::schema::ToolSchema;
use crate::scope::ScopeSet;

/// Maximum characters in a tool title.
///
/// Titles reach a model's discovery list and a user's approval prompt, so they are bounded.
pub const MAX_TOOL_TITLE_CHARS: usize = 80;

/// Maximum characters in a tool description.
///
/// Descriptions are the model's basis for selection, and `docs/architecture/tools-and-connectors.md`
/// is explicit that a description "helps model selection but never determines authorization". A
/// bounded length keeps a connector from using the field as a prompt-injection channel into the
/// discovery list; the authorization point is that policy reads the declared fields, never this text.
pub const MAX_TOOL_DESCRIPTION_CHARS: usize = 1024;

/// Explains why a tool definition was rejected.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum ToolDefinitionError {
    /// The title was empty or oversized.
    #[error("a tool title must be 1 to {MAX_TOOL_TITLE_CHARS} characters")]
    InvalidTitle,
    /// The description was empty or oversized.
    #[error("a tool description must be 1 to {MAX_TOOL_DESCRIPTION_CHARS} characters")]
    InvalidDescription,
    /// The version was malformed.
    #[error("the tool version is invalid")]
    InvalidVersion,
    /// The declared risk was not consistent with the declared effects.
    #[error(transparent)]
    Risk(#[from] RiskError),
    /// The timeout was zero or exceeded the maximum.
    #[error("a tool timeout must be 1 to {MAX_TOOL_TIMEOUT_SECONDS} seconds")]
    InvalidTimeout,
    /// The retry policy was inconsistent with the effects or idempotency.
    #[error(transparent)]
    Retry(#[from] crate::policy::RetryPolicyError),
    /// The declared source did not match the source its identifier implies.
    #[error("the identifier implies source {implied} but the definition declares {declared}")]
    SourceMismatch {
        /// The source the identifier's namespace implies.
        implied: String,
        /// The source the definition declared.
        declared: String,
    },
}

/// Explains why a tool definition was rejected.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolDefinition {
    id: ToolId,
    version: String,
    title: String,
    description: String,
    input_schema: ToolSchema,
    output_schema: ToolSchema,
    effects: EffectSet,
    risk: Risk,
    required_scopes: ScopeSet,
    approval: ApprovalPolicy,
    timeout_seconds: u32,
    retry: RetryPolicy,
    idempotency: Idempotency,
    source: ToolSource,
    availability: Availability,
    sensitivity: ToolSensitivity,
}

impl ToolDefinition {
    /// Creates a validated tool definition.
    ///
    /// # Errors
    ///
    /// Returns [`ToolDefinitionError`] for an empty or oversized title or description, a malformed
    /// version, a risk below the effect floor, a zero or oversized timeout, a retry policy that is
    /// unsafe for the declared effects and idempotency, or a declared source that disagrees with the
    /// identifier.
    ///
    /// Takes the **declared** fields and derives the rest, rather than taking a pre-built [`Risk`]:
    /// the risk arrives as a number because that is how a manifest writes it, and validating it
    /// against the effects inside the one constructor is what makes a consistent definition the only
    /// kind that exists.
    #[allow(clippy::too_many_arguments)]
    pub fn new(parts: ToolDefinitionParts) -> Result<Self, ToolDefinitionError> {
        let ToolDefinitionParts {
            id,
            version,
            title,
            description,
            input_schema,
            output_schema,
            effects,
            risk,
            required_scopes,
            approval,
            timeout_seconds,
            retry,
            idempotency,
            source,
            availability,
            sensitivity,
        } = parts;

        validate_text(&title, MAX_TOOL_TITLE_CHARS)
            .then_some(())
            .ok_or(ToolDefinitionError::InvalidTitle)?;
        validate_text(&description, MAX_TOOL_DESCRIPTION_CHARS)
            .then_some(())
            .ok_or(ToolDefinitionError::InvalidDescription)?;
        ToolId::validate_version(&version).map_err(|_| ToolDefinitionError::InvalidVersion)?;

        // The two cross-field checks. Both are delegated to the types that own the rule so the rule
        // has one home; calling them here is what makes this constructor the door.
        let risk = Risk::declared_for(risk, &effects)?;
        // The retry declaration is *converted* rather than inspected: `RetryDeclaration` is the
        // unvalidated shape a manifest holds, and this is the conversion that makes it a
        // `RetryPolicy` or refuses. It needs the effects and idempotency, which is why the
        // declaration cannot validate itself.
        let retry_policy = retry.validate(&effects, idempotency)?;

        if !(1..=MAX_TOOL_TIMEOUT_SECONDS).contains(&timeout_seconds) {
            return Err(ToolDefinitionError::InvalidTimeout);
        }

        // The source is derived from the identifier, so the declared field must agree with it. A
        // mismatch is not repaired silently: `id` is the registry key and `source` is what
        // third-party handling keys on, so disagreement between them is a manifest that lies.
        let implied = id.source();
        if source != implied {
            return Err(ToolDefinitionError::SourceMismatch {
                implied: implied.as_str().to_owned(),
                declared: source.as_str().to_owned(),
            });
        }

        Ok(Self {
            id,
            version,
            title,
            description,
            input_schema,
            output_schema,
            effects,
            risk,
            required_scopes,
            approval,
            timeout_seconds,
            retry: retry_policy,
            idempotency,
            source,
            availability,
            sensitivity,
        })
    }

    /// Returns the canonical identifier.
    #[must_use]
    pub const fn id(&self) -> &ToolId {
        &self.id
    }

    /// Returns the behaviour/schema version.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Returns the concise model- and user-facing title.
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Returns the tool description.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    /// Returns the input schema.
    #[must_use]
    pub const fn input_schema(&self) -> &ToolSchema {
        &self.input_schema
    }

    /// Returns the output schema.
    #[must_use]
    pub const fn output_schema(&self) -> &ToolSchema {
        &self.output_schema
    }

    /// Returns the declared effects.
    #[must_use]
    pub const fn effects(&self) -> &EffectSet {
        &self.effects
    }

    /// Returns the validated baseline risk.
    #[must_use]
    pub const fn risk(&self) -> Risk {
        self.risk
    }

    /// Returns the required JARVIS capability scopes.
    #[must_use]
    pub const fn required_scopes(&self) -> &ScopeSet {
        &self.required_scopes
    }

    /// Returns the declared approval policy.
    #[must_use]
    pub const fn approval(&self) -> ApprovalPolicy {
        self.approval
    }

    /// Returns the execution deadline in seconds.
    #[must_use]
    pub const fn timeout_seconds(&self) -> u32 {
        self.timeout_seconds
    }

    /// Returns the retry policy.
    #[must_use]
    pub const fn retry(&self) -> &RetryPolicy {
        &self.retry
    }
    /// Returns the idempotency declaration.
    #[must_use]
    pub const fn idempotency(&self) -> Idempotency {
        self.idempotency
    }

    /// Returns the source class, which agrees with the identifier.
    #[must_use]
    pub const fn source(&self) -> ToolSource {
        self.source
    }

    /// Returns the declared availability.
    #[must_use]
    pub const fn availability(&self) -> &Availability {
        &self.availability
    }

    /// Returns the input/output classification.
    #[must_use]
    pub const fn sensitivity(&self) -> ToolSensitivity {
        self.sensitivity
    }

    /// Decides whether an outcome may be retried automatically.
    ///
    /// The combination the architecture requires: automatic retry of `unknown` is forbidden for
    /// non-idempotent effects, and more generally a repeat is only safe when the outcome says
    /// nothing has been claimed to happen **and** the tool has declared that repeats are meaningful.
    /// Neither half is sufficient:
    ///
    /// - a `failed` outcome with a non-idempotent tool is not safely repeatable, because "failed" is
    ///   the outcome's claim about a provider whose response was lost — which is exactly `unknown`
    ///   reported by a provider that answered with an error;
    /// - a `confirmed` outcome with an idempotent tool is not repeatable either, because the effect
    ///   already happened and idempotency is what makes a *duplicate* harmless, not what makes a
    ///   second *intent* meaningful.
    ///
    /// Implemented as one predicate rather than two so a caller cannot combine them wrongly.
    #[must_use]
    pub fn may_retry_automatically(&self, outcome: ToolOutcome) -> bool {
        outcome.is_safe_to_repeat_from_outcome()
            && self.idempotency.makes_repeats_safe()
            && self.retry.allows_retry()
    }

    /// Verifies that a confirmed outcome carries evidence, restating the rule at the call site.
    ///
    /// # Errors
    ///
    /// Returns [`ToolDefinitionError`] only through the outcome's own check; present so a caller
    /// holding a definition has one place that says what confirmation requires, rather than having
    /// to know which module enforces it.
    pub fn confirm(
        &self,
        evidence: impl Into<String>,
    ) -> Result<ToolOutcomeRecord, ToolOutcomeError> {
        ToolOutcomeRecord::confirmed(evidence)
    }
}

/// The declared inputs to [`ToolDefinition::new`].
///
/// A struct rather than fifteen positional parameters, and a struct **without** `Default`: every
/// field must be stated at the call site, so a manifest cannot omit one and silently receive a
/// permissive fallback. `deny_unknown_fields` keeps a mistyped field name in a manifest from being
/// ignored.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ToolDefinitionParts {
    /// The canonical identifier.
    pub id: ToolId,
    /// The behaviour/schema version.
    pub version: String,
    /// The concise title.
    pub title: String,
    /// The description.
    pub description: String,
    /// The input schema.
    pub input_schema: ToolSchema,
    /// The output schema.
    pub output_schema: ToolSchema,
    /// The declared effects.
    pub effects: EffectSet,
    /// The declared baseline risk as a number, validated against the effects.
    pub risk: u8,
    /// The required JARVIS capability scopes.
    pub required_scopes: ScopeSet,
    /// The approval policy.
    pub approval: ApprovalPolicy,
    /// The execution deadline in seconds.
    pub timeout_seconds: u32,
    /// The retry policy.
    pub retry: RetryDeclaration,
    /// The idempotency declaration.
    pub idempotency: Idempotency,
    /// The source class, which must agree with the identifier.
    pub source: ToolSource,
    /// The declared availability.
    pub availability: Availability,
    /// The input/output classification.
    pub sensitivity: ToolSensitivity,
}

/// Checks a bounded, non-empty text field.
fn validate_text(value: &str, limit: usize) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty() && trimmed.chars().count() <= limit
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effect::ToolEffect;
    use crate::policy::RetryPolicyError;
    use crate::risk::RiskError;
    use crate::scope::Scope;
    use jarvis_core::Sensitivity;

    /// The input schema every fixture below uses.
    const INPUT_SCHEMA: &str = r#"{
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "properties": { "query": { "type": "string" } },
        "required": ["query"],
        "additionalProperties": false
    }"#;

    /// The output schema every fixture below uses.
    const OUTPUT_SCHEMA: &str = r#"{
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "properties": { "results": { "type": "array" } },
        "required": ["results"],
        "additionalProperties": false
    }"#;

    fn schema(document: &str) -> ToolSchema {
        ToolSchema::parse(document).unwrap_or_else(|error| panic!("fixture schema: {error}"))
    }

    fn scope(value: &str) -> Scope {
        Scope::new(value).unwrap_or_else(|error| panic!("{value}: {error}"))
    }

    /// Builds the declared parts for a read-only, idempotent tool, which every test then perturbs.
    ///
    /// A whole-definition fixture rather than a builder with defaults: the point of the aggregate is
    /// that every field is stated, and a builder that defaulted fields would let a test forget one
    /// and pass for a reason the production path does not have.
    fn read_only_parts() -> ToolDefinitionParts {
        ToolDefinitionParts {
            id: ToolId::new("jarvis.files.read").unwrap_or_else(|error| panic!("{error}")),
            version: "1.0.0".to_owned(),
            title: "Read a file".to_owned(),
            description: "Reads one file from the workspace and returns its contents.".to_owned(),
            input_schema: schema(INPUT_SCHEMA),
            output_schema: schema(OUTPUT_SCHEMA),
            effects: EffectSet::single(ToolEffect::ReadOnly),
            risk: 0,
            required_scopes: ScopeSet::single(scope("files.read")),
            approval: ApprovalPolicy::Auto,
            timeout_seconds: 30,
            retry: RetryDeclaration {
                attempts: 2,
                backoff_ceiling_seconds: 5,
            },
            idempotency: Idempotency::Required,
            source: ToolSource::Native,
            availability: Availability::Available,
            sensitivity: ToolSensitivity::new(Sensitivity::Internal, Sensitivity::Internal),
        }
    }

    fn definition(parts: ToolDefinitionParts) -> ToolDefinition {
        ToolDefinition::new(parts)
            .unwrap_or_else(|error| panic!("a consistent definition must be accepted: {error}"))
    }

    /// A consistent definition is accepted and every accessor agrees with what was declared.
    #[test]
    fn a_consistent_definition_is_accepted() {
        let definition = definition(read_only_parts());
        assert_eq!(definition.id().to_string(), "jarvis.files.read");
        assert_eq!(definition.version(), "1.0.0");
        assert_eq!(definition.title(), "Read a file");
        assert_eq!(definition.risk(), Risk::Minimal);
        assert_eq!(definition.source(), ToolSource::Native);
        assert_eq!(definition.timeout_seconds(), 30);
        assert_eq!(definition.idempotency(), Idempotency::Required);
        assert_eq!(definition.approval(), ApprovalPolicy::Auto);
        assert!(definition.availability().is_available());
        assert!(definition.effects().contains(ToolEffect::ReadOnly));
        assert_eq!(definition.required_scopes().len(), 1);
        assert!(definition.input_schema().document().is_object());
    }

    /// **The falsification test: a risk below the effect floor is refused at the aggregate.**
    ///
    /// The guard lives in [`Risk::declared_for`], and this proves the aggregate actually calls it. A
    /// `financial` tool declaring risk 0 would otherwise be stored with the posture for reading a
    /// calendar. Each case is genuinely *below* its floor: declaring the floor itself is the case the
    /// companion test shows must be accepted.
    #[test]
    fn a_risk_below_the_effect_floor_is_refused() {
        for (effect, risk, required) in [
            (ToolEffect::Financial, 0, 3),
            (ToolEffect::Financial, 1, 3),
            (ToolEffect::Financial, 2, 3),
            (ToolEffect::ExternalCommunication, 0, 2),
            (ToolEffect::ExternalCommunication, 1, 2),
            (ToolEffect::Write, 0, 1),
        ] {
            let mut parts = read_only_parts();
            parts.effects = EffectSet::single(effect);
            parts.risk = risk;
            assert_eq!(
                ToolDefinition::new(parts),
                Err(ToolDefinitionError::Risk(RiskError::BelowEffectFloor {
                    declared: risk,
                    required
                })),
                "risk {risk} must not be accepted for {effect}, which requires {required}"
            );
        }

        // Declaring exactly the floor is accepted, so the guard is a floor and not an equality.
        for (effect, floor) in [
            (ToolEffect::Financial, 3),
            (ToolEffect::ExternalCommunication, 2),
            (ToolEffect::Write, 1),
        ] {
            let mut parts = read_only_parts();
            parts.effects = EffectSet::single(effect);
            parts.risk = floor;
            assert!(
                ToolDefinition::new(parts).is_ok(),
                "{effect} at its own floor of {floor} must be accepted"
            );
        }
    }

    /// **The falsification test: blind retry of an ambiguous effect is refused at the aggregate.**
    ///
    /// The second cross-field rule. A tool that deletes cannot promise automatic retries without a
    /// provider key making repeats safe, and this proves the aggregate runs that check rather than
    /// storing an unsafe retry policy.
    #[test]
    fn a_blind_retry_of_an_ambiguous_effect_is_refused() {
        let mut parts = read_only_parts();
        parts.effects = EffectSet::single(ToolEffect::Destructive);
        parts.risk = 3;
        parts.idempotency = Idempotency::Unsupported;
        parts.retry = RetryDeclaration {
            attempts: 3,
            backoff_ceiling_seconds: 5,
        };
        assert_eq!(
            ToolDefinition::new(parts),
            Err(ToolDefinitionError::Retry(
                RetryPolicyError::BlindRetryOfAmbiguousEffect
            )),
            "a destructive non-idempotent tool must not retry blindly"
        );

        // The same tool with a provider key is accepted, so the refusal is about the effect and not
        // about the effect set being destructive.
        let mut safe = read_only_parts();
        safe.effects = EffectSet::single(ToolEffect::Destructive);
        safe.risk = 3;
        safe.idempotency = Idempotency::ProviderKey;
        safe.retry = RetryDeclaration {
            attempts: 3,
            backoff_ceiling_seconds: 5,
        };
        let accepted = definition(safe);
        assert_eq!(accepted.retry().blind_retries(), 3);
    }

    /// A zero or oversized timeout is refused.
    #[test]
    fn an_unusable_timeout_is_refused() {
        for timeout in [0, MAX_TOOL_TIMEOUT_SECONDS + 1, u32::MAX] {
            let mut parts = read_only_parts();
            parts.timeout_seconds = timeout;
            assert_eq!(
                ToolDefinition::new(parts),
                Err(ToolDefinitionError::InvalidTimeout),
                "{timeout} must be an unusable timeout"
            );
        }
        let mut boundary = read_only_parts();
        boundary.timeout_seconds = MAX_TOOL_TIMEOUT_SECONDS;
        assert!(
            ToolDefinition::new(boundary).is_ok(),
            "the maximum is usable"
        );
    }

    /// An empty or oversized title or description is refused.
    #[test]
    fn unusable_text_is_refused() {
        let mut empty_title = read_only_parts();
        empty_title.title = "   ".to_owned();
        assert_eq!(
            ToolDefinition::new(empty_title),
            Err(ToolDefinitionError::InvalidTitle)
        );

        let mut long_title = read_only_parts();
        long_title.title = "t".repeat(MAX_TOOL_TITLE_CHARS + 1);
        assert_eq!(
            ToolDefinition::new(long_title),
            Err(ToolDefinitionError::InvalidTitle)
        );

        let mut empty_description = read_only_parts();
        empty_description.description = String::new();
        assert_eq!(
            ToolDefinition::new(empty_description),
            Err(ToolDefinitionError::InvalidDescription)
        );

        let mut long_description = read_only_parts();
        long_description.description = "d".repeat(MAX_TOOL_DESCRIPTION_CHARS + 1);
        assert_eq!(
            ToolDefinition::new(long_description),
            Err(ToolDefinitionError::InvalidDescription)
        );
    }

    /// A malformed version is refused.
    #[test]
    fn a_malformed_version_is_refused() {
        for version in ["", "has space", "a".repeat(64).as_str(), "v/1"] {
            let mut parts = read_only_parts();
            parts.version = version.to_owned();
            assert_eq!(
                ToolDefinition::new(parts),
                Err(ToolDefinitionError::InvalidVersion),
                "{version:?} must be an unusable version"
            );
        }
    }

    /// **A declared source that disagrees with the identifier is refused.**
    ///
    /// `source` is what third-party handling keys on, and `id` is the registry key. A manifest that
    /// claimed `Native` for a `gmail.*` identifier would mark a provider's code as JARVIS's own.
    #[test]
    fn a_source_that_disagrees_with_the_identifier_is_refused() {
        let mut parts = read_only_parts();
        parts.id = ToolId::new("gmail.send_message").unwrap_or_else(|error| panic!("{error}"));
        parts.source = ToolSource::Native;
        assert_eq!(
            ToolDefinition::new(parts),
            Err(ToolDefinitionError::SourceMismatch {
                implied: "connector".to_owned(),
                declared: "native".to_owned()
            })
        );
    }

    /// A third-party source that agrees with its identifier is accepted.
    #[test]
    fn a_third_party_definition_is_accepted() {
        let mut parts = read_only_parts();
        parts.id = ToolId::new("mcp.github.search").unwrap_or_else(|error| panic!("{error}"));
        parts.source = ToolSource::Mcp;
        parts.required_scopes = ScopeSet::single(scope("github.search"));
        let definition = definition(parts);
        assert_eq!(definition.source(), ToolSource::Mcp);
        assert!(definition.source().is_third_party());
        assert_eq!(definition.id().namespace(), "mcp.github");
    }

    /// A tool needing no grant is representable, and one needing grants declares them.
    #[test]
    fn a_definition_may_require_no_scopes() {
        let mut parts = read_only_parts();
        parts.required_scopes = ScopeSet::none();
        let definition = definition(parts);
        assert!(definition.required_scopes().is_empty());
        assert!(
            definition
                .required_scopes()
                .is_satisfied_by(&ScopeSet::none()),
            "requiring nothing is satisfied by holding nothing"
        );
    }

    /// **Automatic retry needs both halves: a repeatable outcome and a repeat-safe declaration.**
    ///
    /// The four combinations, so neither half can be dropped. The critical pair is that an
    /// idempotent tool does **not** make a `confirmed` outcome repeatable — the effect already
    /// happened, and idempotency makes a duplicate harmless rather than a second intent meaningful.
    #[test]
    fn automatic_retry_requires_both_halves() {
        let repeatable = definition(read_only_parts());
        assert!(repeatable.may_retry_automatically(ToolOutcome::Failed));
        assert!(repeatable.may_retry_automatically(ToolOutcome::Requested));

        // A confirmed outcome is not repeatable even for an idempotent tool.
        assert!(!repeatable.may_retry_automatically(ToolOutcome::Confirmed));
        // Nor is an unknown one, which is the architecture's explicit rule.
        assert!(!repeatable.may_retry_automatically(ToolOutcome::Unknown));
        // Nor a submitted one, whose result may have been lost in flight.
        assert!(!repeatable.may_retry_automatically(ToolOutcome::Submitted));

        // A non-idempotent tool makes even a `failed` outcome repeat-unsafe.
        let mut ambiguous = read_only_parts();
        ambiguous.effects = EffectSet::single(ToolEffect::ExternalCommunication);
        ambiguous.risk = 2;
        ambiguous.idempotency = Idempotency::Unsupported;
        ambiguous.retry = RetryDeclaration::none();
        let ambiguous = definition(ambiguous);
        assert!(
            !ambiguous.may_retry_automatically(ToolOutcome::Failed),
            "a non-idempotent tool must not repeat even a failed attempt automatically"
        );

        // A tool declaring no retries makes no outcome repeatable regardless of idempotency.
        let mut no_retry = read_only_parts();
        no_retry.retry = RetryDeclaration::none();
        let no_retry = definition(no_retry);
        assert!(!no_retry.may_retry_automatically(ToolOutcome::Failed));
    }

    /// The declared classification is preserved, and the ceiling is the stricter half.
    #[test]
    fn the_sensitivity_ceiling_is_preserved() {
        let mut parts = read_only_parts();
        parts.sensitivity = ToolSensitivity::new(Sensitivity::Public, Sensitivity::Restricted);
        let definition = definition(parts);
        assert_eq!(definition.sensitivity().output(), Sensitivity::Restricted);
        assert_eq!(definition.sensitivity().ceiling(), Sensitivity::Restricted);
    }

    /// Unavailability with a reason is preserved, so discovery can say why.
    #[test]
    fn an_unavailable_definition_is_representable() {
        let mut parts = read_only_parts();
        parts.availability = Availability::unavailable("no connector account is configured")
            .unwrap_or_else(|| panic!("a real reason"));
        let definition = definition(parts);
        assert!(!definition.availability().is_available());
        assert_eq!(
            definition.availability().reason(),
            Some("no connector account is configured")
        );
    }

    /// A definition round-trips through the manifest form, and re-validates on the way back.
    ///
    /// This is what `P3-002` needs: a manifest is parsed before the fields it must be checked
    /// against are known, so `ToolDefinitionParts` is the unvalidated shape and `new` is the check.
    #[test]
    fn a_manifest_round_trips_and_is_rechecked() {
        let parts = read_only_parts();
        let encoded = serde_json::to_value(&parts).unwrap_or_else(|error| panic!("{error}"));
        let decoded: ToolDefinitionParts =
            serde_json::from_value(encoded).unwrap_or_else(|error| panic!("{error}"));
        let first = definition(read_only_parts());
        let second = ToolDefinition::new(decoded)
            .unwrap_or_else(|error| panic!("the round-tripped manifest must validate: {error}"));
        assert_eq!(first, second);
    }

    /// **An inconsistent manifest is refused after parsing, not merely at hand construction.**
    ///
    /// The property that matters for a stored manifest: the check is on the path from JSON, not just
    /// on the path from Rust code. A manifest claiming risk 0 with a `financial` effect parses (it is
    /// well-formed JSON) and then fails validation.
    #[test]
    fn an_inconsistent_manifest_is_refused_after_parsing() {
        let mut parts = read_only_parts();
        parts.effects = EffectSet::single(ToolEffect::Financial);
        parts.risk = 0;
        let encoded = serde_json::to_value(&parts).unwrap_or_else(|error| panic!("{error}"));
        let decoded: ToolDefinitionParts =
            serde_json::from_value(encoded).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            ToolDefinition::new(decoded),
            Err(ToolDefinitionError::Risk(RiskError::BelowEffectFloor {
                declared: 0,
                required: 3
            }))
        );
    }

    /// An empty effect set cannot come back through a manifest, which is the invariant the derived
    /// `Deserialize` would have lost.
    #[test]
    fn an_empty_effect_set_cannot_be_deserialized() {
        let json = serde_json::json!({
            "id": "jarvis.files.read",
            "version": "1.0.0",
            "title": "t",
            "description": "d",
            "input_schema": serde_json::from_str::<serde_json::Value>(INPUT_SCHEMA)
                .unwrap_or_default(),
            "output_schema": serde_json::from_str::<serde_json::Value>(OUTPUT_SCHEMA)
                .unwrap_or_default(),
            "effects": [],
            "risk": 0,
            "required_scopes": [],
            "approval": "auto",
            "timeout_seconds": 30,
            "retry": { "attempts": 0, "backoff_ceiling_seconds": 0 },
            "idempotency": "required",
            "source": "native",
            "availability": "available",
            "sensitivity": { "input": "internal", "output": "internal" }
        });
        let decoded = serde_json::from_value::<ToolDefinitionParts>(json);
        assert!(
            decoded.is_err(),
            "an effect-free tool must not be deserializable"
        );
    }

    /// An unknown field in a manifest is refused rather than ignored.
    ///
    /// A mistyped key is how a declared restriction silently stops applying: a manifest that wrote
    /// `require_scopes` instead of `required_scopes` would otherwise register with no scope
    /// requirement at all.
    #[test]
    fn an_unknown_manifest_field_is_refused() {
        let mut parts =
            serde_json::to_value(read_only_parts()).unwrap_or_else(|error| panic!("{error}"));
        if let Some(object) = parts.as_object_mut() {
            object.insert("require_scopes".to_owned(), serde_json::json!([]));
        }
        assert!(
            serde_json::from_value::<ToolDefinitionParts>(parts).is_err(),
            "a mistyped field must not be ignored"
        );
    }

    /// A tool schema round-trips through a manifest and is re-checked on the way back.
    ///
    /// A stored schema is re-validated rather than trusted, so a hand-edited row cannot introduce a
    /// dialect or a reference the contract refuses.
    #[test]
    fn a_stored_schema_is_rechecked() {
        let parts = read_only_parts();
        let encoded = serde_json::to_value(&parts).unwrap_or_else(|error| panic!("{error}"));
        let decoded: ToolDefinitionParts =
            serde_json::from_value(encoded).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            decoded.input_schema.document(),
            schema(INPUT_SCHEMA).document()
        );

        // A schema with no dialect is refused on deserialization.
        let hostile = serde_json::json!({ "type": "object" });
        assert!(serde_json::from_value::<ToolSchema>(hostile).is_err());
    }

    /// Confirmation still requires evidence at the definition's own entry point.
    #[test]
    fn confirmation_requires_evidence_through_the_definition() {
        let definition = definition(read_only_parts());
        let Err(error) = definition.confirm("") else {
            panic!("empty evidence must not confirm");
        };
        assert_eq!(error, ToolOutcomeError::ConfirmedWithoutEvidence);
        assert!(definition.confirm("provider-id-9").is_ok());
    }
}
