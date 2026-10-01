//! Whether a human decides before a tool runs, and what it means to tighten that.
//!
//! # Why this lives beside [`Risk`](crate::Risk) in `jarvis-core`
//!
//! Same reason as the risk level: it is **one vocabulary with one meaning** that the tool registry
//! and the configuration layer both need, and those are two adapter crates that may not depend on
//! each other. `docs/architecture/repository-layout.md` allows an adapter to depend on `jarvis-core`
//! and **not on another adapter**, so an operator's configured approval override cannot be expressed
//! in `jarvis-tools`'s type from the storage crate. The alternative — a second policy vocabulary in
//! storage — is two values that must agree with nothing holding both.
//!
//! # Why `Policy` is a real member
//!
//! `Policy` is **not** a synonym for `Ask`: it means "the workspace's policy decides from risk,
//! effect, and context", while `Ask` means "always ask regardless of policy". Collapsing them would
//! make a tool that must always ask indistinguishable from one whose risk happens to require asking
//! today.
//!
//! # Why the ordering exists
//!
//! [`ApprovalPolicy::strictness`] is the mechanism behind the one direction an override may move.
//! `docs/adr/0017-policy-evaluation-outcomes-and-ownership.md` rejects letting workspace policy
//! relax a tool's own approval policy, because a workspace setting would then be a way to remove a
//! guard a tool author put in place. An override can therefore only ever move **toward** scrutiny,
//! and a total order is what makes "toward" a comparison rather than a paragraph.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Explains why an approval policy name was rejected.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ApprovalPolicyError {
    /// The text did not name a policy.
    #[error("the approval policy must be one of auto, policy, ask, or deny")]
    Unknown,
}

/// Whether a human decides before the tool runs.
///
/// The variants are ordered by [`Self::strictness`], not by declaration order, so a comparison of
/// two policies is a comparison of how much scrutiny they demand.
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

    /// Returns how much scrutiny this policy demands, as a total order.
    ///
    /// **This is the direction rule's mechanism.** An operator or workspace setting may only move a
    /// tool's declared policy toward a *higher* value here: `Auto → Policy → Ask → Deny`. Moving the
    /// other way would let a workspace setting relax a guard the tool author declared, which
    /// `ADR-0017` rejects by name.
    ///
    /// The gaps between the numbers are deliberate. `Policy` sits strictly above `Auto` because it
    /// defers the question to a workspace threshold that may ask; it sits strictly below `Ask`
    /// because `Ask` holds the call whatever the threshold says. A reader who assumed `Policy` and
    /// `Ask` were interchangeable would get a comparison that admits a relaxation, so the ordering
    /// states the distinction the enum's own documentation makes.
    #[must_use]
    pub const fn strictness(self) -> u8 {
        match self {
            Self::Auto => 0,
            Self::Policy => 1,
            Self::Ask => 2,
            Self::Deny => 3,
        }
    }

    /// Returns the more restrictive of two policies.
    ///
    /// A `max`, never a replacement, so an override can only tighten. Named rather than left as an
    /// inline comparison at the call site, because "which direction did this go" is exactly the
    /// question a reader of a security rule gets wrong — the predicate reads correct in both
    /// directions and only a concrete case settles it.
    #[must_use]
    pub const fn tighter(self, other: Self) -> Self {
        if self.strictness() >= other.strictness() {
            self
        } else {
            other
        }
    }

    /// Returns every policy, from least to most restrictive.
    #[must_use]
    pub const fn all() -> [Self; 4] {
        [Self::Auto, Self::Policy, Self::Ask, Self::Deny]
    }
}

impl fmt::Display for ApprovalPolicy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for ApprovalPolicy {
    type Err = ApprovalPolicyError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "auto" => Ok(Self::Auto),
            "policy" => Ok(Self::Policy),
            "ask" => Ok(Self::Ask),
            "deny" => Ok(Self::Deny),
            _ => Err(ApprovalPolicyError::Unknown),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every policy round-trips through its name, and `Display` agrees with `FromStr`.
    #[test]
    fn policies_round_trip_through_their_names() {
        for policy in ApprovalPolicy::all() {
            assert_eq!(
                policy.as_str().parse::<ApprovalPolicy>(),
                Ok(policy),
                "{policy} must parse back to itself"
            );
            let encoded =
                serde_json::to_string(&policy).unwrap_or_else(|error| panic!("serialize: {error}"));
            assert_eq!(encoded, format!("\"{}\"", policy.as_str()));
        }
    }

    /// An unknown name is refused rather than defaulted.
    ///
    /// A default here would be the dangerous direction: the likely typo is a policy an operator
    /// meant to *tighten*, and defaulting an unknown value to `Auto` would silently loosen it.
    #[test]
    fn an_unknown_name_is_refused() {
        for value in ["", "AUTO", "always", "manual", "auto "] {
            assert_eq!(
                value.parse::<ApprovalPolicy>(),
                Err(ApprovalPolicyError::Unknown),
                "{value:?} must not parse"
            );
        }
    }

    /// **`Policy` is not `Ask`, and the ordering says so in both directions.**
    ///
    /// The distinction the enum's documentation calls a real member. If `strictness` collapsed the
    /// two, an override from `Policy` to `Ask` would be a no-op and an override from `Ask` to
    /// `Policy` would be *permitted* — a relaxation, through a comparison nobody would read as one.
    #[test]
    fn policy_and_ask_are_strictly_ordered() {
        assert!(ApprovalPolicy::Policy.strictness() < ApprovalPolicy::Ask.strictness());
        assert!(ApprovalPolicy::Auto.strictness() < ApprovalPolicy::Policy.strictness());
        assert!(ApprovalPolicy::Ask.strictness() < ApprovalPolicy::Deny.strictness());
        assert_eq!(
            ApprovalPolicy::Policy.tighter(ApprovalPolicy::Ask),
            ApprovalPolicy::Ask
        );
        assert_eq!(
            ApprovalPolicy::Ask.tighter(ApprovalPolicy::Policy),
            ApprovalPolicy::Ask
        );
    }

    /// **`tighter` never returns a less restrictive policy, for any pair.**
    ///
    /// A sweep rather than a case, because the property is "in every combination" and the rule it
    /// protects is the one `ADR-0017` rejects relaxing. A mutation that inverted the comparison
    /// would pass a single hand-picked pair asserted the wrong way round.
    #[test]
    fn tighter_never_relaxes() {
        for left in ApprovalPolicy::all() {
            for right in ApprovalPolicy::all() {
                let result = left.tighter(right);
                assert!(
                    result.strictness() >= left.strictness()
                        && result.strictness() >= right.strictness(),
                    "{left}.tighter({right}) produced {result}, which is less restrictive"
                );
            }
        }
    }

    /// `Deny` is the only policy that cannot run, and it is the strictest.
    #[test]
    fn deny_is_the_only_unrunnable_policy() {
        for policy in ApprovalPolicy::all() {
            assert_eq!(
                policy.is_runnable(),
                policy != ApprovalPolicy::Deny,
                "{policy} runnability"
            );
        }
        let strictest = ApprovalPolicy::all()
            .into_iter()
            .max_by_key(|policy| policy.strictness())
            .unwrap_or(ApprovalPolicy::Deny);
        assert_eq!(strictest, ApprovalPolicy::Deny);
    }
}
