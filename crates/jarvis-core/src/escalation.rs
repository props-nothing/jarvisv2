//! Context signals that raise a call's effective risk above its declared level.
//!
//! # Why this lives beside [`Risk`](crate::Risk) in `jarvis-core`
//!
//! Same reason as the risk level and the approval policy: it is **one vocabulary with one meaning** that
//! several crates need and none of them may depend on each other for. `docs/architecture/repository-layout.md`
//! allows an adapter to depend on `jarvis-core` and **not on another adapter**, so a wire contract that
//! reports *why* a decision was taken at a higher risk — `jarvis-protocol` — cannot name a signal defined in
//! `jarvis-tools`. The alternative would be a second spelling of the signal set, which is two values that
//! must agree with nothing holding both.
//!
//! # Why a closed set rather than booleans
//!
//! `docs/architecture/tools-and-connectors.md` names the examples: "an unusually large recipient set,
//! production target, external domain, sensitive attachment, or ambiguous identity". Those are genuinely
//! independent — a target can be external **and** bulk **and** production — but one boolean per signal
//! cannot be iterated, cannot be reported as a list, and lets a caller set one without the decision
//! recording it. A set is what makes "why was this held" answerable from the decision alone.
//!
//! # Why the escalation is a `max` and not a sum
//!
//! [`EscalationSignal::escalation`] maps each signal to a level and the caller takes the maximum. Risk is a
//! **level of scrutiny**, not a quantity of badness: two risk-raising signals do not need more scrutiny than
//! the stricter of the two already requires, and summing would make the mapping from signals to posture
//! impossible to state.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::Risk;

/// Explains why an escalation signal name was rejected.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[error("the escalation signal must be one of external, bulk, sensitive, or production")]
pub struct InvalidEscalationSignal;

/// A context signal that can raise the risk of a specific call.
///
/// A typed closed set rather than a string, for the same reason every other stored vocabulary here is one:
/// this value is recorded in a decision and read back, so a free-form name would be a place for content to
/// enter an audit record, and a rename would silently change what an existing row means.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EscalationSignal {
    /// The target is outside the workspace's own domains or organisation.
    External,
    /// The operation affects many targets at once.
    Bulk,
    /// The payload or target carries content above the ordinary classification.
    Sensitive,
    /// The target is a production system.
    Production,
}

impl EscalationSignal {
    /// Returns the stable snake-case code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::External => "external",
            Self::Bulk => "bulk",
            Self::Sensitive => "sensitive",
            Self::Production => "production",
        }
    }

    /// Returns every signal, so a test can sweep them and a wire contract can validate a set.
    #[must_use]
    pub const fn all() -> [Self; 4] {
        [
            Self::External,
            Self::Bulk,
            Self::Sensitive,
            Self::Production,
        ]
    }

    /// Returns the risk this signal alone raises a call to.
    ///
    /// `Bulk` and `Sensitive` reach `High` because of what they change about a call rather than how large it
    /// is: `docs/architecture/tools-and-connectors.md` lists "mass-send" under the risk-3 posture, and a
    /// sensitive payload changes the consequence of the same action. `External` and `Production` reach
    /// `Moderate`.
    #[must_use]
    pub const fn escalation(self) -> Risk {
        match self {
            Self::Bulk | Self::Sensitive => Risk::High,
            Self::External | Self::Production => Risk::Moderate,
        }
    }
}

impl fmt::Display for EscalationSignal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

impl FromStr for EscalationSignal {
    type Err = InvalidEscalationSignal;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "external" => Ok(Self::External),
            "bulk" => Ok(Self::Bulk),
            "sensitive" => Ok(Self::Sensitive),
            "production" => Ok(Self::Production),
            _ => Err(InvalidEscalationSignal),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every signal round-trips through its name, and the set is closed and distinct.
    #[test]
    fn signals_round_trip_and_are_distinct() {
        let mut codes: Vec<&str> = Vec::new();
        for signal in EscalationSignal::all() {
            let encoded =
                serde_json::to_string(&signal).unwrap_or_else(|error| panic!("serialize: {error}"));
            assert_eq!(encoded, format!("\"{}\"", signal.code()));
            let decoded: EscalationSignal = serde_json::from_str(&encoded)
                .unwrap_or_else(|error| panic!("deserialize: {error}"));
            assert_eq!(decoded, signal);
            assert!(
                !codes.contains(&signal.code()),
                "{} must be distinct from every other code",
                signal.code()
            );
            codes.push(signal.code());
        }
        assert_eq!(codes.len(), EscalationSignal::all().len());
    }

    /// **No signal lowers risk: every escalation is above the minimum, and none exceeds the maximum.**
    ///
    /// The property that makes the escalation safe in one direction. A signal that mapped to `Minimal`
    /// would be one an operator could name to *reduce* scrutiny, which is the inverse of what the set is
    /// for — and a single hand-picked case would not catch it.
    #[test]
    fn every_signal_raises_risk_within_the_closed_range() {
        for signal in EscalationSignal::all() {
            let raised = signal.escalation();
            assert!(
                raised > Risk::Minimal,
                "{signal} must raise risk above the minimum, not leave it unchanged"
            );
            assert!(
                raised <= Risk::High,
                "{signal} must not exceed the maximum risk level"
            );
        }
    }

    /// The two signals the guidance table puts at the risk-3 posture are the two that reach `High`.
    ///
    /// Asserted by name rather than by a count, because "mass-send is risk 3" is a documented claim and a
    /// test that only counted the high signals would pass if the wrong two were chosen.
    #[test]
    fn the_documented_high_signals_are_the_ones_that_reach_high() {
        assert_eq!(EscalationSignal::Bulk.escalation(), Risk::High);
        assert_eq!(EscalationSignal::Sensitive.escalation(), Risk::High);
        assert_eq!(EscalationSignal::External.escalation(), Risk::Moderate);
        assert_eq!(EscalationSignal::Production.escalation(), Risk::Moderate);
    }
}
