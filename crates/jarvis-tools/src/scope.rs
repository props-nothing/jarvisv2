//! Capability scopes: the grants a tool requires before it may run.
//!
//! # These are JARVIS scopes, not provider OAuth scopes
//!
//! The distinction is load-bearing and easy to get wrong. `docs/architecture/tools-and-connectors.md`
//! gives a tool `required_scopes` = "actor/client/workspace grants", and separately gives a
//! **connector manifest** its "auth methods and required/optional scopes". Those are two different
//! vocabularies:
//!
//! - a JARVIS scope is a capability this project can grant an actor (`gmail.send`), evaluated
//!   against identity and workspace policy;
//! - a provider OAuth scope is a string the **provider** defines
//!   (`https://www.googleapis.com/auth/gmail.send`), attached to a connector's token and never
//!   something JARVIS can grant on its own.
//!
//! A tool contract naming a provider scope would be unsatisfiable: nothing in the JARVIS
//! authorization model could ever grant it, so the tool would be permanently unavailable. The
//! provider scope belongs to the connector's token lifecycle, which is `P5`'s; this type is the
//! capability vocabulary the policy engine evaluates.
//!
//! # Why a wildcard exists in the type
//!
//! Capability grants commonly include a whole resource (`files.*`). Whether a wildcard is *granted*
//! is a policy decision (`P3-003`), but the type has to be able to represent one: adding a variant
//! later would change a durable contract every stored grant is written against.

use std::collections::BTreeSet;
use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Maximum characters in a whole scope.
///
/// This is the **binding** bound, and the segment bound below is deliberately looser than half of it
/// so that it is. A total of 96 with two 40-character halves would have made this check unreachable —
/// the parser could never be handed a scope longer than 81 — and an unreachable limit is worse than
/// no limit: it reads as a protection while enforcing nothing. At 64, a legitimately long resource
/// with a short action is accepted and two maximal halves are refused.
pub const MAX_SCOPE_CHARS: usize = 64;

/// Maximum characters in a scope's resource or action half.
pub const MAX_SCOPE_SEGMENT_CHARS: usize = 40;

/// The action that covers every action on a resource.
pub const WILDCARD_ACTION: &str = "*";

/// Explains why a scope was rejected.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ScopeError {
    /// The scope was empty or whitespace-only.
    #[error("a scope must not be empty")]
    Empty,
    /// The scope exceeded the bounded length.
    #[error("a scope exceeds {MAX_SCOPE_CHARS} characters")]
    TooLong,
    /// The scope was not `resource.action`.
    #[error("a scope must be written resource.action")]
    NotQualified,
    /// A half was empty, too long, or contained a character outside the allowed set.
    #[error("the scope half {segment:?} is invalid")]
    Segment {
        /// Which half was rejected, without echoing the offending value.
        segment: &'static str,
    },
}

/// A capability grant: `resource.action`, where the action may be `*`.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct Scope {
    resource: String,
    action: String,
}

impl Scope {
    /// Parses and validates a scope.
    ///
    /// # Errors
    ///
    /// Returns [`ScopeError`] for an empty, oversized, unqualified, or malformed scope.
    pub fn new(value: impl Into<String>) -> Result<Self, ScopeError> {
        value.into().parse()
    }

    /// Builds a scope from its halves.
    ///
    /// # Errors
    ///
    /// Returns [`ScopeError`] for a malformed resource, or an action that is neither a valid action
    /// nor [`WILDCARD_ACTION`].
    pub fn from_parts(resource: &str, action: &str) -> Result<Self, ScopeError> {
        validate_simple(resource, "resource")?;
        // A wildcard action is not an action-shaped word, so it is accepted as its own case rather
        // than as a value of the general rule.
        if action != WILDCARD_ACTION {
            validate_simple(action, "action")?;
        }
        Ok(Self {
            resource: resource.to_owned(),
            action: action.to_owned(),
        })
    }

    /// Returns the resource half.
    #[must_use]
    pub fn resource(&self) -> &str {
        &self.resource
    }

    /// Returns the action half, which may be [`WILDCARD_ACTION`].
    #[must_use]
    pub fn action(&self) -> &str {
        &self.action
    }

    /// Returns whether this scope grants every action on its resource.
    #[must_use]
    pub fn is_wildcard(&self) -> bool {
        self.action == WILDCARD_ACTION
    }

    /// Returns whether holding this scope satisfies a requirement for `required`.
    ///
    /// Exact match, or a wildcard on the same resource. A wildcard on a *different* resource does
    /// not match, which is the whole risk of a capability system: `files.*` must not satisfy
    /// `gmail.send`.
    ///
    /// This is the single-scope rule only. Which scopes an actor actually *holds* — and whether a
    /// wildcard may be issued at all — is `P3-003`'s decision, taken from identity and workspace
    /// policy rather than from the tool contract.
    #[must_use]
    pub fn covers(&self, required: &Self) -> bool {
        if self.resource != required.resource {
            return false;
        }
        self.is_wildcard() || self.action == required.action
    }
}

/// Validates a half that has no wildcard form.
///
/// Lowercase ASCII, digits, `_`, `-`, and `.` — the same narrow set as a tool identifier segment,
/// for the same reasons: a scope is a lookup key and reaches a model in a discovery list, so an
/// uppercase letter would create two keys that look identical, and a dot is permitted so a resource
/// may be a small path (`drive.files`).
fn validate_simple(value: &str, segment: &'static str) -> Result<(), ScopeError> {
    let acceptable = !value.is_empty()
        && value.chars().count() <= MAX_SCOPE_SEGMENT_CHARS
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-' | '.'));
    if acceptable {
        Ok(())
    } else {
        Err(ScopeError::Segment { segment })
    }
}

impl fmt::Display for Scope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}", self.resource, self.action)
    }
}

impl std::str::FromStr for Scope {
    type Err = ScopeError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.trim().is_empty() {
            return Err(ScopeError::Empty);
        }
        if value.chars().count() > MAX_SCOPE_CHARS {
            return Err(ScopeError::TooLong);
        }
        // On the **last** dot, so a resource may be a small path (`drive.files.read`).
        let Some((resource, action)) = value.rsplit_once('.') else {
            return Err(ScopeError::NotQualified);
        };
        Self::from_parts(resource, action)
    }
}

impl TryFrom<String> for Scope {
    type Error = ScopeError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl From<Scope> for String {
    fn from(scope: Scope) -> Self {
        scope.to_string()
    }
}

/// The scopes a tool requires.
///
/// May be **empty**, which is a real declaration: a pure computation that reads nothing and writes
/// nothing needs no grant, and forcing a placeholder scope would make every such tool demand a
/// permission that means nothing. Distinct from [`crate::EffectSet`], which may not be empty —
/// effects describe consequences, and a tool with unstated consequences is one policy cannot reason
/// about, whereas a tool with no required grant is simply one that needs no grant.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ScopeSet {
    scopes: BTreeSet<Scope>,
}

impl ScopeSet {
    /// Creates a set from any number of scopes.
    ///
    /// A `BTreeSet` for the same reason as [`crate::EffectSet`]: a duplicate requirement is not a
    /// second requirement, and iteration order is stable without the caller sorting.
    #[must_use]
    pub fn new(scopes: impl IntoIterator<Item = Scope>) -> Self {
        Self {
            scopes: scopes.into_iter().collect(),
        }
    }

    /// Creates a set holding exactly one scope.
    #[must_use]
    pub fn single(scope: Scope) -> Self {
        let mut scopes = BTreeSet::new();
        scopes.insert(scope);
        Self { scopes }
    }

    /// Creates a set requiring nothing.
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    /// Returns whether no grant is required.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.scopes.is_empty()
    }

    /// Returns how many scopes are required.
    #[must_use]
    pub fn len(&self) -> usize {
        self.scopes.len()
    }

    /// Returns whether the set contains a scope exactly.
    #[must_use]
    pub fn contains(&self, scope: &Scope) -> bool {
        self.scopes.contains(scope)
    }

    /// Returns whether every requirement is covered by one of `held`.
    ///
    /// A pure function over two declared sets, and nothing more: it answers "do these grants satisfy
    /// this contract", not "may this actor run this tool". The second question also needs identity,
    /// workspace policy, channel ceiling, and approval, and `P3-003` owns it. Keeping this pure is
    /// what lets the interesting part of that decision be read as a function of data.
    #[must_use]
    pub fn is_satisfied_by(&self, held: &Self) -> bool {
        self.scopes
            .iter()
            .all(|required| held.scopes.iter().any(|granted| granted.covers(required)))
    }

    /// Returns the scopes, ordered.
    pub fn iter(&self) -> impl Iterator<Item = &Scope> + '_ {
        self.scopes.iter()
    }
}

impl fmt::Display for ScopeSet {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let joined = self
            .scopes
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        formatter.write_str(&joined)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(value: &str) -> Scope {
        Scope::new(value).unwrap_or_else(|error| panic!("{value}: {error}"))
    }

    /// A well-formed scope round-trips through its string form.
    #[test]
    fn a_scope_round_trips() {
        let parsed = scope("gmail.send");
        assert_eq!(parsed.resource(), "gmail");
        assert_eq!(parsed.action(), "send");
        assert_eq!(parsed.to_string(), "gmail.send");

        let encoded = serde_json::to_string(&parsed).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(encoded, "\"gmail.send\"");
        let decoded: Scope =
            serde_json::from_str(&encoded).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(decoded, parsed);
    }

    /// A resource may be a small path, split on the last dot.
    #[test]
    fn a_dotted_resource_is_kept_whole() {
        let parsed = scope("drive.files.read");
        assert_eq!(parsed.resource(), "drive.files");
        assert_eq!(parsed.action(), "read");
        assert_eq!(parsed.to_string(), "drive.files.read");
    }

    /// Malformed scopes are refused with a specific reason.
    #[test]
    fn malformed_scopes_are_refused() {
        assert_eq!(Scope::new("send"), Err(ScopeError::NotQualified));
        assert_eq!(Scope::new(""), Err(ScopeError::Empty));
        assert_eq!(Scope::new("   "), Err(ScopeError::Empty));
        assert_eq!(
            Scope::new(".send"),
            Err(ScopeError::Segment {
                segment: "resource"
            })
        );
        assert_eq!(
            Scope::new("gmail."),
            Err(ScopeError::Segment { segment: "action" })
        );
        assert_eq!(
            Scope::new("Gmail.send"),
            Err(ScopeError::Segment {
                segment: "resource"
            })
        );
    }

    /// A provider OAuth scope is refused, because JARVIS could never grant it.
    ///
    /// This is the vocabulary boundary stated as a test. A URL-shaped scope is legal in the
    /// provider's world and meaningless in JARVIS's: `required_scopes` is evaluated against JARVIS
    /// grants, so a tool naming a Google scope would be permanently unrunnable, and the failure
    /// would look like a policy problem rather than a wrong vocabulary.
    #[test]
    fn a_provider_oauth_scope_is_refused() {
        for provider_scope in [
            "https://www.googleapis.com/auth/gmail.send",
            "https://www.googleapis.com/auth/gmail.send",
        ] {
            assert!(
                Scope::new(provider_scope).is_err(),
                "{provider_scope} is a provider scope, not a JARVIS capability"
            );
        }
        // The equivalent JARVIS capability is accepted, so the refusal is about the vocabulary.
        assert!(Scope::new("gmail.send").is_ok());
    }

    /// A character that changes a context is refused.
    #[test]
    fn characters_that_change_a_context_are_refused() {
        for hostile in [
            "gmail/send",
            "gmail send",
            "gmail\\send",
            "gmail?send",
            "gmail#send",
            "gmail\nsend",
            "gmail:send",
        ] {
            assert!(Scope::new(hostile).is_err(), "{hostile:?} must be refused");
        }
    }

    /// Overlong scopes and halves are refused.
    ///
    /// Both bounds are exercised, which is the point: the total bound is only meaningful because it
    /// is tighter than two maximal halves, and this asserts that it actually fires.
    #[test]
    fn overlong_scopes_are_refused() {
        assert_eq!(
            Scope::new(format!("{}.send", "a".repeat(MAX_SCOPE_SEGMENT_CHARS + 1))),
            Err(ScopeError::Segment {
                segment: "resource"
            })
        );
        assert_eq!(
            Scope::new(format!("files.{}", "a".repeat(MAX_SCOPE_SEGMENT_CHARS + 1))),
            Err(ScopeError::Segment { segment: "action" })
        );

        // Two maximal halves exceed the total, so the total bound is the one that fires here.
        let long_scope = format!(
            "{}.{}",
            "a".repeat(MAX_SCOPE_SEGMENT_CHARS),
            "b".repeat(MAX_SCOPE_SEGMENT_CHARS)
        );
        assert!(
            long_scope.chars().count() > MAX_SCOPE_CHARS,
            "the fixture must actually exceed the total bound"
        );
        assert_eq!(Scope::new(long_scope), Err(ScopeError::TooLong));

        // A long resource with a short action is accepted, so the total bound is not simply a
        // smaller segment bound wearing a different name.
        let long_resource = format!("{}.read", "a".repeat(MAX_SCOPE_SEGMENT_CHARS));
        assert!(Scope::new(long_resource).is_ok());
    }

    /// An exact grant covers its requirement, and neither half may be mismatched.
    #[test]
    fn an_exact_grant_covers_only_its_own_scope() {
        let granted = scope("gmail.send");
        assert!(granted.covers(&scope("gmail.send")));
        assert!(!granted.covers(&scope("gmail.read")));
        assert!(!granted.covers(&scope("calendar.send")));
    }

    /// **A wildcard covers its own resource only.**
    ///
    /// The central risk of a capability system: a broad grant must not satisfy a requirement on a
    /// different resource. `files.*` covering `gmail.send` would be privilege escalation by
    /// wildcard, and this is the test that fails if the resource comparison is dropped.
    #[test]
    fn a_wildcard_covers_its_resource_and_no_other() {
        let wildcard = scope("files.*");
        assert!(wildcard.is_wildcard());
        assert!(wildcard.covers(&scope("files.read")));
        assert!(wildcard.covers(&scope("files.delete")));
        assert!(wildcard.covers(&scope("files.*")));
        assert!(
            !wildcard.covers(&scope("gmail.send")),
            "a wildcard on one resource must never cover another"
        );
        assert!(!wildcard.covers(&scope("drive.files.read")));
    }

    /// An exact grant does not cover a wildcard requirement.
    ///
    /// The direction that matters: `files.read` is narrower than `files.*`, so it must not satisfy
    /// a contract that demands every action on the resource.
    #[test]
    fn an_exact_grant_does_not_cover_a_wildcard_requirement() {
        assert!(!scope("files.read").covers(&scope("files.*")));
    }

    /// An empty requirement set is satisfied by anything, including nothing.
    #[test]
    fn no_requirement_is_always_satisfied() {
        assert!(ScopeSet::none().is_empty());
        assert!(ScopeSet::none().is_satisfied_by(&ScopeSet::none()));
        assert!(ScopeSet::none().is_satisfied_by(&ScopeSet::single(scope("gmail.send"))));
    }

    /// A requirement is satisfied by a covering grant, and not by an unrelated one.
    #[test]
    fn requirements_are_satisfied_by_covering_grants() {
        let required = ScopeSet::new([scope("gmail.send"), scope("files.read")]);
        assert_eq!(required.len(), 2);

        let enough = ScopeSet::new([scope("gmail.send"), scope("files.read")]);
        assert!(required.is_satisfied_by(&enough));

        let via_wildcard = ScopeSet::new([scope("gmail.send"), scope("files.*")]);
        assert!(required.is_satisfied_by(&via_wildcard));

        let missing_one = ScopeSet::single(scope("gmail.send"));
        assert!(
            !required.is_satisfied_by(&missing_one),
            "every requirement must be covered, not merely one"
        );

        let wrong_resource = ScopeSet::new([scope("gmail.send"), scope("calendar.*")]);
        assert!(!required.is_satisfied_by(&wrong_resource));
    }

    /// The set deduplicates and orders, so two declarations of one grant are one grant.
    #[test]
    fn the_set_deduplicates_and_orders() {
        let set = ScopeSet::new([
            scope("gmail.send"),
            scope("files.read"),
            scope("gmail.send"),
        ]);
        assert_eq!(set.len(), 2);
        let rendered: Vec<String> = set.iter().map(ToString::to_string).collect();
        assert_eq!(rendered, vec!["files.read", "gmail.send"]);
        assert!(set.contains(&scope("gmail.send")));
        assert!(!set.contains(&scope("gmail.delete")));
    }
}
