//! Baseline risk as a typed level, with the effect floor expressed as a number.
//!
//! # Why this lives in `jarvis-core`
//!
//! Risk is **one vocabulary with one meaning** that two adapter crates both need and neither may
//! depend on the other for. `docs/architecture/repository-layout.md`'s dependency graph allows an
//! adapter to depend on `jarvis-core` and `jarvis-protocol` and **not on another adapter**, so
//! `jarvis-storage` — which parses an operator's configured risk ceiling — cannot see
//! `jarvis-tools`'s definition of it. The alternative to moving the type would be a second risk
//! vocabulary in the storage crate, which is the defect class this repository removes: two values
//! that must agree with nothing holding both.
//!
//! This is the same reasoning that put [`Sensitivity`](crate::Sensitivity) in core rather than in
//! `jarvis-tools`. A risk level means the same thing wherever it is read.
//!
//! # Why a type rather than a number
//!
//! `docs/architecture/tools-and-connectors.md` gives risk four levels with a named posture each
//! ("auto when scoped", "fresh approval", "always ask or deny"). Those postures are decisions a
//! caller makes from the level, so the level is the thing that must be closed — a `u8` risk would
//! carry `7`, and [`Risk::for_approval`] would silently treat it as level 3 while any table-driven
//! code that matches on `0..=3` would not.
//!
//! # Why the floor check takes a number
//!
//! [`Risk::declared_for`] takes the floor rather than an effect set, because an effect set is
//! `jarvis-tools`'s type and this crate cannot name it. The rule has not moved: `jarvis-tools`
//! passes `EffectSet::risk_floor()`, and it is still the case that the direction which matters is
//! *under*-declaration — a tool claiming risk 0 while spending money would be run with the posture
//! for reading a calendar. Over-declaration is permitted and deliberately so.

use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The highest risk level.
pub const MAX_RISK_LEVEL: u8 = 3;

/// Explains why a declared risk was rejected.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum RiskError {
    /// The level was outside `0..=MAX_RISK_LEVEL`.
    #[error("risk must be between 0 and {MAX_RISK_LEVEL}")]
    OutOfRange,
    /// The declared risk was below what the declared effects require.
    #[error("risk {declared} is below the risk {required} the declared effects require")]
    BelowEffectFloor {
        /// The risk the tool declared.
        declared: u8,
        /// The minimum the declared effects allow.
        required: u8,
    },
}

/// A baseline risk level, from 0 (least consequential) to 3 (most).
///
/// The variant names are descriptive rather than ordinal so a reading of `Risk::Level2` carries what
/// it means; the ordering is available through [`Self::level`] and the `Ord` implementation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    /// Read-only, scoped operations. Auto when scoped.
    Minimal,
    /// Reversible local changes. Auto or ask by workspace policy.
    Low,
    /// A single outward effect: sending, booking, modifying a shared record. Fresh approval.
    Moderate,
    /// Deletion, spending, deployment, permission changes, mass-send. Always ask or deny.
    High,
}

impl Risk {
    /// Returns the risk for a numeric level.
    ///
    /// # Errors
    ///
    /// Returns [`RiskError::OutOfRange`] for a level above [`MAX_RISK_LEVEL`], so a corrupted or
    /// hostile stored value cannot become a level nothing has a posture for.
    pub const fn from_level(level: u8) -> Result<Self, RiskError> {
        match level {
            0 => Ok(Self::Minimal),
            1 => Ok(Self::Low),
            2 => Ok(Self::Moderate),
            3 => Ok(Self::High),
            _ => Err(RiskError::OutOfRange),
        }
    }

    /// Returns the numeric level.
    #[must_use]
    pub const fn level(self) -> u8 {
        match self {
            Self::Minimal => 0,
            Self::Low => 1,
            Self::Moderate => 2,
            Self::High => 3,
        }
    }

    /// Returns the stable wire and storage name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Moderate => "moderate",
            Self::High => "high",
        }
    }

    /// Returns every level, from least to most consequential.
    #[must_use]
    pub const fn all() -> [Self; 4] {
        [Self::Minimal, Self::Low, Self::Moderate, Self::High]
    }

    /// Returns the approval posture this level implies by default.
    ///
    /// The guidance table in `docs/architecture/tools-and-connectors.md`: risk 0 is auto when
    /// scoped, risk 1 is auto or ask by workspace policy, risk 2 is fresh approval, and risk 3 is
    /// always-ask or deny. This is a **default a tool may tighten**, not a ceiling it may lower.
    ///
    /// Takes [`Risk`] rather than a number so the last arm cannot be a silent catch-all: a
    /// `_ => Self::Ask` would be correct for level 3 and would also be the answer for level 7, which
    /// no table row covers.
    #[must_use]
    pub const fn for_approval(self) -> crate::ApprovalPolicy {
        match self {
            Self::Minimal => crate::ApprovalPolicy::Auto,
            Self::Low => crate::ApprovalPolicy::Policy,
            Self::Moderate | Self::High => crate::ApprovalPolicy::Ask,
        }
    }

    /// Validates a declared risk against the floor the declared effects require.
    ///
    /// # Errors
    ///
    /// Returns [`RiskError::OutOfRange`] when the level does not name a risk, or
    /// [`RiskError::BelowEffectFloor`] when the level is below `floor`.
    ///
    /// The `declared` argument is a `u8` rather than a [`Risk`] because the two failure modes are
    /// different errors: a level no risk names is a malformed declaration, and a level that names a
    /// risk but hides an effect is an inconsistent one. Accepting the raw number lets this function
    /// report which of the two happened.
    ///
    /// `floor` arrives as a number rather than as an effect set because effect sets belong to
    /// `jarvis-tools` and this crate cannot name them — see the module documentation. The caller
    /// supplies `EffectSet::risk_floor()`, so the rule still has exactly one home.
    pub fn declared_for(declared: u8, floor: u8) -> Result<Self, RiskError> {
        let risk = Self::from_level(declared)?;
        if declared < floor {
            return Err(RiskError::BelowEffectFloor {
                declared,
                required: floor,
            });
        }
        Ok(risk)
    }
}

impl fmt::Display for Risk {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every level round-trips through its number and its name.
    #[test]
    fn levels_round_trip() {
        for level in 0..=MAX_RISK_LEVEL {
            let risk =
                Risk::from_level(level).unwrap_or_else(|error| panic!("level {level}: {error}"));
            assert_eq!(risk.level(), level);
            let encoded =
                serde_json::to_string(&risk).unwrap_or_else(|error| panic!("serialize: {error}"));
            assert_eq!(encoded, format!("\"{}\"", risk.as_str()));
            let decoded: Risk = serde_json::from_str(&encoded)
                .unwrap_or_else(|error| panic!("deserialize: {error}"));
            assert_eq!(decoded, risk);
        }
        assert_eq!(Risk::all().len(), usize::from(MAX_RISK_LEVEL) + 1);
    }

    /// A level above the maximum is refused rather than saturated.
    ///
    /// Saturation would be the dangerous choice: `for_approval(7)` behaves as level 3, so a tool
    /// declaring 7 would get the strictest posture by accident while every table lookup missed.
    #[test]
    fn a_level_above_the_maximum_is_refused() {
        for level in [4, 5, 255] {
            assert_eq!(Risk::from_level(level), Err(RiskError::OutOfRange));
        }
    }

    /// Ordering follows the level, so a comparison is a real comparison.
    #[test]
    fn ordering_follows_the_level() {
        assert!(Risk::Minimal < Risk::High);
        assert!(Risk::Low <= Risk::Low);
        let mut levels: Vec<u8> = [Risk::High, Risk::Minimal, Risk::Moderate, Risk::Low]
            .iter()
            .map(|risk| risk.level())
            .collect();
        levels.sort_unstable();
        assert_eq!(levels, vec![0, 1, 2, 3]);
    }

    /// **A declared risk below the floor is refused, and at the floor it is accepted.**
    ///
    /// The two halves are one claim with two directions, so both are asserted: an implementation
    /// that refused everything would satisfy the refusal assertions alone, which is the shape a
    /// missing control hides behind.
    #[test]
    fn a_declared_risk_is_refused_below_the_floor_and_accepted_at_it() {
        for declared in 0..3 {
            assert_eq!(
                Risk::declared_for(declared, 3),
                Err(RiskError::BelowEffectFloor {
                    declared,
                    required: 3
                }),
                "risk {declared} must not be accepted below a floor of 3"
            );
        }
        assert_eq!(
            Risk::declared_for(3, 3),
            Ok(Risk::High),
            "the floor itself is acceptable"
        );
    }

    /// Over-declaration is permitted, because context can raise risk.
    ///
    /// The reverse of the previous test, and it has to pass: without it the guard could be
    /// implemented as equality and the intent to *permit* extra scrutiny would be lost.
    #[test]
    fn a_declared_risk_above_the_floor_is_permitted() {
        for declared in 1..=MAX_RISK_LEVEL {
            assert_eq!(
                Risk::declared_for(declared, 1),
                Risk::from_level(declared),
                "risk {declared} is more scrutiny than required and must be allowed"
            );
        }
        assert_eq!(
            Risk::declared_for(0, 1),
            Err(RiskError::BelowEffectFloor {
                declared: 0,
                required: 1
            }),
            "but under-declaration is not"
        );
    }

    /// An out-of-range number is reported as out-of-range, not as an inconsistent declaration.
    ///
    /// The two errors are distinguishable, which is the reason `declared_for` takes a number.
    #[test]
    fn an_out_of_range_number_is_reported_as_such() {
        assert_eq!(
            Risk::declared_for(9, 0),
            Err(RiskError::OutOfRange),
            "out of range must not be reported as below-the-floor"
        );
    }

    /// The guidance table's own mapping, asserted level by level.
    #[test]
    fn the_default_posture_follows_the_guidance_table() {
        assert_eq!(Risk::Minimal.for_approval(), crate::ApprovalPolicy::Auto);
        assert_eq!(Risk::Low.for_approval(), crate::ApprovalPolicy::Policy);
        assert_eq!(Risk::Moderate.for_approval(), crate::ApprovalPolicy::Ask);
        assert_eq!(Risk::High.for_approval(), crate::ApprovalPolicy::Ask);
    }
}
