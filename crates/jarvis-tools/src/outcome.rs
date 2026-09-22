//! Tool outcome states: what a call's result actually establishes.
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
//! The states form a progression from "nothing has happened" to "the effect happened", and there
//! are two branches at the end: a known outcome and an unknown one. [`ToolOutcome::reached_provider`]
//! and [`ToolOutcome::may_have_had_an_effect`] encode the two questions a caller actually asks,
//! rather than leaving each caller to compare against a list.

use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Maximum characters in an outcome's bounded detail.
pub const MAX_OUTCOME_DETAIL_CHARS: usize = 512;

/// What a tool call establishes.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
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
    /// Not the whole decision: a non-idempotent effect is unsafe to repeat even when the outcome
    /// says nothing happened, because "nothing happened" is the outcome's claim and the idempotency
    /// declaration is the tool's. `P3-005` combines the two; this method answers only its half, and
    /// naming it `from_outcome` keeps that visible.
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
            Self::Authorized => matches!(
                next,
                Self::Submitted | Self::Failed | Self::Cancelled | Self::Unknown
            ),
            // `Failed` here means the call was not made despite being authorized, which a refusal
            // before transmission produces. It is a real outcome: nothing happened, and the reason
            // is known.
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

/// Explains why an outcome record was rejected.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
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
/// proof.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolOutcomeRecord {
    outcome: ToolOutcome,
    evidence: Option<String>,
    reason: Option<String>,
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
        let trimmed = evidence.trim();
        if trimmed.is_empty() {
            return Err(ToolOutcomeError::ConfirmedWithoutEvidence);
        }
        if trimmed.chars().count() > MAX_OUTCOME_DETAIL_CHARS {
            return Err(ToolOutcomeError::DetailTooLong);
        }
        Ok(Self {
            outcome: ToolOutcome::Confirmed,
            evidence: Some(trimmed.to_owned()),
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
        let trimmed = reason.trim();
        if trimmed.is_empty() {
            return Err(ToolOutcomeError::FailedWithoutReason);
        }
        if trimmed.chars().count() > MAX_OUTCOME_DETAIL_CHARS {
            return Err(ToolOutcomeError::DetailTooLong);
        }
        Ok(Self {
            outcome: ToolOutcome::Failed,
            evidence: None,
            reason: Some(trimmed.to_owned()),
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
        self.evidence.as_deref()
    }

    /// Returns the bounded failure reason, when the outcome failed.
    #[must_use]
    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every state has a distinct name that round-trips.
    #[test]
    fn names_are_unique_and_round_trip() {
        let mut names: Vec<&str> = ToolOutcome::all().iter().map(|o| o.as_str()).collect();
        let total = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), total);

        for outcome in ToolOutcome::all() {
            let encoded = serde_json::to_string(&outcome)
                .unwrap_or_else(|error| panic!("serialize: {error}"));
            assert_eq!(encoded, format!("\"{}\"", outcome.as_str()));
            let decoded: ToolOutcome = serde_json::from_str(&encoded)
                .unwrap_or_else(|error| panic!("deserialize: {error}"));
            assert_eq!(decoded, outcome);
        }
    }

    /// `unknown` is its own state and is terminal, not an error and not a failure.
    #[test]
    fn unknown_is_a_distinct_terminal_state() {
        assert!(ToolOutcome::Unknown.is_terminal());
        assert!(ToolOutcome::Unknown.reached_provider());
        assert!(ToolOutcome::Unknown.may_have_had_an_effect());
        assert!(!ToolOutcome::Failed.may_have_had_an_effect());
        assert!(
            !ToolOutcome::Unknown.is_safe_to_repeat_from_outcome(),
            "an unknown outcome at an un-deduplicated provider is not safely repeatable"
        );
    }

    /// `confirmed` is not "may have had an effect": it is proof that it did.
    #[test]
    fn confirmed_is_proven_not_merely_possible() {
        assert!(ToolOutcome::Confirmed.reached_provider());
        assert!(!ToolOutcome::Confirmed.may_have_had_an_effect());
    }

    /// The states before submission did not reach the provider, so they cost nothing and are safe
    /// to repeat.
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

    /// **`confirmed` cannot be recorded without evidence.**
    ///
    /// The falsification test for the outcome-honesty rule: a confirmation is a claim that the
    /// provider supplied proof, so a record with no proof must not be representable.
    #[test]
    fn a_confirmation_requires_evidence() {
        assert_eq!(
            ToolOutcomeRecord::new(ToolOutcome::Confirmed),
            Err(ToolOutcomeError::ConfirmedWithoutEvidence)
        );
        assert_eq!(
            ToolOutcomeRecord::confirmed(""),
            Err(ToolOutcomeError::ConfirmedWithoutEvidence)
        );
        assert_eq!(
            ToolOutcomeRecord::confirmed("   "),
            Err(ToolOutcomeError::ConfirmedWithoutEvidence)
        );

        let confirmed = ToolOutcomeRecord::confirmed("provider-message-id-12345")
            .unwrap_or_else(|error| panic!("real evidence: {error}"));
        assert_eq!(confirmed.outcome(), ToolOutcome::Confirmed);
        assert_eq!(confirmed.evidence(), Some("provider-message-id-12345"));
        assert_eq!(confirmed.reason(), None);
    }

    /// **`failed` cannot be recorded without a reason.**
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

    /// An oversized detail is refused, and the bound is stated in the error.
    #[test]
    fn an_oversized_detail_is_refused() {
        assert_eq!(
            ToolOutcomeRecord::confirmed("x".repeat(MAX_OUTCOME_DETAIL_CHARS + 1)),
            Err(ToolOutcomeError::DetailTooLong)
        );
    }

    /// **`confirmed` is unreachable from an un-submitted state.**
    ///
    /// Evidence of completion cannot exist for a request that was never made, so the progression is
    /// refused rather than left to a caller's discipline.
    #[test]
    fn confirmation_is_unreachable_without_submission() {
        assert!(!ToolOutcome::Requested.can_advance_to(ToolOutcome::Confirmed));
        assert!(!ToolOutcome::Authorized.can_advance_to(ToolOutcome::Confirmed));
        assert!(!ToolOutcome::Requested.can_advance_to(ToolOutcome::Submitted));
        // The two steps in between are the only way there.
        assert!(ToolOutcome::Requested.can_advance_to(ToolOutcome::Authorized));
        assert!(ToolOutcome::Authorized.can_advance_to(ToolOutcome::Submitted));
        assert!(ToolOutcome::Submitted.can_advance_to(ToolOutcome::Confirmed));
    }

    /// A terminal state cannot advance, which is what makes an outcome a record rather than a
    /// mutable field.
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

    /// Cancellation is reachable from every non-terminal state, because stopping is always possible
    /// before an effect boundary.
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
