//! Effect classes: what a tool could do if it runs.
//!
//! `docs/architecture/tools-and-connectors.md` makes effects "independent and composable", so this
//! is a **set**, not an enum with a maximum. A tool that reads a document, sends a summary, and
//! charges for it is all three, and collapsing that to "the worst one" loses the fact that it also
//! reads — which is what a reviewer needs in order to understand the capability.

use std::collections::BTreeSet;
use std::fmt;

use serde::{Deserialize, Serialize};

/// What a tool could do if it runs.
///
/// The wire and stored form is the lowercase name, spelled once in [`Self::as_str`] so a derive
/// attribute cannot silently change it.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolEffect {
    /// Reads data and changes nothing.
    ReadOnly,
    /// Writes local or provider state that can be reversed.
    Write,
    /// Sends something to a party outside JARVIS.
    ExternalCommunication,
    /// Destroys or irreversibly alters data.
    Destructive,
    /// Executes code.
    CodeExecution,
    /// Moves or commits money.
    Financial,
    /// Changes permissions, identity, or access.
    Privileged,
}

impl ToolEffect {
    /// Returns the stable wire and storage name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::Write => "write",
            Self::ExternalCommunication => "external_communication",
            Self::Destructive => "destructive",
            Self::CodeExecution => "code_execution",
            Self::Financial => "financial",
            Self::Privileged => "privileged",
        }
    }

    /// Returns whether this effect can change the world at all.
    ///
    /// `read_only` is the only one that cannot. Everything else can, and the distinction is what
    /// lets a caller treat "may this be retried blindly" and "does this need a receipt" differently
    /// from a pure read.
    #[must_use]
    pub const fn is_mutating(self) -> bool {
        !matches!(self, Self::ReadOnly)
    }

    /// Returns the **minimum** baseline risk this effect can carry.
    ///
    /// A floor, not a value: a tool that only reads is risk 0, but a tool that reads one record and
    /// returns it is different from one that reads every record in a workspace, and that difference
    /// is expressed by the tool's declared risk. What this prevents is a tool declaring itself
    /// `read_only` while requesting risk that hides an effect it has — the declaration and the risk
    /// have to be consistent.
    #[must_use]
    // The three risk-3 effects share a floor but not a meaning. Merging them into one arm would
    // need a helper that returned "3" for a list of unrelated classes, which reads as though the
    // grouping were principled. The match is exhaustive per effect on purpose; the comment on
    // `CodeExecution` explains why it is separate.
    #[allow(clippy::match_same_arms)]
    pub const fn risk_floor(self) -> u8 {
        match self {
            Self::ReadOnly => 0,
            Self::Write => 1,
            Self::ExternalCommunication => 2,
            Self::Destructive => 3,
            Self::Financial => 3,
            Self::Privileged => 3,
            // Executing code is risk 3 by default because its effects are not knowable from the
            // contract: the code decides. A sandbox does not lower this floor; `P3-011` owns the
            // sandbox, and a sandboxed executor still declares its worst case.
            Self::CodeExecution => 3,
        }
    }

    /// Returns every effect, ordered from least to most consequential.
    #[must_use]
    pub const fn all() -> [Self; 7] {
        [
            Self::ReadOnly,
            Self::Write,
            Self::ExternalCommunication,
            Self::CodeExecution,
            Self::Destructive,
            Self::Financial,
            Self::Privileged,
        ]
    }
}

impl fmt::Display for ToolEffect {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A non-empty set of effect classes.
///
/// Non-empty is the whole point: a tool with no declared effect would be a tool whose consequences
/// nobody stated, and policy cannot reason about "effects: unknown". An empty set is therefore not
/// representable rather than merely discouraged.
///
/// Serialization is derived but **deserialization is not**, because a derived `Deserialize` would
/// make an empty array representable — the one value this type exists to exclude. `try_from` routes
/// the wire form through [`EffectSet::new`], so an empty list is refused during parsing rather than
/// producing an effect-free tool.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(into = "Vec<ToolEffect>")]
pub struct EffectSet {
    effects: BTreeSet<ToolEffect>,
}

impl From<EffectSet> for Vec<ToolEffect> {
    fn from(set: EffectSet) -> Self {
        set.to_vec()
    }
}

impl<'de> Deserialize<'de> for EffectSet {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let effects = Vec::<ToolEffect>::deserialize(deserializer)?;
        Self::new(effects)
            .ok_or_else(|| serde::de::Error::custom("an effect set must not be empty"))
    }
}

impl EffectSet {
    /// Creates a set from at least one effect.
    ///
    /// A `BTreeSet` rather than a `Vec`: the set is a set, so a duplicate declaration is not a
    /// second effect, and iteration order is stable without the caller having to sort.
    #[must_use]
    pub fn new(effects: impl IntoIterator<Item = ToolEffect>) -> Option<Self> {
        let effects: BTreeSet<ToolEffect> = effects.into_iter().collect();
        (!effects.is_empty()).then_some(Self { effects })
    }

    /// Creates a set holding exactly one effect.
    #[must_use]
    pub fn single(effect: ToolEffect) -> Self {
        let mut effects = BTreeSet::new();
        effects.insert(effect);
        Self { effects }
    }

    /// Returns whether the set contains an effect.
    #[must_use]
    pub fn contains(&self, effect: ToolEffect) -> bool {
        self.effects.contains(&effect)
    }

    /// Returns whether any effect in the set can change the world.
    #[must_use]
    pub fn is_mutating(&self) -> bool {
        self.effects.iter().any(|effect| effect.is_mutating())
    }

    /// Returns the highest risk floor among the declared effects.
    ///
    /// The maximum rather than a sum: two risk-1 effects are not risk 2. Risk is a level of
    /// scrutiny, and a tool that writes and then sends a message still needs the scrutiny that
    /// sending requires — not more, and not less.
    #[must_use]
    pub fn risk_floor(&self) -> u8 {
        self.effects
            .iter()
            .map(|effect| effect.risk_floor())
            .max()
            .unwrap_or(0)
    }

    /// Returns the effects, ordered from least to most consequential.
    pub fn iter(&self) -> impl Iterator<Item = ToolEffect> + '_ {
        self.effects.iter().copied()
    }

    /// Returns the declared effects as a slice-like collection.
    #[must_use]
    pub fn to_vec(&self) -> Vec<ToolEffect> {
        self.effects.iter().copied().collect()
    }
}

impl fmt::Display for EffectSet {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
        for effect in self.iter() {
            if !first {
                formatter.write_str(",")?;
            }
            formatter.write_str(effect.as_str())?;
            first = false;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every effect has a distinct name, so a stored name maps back to exactly one effect.
    #[test]
    fn names_are_unique_and_round_trip() {
        let mut names: Vec<&str> = ToolEffect::all().iter().map(|e| e.as_str()).collect();
        let total = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), total, "two effects share a name");

        for effect in ToolEffect::all() {
            let encoded =
                serde_json::to_string(&effect).unwrap_or_else(|error| panic!("serialize: {error}"));
            assert_eq!(
                encoded,
                format!("\"{}\"", effect.as_str()),
                "the wire form must be the stored form"
            );
            let decoded: ToolEffect = serde_json::from_str(&encoded)
                .unwrap_or_else(|error| panic!("deserialize: {error}"));
            assert_eq!(decoded, effect);
        }
    }

    /// Only `read_only` is non-mutating, which is the property retry and receipt logic depends on.
    #[test]
    fn only_reading_is_non_mutating() {
        for effect in ToolEffect::all() {
            assert_eq!(
                effect.is_mutating(),
                effect != ToolEffect::ReadOnly,
                "{effect} has the wrong mutation classification"
            );
        }
    }

    /// An empty effect set is not representable.
    #[test]
    fn an_empty_effect_set_cannot_be_built() {
        assert!(EffectSet::new(Vec::new()).is_none());
        assert!(EffectSet::new([ToolEffect::ReadOnly]).is_some());
    }

    /// A duplicate declaration is one effect, not two.
    #[test]
    fn duplicates_collapse() {
        let set = EffectSet::new([ToolEffect::Write, ToolEffect::Write])
            .unwrap_or_else(|| panic!("non-empty"));
        assert_eq!(set.to_vec(), vec![ToolEffect::Write]);
    }

    /// The risk floor is the **maximum** of the declared effects, not a sum.
    #[test]
    fn the_risk_floor_is_a_maximum_not_a_sum() {
        let write_and_send = EffectSet::new([ToolEffect::Write, ToolEffect::ExternalCommunication])
            .unwrap_or_else(|| panic!("non-empty"));
        // Write is 1 and external communication is 2; the floor is 2, not 3.
        assert_eq!(write_and_send.risk_floor(), 2);

        let everything = EffectSet::new([
            ToolEffect::ReadOnly,
            ToolEffect::Write,
            ToolEffect::ExternalCommunication,
            ToolEffect::Destructive,
        ])
        .unwrap_or_else(|| panic!("non-empty"));
        assert_eq!(
            everything.risk_floor(),
            3,
            "one risk-3 effect makes the whole tool risk 3"
        );
    }

    /// Reading is floor 0, so a read-only tool can be declared at the lowest risk.
    #[test]
    fn reading_has_the_lowest_floor() {
        assert_eq!(EffectSet::single(ToolEffect::ReadOnly).risk_floor(), 0);
        assert!(!EffectSet::single(ToolEffect::ReadOnly).is_mutating());
    }

    /// Code execution is floor 3 without a sandbox argument, because the code decides its effects.
    #[test]
    fn code_execution_is_high_risk_by_default() {
        assert_eq!(ToolEffect::CodeExecution.risk_floor(), 3);
    }
}
