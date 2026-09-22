//! Declared policy inputs: approval, idempotency, retry, source, availability, and sensitivity.
//!
//! `docs/architecture/security.md` lists the authorization inputs as "actor, client, workspace,
//! capability, connector account, resource, channel, effect, risk, and policy version". The ones
//! that belong to the **tool** rather than to the request are here. `P3-003` evaluates them; this
//! module only declares them, and keeping the declaration separate is what lets the policy engine
//! be read and tested as a function of data rather than of intent.
//!
//! # `RetryPolicy` encodes effect-awareness rather than documenting it
//!
//! `docs/architecture/tools-and-connectors.md` requires a "normalized and effect-aware" retry
//! policy, and the honest part of that is that **a non-idempotent effect must not be retried
//! blindly** — the section on outcome honesty says automatic retry of `unknown` is forbidden for
//! non-idempotent effects. Retry and idempotency are therefore checked against each other at
//! construction: a tool cannot declare blind retries for an effect that might already have happened.

use std::fmt;

use jarvis_core::Sensitivity;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::effect::EffectSet;
use crate::risk::Risk;

/// Maximum seconds a tool execution may be given.
///
/// Bounded because a tool with no deadline is a tool that can hold a run open forever, and the run
/// state machine has no timeout of its own. Ten minutes is generous for a synchronous provider call
/// and short enough that a wedged adapter is reported rather than waited on.
pub const MAX_TOOL_TIMEOUT_SECONDS: u32 = 600;

/// Maximum automatic retry attempts after the first try.
pub const MAX_TOOL_RETRY_ATTEMPTS: u8 = 5;

/// Maximum backoff ceiling in seconds.
pub const MAX_TOOL_BACKOFF_SECONDS: u32 = 300;

/// Who a capability comes from.
///
/// A closed set, because the source changes what a tool's output may be trusted for and what
/// `jarvis-tools` may assume about its behaviour. `Mcp` and `Runtime` tools are written by a third
/// party and reached over a protocol; `Native` tools are written here.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolSource {
    /// A tool implemented inside JARVIS.
    Native,
    /// A tool registered by a connector for a provider account.
    Connector,
    /// A tool reached through a Model Context Protocol server.
    Mcp,
    /// A tool provided by an external agent runtime.
    Runtime,
    /// A tool installed as an extension.
    Extension,
}

impl ToolSource {
    /// Returns the stable wire and storage name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Connector => "connector",
            Self::Mcp => "mcp",
            Self::Runtime => "runtime",
            Self::Extension => "extension",
        }
    }

    /// Returns the source a namespace implies.
    ///
    /// A convention this crate can enforce, which is better than a field a caller sets: a tool
    /// cannot claim to be native while living in a connector's namespace. `jarvis` is reserved for
    /// this repository, and the remaining prefixes are the classes
    /// `docs/architecture/tools-and-connectors.md` lists. A namespace matching none of them is
    /// treated as a connector, because a provider account is the only source that needs no marker —
    /// everything else is a JARVIS-chosen vocabulary this project controls.
    ///
    /// Every marker is matched as a **prefix**, including `jarvis`. Matching `jarvis` by exact
    /// equality would classify `jarvis.files.read` as a connector while classifying `mcp.github` as
    /// MCP — the same shape of namespace, two different answers, because one marker happened to be
    /// written with an equality test. The namespace is a qualified path, so a marker is a leading
    /// segment.
    #[must_use]
    pub fn from_namespace(namespace: &str) -> Self {
        if has_marker(namespace, "jarvis") {
            Self::Native
        } else if has_marker(namespace, "mcp") {
            Self::Mcp
        } else if has_marker(namespace, "runtime") {
            Self::Runtime
        } else if has_marker(namespace, "extension") {
            Self::Extension
        } else {
            Self::Connector
        }
    }

    /// Returns whether the source is code this project did not write.
    #[must_use]
    pub const fn is_third_party(self) -> bool {
        matches!(self, Self::Mcp | Self::Runtime | Self::Extension)
    }
}

/// Returns whether a namespace begins with a source marker.
///
/// A marker matches the whole namespace or a leading segment of it, never a prefix of a segment: a
/// connector named `mcpfoo` must not be classified as MCP. Without the segment check, a *connector*
/// could pick a name that starts with a reserved marker and be attributed to a class it is not.
fn has_marker(namespace: &str, marker: &str) -> bool {
    namespace == marker
        || namespace
            .strip_prefix(marker)
            .is_some_and(|rest| rest.starts_with('.'))
}

impl fmt::Display for ToolSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Whether a human decides before the tool runs.
///
/// `Policy` is a real member rather than a synonym for `Ask`: it means "the workspace's policy
/// decides from risk, effect, and context", while `Ask` means "always ask regardless of policy".
/// Collapsing them would make a tool that must always ask indistinguishable from one whose risk
/// happens to require asking today.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalPolicy {
    /// Run without asking, when the actor holds the required scopes.
    Auto,
    /// Ask unless policy already permits it in this context.
    Policy,
    /// Always ask, whatever policy says.
    Ask,
    /// Never run, even with approval a human could give.
    Deny,
}

impl ApprovalPolicy {
    /// Returns the stable wire and storage name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Policy => "policy",
            Self::Ask => "ask",
            Self::Deny => "deny",
        }
    }

    /// Returns whether this policy can ever permit execution.
    #[must_use]
    pub const fn is_runnable(self) -> bool {
        !matches!(self, Self::Deny)
    }

    /// Returns the approval policy a baseline risk implies.
    ///
    /// The guidance table in `docs/architecture/tools-and-connectors.md`: risk 0 is auto when
    /// scoped, risk 1 is auto or ask by workspace policy, risk 2 is fresh approval, and risk 3 is
    /// always-ask or deny. This is a **default a tool may tighten**, not a ceiling it may lower —
    /// `P3-003` is where "context can raise risk but cannot lower a hard policy floor" is enforced.
    ///
    /// Takes [`Risk`] rather than a number so the last arm cannot be a silent catch-all: the
    /// previous `_ => Self::Ask` was correct for level 3 and would also have been the answer for
    /// level 7, which no table row covers.
    #[must_use]
    pub const fn for_risk(risk: Risk) -> Self {
        match risk {
            Risk::Minimal => Self::Auto,
            Risk::Low => Self::Policy,
            Risk::Moderate | Risk::High => Self::Ask,
        }
    }
}

impl fmt::Display for ApprovalPolicy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Whether repeating a call is meaningful.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Idempotency {
    /// The provider offers no way to make a repeat safe.
    Unsupported,
    /// A caller may supply a key, and the provider honours it when it does.
    Optional,
    /// A caller must supply a key, and a repeat without one is refused.
    Required,
    /// The provider has its own deduplication, so JARVIS forwards rather than supplies a key.
    ProviderKey,
}

impl Idempotency {
    /// Returns the stable wire and storage name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unsupported => "unsupported",
            Self::Optional => "optional",
            Self::Required => "required",
            Self::ProviderKey => "provider_key",
        }
    }

    /// Returns whether a caller must supply a key.
    #[must_use]
    pub const fn requires_key(self) -> bool {
        matches!(self, Self::Required)
    }

    /// Returns whether a repeated call is safe to make at all.
    ///
    /// `ProviderKey` counts as safe because the provider deduplicates; `Optional` does not, because
    /// a repeat *without* a key is a second effect and the declaration only says a key is accepted.
    #[must_use]
    pub const fn makes_repeats_safe(self) -> bool {
        matches!(self, Self::Required | Self::ProviderKey)
    }
}

impl fmt::Display for Idempotency {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Explains why a retry policy was rejected.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum RetryPolicyError {
    /// More attempts were requested than the bound allows.
    #[error("a tool retry policy allows at most {MAX_TOOL_RETRY_ATTEMPTS} attempts")]
    TooManyAttempts,
    /// The backoff ceiling exceeded the bound.
    #[error("a tool backoff ceiling must not exceed {MAX_TOOL_BACKOFF_SECONDS} seconds")]
    BackoffTooLong,
    /// Blind retries were declared for an effect that might already have happened.
    #[error("a tool with a possibly-non-idempotent effect must not retry blindly")]
    BlindRetryOfAmbiguousEffect,
}

/// A declared retry policy, before it has been checked against effects and idempotency.
///
/// This exists because the check needs data this struct does not own. A manifest states "three blind
/// retries with a 30-second ceiling", and whether that is legal depends on the tool's effects and
/// idempotency — so the declaration cannot validate itself, and a `Deserialize` implementation on
/// [`RetryPolicy`] would have to either invent those inputs or skip the check entirely.
///
/// Keeping the unvalidated shape separate means the check happens at exactly one place
/// ([`RetryDeclaration::validate`], called from [`crate::ToolDefinition::new`]) and a manifest can be
/// parsed before the fields it must be checked against are known. `deny_unknown_fields` so a
/// mistyped key in a manifest is an error rather than an ignored line.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RetryDeclaration {
    /// How many automatic attempts are permitted after the first.
    pub attempts: u8,
    /// The ceiling on the delay before an attempt, in seconds.
    pub backoff_ceiling_seconds: u32,
}

impl RetryDeclaration {
    /// Declares no automatic retry.
    #[must_use]
    pub const fn none() -> Self {
        Self {
            attempts: 0,
            backoff_ceiling_seconds: 0,
        }
    }

    /// Validates the declaration against the effects and idempotency it must be safe for.
    ///
    /// # Errors
    ///
    /// Returns [`RetryPolicyError`] for the same reasons [`RetryPolicy::blind`] does.
    pub fn validate(
        self,
        effects: &EffectSet,
        idempotency: Idempotency,
    ) -> Result<RetryPolicy, RetryPolicyError> {
        RetryPolicy::blind(
            self.attempts,
            self.backoff_ceiling_seconds,
            effects,
            idempotency,
        )
    }
}

/// How a failed attempt may be repeated.
///
/// `blind_retries` is the dangerous field, so it is validated against both the effects and the
/// idempotency declaration rather than trusted. "Retry the read" is ordinary; "retry the payment"
/// is a second payment, and the only defence is refusing to represent the second as a retry.
///
/// Deliberately **not** `Deserialize`: a policy is only meaningful once checked against the effects
/// and idempotency of the tool it belongs to, and a `Deserialize` impl has no access to them. A
/// manifest parses into a [`RetryDeclaration`] instead and
/// [`crate::ToolDefinition::new`] performs the conversion — so a `RetryPolicy` value in memory is
/// one that was checked, and the type says so.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetryPolicy {
    blind_retries: u8,
    backoff_ceiling_seconds: u32,
}

impl RetryPolicy {
    /// Creates a policy that never retries.
    #[must_use]
    pub const fn none() -> Self {
        Self {
            blind_retries: 0,
            backoff_ceiling_seconds: 0,
        }
    }

    /// Creates a policy allowing bounded blind retries.
    ///
    /// # Errors
    ///
    /// Returns [`RetryPolicyError`] when the counts exceed their bounds, or when blind retries are
    /// requested for a tool whose effect might already have happened.
    ///
    /// # Why this is checked here and not by the caller
    ///
    /// The rule is a property of (effects, idempotency, retries) together, and no one field carries
    /// it. A policy object that could be built with incompatible fields would move the check to
    /// every caller, where the first one to forget is the one that sends the payment twice.
    pub fn blind(
        attempts: u8,
        backoff_ceiling_seconds: u32,
        effects: &EffectSet,
        idempotency: Idempotency,
    ) -> Result<Self, RetryPolicyError> {
        if attempts > MAX_TOOL_RETRY_ATTEMPTS {
            return Err(RetryPolicyError::TooManyAttempts);
        }
        if backoff_ceiling_seconds > MAX_TOOL_BACKOFF_SECONDS {
            return Err(RetryPolicyError::BackoffTooLong);
        }
        // A mutating effect whose repeats are not made safe cannot be retried blindly. A read-only
        // tool can: repeating a read changes nothing, so there is no second effect to worry about.
        if attempts > 0 && effects.is_mutating() && !idempotency.makes_repeats_safe() {
            return Err(RetryPolicyError::BlindRetryOfAmbiguousEffect);
        }
        Ok(Self {
            blind_retries: attempts,
            backoff_ceiling_seconds,
        })
    }

    /// Returns how many blind retries are permitted after the first attempt.
    #[must_use]
    pub const fn blind_retries(self) -> u8 {
        self.blind_retries
    }

    /// Returns the backoff ceiling in seconds.
    #[must_use]
    pub const fn backoff_ceiling_seconds(self) -> u32 {
        self.backoff_ceiling_seconds
    }

    /// Returns whether the policy permits any automatic retry.
    #[must_use]
    pub const fn allows_retry(self) -> bool {
        self.blind_retries > 0
    }
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self::none()
    }
}

/// Whether a tool can run right now.
///
/// Declared rather than probed here: an availability check needs the connector's account and token
/// state, which this crate does not own. `docs/architecture/tools-and-connectors.md` lists
/// availability as "health and configuration requirements", so what a tool records is *what it
/// needs*, and the daemon decides whether it has it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub enum Availability {
    /// The tool can be offered and called.
    Available,
    /// The tool is offered but cannot currently run, with a bounded reason.
    Unavailable {
        /// Why it is unavailable, safe to show an operator.
        reason: String,
    },
}

impl Availability {
    /// Creates an unavailable state with a bounded reason.
    ///
    /// # Errors
    ///
    /// Returns `None`-like failure as an empty `Option` when the reason is empty or oversized, so a
    /// caller must supply a real explanation rather than an empty string that renders as nothing.
    #[must_use]
    pub fn unavailable(reason: impl Into<String>) -> Option<Self> {
        let reason = reason.into();
        let trimmed = reason.trim();
        if trimmed.is_empty() || trimmed.chars().count() > MAX_AVAILABILITY_REASON_CHARS {
            return None;
        }
        Some(Self::Unavailable {
            reason: trimmed.to_owned(),
        })
    }

    /// Returns whether the tool can currently run.
    #[must_use]
    pub const fn is_available(&self) -> bool {
        matches!(self, Self::Available)
    }

    /// Returns the bounded reason, when there is one.
    #[must_use]
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Available => None,
            Self::Unavailable { reason } => Some(reason),
        }
    }
}

/// Maximum characters in an unavailability reason.
///
/// Bounded because the reason reaches model-facing discovery output. A connector's error text is
/// external content, and external content in a tool list is an injection surface: the reason is
/// therefore bounded here and reaches the model as a *reason*, never as an instruction.
pub const MAX_AVAILABILITY_REASON_CHARS: usize = 200;

/// The classification a tool's input and output carry.
///
/// Both halves are required: a tool that reads confidential mail and returns a public summary is
/// not the same as one that returns the mail, and a single field would have to be the maximum,
/// which would over-restrict the summary case and hide what the tool actually consumes.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ToolSensitivity {
    input: Sensitivity,
    output: Sensitivity,
}

impl ToolSensitivity {
    /// Declares the input and output classifications.
    #[must_use]
    pub const fn new(input: Sensitivity, output: Sensitivity) -> Self {
        Self { input, output }
    }

    /// Returns the classification of what the tool consumes.
    #[must_use]
    pub const fn input(self) -> Sensitivity {
        self.input
    }

    /// Returns the classification of what the tool produces.
    #[must_use]
    pub const fn output(self) -> Sensitivity {
        self.output
    }

    /// Returns the more restrictive of the two.
    ///
    /// What a *placement* decision needs: a tool whose output is restricted cannot run on a remote
    /// model even if its input is public.
    #[must_use]
    pub const fn ceiling(self) -> Sensitivity {
        if self.input.level() >= self.output.level() {
            self.input
        } else {
            self.output
        }
    }
}

impl Default for ToolSensitivity {
    fn default() -> Self {
        // Not `Public`: a tool whose classification was never declared must not be treated as safe
        // to disclose, matching `Sensitivity`'s own default.
        Self::new(Sensitivity::Internal, Sensitivity::Internal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effect::ToolEffect;
    use crate::risk::RiskError;

    /// Every closed set has distinct names that round-trip through the wire form.
    #[test]
    fn closed_sets_round_trip() {
        for source in [
            ToolSource::Native,
            ToolSource::Connector,
            ToolSource::Mcp,
            ToolSource::Runtime,
            ToolSource::Extension,
        ] {
            let encoded =
                serde_json::to_string(&source).unwrap_or_else(|error| panic!("serialize: {error}"));
            assert_eq!(encoded, format!("\"{}\"", source.as_str()));
            let decoded: ToolSource = serde_json::from_str(&encoded)
                .unwrap_or_else(|error| panic!("deserialize: {error}"));
            assert_eq!(decoded, source);
        }

        for policy in [
            ApprovalPolicy::Auto,
            ApprovalPolicy::Policy,
            ApprovalPolicy::Ask,
            ApprovalPolicy::Deny,
        ] {
            let encoded =
                serde_json::to_string(&policy).unwrap_or_else(|error| panic!("serialize: {error}"));
            assert_eq!(encoded, format!("\"{}\"", policy.as_str()));
        }

        for idempotency in [
            Idempotency::Unsupported,
            Idempotency::Optional,
            Idempotency::Required,
            Idempotency::ProviderKey,
        ] {
            let encoded = serde_json::to_string(&idempotency)
                .unwrap_or_else(|error| panic!("serialize: {error}"));
            assert_eq!(encoded, format!("\"{}\"", idempotency.as_str()));
        }
    }

    /// **A non-idempotent mutating effect cannot be retried blindly.**
    ///
    /// This is the falsification test for the effect-aware retry rule: a sending tool with no
    /// deduplication must not be able to declare retries, because the second call is a second
    /// message and the outcome of the first may be unknown.
    #[test]
    fn a_mutating_non_idempotent_tool_cannot_retry_blindly() {
        let send = EffectSet::single(ToolEffect::ExternalCommunication);
        let refused = RetryPolicy::blind(3, 30, &send, Idempotency::Unsupported);
        assert_eq!(
            refused,
            Err(RetryPolicyError::BlindRetryOfAmbiguousEffect),
            "a send that is not idempotent must not be retried blindly"
        );

        // The same tool WITH provider-side deduplication may retry, because a repeat is safe.
        assert!(RetryPolicy::blind(3, 30, &send, Idempotency::ProviderKey).is_ok());
        // And with a required caller key, because the provider will refuse the duplicate.
        assert!(RetryPolicy::blind(3, 30, &send, Idempotency::Required).is_ok());
    }

    /// A read-only tool may retry blindly, because repeating a read changes nothing.
    #[test]
    fn a_read_only_tool_may_retry_blindly() {
        let read = EffectSet::single(ToolEffect::ReadOnly);
        let policy = RetryPolicy::blind(3, 30, &read, Idempotency::Unsupported)
            .unwrap_or_else(|error| panic!("a read must be retryable: {error}"));
        assert_eq!(policy.blind_retries(), 3);
        assert!(policy.allows_retry());
    }

    /// **`Optional` idempotency does not make repeats safe.**
    ///
    /// A common misreading: "the provider accepts a key" is not "the provider deduplicates". Only
    /// `Required` and `ProviderKey` mean a repeat cannot produce a second effect.
    #[test]
    fn optional_idempotency_does_not_make_repeats_safe() {
        assert!(!Idempotency::Optional.makes_repeats_safe());
        assert!(Idempotency::Required.makes_repeats_safe());
        assert!(Idempotency::ProviderKey.makes_repeats_safe());
        assert!(!Idempotency::Unsupported.makes_repeats_safe());

        let write = EffectSet::single(ToolEffect::Write);
        assert_eq!(
            RetryPolicy::blind(1, 10, &write, Idempotency::Optional),
            Err(RetryPolicyError::BlindRetryOfAmbiguousEffect),
            "accepting a key is not the same as deduplicating without one"
        );
    }

    /// Zero retries is always permitted, including for an ambiguous effect.
    ///
    /// A policy of "do not retry" is the safe declaration, so it must not be refused for the effect
    /// that most needs it.
    #[test]
    fn no_retries_is_always_permitted() {
        let spend = EffectSet::single(ToolEffect::Financial);
        assert!(RetryPolicy::blind(0, 0, &spend, Idempotency::Unsupported).is_ok());
        assert!(!RetryPolicy::none().allows_retry());
    }

    /// The bounds are enforced rather than documented.
    #[test]
    fn retry_bounds_are_enforced() {
        let read = EffectSet::single(ToolEffect::ReadOnly);
        assert_eq!(
            RetryPolicy::blind(
                MAX_TOOL_RETRY_ATTEMPTS + 1,
                10,
                &read,
                Idempotency::Unsupported
            ),
            Err(RetryPolicyError::TooManyAttempts)
        );
        assert_eq!(
            RetryPolicy::blind(
                1,
                MAX_TOOL_BACKOFF_SECONDS + 1,
                &read,
                Idempotency::Unsupported
            ),
            Err(RetryPolicyError::BackoffTooLong)
        );
    }

    /// The approval default rises with risk, and the riskiest level never becomes auto.
    ///
    /// The out-of-range case this test used to cover (`for_risk(9)`) is now unreachable: the
    /// argument is a [`Risk`], and `Risk::from_level` refuses 9 before the policy is chosen. The
    /// check did not disappear, it moved to where the level is parsed.
    #[test]
    fn the_approval_default_rises_with_risk() {
        assert_eq!(
            ApprovalPolicy::for_risk(Risk::Minimal),
            ApprovalPolicy::Auto
        );
        assert_eq!(ApprovalPolicy::for_risk(Risk::Low), ApprovalPolicy::Policy);
        assert_eq!(
            ApprovalPolicy::for_risk(Risk::Moderate),
            ApprovalPolicy::Ask
        );
        assert_eq!(ApprovalPolicy::for_risk(Risk::High), ApprovalPolicy::Ask);
        assert_eq!(
            Risk::from_level(9),
            Err(RiskError::OutOfRange),
            "an out-of-range risk must be refused before a policy is chosen"
        );
        assert!(!ApprovalPolicy::Deny.is_runnable());
        assert!(ApprovalPolicy::Auto.is_runnable());
    }

    /// Third-party sources are identified as such, because their output is treated differently.
    #[test]
    fn third_party_sources_are_identified() {
        assert!(!ToolSource::Native.is_third_party());
        assert!(!ToolSource::Connector.is_third_party());
        assert!(ToolSource::Mcp.is_third_party());
        assert!(ToolSource::Runtime.is_third_party());
        assert!(ToolSource::Extension.is_third_party());
    }

    /// A marker matches a whole namespace or a leading segment, and never a partial segment.
    ///
    /// Both halves of the boundary are load-bearing. Without the segment check a connector could
    /// name itself `mcpfoo` and be attributed to MCP; without the prefix check `jarvis.files.read`
    /// would fall through to Connector while `mcp.github` resolved to MCP, which is what the first
    /// version of this function did.
    #[test]
    fn a_source_marker_matches_a_leading_segment_only() {
        for (namespace, expected) in [
            ("jarvis", ToolSource::Native),
            ("jarvis.files", ToolSource::Native),
            ("mcp", ToolSource::Mcp),
            ("mcp.github", ToolSource::Mcp),
            ("runtime.agent.session", ToolSource::Runtime),
            ("extension.example", ToolSource::Extension),
            // A partial segment is not a marker: these are connectors that merely look like one.
            ("mcpfoo", ToolSource::Connector),
            ("jarvisfiles", ToolSource::Connector),
            ("runtimexyz", ToolSource::Connector),
            ("gmail", ToolSource::Connector),
        ] {
            assert_eq!(
                ToolSource::from_namespace(namespace),
                expected,
                "{namespace} resolved to the wrong source"
            );
        }
    }

    /// An unavailability reason must be a real explanation.
    #[test]
    fn an_unavailability_reason_must_be_usable() {
        assert!(Availability::unavailable("").is_none());
        assert!(Availability::unavailable("   ").is_none());
        assert!(Availability::unavailable("x".repeat(MAX_AVAILABILITY_REASON_CHARS + 1)).is_none());

        let unavailable = Availability::unavailable("the account is not connected")
            .unwrap_or_else(|| panic!("a real reason must be accepted"));
        assert!(!unavailable.is_available());
        assert_eq!(unavailable.reason(), Some("the account is not connected"));
        assert!(Availability::Available.is_available());
    }

    /// The sensitivity ceiling is the more restrictive half, which is what placement reads.
    #[test]
    fn the_sensitivity_ceiling_is_the_more_restrictive_half() {
        let reads_private_returns_public =
            ToolSensitivity::new(Sensitivity::Confidential, Sensitivity::Public);
        assert_eq!(
            reads_private_returns_public.ceiling(),
            Sensitivity::Confidential
        );

        let reads_public_returns_private =
            ToolSensitivity::new(Sensitivity::Public, Sensitivity::Restricted);
        assert_eq!(
            reads_public_returns_private.ceiling(),
            Sensitivity::Restricted
        );
    }

    /// An undeclared classification is not public.
    #[test]
    fn the_default_classification_is_not_public() {
        assert_eq!(
            ToolSensitivity::default().ceiling(),
            Sensitivity::Internal,
            "an undeclared tool must not be treated as safe to disclose"
        );
    }
}
