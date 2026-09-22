//! Tool call outcome states: what a call's result actually establishes.
//!
//! # Why these live in `jarvis-core` rather than with the tool contract
//!
//! `docs/architecture/repository-layout.md` gives `jarvis-core` the "tool... and call state machines"
//! and, separately, lists `jarvis-tools` as an adapter. A state machine is core's, and the dependency
//! direction is `Adapters --> Core`, so an adapter must not own a vocabulary the storage adapter also
//! needs. `jarvis-storage` cannot depend on `jarvis-tools` (adapter-to-adapter is not an allowed
//! direction), and duplicating the enum in both would give the same fact two homes.
//!
//! The **contract-adjacent** types stay in `jarvis-tools`: `ToolOutcomeRecord` pairs an outcome with
//! the evidence or reason that justifies it, which is a validation rule about a tool contract's
//! outcome, and `EffectSet`/`Risk`/`ToolDefinition` are all there. This module is the vocabulary;
//! that one is the rule.
//!
//! # Why "unknown" is a state rather than an error
//!
//! `docs/architecture/tools-and-connectors.md` requires normalizing `unknown` as "the result cannot
//! establish whether an effect occurred". That is not a failure and not a success: a request that
//! reached a provider whose response was lost may have sent the message. Modelling it as an error
//! loses the distinction between "it did not happen" and "nobody can tell", and the difference
//! decides whether retrying is safe — which for a non-idempotent effect it is not.
//!
//! # The order is the meaning
//!
//! The states form a progression from "nothing has happened" to "the effect happened", with two
//! branches at the end. [`ToolOutcome::reached_provider`] and [`ToolOutcome::may_have_had_an_effect`]
//! encode the two questions a caller actually asks, rather than leaving each caller to compare
//! against a list and possibly get the pair backwards — which turns one sent message into two.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::SafeMessage;

/// Maximum characters in an outcome's bounded detail.
pub const MAX_OUTCOME_DETAIL_CHARS: usize = 512;

/// What a tool call establishes.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ToolOutcome {
    /// The intent exists but has not reached an adapter.
    Requested,
    /// Policy and approval passed; the call may now be made.
    Authorized,
    /// The provider accepted the request.
    Submitted,
    /// The provider supplied evidence that the effect completed.
    Confirmed,
    /// Evidence says no effect occurred.
    Failed,
    /// The result cannot establish whether an effect occurred.
    Unknown,
    /// Stopped before a known effect boundary.
    Cancelled,
}

impl ToolOutcome {
    /// Returns the stable wire and storage name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Requested => "requested",
            Self::Authorized => "authorized",
            Self::Submitted => "submitted",
            Self::Confirmed => "confirmed",
            Self::Failed => "failed",
            Self::Unknown => "unknown",
            Self::Cancelled => "cancelled",
        }
    }

    /// Returns whether this state ends the call's progress.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Confirmed | Self::Failed | Self::Unknown | Self::Cancelled
        )
    }

    /// Returns whether the request reached the provider.
    ///
    /// The threshold that matters for cost and for audit: everything from `Submitted` on may have
    /// been billed, recorded by the provider, or seen by a third party.
    #[must_use]
    pub const fn reached_provider(self) -> bool {
        matches!(
            self,
            Self::Submitted | Self::Confirmed | Self::Failed | Self::Unknown
        )
    }

    /// Returns whether this state came from an adapter's report.
    ///
    /// `false` only for `Requested` and `Authorized`: those are states the **pipeline** reaches before
    /// anything is handed to an adapter, so nothing has reported anything yet. Every other state,
    /// including `Submitted`, is an adapter's answer — a provider accepting a request is a report just
    /// as much as a confirmation is, and a store that stamped a report time only for terminal outcomes
    /// would make a submitted call look like one nobody had answered.
    #[must_use]
    pub const fn is_reported(self) -> bool {
        !matches!(self, Self::Requested | Self::Authorized)
    }

    /// Returns whether an effect may have happened without being provable.
    ///
    /// `true` for `Submitted` and `Unknown`, and `false` for `Confirmed` (it is proven) and `Failed`
    /// (it is disproven). The distinction is what a retry decision needs: a `Submitted` call whose
    /// result was lost may have sent the message, so repeating it is a second message.
    #[must_use]
    pub const fn may_have_had_an_effect(self) -> bool {
        matches!(self, Self::Submitted | Self::Unknown)
    }

    /// Returns whether repeating the call is unambiguously safe **from the outcome alone**.
    ///
    /// Not the whole decision: a non-idempotent effect is unsafe to repeat even when the outcome says
    /// nothing happened, because "nothing happened" is the outcome's claim and the idempotency
    /// declaration is the tool's. The tool contract combines the two; this method answers only its
    /// half, and naming it `from_outcome` keeps that visible.
    #[must_use]
    pub const fn is_safe_to_repeat_from_outcome(self) -> bool {
        matches!(
            self,
            Self::Requested | Self::Authorized | Self::Failed | Self::Cancelled
        )
    }

    /// Returns every state, from least to most settled.
    #[must_use]
    pub const fn all() -> [Self; 7] {
        [
            Self::Requested,
            Self::Authorized,
            Self::Submitted,
            Self::Confirmed,
            Self::Failed,
            Self::Unknown,
            Self::Cancelled,
        ]
    }

    /// Returns whether `next` is a legal progression from this state.
    ///
    /// A terminal state cannot advance, and an un-submitted state cannot become `Confirmed` —
    /// evidence of completion cannot exist for a request that was never made. Written as a table so
    /// the rule is reviewable, matching how the run state machine records its own edges.
    #[must_use]
    pub const fn can_advance_to(self, next: Self) -> bool {
        match self {
            Self::Requested => matches!(
                next,
                Self::Authorized | Self::Failed | Self::Cancelled | Self::Unknown
            ),
            // `Failed` here means the call was not made despite being authorized, which a refusal
            // before transmission produces. It is a real outcome: nothing happened, and the reason
            // is known.
            Self::Authorized => matches!(
                next,
                Self::Submitted | Self::Failed | Self::Cancelled | Self::Unknown
            ),
            Self::Submitted => matches!(
                next,
                Self::Confirmed | Self::Failed | Self::Unknown | Self::Cancelled
            ),
            Self::Confirmed | Self::Failed | Self::Unknown | Self::Cancelled => false,
        }
    }
}

impl fmt::Display for ToolOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for ToolOutcome {
    type Err = InvalidToolOutcome;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "requested" => Ok(Self::Requested),
            "authorized" => Ok(Self::Authorized),
            "submitted" => Ok(Self::Submitted),
            "confirmed" => Ok(Self::Confirmed),
            "failed" => Ok(Self::Failed),
            "unknown" => Ok(Self::Unknown),
            "cancelled" => Ok(Self::Cancelled),
            _ => Err(InvalidToolOutcome),
        }
    }
}

impl Serialize for ToolOutcome {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ToolOutcome {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}

/// Indicates that text did not name a tool outcome.
#[derive(Clone, Copy, Debug, Eq, thiserror::Error, PartialEq)]
#[error(
    "tool outcome must be one of requested, authorized, submitted, confirmed, failed, unknown, cancelled"
)]
pub struct InvalidToolOutcome;

/// Explains why a tool outcome record was rejected.
#[derive(Clone, Copy, Debug, Eq, thiserror::Error, PartialEq)]
pub enum ToolOutcomeError {
    /// A `confirmed` outcome carried no provider evidence.
    #[error("a confirmed outcome must carry the evidence that confirmed it")]
    ConfirmedWithoutEvidence,
    /// A `failed` outcome carried no bounded reason.
    #[error("a failed outcome must carry a reason")]
    FailedWithoutReason,
    /// A detail exceeded the bounded length.
    #[error("the outcome detail exceeds {MAX_OUTCOME_DETAIL_CHARS} characters")]
    DetailTooLong,
}

/// An outcome together with its evidence.
///
/// The pairing is enforced: a `confirmed` outcome without evidence is refused, because "the provider
/// supplied evidence of completed effect" is the definition of `confirmed`, and a confirmation with
/// nothing behind it is exactly the "success-sounding string" the architecture forbids treating as
/// proof. This is the rule; [`ToolOutcome`] is only the vocabulary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolOutcomeRecord {
    outcome: ToolOutcome,
    evidence: Option<SafeMessage>,
    reason: Option<SafeMessage>,
}

impl ToolOutcomeRecord {
    /// Records an outcome that needs neither evidence nor a reason.
    ///
    /// # Errors
    ///
    /// Returns [`ToolOutcomeError`] for `confirmed` (which requires evidence) or `failed` (which
    /// requires a reason), so the two states that make a claim cannot be recorded without one.
    pub fn new(outcome: ToolOutcome) -> Result<Self, ToolOutcomeError> {
        match outcome {
            ToolOutcome::Confirmed => Err(ToolOutcomeError::ConfirmedWithoutEvidence),
            ToolOutcome::Failed => Err(ToolOutcomeError::FailedWithoutReason),
            other => Ok(Self {
                outcome: other,
                evidence: None,
                reason: None,
            }),
        }
    }

    /// Records a `confirmed` outcome with the provider evidence behind it.
    ///
    /// # Errors
    ///
    /// Returns [`ToolOutcomeError`] when the evidence is empty or oversized. Evidence is a provider
    /// identifier or receipt — a location-like value, not prose — so an empty one is not evidence.
    pub fn confirmed(evidence: impl Into<String>) -> Result<Self, ToolOutcomeError> {
        let evidence = evidence.into();
        let evidence = bounded(&evidence, ToolOutcomeError::ConfirmedWithoutEvidence)?;
        Ok(Self {
            outcome: ToolOutcome::Confirmed,
            evidence: Some(evidence),
            reason: None,
        })
    }

    /// Records a `failed` outcome with its bounded reason.
    ///
    /// # Errors
    ///
    /// Returns [`ToolOutcomeError`] when the reason is empty or oversized, for the reason `confirmed`
    /// requires evidence: `failed` is a claim that nothing happened, and a claim needs a basis.
    pub fn failed(reason: impl Into<String>) -> Result<Self, ToolOutcomeError> {
        let reason = reason.into();
        let reason = bounded(&reason, ToolOutcomeError::FailedWithoutReason)?;
        Ok(Self {
            outcome: ToolOutcome::Failed,
            evidence: None,
            reason: Some(reason),
        })
    }

    /// Returns the outcome state.
    #[must_use]
    pub const fn outcome(&self) -> ToolOutcome {
        self.outcome
    }

    /// Returns the provider evidence, when the outcome is confirmed.
    #[must_use]
    pub fn evidence(&self) -> Option<&str> {
        self.evidence.as_ref().map(SafeMessage::as_str)
    }

    /// Returns the bounded failure reason, when the outcome failed.
    #[must_use]
    pub fn reason(&self) -> Option<&str> {
        self.reason.as_ref().map(SafeMessage::as_str)
    }

    /// Rebuilds a record from stored text, applying the same rules as construction.
    ///
    /// # Errors
    ///
    /// Returns [`ToolOutcomeError`] when a stored pair contradicts the honesty rules, so a row written
    /// by another build cannot produce a record in-memory construction would have refused.
    pub fn from_stored(
        outcome: ToolOutcome,
        evidence: Option<&str>,
        reason: Option<&str>,
    ) -> Result<Self, ToolOutcomeError> {
        let evidence = evidence
            .map(|value| bounded(value, ToolOutcomeError::ConfirmedWithoutEvidence))
            .transpose()?;
        let reason = reason
            .map(|value| bounded(value, ToolOutcomeError::FailedWithoutReason))
            .transpose()?;
        match (outcome, evidence.is_some(), reason.is_some()) {
            (ToolOutcome::Confirmed, false, _) => Err(ToolOutcomeError::ConfirmedWithoutEvidence),
            (ToolOutcome::Failed, _, false) => Err(ToolOutcomeError::FailedWithoutReason),
            (other, _, _) => Ok(Self {
                outcome: other,
                evidence,
                reason,
            }),
        }
    }
}

/// Bounds and trims an outcome detail, reporting `empty_error` when there is nothing to bound.
///
/// Borrows rather than consuming, because a detail is only ever read here: the caller keeps ownership
/// of the value it supplied and this function's only output is the bounded copy.
///
/// The two failures are reported separately on purpose. "You supplied no evidence" and "your evidence
/// is too long" are different mistakes with different fixes, and collapsing them into one error would
/// send a reader looking for a length problem in an empty string.
fn bounded(value: &str, empty_error: ToolOutcomeError) -> Result<SafeMessage, ToolOutcomeError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(empty_error);
    }
    if trimmed.chars().count() > MAX_OUTCOME_DETAIL_CHARS {
        return Err(ToolOutcomeError::DetailTooLong);
    }
    SafeMessage::new(trimmed).map_err(|_| ToolOutcomeError::DetailTooLong)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every state has a distinct name that round-trips through text and serde.
    #[test]
    fn names_are_unique_and_round_trip() {
        let mut names: Vec<&str> = ToolOutcome::all().iter().map(|o| o.as_str()).collect();
        let total = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), total);

        for outcome in ToolOutcome::all() {
            assert_eq!(outcome.as_str().parse::<ToolOutcome>(), Ok(outcome));
            let encoded = serde_json::to_string(&outcome)
                .unwrap_or_else(|error| panic!("serialize: {error}"));
            assert_eq!(encoded, format!("\"{}\"", outcome.as_str()));
            let decoded: ToolOutcome = serde_json::from_str(&encoded)
                .unwrap_or_else(|error| panic!("deserialize: {error}"));
            assert_eq!(decoded, outcome);
        }
        assert!("delivered".parse::<ToolOutcome>().is_err());
        assert!(serde_json::from_str::<ToolOutcome>("\"delivered\"").is_err());
    }

    /// `unknown` says an effect may have happened and is not safely repeatable; `failed` says it did
    /// not and is.
    ///
    /// Both directions, because getting this pair backwards turns one sent message into two.
    #[test]
    fn unknown_and_failed_answer_the_retry_question_oppositely() {
        assert!(ToolOutcome::Unknown.may_have_had_an_effect());
        assert!(!ToolOutcome::Unknown.is_safe_to_repeat_from_outcome());
        assert!(!ToolOutcome::Failed.may_have_had_an_effect());
        assert!(ToolOutcome::Failed.is_safe_to_repeat_from_outcome());
        assert!(ToolOutcome::Unknown.is_terminal());
        assert!(ToolOutcome::Failed.is_terminal());
    }

    /// `confirmed` is proof, not possibility.
    #[test]
    fn confirmed_is_proven_not_merely_possible() {
        assert!(ToolOutcome::Confirmed.reached_provider());
        assert!(!ToolOutcome::Confirmed.may_have_had_an_effect());
        assert!(!ToolOutcome::Confirmed.is_safe_to_repeat_from_outcome());
    }

    /// **Every state after `Authorized` came from an adapter report, and the two before it did not.**
    ///
    /// The rule a store uses to decide whether to stamp a report time. `Submitted` is the case that is
    /// easy to get wrong: it is not terminal, so a predicate phrased as "terminal outcomes were
    /// reported" would leave a submitted call looking like one nobody answered.
    #[test]
    fn only_the_pre_adapter_states_are_unreported() {
        for state in ToolOutcome::all() {
            let expected = !matches!(state, ToolOutcome::Requested | ToolOutcome::Authorized);
            assert_eq!(
                state.is_reported(),
                expected,
                "{state} report classification"
            );
        }
        assert!(ToolOutcome::Submitted.is_reported());
        assert!(ToolOutcome::Cancelled.is_reported());
        assert!(!ToolOutcome::Requested.is_reported());
        // The two predicates answer different questions and are not the same test.
        assert!(ToolOutcome::Cancelled.is_reported());
        assert!(!ToolOutcome::Cancelled.reached_provider());
    }

    /// The states before submission did not reach the provider.
    #[test]
    fn pre_submission_states_did_not_reach_the_provider() {
        for outcome in [ToolOutcome::Requested, ToolOutcome::Authorized] {
            assert!(
                !outcome.reached_provider(),
                "{outcome} must not claim a provider call"
            );
            assert!(!outcome.is_terminal(), "{outcome} is not settled");
            assert!(outcome.is_safe_to_repeat_from_outcome());
        }
    }

    /// `confirmed` cannot be recorded without evidence.
    #[test]
    fn a_confirmation_requires_evidence() {
        assert_eq!(
            ToolOutcomeRecord::new(ToolOutcome::Confirmed),
            Err(ToolOutcomeError::ConfirmedWithoutEvidence)
        );
        assert_eq!(
            ToolOutcomeRecord::confirmed(""),
            Err(ToolOutcomeError::ConfirmedWithoutEvidence),
            "an empty evidence is absent evidence, not an oversized one"
        );
        assert_eq!(
            ToolOutcomeRecord::confirmed("   "),
            Err(ToolOutcomeError::ConfirmedWithoutEvidence)
        );
        assert_eq!(
            ToolOutcomeRecord::confirmed("x".repeat(MAX_OUTCOME_DETAIL_CHARS + 1)),
            Err(ToolOutcomeError::DetailTooLong),
            "and an oversized one is reported as oversized"
        );

        let confirmed = ToolOutcomeRecord::confirmed("provider-message-id-12345")
            .unwrap_or_else(|error| panic!("real evidence: {error}"));
        assert_eq!(confirmed.outcome(), ToolOutcome::Confirmed);
        assert_eq!(confirmed.evidence(), Some("provider-message-id-12345"));
        assert_eq!(confirmed.reason(), None);
    }

    /// `failed` cannot be recorded without a reason.
    #[test]
    fn a_failure_requires_a_reason() {
        assert_eq!(
            ToolOutcomeRecord::new(ToolOutcome::Failed),
            Err(ToolOutcomeError::FailedWithoutReason)
        );
        let failed = ToolOutcomeRecord::failed("the provider returned 403")
            .unwrap_or_else(|error| panic!("a real reason: {error}"));
        assert_eq!(failed.outcome(), ToolOutcome::Failed);
        assert_eq!(failed.reason(), Some("the provider returned 403"));
        assert_eq!(failed.evidence(), None);
    }

    /// The states that make no claim need neither evidence nor a reason.
    #[test]
    fn a_plain_outcome_needs_nothing_extra() {
        for outcome in [
            ToolOutcome::Requested,
            ToolOutcome::Authorized,
            ToolOutcome::Submitted,
            ToolOutcome::Unknown,
            ToolOutcome::Cancelled,
        ] {
            let record = ToolOutcomeRecord::new(outcome)
                .unwrap_or_else(|error| panic!("{outcome}: {error}"));
            assert_eq!(record.outcome(), outcome);
        }
    }

    /// An oversized detail is refused.
    #[test]
    fn an_oversized_detail_is_refused() {
        assert_eq!(
            ToolOutcomeRecord::confirmed("x".repeat(MAX_OUTCOME_DETAIL_CHARS + 1)),
            Err(ToolOutcomeError::DetailTooLong)
        );
    }

    /// The stored reconstruction applies the same honesty rules as construction.
    #[test]
    fn a_stored_record_is_rechecked() {
        assert_eq!(
            ToolOutcomeRecord::from_stored(ToolOutcome::Confirmed, None, None),
            Err(ToolOutcomeError::ConfirmedWithoutEvidence)
        );
        assert_eq!(
            ToolOutcomeRecord::from_stored(ToolOutcome::Failed, None, None),
            Err(ToolOutcomeError::FailedWithoutReason)
        );
        let rebuilt = ToolOutcomeRecord::from_stored(ToolOutcome::Confirmed, Some("id-1"), None)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(rebuilt.evidence(), Some("id-1"));
        let unknown = ToolOutcomeRecord::from_stored(ToolOutcome::Unknown, None, None)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(unknown.outcome(), ToolOutcome::Unknown);
    }

    /// `confirmed` is unreachable from an un-submitted state.
    #[test]
    fn confirmation_is_unreachable_without_submission() {
        assert!(!ToolOutcome::Requested.can_advance_to(ToolOutcome::Confirmed));
        assert!(!ToolOutcome::Authorized.can_advance_to(ToolOutcome::Confirmed));
        assert!(!ToolOutcome::Requested.can_advance_to(ToolOutcome::Submitted));
        assert!(ToolOutcome::Requested.can_advance_to(ToolOutcome::Authorized));
        assert!(ToolOutcome::Authorized.can_advance_to(ToolOutcome::Submitted));
        assert!(ToolOutcome::Submitted.can_advance_to(ToolOutcome::Confirmed));
    }

    /// A terminal state cannot advance.
    #[test]
    fn a_terminal_state_cannot_advance() {
        for terminal in [
            ToolOutcome::Confirmed,
            ToolOutcome::Failed,
            ToolOutcome::Unknown,
            ToolOutcome::Cancelled,
        ] {
            assert!(terminal.is_terminal(), "{terminal} must be terminal");
            for next in ToolOutcome::all() {
                assert!(
                    !terminal.can_advance_to(next),
                    "{terminal} must not advance to {next}"
                );
            }
        }
    }

    /// Cancellation is reachable from every non-terminal state.
    #[test]
    fn cancellation_is_reachable_before_settlement() {
        for state in [
            ToolOutcome::Requested,
            ToolOutcome::Authorized,
            ToolOutcome::Submitted,
        ] {
            assert!(
                state.can_advance_to(ToolOutcome::Cancelled),
                "{state} must be cancellable"
            );
        }
    }
}
