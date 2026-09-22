//! Who is asking for a tool call, and what they may be asked to prove.
//!
//! Separate from [`crate::tool_pipeline`] because an actor is an input to a call while the pipeline is
//! the thing that runs one, and because a caller that only needs to describe an actor should not have
//! to build a pipeline to do it.
//!
//! # Why the actor is assembled from stored identity, not from the request
//!
//! `docs/architecture/identity-and-workspaces.md` is explicit that access is established by
//! authentication and that a free-standing workspace identifier is not proof of it. So the workspace
//! and run come from the profile's seeded local identity and the stored run, and a client-supplied
//! workspace would let a caller name a workspace it was never granted. [`ToolActor`] carries
//! identifiers read from those sources rather than parsed from a request body.

use jarvis_core::SessionChannel;

use jarvis_tools::{ActorAuthority, AuthenticationStrength};
use jarvis_tools::{Scope, ScopeSet};

/// The scope a caller holds to read files inside a workspace.
pub const FILES_READ_SCOPE: &str = "files.read";

/// The identity and grants one tool call is made under.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolActor {
    workspace_id: String,
    run_id: String,
    scopes: ScopeSet,
    channel: SessionChannel,
    claimed_strength: AuthenticationStrength,
    policy_version: String,
}

impl ToolActor {
    /// Describes an actor with read access to a workspace, over a channel at a stated strength.
    ///
    /// This is the **only** constructor, and it grants exactly `files.read`. That is deliberate rather
    /// than lazy: a builder with a `with_scopes` method would invite a caller to grant whatever a task
    /// happened to need, and a grant is exactly what a bug here would widen. A second tool area adds a
    /// second constructor whose name says what it grants.
    ///
    /// If the fixed scope literal were somehow rejected, the actor gets **no scopes** rather than a
    /// substitute one: a malformed literal is an authoring error, and failing closed means the call is
    /// refused for a missing scope instead of allowed for an invented one. The test module asserts the
    /// literal is accepted, so this path is proved unreachable rather than assumed to be.
    ///
    /// `policy_version` is recorded on the receipt and the call row so a stored decision can name the
    /// policy that produced it. It is a version **label** rather than a verifiable version, which
    /// `docs/adr/0021-an-authorization-receipt-derives-from-its-decision.md` records as a limit.
    #[must_use]
    pub fn workspace_reader(
        workspace_id: impl Into<String>,
        run_id: impl Into<String>,
        channel: SessionChannel,
        claimed_strength: AuthenticationStrength,
        policy_version: impl Into<String>,
    ) -> Self {
        let scopes = match Scope::new(FILES_READ_SCOPE) {
            Ok(scope) => ScopeSet::single(scope),
            Err(_) => ScopeSet::none(),
        };
        Self {
            workspace_id: workspace_id.into(),
            run_id: run_id.into(),
            scopes,
            channel,
            claimed_strength,
            policy_version: policy_version.into(),
        }
    }

    /// Returns the workspace the call is made in.
    #[must_use]
    pub fn workspace_id(&self) -> &str {
        &self.workspace_id
    }

    /// Returns the run that made the call.
    #[must_use]
    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    /// Returns the policy version label for the receipt.
    #[must_use]
    pub fn policy_version(&self) -> &str {
        &self.policy_version
    }

    /// Returns the channel the request arrived on.
    #[must_use]
    pub const fn channel(&self) -> SessionChannel {
        self.channel
    }

    /// Returns the strength the caller's client claims.
    ///
    /// A **claim**, which `P3-003` caps by the channel rather than trusting. An actor describing a
    /// strength it could not have established reaches the same decision as one that claimed less,
    /// because the cap is applied in the policy engine and not here.
    #[must_use]
    pub const fn claimed_strength(&self) -> AuthenticationStrength {
        self.claimed_strength
    }

    /// Borrows the actor in the form the policy engine consumes.
    #[must_use]
    pub fn authority(&self) -> ActorAuthority {
        ActorAuthority::active(self.scopes.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fixed scope literal is accepted, so the fail-closed path is unreachable in practice.
    ///
    /// This is what makes the `ScopeSet::none()` fallback honest rather than a silent way to lose a
    /// grant: if the literal ever became invalid, this test fails and the fallback stops being a
    /// hypothetical.
    #[test]
    fn the_reader_scope_is_a_valid_scope() {
        assert!(
            Scope::new(FILES_READ_SCOPE).is_ok(),
            "the fixed reader scope must be well-formed, or every reader actor silently holds nothing"
        );
    }

    /// The actor grants exactly the read scope, and carries the identity it was built with.
    #[test]
    fn a_workspace_reader_holds_only_the_read_scope() {
        let actor = ToolActor::workspace_reader(
            "local",
            "0198f000-0000-7000-8000-0000000000c3",
            SessionChannel::Cli,
            AuthenticationStrength::Present,
            "policy-3",
        );

        assert_eq!(actor.workspace_id(), "local");
        assert_eq!(actor.run_id(), "0198f000-0000-7000-8000-0000000000c3");
        assert_eq!(actor.policy_version(), "policy-3");
        assert_eq!(actor.claimed_strength(), AuthenticationStrength::Present);
        assert_eq!(actor.channel(), SessionChannel::Cli);

        let authority = actor.authority();
        assert_eq!(authority.scopes().len(), 1);
        assert!(
            authority
                .scopes()
                .iter()
                .any(|scope| scope.resource() == "files" && scope.action() == "read"),
            "the actor must hold files.read and nothing else: {authority:?}"
        );
    }
}
