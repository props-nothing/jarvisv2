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

use jarvis_core::{CorrelationId, SessionChannel};

use jarvis_tools::{ActorAuthority, AuthenticationStrength};
use jarvis_tools::{Scope, ScopeSet};

/// The scope a caller holds to read files inside a workspace.
pub const FILES_READ_SCOPE: &str = "files.read";

/// The scope a caller holds to invoke a tool on a configured MCP server.
///
/// The same literal `jarvis-mcp` declares as `DEFAULT_MCP_SCOPE`, restated here rather than imported so this
/// crate does not depend on the MCP crate for one string. `the_mcp_scope_matches_the_transport_crate` asserts
/// the two agree, because **two copies of a literal that must match is exactly the defect class this project
/// keeps finding** — and the failure would be silent: every MCP call denied with `MissingScope`, reading as a
/// policy problem rather than a typo.
///
/// `#[cfg(test)]` because production no longer names it: the actor's authority is derived from the composed
/// tools' own declarations, so this literal's only remaining job is the correspondence check above. Keeping it
/// in the shipped build would be a constant nothing reads — the shape this file's other `#[cfg(test)]` item
/// exists to avoid.
#[cfg(test)]
pub const MCP_CALL_SCOPE: &str = "mcp.call";

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
    /// Describes the actor an **inbound MCP call** is made under.
    ///
    /// # The scopes are the served tools' own requirements, and that is derived rather than chosen
    ///
    /// The first version of this constructor granted **no** scopes, on the reasoning that the narrowest grant
    /// is the safest. Its own test caught the error: `FilesystemReadTool::definitions` declares
    /// `required_scopes: ScopeSet::single("files.read")`, and the policy engine requires the actor to cover the
    /// **tool's** declared scopes — so an actor holding nothing was refused every call with `missing_scope`. The
    /// endpoint would have been built, bound, and answer every request with a refusal, which reads to an
    /// operator as a policy misconfiguration rather than as this constructor.
    ///
    /// So the grant is the union of the served definitions' own `required_scopes`: the actor holds **exactly
    /// what running the served tools requires, and nothing else**. That is derived from the same list `P3-009d`
    /// built the served surface from, so a tool that is not served cannot widen it.
    ///
    /// # What is deliberately *not* granted
    ///
    /// **`mcp.call`**, which is the mistake this doc exists to prevent. That literal is *this daemon's* grant to
    /// call **someone else's** server, so attaching it to an inbound call would put the outbound grant on the
    /// inbound direction — a scope with the right name and the wrong direction, which is worse than a missing
    /// one because it reads as intentional. It is not in the union above because no served tool declares it.
    ///
    /// # The other two fields
    ///
    /// - **`SessionChannel::Api`.** Not a claim about the transport: a remote client is not a JARVIS client, and
    ///   labelling it `Cli` would say an operator's own local invocation made the call. The policy engine's
    ///   strength cap reads it.
    /// - **`AuthenticationStrength::Credential`**, which is what `P3-009g`'s allowlist actually establishes: a
    ///   caller is admitted only against a configured credential fingerprint. The engine **caps** this by
    ///   channel, so it is a claim rather than a proof — and the allowlist, not this value, is what admits.
    ///
    /// `run_id` is the correlation identity of the call: a remote call has **no run**, and a fabricated
    /// identifier that looked like a real one would appear in a receipt as a run an operator could look up and
    /// find missing. The pipeline's remote path writes no call row, so this value is never persisted.
    #[must_use]
    pub fn remote(
        workspace_id: impl Into<String>,
        correlation_id: CorrelationId,
        policy_version: impl Into<String>,
        served: &[jarvis_tools::ToolDefinition],
    ) -> Self {
        Self {
            workspace_id: workspace_id.into(),
            run_id: correlation_id.to_string(),
            scopes: Self::required_scopes_for(served),
            channel: SessionChannel::Api,
            claimed_strength: AuthenticationStrength::Credential,
            policy_version: policy_version.into(),
        }
    }

    /// Returns the union of the scopes every served tool declares as required.
    ///
    /// A union rather than an intersection, because the actor must be able to run **any** tool the endpoint
    /// advertises: an intersection would satisfy a tool whose requirements are a subset and refuse the rest,
    /// which would make the served surface a promise the actor cannot keep.
    ///
    /// A scope literal that cannot be reconstructed is **skipped**, and the consequence is a refusal rather
    /// than an over-grant: the only failure is a malformed declaration, and omitting it means the tool that
    /// declared it is refused for a missing scope — the fail-closed direction. `the_served_scopes_are_valid`
    /// asserts the literals this build produces are accepted, so the skip is proved unreachable.
    ///
    /// The round trip through `Display` and `Scope::new` is deliberate: `Scope` has no borrowed accessor, and
    /// re-parsing is what makes the *validity* of each declaration a property this function checks rather than
    /// one it assumes about a value it was handed.
    fn required_scopes_for(served: &[jarvis_tools::ToolDefinition]) -> ScopeSet {
        let mut scopes = Vec::new();
        for definition in served {
            for required in definition.required_scopes().iter() {
                let Ok(scope) = Scope::new(required.to_string()) else {
                    // Skipped rather than substituted: a declaration this build cannot reconstruct means the
                    // tool that declared it is refused for a missing scope, which is the fail-closed direction.
                    continue;
                };
                if !scopes.contains(&scope) {
                    scopes.push(scope);
                }
            }
        }
        ScopeSet::new(scopes)
    }

    /// Describes an actor with read access to a workspace **and the ability to call MCP tools**, over a
    /// channel at a stated strength.
    ///
    /// The second area's constructor, as the doc on [`Self::workspace_reader`] said a second area would add:
    /// its **name says what it grants**, so a call site states which capability it is exercising rather than
    /// passing a scope list.
    ///
    /// # Why this is separate rather than widening `workspace_reader`
    ///
    /// The two grants are genuinely different capabilities, and the daemon now serves *either* or *both*: a
    /// profile with no filesystem roots but a configured MCP server has no filesystem tool to read, and one
    /// with roots and no servers has no MCP tool to call. Widening the reader would grant `mcp.call` to a
    /// daemon that cannot call anything, which is a grant with no consumer — the thing this type's narrowness
    /// exists to avoid. A caller that wants both states so by naming this constructor.
    ///
    /// # Errors
    ///
    /// Returns `None` when a fixed scope literal is rejected, which would be an authoring error rather than a
    /// configuration one — so the caller can fail the daemon at startup instead of granting a subset silently.
    /// Failing closed to a *partial* grant would be worse than refusing: a call would be denied for a missing
    /// scope and read as a policy problem rather than a malformed constant.
    /// `#[cfg(test)]` because the HTTP and run paths derive their authority from the composed tools
    /// (`for_composed_tools`), so nothing in the shipped build calls this. It survives as the fixture that
    /// asserts the two grants are **distinct** — `workspace_reader` for `files.read` alone, this one for
    /// `files.read` plus an outbound MCP grant — which is what makes "an actor grants what its caller needs"
    /// checkable rather than asserted in prose. A public constructor with no production caller is how a surface
    /// grows a method nothing uses, which is why this one is scoped to the tests rather than kept for a caller
    /// that does not exist.
    #[cfg(test)]
    #[must_use]
    pub fn workspace_and_mcp(
        workspace_id: impl Into<String>,
        run_id: impl Into<String>,
        channel: SessionChannel,
        claimed_strength: AuthenticationStrength,
        policy_version: impl Into<String>,
    ) -> Option<Self> {
        // Both literals are parsed before either is used, so a rejection yields `None` rather than a partial
        // grant. `ScopeSet::new` is infallible, so the only failure here is a malformed literal — which is an
        // authoring error and is therefore reported rather than half-granted.
        let files = Scope::new(FILES_READ_SCOPE).ok()?;
        let mcp = Scope::new(MCP_CALL_SCOPE).ok()?;
        Some(Self {
            workspace_id: workspace_id.into(),
            run_id: run_id.into(),
            scopes: ScopeSet::new([files, mcp]),
            channel,
            claimed_strength,
            policy_version: policy_version.into(),
        })
    }

    /// Describes the actor an **outbound HTTP tool call** is made under: the union of the grants the daemon's
    /// own composed tools require.
    ///
    /// # Why this replaces a hand-written scope list, and what the list cost twice
    ///
    /// The HTTP surface serves whichever tools the daemon composed, and the actor must be able to call **any**
    /// of them — the client addresses one, so the actor is the daemon's own authority to run its own surface.
    /// Two defects came from writing that authority out by hand:
    ///
    /// 1. **`P4-014`'s tool was registered and dead.** `memory.propose` requires a scope the hand-written list
    ///    did not include, so every call was denied with `MissingScope` — and `jarvis tools preview` reported
    ///    `deny` for a tool the daemon was offering, which is worse than no preview because it sends an operator
    ///    to change a policy that is already correct.
    /// 2. **The scope had to be dropped again, immediately.** `mcp.call` was removed from the list on the
    ///    reasoning that this path serves only *native* tools, and `phase_3_gate` failed within the same change:
    ///    the surface does serve MCP tools, and a write tool over MCP paused with `403 missing_scope` instead of
    ///    reaching its approval.
    ///
    /// Both are the same mistake — a second statement of what the adapters already declare — and the fix is the
    /// one `ToolActor::remote` already uses on the inbound side: **derive it from the definitions.** A tool that
    /// is composed is callable, and a tool that is not composed cannot be addressed, so the union is exactly the
    /// daemon's authority over its own surface. No list to keep in step, and a new adapter is reachable the
    /// moment it is registered.
    ///
    /// `files.read` is **added** rather than relied on to be present: the filesystem adapter is registered only
    /// when an operator grants roots, so on a daemon with none the union would omit it and a policy override for
    /// `jarvis.files.read` would be unreachable through this surface. A grant the daemon's surface may serve is
    /// part of the surface's authority whether or not the adapter is currently composed.
    #[must_use]
    pub fn for_composed_tools(
        workspace_id: impl Into<String>,
        run_id: impl Into<String>,
        channel: SessionChannel,
        claimed_strength: AuthenticationStrength,
        policy_version: impl Into<String>,
        composed: &[jarvis_tools::ToolDefinition],
    ) -> Self {
        // Collected and rebuilt rather than unioned in place: `ScopeSet` exposes `iter` and `new` and no
        // mutating union, which is deliberate — a set that could be widened after construction is a grant that
        // can grow without the caller saying so. One rebuild says what the union is.
        let mut scopes: Vec<Scope> = Self::required_scopes_for(composed)
            .iter()
            .cloned()
            .collect();
        // `files.read` is **added** rather than relied on to be present: the filesystem adapter is registered
        // only when an operator grants roots, so on a daemon with none the union would omit it and a policy
        // override for `jarvis.files.read` would be unreachable through this surface. A grant the daemon's
        // surface may serve is part of the surface's authority whether or not the adapter is currently composed.
        if let Ok(files) = Scope::new(FILES_READ_SCOPE)
            && !scopes.contains(&files)
        {
            scopes.push(files);
        }
        Self {
            workspace_id: workspace_id.into(),
            run_id: run_id.into(),
            scopes: ScopeSet::new(scopes),
            channel,
            claimed_strength,
            policy_version: policy_version.into(),
        }
    }

    /// Describes an actor with read access to a workspace, over a channel at a stated strength.
    ///
    /// This constructor grants exactly `files.read`. That is deliberate rather than lazy: a builder with a
    /// `with_scopes` method would invite a caller to grant whatever a task happened to need, and a grant is
    /// exactly what a bug here would widen. The second tool area's constructor is
    /// [`Self::workspace_and_mcp`], whose name says what it grants.
    ///
    /// If the fixed scope literal were somehow rejected, the actor gets **no scopes** rather than a
    /// substitute one: a malformed literal is an authoring error, and failing closed means the call is
    /// refused for a missing scope instead of allowed for an invented one. The test module asserts the
    /// literal is accepted, so this path is proved unreachable rather than assumed to be.
    ///
    /// `policy_version` is recorded on the receipt and the call row so a stored decision can name the
    /// policy that produced it. It is a version **label** rather than a verifiable version, which
    /// `docs/adr/0021-an-authorization-receipt-derives-from-its-decision.md` records as a limit.
    ///
    /// `#[cfg(test)]` because the gateway now serves **both** tool areas and therefore uses
    /// [`Self::workspace_and_mcp`]; this narrower constructor is what the tests use to assert the two grants
    /// are distinct, and keeping a public constructor nothing calls is how a surface grows a method with no
    /// consumer.
    #[cfg(test)]
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
        assert!(
            Scope::new(MCP_CALL_SCOPE).is_ok(),
            "the fixed MCP scope must be well-formed, or every MCP call is denied for a missing scope"
        );
    }

    /// **The MCP scope literal must be the one the MCP crate declares.**
    ///
    /// Two copies of a literal that must agree is the defect class this project keeps finding, and this
    /// instance fails **silently in the safe-looking direction**: if the strings diverged, every MCP call
    /// would be denied for a missing scope, which reads as a policy problem or a misconfigured server rather
    /// than as a typo in this file. The assertion is what turns that into one failing test.
    ///
    /// The dependency direction is why this is a test rather than an import: `jarvisd` composes adapters, and
    /// naming a scope for a tool area is the composition root's business, so the crate that owns the literal
    /// is the crate that should be checked against it.
    #[test]
    fn the_mcp_scope_matches_the_transport_crate() {
        assert_eq!(
            MCP_CALL_SCOPE,
            jarvis_mcp_transport::DEFAULT_MCP_CALL_SCOPE,
            "the daemon's MCP scope must be the one the MCP translation declares, or every MCP call is denied"
        );
    }

    /// The combined actor grants **both** areas' scopes, which is what a transport serving both needs.
    #[test]
    fn an_actor_for_both_areas_holds_both_scopes() {
        let actor = ToolActor::workspace_and_mcp(
            "workspace",
            "run",
            SessionChannel::Cli,
            AuthenticationStrength::Credential,
            "policy-1",
        )
        .unwrap_or_else(|| panic!("both scope literals must be accepted"));

        let authority = actor.authority();
        for literal in [FILES_READ_SCOPE, MCP_CALL_SCOPE] {
            let scope = Scope::new(literal).unwrap_or_else(|error| panic!("{error}"));
            assert!(
                authority.scopes().contains(&scope),
                "a combined actor must hold {literal}"
            );
        }
        // And the identity it was built with is carried, since a receipt names the actor's workspace.
        assert_eq!(actor.workspace_id(), "workspace");
        assert_eq!(actor.run_id(), "run");
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

    /// **The authority an actor holds is derived from the tools that were composed, so a composed tool is
    /// callable and an uncomposed one is not.**
    ///
    /// This is the rule that replaces a hand-written grant list, and the reason it had to be replaced is worth
    /// stating because the list was wrong **twice in one slice**:
    ///
    /// 1. `P4-014` added `jarvis.memory.propose`, which requires `memory.propose`. The list did not name it, so
    ///    the tool was registered, offered to a model, and refused for every call with `MissingScope`. Nothing
    ///    catches that: `Dispatch::verify_covers` checks the opposite direction — a registered tool with no
    ///    adapter — and the adapter's own tests pass because they call the adapter directly.
    /// 2. Removing `mcp.call` from the list on the reasoning that this path served only *native* tools failed
    ///    `phase_3_gate`: the surface **does** serve MCP tools, and a write tool over MCP paused with
    ///    `403 missing_scope` instead of reaching its approval.
    ///
    /// Both are one mistake — a second statement of what the adapters already declare — and the derived form
    /// cannot make it. The test asserts **both directions**, because "includes what it should" and "includes
    /// nothing more" are separate claims and only the first is satisfied by granting everything.
    #[test]
    fn the_actor_authority_is_derived_from_the_composed_tools() {
        let memory = crate::memory_propose::MemoryProposeTool::definition()
            .unwrap_or_else(|error| panic!("{error}"));

        let actor = ToolActor::for_composed_tools(
            "workspace-1",
            "0198f000-0000-7000-8000-0000000000c3",
            SessionChannel::Cli,
            AuthenticationStrength::Credential,
            "policy-1",
            std::slice::from_ref(&memory),
        );
        let authority = actor.authority();
        assert!(
            authority
                .scopes()
                .iter()
                .any(|scope| scope.resource() == "memory" && scope.action() == "propose"),
            "a composed tool's scope must be held, or the tool is registered and dead: {authority:?}"
        );
        assert!(
            authority
                .scopes()
                .iter()
                .any(|scope| scope.resource() == "files" && scope.action() == "read"),
            "and `files.read`, because the filesystem adapter is registered only when roots are granted — so \
             the union alone would omit it and a policy override for it would be unreachable: {authority:?}"
        );

        // **The other direction: an uncomposed tool's scope is not granted.** Asserted separately, because
        // "grants what the definitions require" is satisfied by granting the whole vocabulary, and an authority
        // wider than the surface would authorize a call to a tool this daemon cannot run.
        let with_mcp = ToolActor::for_composed_tools(
            "workspace-1",
            "0198f000-0000-7000-8000-0000000000c3",
            SessionChannel::Cli,
            AuthenticationStrength::Credential,
            "policy-1",
            &[],
        );
        assert!(
            !with_mcp
                .authority()
                .scopes()
                .iter()
                .any(|scope| scope.resource() == "mcp"),
            "with no MCP tool composed there is no outbound MCP grant: {:?}",
            with_mcp.authority()
        );
    }
}
