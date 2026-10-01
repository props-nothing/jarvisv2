//! Baseline risk, re-exported from `jarvis-core` with the effect floor bound here.
//!
//! # Why the type is not defined here
//!
//! `Risk` moved to `jarvis-core` because the **configuration layer** needs to name a risk ceiling
//! in an operator's document, and `docs/architecture/repository-layout.md`'s dependency graph
//! allows an adapter to depend on `jarvis-core` and **not on another adapter**. `jarvis-storage`
//! therefore cannot see a risk type defined in `jarvis-tools`, and the only alternative would have
//! been a second risk vocabulary in storage — two values that must agree with nothing holding both,
//! which is the defect class this repository removes.
//!
//! # What stays here
//!
//! The **floor binding**. Core's [`Risk::declared_for`] takes the floor as a number because an
//! effect set is this crate's type; [`declared_for_effects`] is the one call site that supplies it
//! from [`EffectSet::risk_floor`], so the rule that "a financial tool cannot be risk 0" still has
//! exactly one home in the crate that owns effects.
//!
//! # Why the floor is enforced at all
//!
//! Effect classes determine a **minimum** risk ([`EffectSet::risk_floor`]): a `financial` tool cannot
//! be risk 0, whatever its author declares. If the declared risk and the declared effects disagree,
//! the pair is a lie about what the tool does, and the direction that matters is *under*-declaration:
//! a tool claiming risk 0 while spending money would be run with the posture for reading a calendar.
//!
//! Over-declaration is permitted, and deliberately: the same effect can warrant different scrutiny
//! in different contexts, so a connector may declare risk 2 on a `write` whose floor is 1. Context
//! can raise risk but cannot lower a hard floor, and this is that rule made unrepresentable-if-broken.

use crate::effect::EffectSet;

// Re-exported so every existing `crate::Risk`, `crate::MAX_RISK_LEVEL`, and `crate::RiskError`
// path keeps resolving, and so a caller in this crate never has to name `jarvis_core` for a value
// the crate's own public API promises.
pub use jarvis_core::{MAX_RISK_LEVEL, Risk, RiskError};

/// Validates a declared risk against the floor the supplied effects require.
///
/// # Errors
///
/// Returns [`RiskError::OutOfRange`] when the level does not name a risk, or
/// [`RiskError::BelowEffectFloor`] when the level is below [`EffectSet::risk_floor`].
///
/// This is the crate's single binding of the floor to a risk declaration. It exists rather than
/// calling [`Risk::declared_for`] directly at each site so that the argument order — declared level,
/// then derived floor — is written once, because both are numbers and a transposition would compare
/// a declared risk against itself and pass.
pub fn declared_for_effects(declared: u8, effects: &EffectSet) -> Result<Risk, RiskError> {
    Risk::declared_for(declared, effects.risk_floor())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effect::ToolEffect;

    /// **The falsification test: a declared risk below the effect floor is refused.**
    ///
    /// `docs/architecture/tools-and-connectors.md` lists `financial` under the risk-3 posture
    /// ("delete, spend, deploy"), so a tool that can move money and declares risk 0 must not be
    /// representable. Without this guard the declaration is what policy believes, and policy would
    /// run the spend with the posture for reading a calendar.
    ///
    /// This asserts the **binding** rather than the rule itself: the rule is `jarvis-core`'s and is
    /// tested there, and what is testable here is that the floor arrived from the effects.
    #[test]
    fn a_declared_risk_below_the_effect_floor_is_refused() {
        let spending = EffectSet::single(ToolEffect::Financial);
        assert_eq!(spending.risk_floor(), 3);

        for declared in 0..3 {
            assert_eq!(
                declared_for_effects(declared, &spending),
                Err(RiskError::BelowEffectFloor {
                    declared,
                    required: 3
                }),
                "risk {declared} must not be accepted for a financial tool"
            );
        }
        assert_eq!(
            declared_for_effects(3, &spending),
            Ok(Risk::High),
            "the floor itself is acceptable"
        );
    }

    /// A composable set takes the floor from its strongest effect.
    #[test]
    fn the_floor_comes_from_the_strongest_effect() {
        let mixed = EffectSet::new([
            ToolEffect::ReadOnly,
            ToolEffect::Write,
            ToolEffect::ExternalCommunication,
        ])
        .unwrap_or_else(|| panic!("non-empty"));
        assert_eq!(
            declared_for_effects(1, &mixed),
            Err(RiskError::BelowEffectFloor {
                declared: 1,
                required: 2
            })
        );
        assert_eq!(declared_for_effects(2, &mixed), Ok(Risk::Moderate));
    }

    /// A read-only tool may declare any level from 0 up, because its floor is 0.
    #[test]
    fn a_read_only_tool_may_declare_from_zero() {
        let reading = EffectSet::single(ToolEffect::ReadOnly);
        assert_eq!(reading.risk_floor(), 0);
        for declared in 0..=MAX_RISK_LEVEL {
            assert!(declared_for_effects(declared, &reading).is_ok());
        }
    }
}
