//! Baseline risk as a typed level, with the effect floor enforced at construction.
//!
//! # Why a type rather than a number
//!
//! `docs/architecture/tools-and-connectors.md` gives risk four levels with a named posture each
//! ("auto when scoped", "fresh approval", "always ask or deny"). Those postures are decisions a
//! caller makes from the level, so the level is the thing that must be closed — a `u8` risk would
//! carry `7`, and [`crate::ApprovalPolicy::for_risk`] would silently treat it as level 3 while any
//! table-driven code that matches on `0..=3` would not.
//!
//! # Why the floor is enforced here
//!
//! Effect classes determine a **minimum** risk ([`EffectSet::risk_floor`]): a `financial` tool cannot
//! be risk 0, whatever its author declares. If the declared risk and the declared effects disagree,
//! the pair is a lie about what the tool does, and the direction that matters is *under*-declaration:
//! a tool claiming risk 0 while spending money would be run with the posture for reading a calendar.
//!
//! Over-declaration is permitted, and deliberately: the same effect can warrant different scrutiny
//! in different contexts, so a connector may declare risk 2 on a `write` whose floor is 1. Context
//! can raise risk but cannot lower a hard floor, and this is that rule made unrepresentable-if-broken.

use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::effect::EffectSet;

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

    /// Validates a declared risk against the effects the tool declares.
    ///
    /// # Errors
    ///
    /// Returns [`RiskError::OutOfRange`] when the level does not name a risk, or
    /// [`RiskError::BelowEffectFloor`] when the level is below [`EffectSet::risk_floor`].
    ///
    /// The `declared` argument is a `u8` rather than a [`Risk`] because the two failure modes are
    /// different errors: a level no risk names is a malformed declaration, and a level that names a
    /// risk but hides an effect is an inconsistent one. Accepting the raw number lets this function
    /// report which of the two happened.
    pub fn declared_for(declared: u8, effects: &EffectSet) -> Result<Self, RiskError> {
        let risk = Self::from_level(declared)?;
        let required = effects.risk_floor();
        if declared < required {
            return Err(RiskError::BelowEffectFloor { declared, required });
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
    use crate::effect::ToolEffect;

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
    }

    /// A level above the maximum is refused rather than saturated.
    ///
    /// Saturation would be the dangerous choice: `for_risk(7)` behaves as level 3, so a tool
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

    /// **The falsification test: a declared risk below the effect floor is refused.**
    ///
    /// `docs/architecture/tools-and-connectors.md` lists `financial` under the risk-3 posture
    /// ("delete, spend, deploy"), so a tool that can move money and declares risk 0 must not be
    /// representable. Without this guard the declaration is what policy believes, and policy would
    /// run the spend with the posture for reading a calendar.
    #[test]
    fn a_declared_risk_below_the_effect_floor_is_refused() {
        let spending = EffectSet::single(ToolEffect::Financial);
        assert_eq!(spending.risk_floor(), 3);

        for declared in 0..3 {
            assert_eq!(
                Risk::declared_for(declared, &spending),
                Err(RiskError::BelowEffectFloor {
                    declared,
                    required: 3
                }),
                "risk {declared} must not be accepted for a financial tool"
            );
        }
        assert_eq!(
            Risk::declared_for(3, &spending),
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
        let writing = EffectSet::single(ToolEffect::Write);
        assert_eq!(writing.risk_floor(), 1);

        for declared in 1..=MAX_RISK_LEVEL {
            assert_eq!(
                Risk::declared_for(declared, &writing),
                Risk::from_level(declared),
                "risk {declared} is more scrutiny than required and must be allowed"
            );
        }
        assert_eq!(
            Risk::declared_for(0, &writing),
            Err(RiskError::BelowEffectFloor {
                declared: 0,
                required: 1
            }),
            "but under-declaration is not"
        );
    }

    /// A read-only tool may declare any level from 0 up.
    #[test]
    fn a_read_only_tool_may_declare_from_zero() {
        let reading = EffectSet::single(ToolEffect::ReadOnly);
        assert_eq!(reading.risk_floor(), 0);
        for declared in 0..=MAX_RISK_LEVEL {
            assert!(Risk::declared_for(declared, &reading).is_ok());
        }
    }

    /// An out-of-range number is reported as out-of-range, not as an inconsistent declaration.
    ///
    /// The two errors are distinguishable, which is the reason `declared_for` takes a number.
    #[test]
    fn an_out_of_range_number_is_reported_as_such() {
        let reading = EffectSet::single(ToolEffect::ReadOnly);
        assert_eq!(
            Risk::declared_for(9, &reading),
            Err(RiskError::OutOfRange),
            "out of range must not be reported as below-the-floor"
        );
    }

    /// A composable set takes the floor from its highest effect.
    #[test]
    fn the_floor_comes_from_the_strongest_effect() {
        let mixed = EffectSet::new([
            ToolEffect::ReadOnly,
            ToolEffect::Write,
            ToolEffect::ExternalCommunication,
        ])
        .unwrap_or_else(|| panic!("non-empty"));
        assert_eq!(
            Risk::declared_for(1, &mixed),
            Err(RiskError::BelowEffectFloor {
                declared: 1,
                required: 2
            })
        );
        assert_eq!(Risk::declared_for(2, &mixed), Ok(Risk::Moderate));
    }
}
