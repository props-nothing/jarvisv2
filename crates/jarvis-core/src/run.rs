//! The native run state machine: states, legal transitions, and concurrency expectations.
//!
//! `docs/architecture/runtime-and-models.md` declares the diagram; this module is its
//! executable form. `agent_runs.state` in migration `0003_conversation_state.sql` uses
//! the same ten spellings, so a state the domain rejects cannot be persisted through a
//! direct write.
//!
//! # Why the transitions live here and not in SQLite
//!
//! A `CHECK` constraint is a property of a single row, so it can express "a terminal row
//! carries an outcome and an end time" but it cannot express a *legal transition*, which
//! depends on the previous row. [ADR-0003](../../../../docs/adr/0003-sqlite-local-postgres-server.md)
//! and `docs/architecture/storage.md` therefore put the transition table in Rust and keep
//! the row property in SQLite. The split is deliberate: SQLite makes illegal *rows*
//! unrepresentable, and this module makes illegal *edges* unrepresentable.
//!
//! The repository enforces a transition by writing it only when the stored state and
//! version still match [`ExpectedRunState`], which is the same optimistic-concurrency
//! discipline `docs/architecture/storage.md` requires for interactive state machines.
//!
//! # Ordering of validation
//!
//! [`RunTransition::apply`] checks in a fixed order and returns one specific error. This
//! matters for diagnosis: a transition out of an already-settled run and a transition
//! that merely skips a step are not the same defect, so they do not share an error
//! variant even though both would be "rejected" by a single legality check.
//!
//! ```
//! use jarvis_core::{ExpectedRunState, RunState, RunTransition};
//!
//! let expected = ExpectedRunState::new(RunState::Received, 1);
//! let transition = RunTransition::new(RunState::ContextBuilding, None, None)?;
//! transition.apply(expected)?;
//!
//! // Skipping a step is refused rather than silently accepted.
//! let skipped = RunTransition::new(RunState::Executing, None, None)?;
//! assert!(skipped.apply(expected).is_err());
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use thiserror::Error;

/// Explains why a run state or outcome spelling was rejected.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum InvalidRunState {
    /// The text did not name a known run state.
    #[error("unknown run state")]
    UnknownState,
    /// The text did not name a known terminal outcome.
    #[error("unknown run outcome")]
    UnknownOutcome,
}

/// Explains why a bounded run failure code was rejected.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum InvalidRunErrorCode {
    /// The code was empty.
    #[error("run error code is empty")]
    Empty,
    /// The code exceeded the 64-character storage bound.
    #[error("run error code exceeds 64 characters")]
    TooLong,
    /// The code contained a character outside the stable machine-readable set.
    #[error("run error code must use ascii lowercase letters, digits, '_', '-', or '.'")]
    NotPortable,
}

/// A stable, bounded, machine-readable reason a run failed.
///
/// Deliberately ASCII-only. `agent_runs.error_code` is bounded by SQLite `length()` on a
/// TEXT value, which counts **characters**; requiring ASCII makes the character count and
/// the byte count identical and removes that ambiguity rather than relying on it.
///
/// The set is restricted to identifiers a log or metric label can carry unchanged, so a
/// provider's free text cannot become an error code without being normalized first.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct RunErrorCode(String);

impl RunErrorCode {
    /// Validates a failure code.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidRunErrorCode`] when the code is empty, longer than 64 characters,
    /// or contains a character outside `[a-z0-9_.-]`.
    pub fn new(value: impl Into<String>) -> Result<Self, InvalidRunErrorCode> {
        let value = value.into();
        if value.is_empty() {
            return Err(InvalidRunErrorCode::Empty);
        }
        if value.chars().count() > MAX_RUN_ERROR_CODE_CHARS {
            return Err(InvalidRunErrorCode::TooLong);
        }
        let portable = value.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '_' | '-' | '.')
        });
        if portable {
            Ok(Self(value))
        } else {
            Err(InvalidRunErrorCode::NotPortable)
        }
    }

    /// Borrows the validated code.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Maximum stored length of a run failure code, matching the migration's `CHECK`.
pub const MAX_RUN_ERROR_CODE_CHARS: usize = 64;

impl fmt::Display for RunErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// The lifecycle position of one agent run.
///
/// Serialization is manual rather than derived so the wire and storage spelling is a
/// single stable snake-case string that a derive attribute cannot silently change.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RunState {
    /// The run has an identity and an accepted objective, and nothing has started.
    Received,
    /// Bounded, sourced context is being assembled.
    ContextBuilding,
    /// A plan is being produced.
    Planning,
    /// A planned step is running.
    Executing,
    /// The result of a step is being interpreted.
    Observing,
    /// Durable human approval is required before continuing.
    AwaitingApproval,
    /// The answer is being produced.
    Responding,
    /// The run finished successfully. Terminal.
    Completed,
    /// The run stopped because it was cancelled. Terminal.
    Cancelled,
    /// The run stopped because of a failure. Terminal.
    Failed,
}

impl RunState {
    /// Returns the stable snake-case wire/storage code.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Received => "received",
            Self::ContextBuilding => "context_building",
            Self::Planning => "planning",
            Self::Executing => "executing",
            Self::Observing => "observing",
            Self::AwaitingApproval => "awaiting_approval",
            Self::Responding => "responding",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
        }
    }

    /// Returns whether this state ends the run.
    ///
    /// A terminal state can never move again. A cancelled run is a settled run, not a
    /// paused one, which is why `awaiting_approval` is *not* terminal: approval may
    /// resume it, and only an explicit cancellation ends it.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled | Self::Failed)
    }

    /// Returns the terminal outcome this state must carry.
    #[must_use]
    pub const fn required_outcome(self) -> Option<RunOutcome> {
        match self {
            Self::Completed => Some(RunOutcome::Succeeded),
            Self::Cancelled => Some(RunOutcome::Cancelled),
            Self::Failed => Some(RunOutcome::Failed),
            _ => None,
        }
    }

    /// Returns whether a transition from this state to `next` is legal.
    ///
    /// This is the transition table from `docs/architecture/runtime-and-models.md`,
    /// written out per state so a reviewer can compare it against the documented diagram
    /// line by line. It is deliberately not collapsed into a "failure is always allowed"
    /// rule, because the two exceptions below are real and the rule would hide them.
    ///
    /// Two asymmetries are intentional:
    ///
    /// - `awaiting_approval` is cancellable but **not** failable. No machine work is in
    ///   progress while a human decides, so there is nothing that can fail; the
    ///   documented `cancelled/expired` edge covers an approval that times out.
    /// - `responding` is both failable and cancellable. The documented diagram omits
    ///   these two edges, which would leave a generation that fails mid-answer with
    ///   `completed` as its only legal successor. That records a failed answer as a
    ///   success, so the edges are required rather than optional.
    #[must_use]
    pub const fn can_transition_to(self, next: Self) -> bool {
        match self {
            Self::Received => {
                matches!(next, Self::ContextBuilding | Self::Failed | Self::Cancelled)
            }
            Self::ContextBuilding => {
                matches!(next, Self::Planning | Self::Failed | Self::Cancelled)
            }
            Self::Planning => matches!(
                next,
                Self::Executing | Self::AwaitingApproval | Self::Failed | Self::Cancelled
            ),
            Self::Executing => matches!(next, Self::Observing | Self::Failed | Self::Cancelled),
            Self::Observing => matches!(
                next,
                Self::Planning | Self::Responding | Self::Failed | Self::Cancelled
            ),
            Self::AwaitingApproval => {
                matches!(next, Self::Executing | Self::Responding | Self::Cancelled)
            }
            Self::Responding => {
                matches!(next, Self::Completed | Self::Failed | Self::Cancelled)
            }
            Self::Completed | Self::Cancelled | Self::Failed => false,
        }
    }

    /// Returns every state from least to most settled.
    #[must_use]
    pub const fn all() -> [Self; 10] {
        [
            Self::Received,
            Self::ContextBuilding,
            Self::Planning,
            Self::Executing,
            Self::Observing,
            Self::AwaitingApproval,
            Self::Responding,
            Self::Completed,
            Self::Cancelled,
            Self::Failed,
        ]
    }
}

impl fmt::Display for RunState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for RunState {
    type Err = InvalidRunState;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "received" => Ok(Self::Received),
            "context_building" => Ok(Self::ContextBuilding),
            "planning" => Ok(Self::Planning),
            "executing" => Ok(Self::Executing),
            "observing" => Ok(Self::Observing),
            "awaiting_approval" => Ok(Self::AwaitingApproval),
            "responding" => Ok(Self::Responding),
            "completed" => Ok(Self::Completed),
            "cancelled" => Ok(Self::Cancelled),
            "failed" => Ok(Self::Failed),
            _ => Err(InvalidRunState::UnknownState),
        }
    }
}

impl Serialize for RunState {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for RunState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(de::Error::custom)
    }
}

/// How a terminal run settled.
///
/// Separate from [`RunState`] because the storage layer records both and the two must
/// agree: `completed` with outcome `failed` is not a representable run.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RunOutcome {
    /// The objective was achieved.
    Succeeded,
    /// The run stopped because it was cancelled or its approval expired.
    Cancelled,
    /// The run stopped because of a failure.
    Failed,
}

impl RunOutcome {
    /// Returns the stable snake-case wire/storage code.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
        }
    }
}

impl fmt::Display for RunOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for RunOutcome {
    type Err = InvalidRunState;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "succeeded" => Ok(Self::Succeeded),
            "cancelled" => Ok(Self::Cancelled),
            "failed" => Ok(Self::Failed),
            _ => Err(InvalidRunState::UnknownOutcome),
        }
    }
}

impl Serialize for RunOutcome {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for RunOutcome {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(de::Error::custom)
    }
}

/// The state and version a caller believes a run is in.
///
/// This is the optimistic-concurrency primitive. A transition is applied only when the
/// stored row still matches, so two writers cannot both advance one run and a stale
/// client cannot move a run that another writer already settled.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExpectedRunState {
    state: RunState,
    version: i64,
}

impl ExpectedRunState {
    /// Creates an expectation from a previously read state and version.
    #[must_use]
    pub const fn new(state: RunState, version: i64) -> Self {
        Self { state, version }
    }

    /// Returns the state the caller believes is stored.
    #[must_use]
    pub const fn state(self) -> RunState {
        self.state
    }

    /// Returns the row version the caller believes is stored.
    #[must_use]
    pub const fn version(self) -> i64 {
        self.version
    }
}

/// Explains why a requested run transition was refused.
///
/// Every variant means the run was **not** changed. Validation never partially applies.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum RunTransitionError {
    /// The run already settled, so no transition is possible.
    #[error("run in terminal state {from} cannot transition")]
    TerminalStateImmutable {
        /// The settled state the run is in.
        from: RunState,
    },
    /// The transition is not in the documented table.
    #[error("illegal run transition {from} -> {to}")]
    IllegalTransition {
        /// The state the run is in.
        from: RunState,
        /// The requested target state.
        to: RunState,
    },
    /// A terminal target was requested without the outcome it must carry.
    #[error("terminal run state {to} requires a terminal outcome")]
    OutcomeRequired {
        /// The requested terminal state.
        to: RunState,
    },
    /// A non-terminal target was requested with a terminal outcome.
    #[error("non-terminal run state {to} cannot carry a terminal outcome")]
    OutcomeForbidden {
        /// The requested non-terminal state.
        to: RunState,
    },
    /// The supplied outcome does not belong to the requested terminal state.
    #[error("run state {to} requires outcome {expected}, not {supplied}")]
    OutcomeMismatch {
        /// The requested terminal state.
        to: RunState,
        /// The outcome that state requires.
        expected: RunOutcome,
        /// The outcome the caller supplied.
        supplied: RunOutcome,
    },
    /// A failed run was requested without a failure code.
    #[error("failed run requires an error code")]
    FailureCodeRequired,
    /// A failure code was supplied for a target that is not a failure.
    #[error("run state {to} cannot carry a failure code")]
    FailureCodeForbidden {
        /// The requested target state.
        to: RunState,
    },
}

/// A validated request to move one run to another state.
///
/// Construction validates the *target* against its own outcome and failure code, because
/// "a terminal state carries an outcome" is a property of the target alone. [`Self::apply`]
/// additionally validates the *edge*, which needs the current state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunTransition {
    to: RunState,
    outcome: Option<RunOutcome>,
    error_code: Option<RunErrorCode>,
}

impl RunTransition {
    /// Builds a transition to `to`, validating it against the target rules.
    ///
    /// # Errors
    ///
    /// Returns [`RunTransitionError::OutcomeRequired`], `OutcomeForbidden`,
    /// `OutcomeMismatch`, `FailureCodeRequired`, or `FailureCodeForbidden`.
    pub fn new(
        to: RunState,
        outcome: Option<RunOutcome>,
        error_code: Option<RunErrorCode>,
    ) -> Result<Self, RunTransitionError> {
        match to.required_outcome() {
            None => {
                if outcome.is_some() {
                    return Err(RunTransitionError::OutcomeForbidden { to });
                }
            }
            Some(expected) => {
                let supplied = outcome.ok_or(RunTransitionError::OutcomeRequired { to })?;
                if supplied != expected {
                    return Err(RunTransitionError::OutcomeMismatch {
                        to,
                        expected,
                        supplied,
                    });
                }
            }
        }

        if to == RunState::Failed {
            if error_code.is_none() {
                return Err(RunTransitionError::FailureCodeRequired);
            }
        } else if error_code.is_some() {
            return Err(RunTransitionError::FailureCodeForbidden { to });
        }

        Ok(Self {
            to,
            outcome,
            error_code,
        })
    }

    /// Builds a transition that settles a run as successfully completed.
    ///
    /// # Errors
    ///
    /// Cannot fail; the constructor exists so callers get a `RunTransition` without
    /// restating the outcome rules.
    #[must_use]
    pub const fn completed() -> Self {
        Self {
            to: RunState::Completed,
            outcome: Some(RunOutcome::Succeeded),
            error_code: None,
        }
    }

    /// Builds a transition that settles a run as failed with a bounded reason.
    #[must_use]
    pub const fn failed(error_code: RunErrorCode) -> Self {
        Self {
            to: RunState::Failed,
            outcome: Some(RunOutcome::Failed),
            error_code: Some(error_code),
        }
    }

    /// Returns the target state.
    #[must_use]
    pub const fn to(&self) -> RunState {
        self.to
    }

    /// Returns the terminal outcome, present exactly when the target is terminal.
    #[must_use]
    pub const fn outcome(&self) -> Option<RunOutcome> {
        self.outcome
    }

    /// Returns the bounded failure code, present exactly when the target is `failed`.
    #[must_use]
    pub const fn error_code(&self) -> Option<&RunErrorCode> {
        self.error_code.as_ref()
    }

    /// Checks the edge against the state the caller believes is stored.
    ///
    /// # Errors
    ///
    /// Returns [`RunTransitionError::TerminalStateImmutable`] when the run already
    /// settled, or [`RunTransitionError::IllegalTransition`] when the edge is not in the
    /// documented table. The two are separate variants because "already settled" and
    /// "out of order" are different defects with different remedies.
    pub fn apply(&self, expected: ExpectedRunState) -> Result<(), RunTransitionError> {
        // Checked first: for a settled run *every* transition is refused, and reporting
        // "illegal transition" for one of them would send the reader looking for an
        // ordering bug instead of a duplicate settlement.
        if expected.state.is_terminal() {
            return Err(RunTransitionError::TerminalStateImmutable {
                from: expected.state,
            });
        }
        if !expected.state.can_transition_to(self.to) {
            return Err(RunTransitionError::IllegalTransition {
                from: expected.state,
                to: self.to,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(value: &str) -> RunErrorCode {
        match RunErrorCode::new(value) {
            Ok(code) => code,
            Err(error) => panic!("valid fixture code: {error}"),
        }
    }

    fn transition(to: RunState) -> RunTransition {
        match RunTransition::new(to, to.required_outcome(), None) {
            Ok(transition) => transition,
            Err(error) => panic!("reachable target transitions: {error}"),
        }
    }

    #[test]
    fn state_and_outcome_spellings_match_the_migration() {
        // The migration's CHECK lists these exact strings. Asserting the full set, rather
        // than a sample, is what makes a future rename fail here instead of at runtime in
        // the storage layer.
        let states = RunState::all()
            .iter()
            .map(|state| state.as_str().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            states,
            vec![
                "received",
                "context_building",
                "planning",
                "executing",
                "observing",
                "awaiting_approval",
                "responding",
                "completed",
                "cancelled",
                "failed",
            ]
        );
        assert_eq!(RunOutcome::Succeeded.as_str(), "succeeded");
        assert_eq!(RunOutcome::Cancelled.as_str(), "cancelled");
        assert_eq!(RunOutcome::Failed.as_str(), "failed");

        for state in RunState::all() {
            assert_eq!(state.as_str().parse::<RunState>(), Ok(state));
            assert_eq!(
                serde_json::to_string(&state).ok().as_deref(),
                Some(format!("\"{}\"", state.as_str()).as_str())
            );
        }
    }

    #[test]
    fn terminal_states_carry_exactly_one_outcome() {
        for state in RunState::all() {
            let outcome = state.required_outcome();
            assert_eq!(
                outcome.is_some(),
                state.is_terminal(),
                "terminality and required outcome must agree for {state}"
            );
        }
        assert_eq!(
            RunState::Completed.required_outcome(),
            Some(RunOutcome::Succeeded)
        );
        assert_eq!(
            RunState::Cancelled.required_outcome(),
            Some(RunOutcome::Cancelled)
        );
        assert_eq!(
            RunState::Failed.required_outcome(),
            Some(RunOutcome::Failed)
        );
    }

    #[test]
    fn every_documented_edge_is_legal_and_the_reverse_is_not() {
        // Pairs taken from the diagram in docs/architecture/runtime-and-models.md. The
        // reverse assertions are the falsifying half: a table that allowed everything
        // would satisfy the positive cases alone.
        let legal = [
            (RunState::Received, RunState::ContextBuilding),
            (RunState::ContextBuilding, RunState::Planning),
            (RunState::Planning, RunState::Executing),
            (RunState::Planning, RunState::AwaitingApproval),
            (RunState::Executing, RunState::Observing),
            (RunState::Observing, RunState::Planning),
            (RunState::Observing, RunState::Responding),
            (RunState::AwaitingApproval, RunState::Executing),
            (RunState::AwaitingApproval, RunState::Responding),
            (RunState::Responding, RunState::Completed),
        ];
        for (from, to) in legal {
            assert!(
                from.can_transition_to(to),
                "{from} -> {to} is documented as legal"
            );
            // Falsifying half: this is what caught three edges that read as plausible
            // (`executing -> responding`, `executing -> awaiting_approval`, and
            // `planning -> observing`) but are not in the documented diagram, each of
            // which happened to mirror a legal edge in the opposite direction.
            assert!(
                !to.can_transition_to(from) || legal.contains(&(to, from)),
                "{to} -> {from} must not become legal by symmetry"
            );
        }
    }

    #[test]
    fn failure_and_cancellation_edges_follow_where_work_is_in_flight() {
        let failable = [
            RunState::Received,
            RunState::ContextBuilding,
            RunState::Planning,
            RunState::Executing,
            RunState::Observing,
            RunState::Responding,
        ];
        for state in RunState::all() {
            assert_eq!(
                state.can_transition_to(RunState::Failed),
                failable.contains(&state),
                "failure reachability for {state}"
            );
            // Every non-terminal state is cancellable: a human may cancel at any point.
            if !state.is_terminal() {
                assert!(
                    state.can_transition_to(RunState::Cancelled),
                    "{state} -> cancelled"
                );
            }
        }

        // The two documented asymmetries, asserted directly so a future blanket
        // "failure is always allowed" simplification fails here.
        assert!(
            !RunState::AwaitingApproval.can_transition_to(RunState::Failed),
            "no machine work is in flight while a human decides, so there is nothing to fail"
        );
        assert!(
            RunState::AwaitingApproval.can_transition_to(RunState::Cancelled),
            "an approval expires, which the diagram records as a cancellation"
        );
        assert!(
            RunState::Responding.can_transition_to(RunState::Failed),
            "a generation failure must not be forced to record itself as completed"
        );

        // And from no terminal state, because terminal immutability forbids it.
        for state in [RunState::Completed, RunState::Cancelled, RunState::Failed] {
            assert!(!state.can_transition_to(RunState::Failed));
            assert!(!state.can_transition_to(RunState::Cancelled));
            assert!(!state.can_transition_to(RunState::Received));
        }
    }

    #[test]
    fn every_non_terminal_state_reaches_a_terminal_state() {
        // A run that can enter a state with no exit can never settle, which is a liveness
        // defect a per-edge test would miss entirely.
        for state in RunState::all() {
            if state.is_terminal() {
                continue;
            }
            let exits = RunState::all()
                .into_iter()
                .filter(|next| state.can_transition_to(*next))
                .collect::<Vec<_>>();
            assert!(!exits.is_empty(), "{state} has no legal successor");
            assert!(
                exits.iter().any(|next| next.is_terminal()),
                "{state} can never settle: successors are {exits:?}"
            );
        }
    }

    #[test]
    fn skipping_a_step_is_refused_as_an_illegal_transition() {
        let skipped = transition(RunState::Executing);
        assert_eq!(
            skipped.apply(ExpectedRunState::new(RunState::Received, 1)),
            Err(RunTransitionError::IllegalTransition {
                from: RunState::Received,
                to: RunState::Executing,
            })
        );

        // A self-transition is not progress and must not be accepted as a no-op.
        let repeat = transition(RunState::Responding);
        assert_eq!(
            repeat.apply(ExpectedRunState::new(RunState::Responding, 4)),
            Err(RunTransitionError::IllegalTransition {
                from: RunState::Responding,
                to: RunState::Responding,
            })
        );
    }

    #[test]
    fn a_settled_run_reports_terminal_immutability_even_for_a_legal_edge() {
        // `responding -> completed` is legal in general. From an already-completed run it
        // must report the *terminal* reason, not the legality reason, or a duplicate
        // settlement looks like an ordering bug.
        let completed = transition(RunState::Completed);
        assert_eq!(
            completed.apply(ExpectedRunState::new(RunState::Completed, 9)),
            Err(RunTransitionError::TerminalStateImmutable {
                from: RunState::Completed
            })
        );
    }

    #[test]
    fn a_terminal_target_without_an_outcome_is_refused() {
        assert_eq!(
            RunTransition::new(RunState::Completed, None, None),
            Err(RunTransitionError::OutcomeRequired {
                to: RunState::Completed
            })
        );
    }

    #[test]
    fn an_outcome_on_a_non_terminal_target_is_refused() {
        assert_eq!(
            RunTransition::new(RunState::Planning, Some(RunOutcome::Succeeded), None),
            Err(RunTransitionError::OutcomeForbidden {
                to: RunState::Planning
            })
        );
    }

    #[test]
    fn a_mismatched_outcome_is_refused_with_the_required_one_named() {
        assert_eq!(
            RunTransition::new(RunState::Completed, Some(RunOutcome::Failed), None),
            Err(RunTransitionError::OutcomeMismatch {
                to: RunState::Completed,
                expected: RunOutcome::Succeeded,
                supplied: RunOutcome::Failed,
            })
        );
    }

    #[test]
    fn a_failure_needs_a_code_and_a_non_failure_must_not_have_one() {
        assert_eq!(
            RunTransition::new(RunState::Failed, Some(RunOutcome::Failed), None),
            Err(RunTransitionError::FailureCodeRequired)
        );
        assert_eq!(
            RunTransition::new(
                RunState::Cancelled,
                Some(RunOutcome::Cancelled),
                Some(code("provider_timeout"))
            ),
            Err(RunTransitionError::FailureCodeForbidden {
                to: RunState::Cancelled
            })
        );

        let failed = match RunTransition::new(
            RunState::Failed,
            Some(RunOutcome::Failed),
            Some(code("provider_timeout")),
        ) {
            Ok(failed) => failed,
            Err(error) => panic!("a failed transition with a code is valid: {error}"),
        };
        assert_eq!(
            failed.error_code().map(RunErrorCode::as_str),
            Some("provider_timeout")
        );
        assert_eq!(
            failed.apply(ExpectedRunState::new(RunState::Executing, 3)),
            Ok(())
        );
    }

    #[test]
    fn run_error_codes_are_bounded_and_ascii_only() {
        assert_eq!(RunErrorCode::new(""), Err(InvalidRunErrorCode::Empty));
        assert_eq!(
            RunErrorCode::new("a".repeat(MAX_RUN_ERROR_CODE_CHARS + 1)),
            Err(InvalidRunErrorCode::TooLong)
        );
        assert_eq!(
            RunErrorCode::new("a".repeat(MAX_RUN_ERROR_CODE_CHARS)).map(|code| code.as_str().len()),
            Ok(MAX_RUN_ERROR_CODE_CHARS)
        );
        // A provider's own message must not be storable as a code: uppercase, spaces, and
        // newlines are all rejected, so free text cannot smuggle itself into a label.
        for rejected in ["Provider Timeout", "PROVIDER_TIMEOUT", "line\nbreak", "é"] {
            assert_eq!(
                RunErrorCode::new(rejected),
                Err(InvalidRunErrorCode::NotPortable),
                "{rejected} must be rejected"
            );
        }
    }

    #[test]
    fn a_failure_code_is_ascii_so_character_and_byte_bounds_agree() {
        // The migration bounds this column with `length()` on TEXT, which counts
        // characters. Requiring ASCII is what makes that bound unambiguous, so this test
        // pins the property the earlier one relies on.
        let code = code("a.b-c_0");
        assert!(code.as_str().is_ascii());
        assert_eq!(code.as_str().chars().count(), code.as_str().len());
    }
}
