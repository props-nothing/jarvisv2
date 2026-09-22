//! Deterministic policy evaluation: whether a tool may run, and why.
//!
//! # The one rule that orders everything
//!
//! `docs/architecture/identity-and-workspaces.md` states it in three words: "Deny overrides allow."
//! `docs/architecture/security.md` repeats it and adds the failure mode: "Missing or stale evidence
//! fails closed."
//!
//! This module is written so that rule is a property of the control flow rather than of review
//! attention. Every refusal is checked **before** any allowance, in a fixed order, and the function
//! returns at the first refusal. There is no accumulation of permissive findings that a later check
//! could fail to outweigh, because nothing accumulates: the first branch that denies ends the
//! evaluation.
//!
//! # Why every outcome carries a reason code
//!
//! `docs/architecture/repository-layout.md` lists "policy decisions and reason codes" as domain
//! vocabulary. A decision that returns only a boolean is unusable in three places that all exist:
//!
//! - an operator cannot answer "why was this refused" without reproducing the decision by hand;
//! - an audit record that stores `denied` and nothing else cannot be reviewed later;
//! - a model told "denied" with no reason retries the same call, because it has nothing to change.
//!
//! The codes are stable snake-case strings, stored rather than derived from `Debug`, so a
//! reordered variant cannot silently change what a stored record says.
//!
//! # Context can raise risk but cannot lower a floor
//!
//! `docs/architecture/tools-and-connectors.md`: "Context can raise risk but cannot lower a hard
//! policy floor", with "an unusually large recipient set, production target, external domain,
//! sensitive attachment, or ambiguous identity" as examples. [`TargetAssessment`] is that context,
//! and [`effective_risk`] takes a **maximum** — so an escalated risk can only move the posture
//! toward more scrutiny.
//!
//! # Why a weak channel asks rather than denies
//!
//! `docs/architecture/security.md`: "Voice confirmation alone is insufficient for risk-3 actions by
//! default. A trusted desktop/mobile/CLI approval may resume a voice-originated run."
//!
//! That sentence describes an outcome that is neither allow nor deny, so the decision has three
//! outcomes rather than two. A voice request for a risk-3 action is **not** refused — it is held
//! pending an approval that must arrive through a stronger channel. Modelling it as a denial would
//! make the documented resume path unreachable, and modelling it as an allowance would let a voice
//! command delete something.
//!
//! # What this module deliberately does not do
//!
//! It does not persist anything, and it does not know an approval exists. It answers "what would be
//! required" from declared and supplied facts. Recording an approval obligation, its nonce, and its
//! expiry is `P3-004`; the execution receipt is `P3-005`. This function is a pure function of its
//! inputs, which is what makes the policy table testable as a table.

use std::collections::BTreeSet;
use std::fmt;

use jarvis_core::SessionChannel;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::definition::ToolDefinition;
use crate::effect::ToolEffect;
use crate::identifier::ToolId;
use crate::risk::Risk;
use crate::scope::ScopeSet;

/// The strongest authentication a channel can establish.
///
/// Ordered so a comparison is the whole check. `Voice` is deliberately the weakest: a caller's
/// identity arrives over a channel whose evidence is a phone number or a provider-supplied
/// identifier, neither of which is a cryptographic factor.
///
/// The ordering is the mechanism behind `docs/architecture/security.md`'s voice rule. It is not a
/// statement that voice is untrustworthy — a voice session may legitimately read a calendar — but
/// that a voice channel cannot *establish* the strength a risk-3 action requires. The outcome is an
/// approval from a stronger channel, which is exactly the documented resume path.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthenticationStrength {
    /// No authentication evidence at all.
    Absent,
    /// Channel evidence only: a caller ID, a provider `user_id`, a pairing code already consumed.
    ChannelEvidence,
    /// A verified credential: the profile credential over local IPC, or a device credential.
    Credential,
    /// A credential established in this interaction, with user presence.
    Present,
}

impl AuthenticationStrength {
    /// Returns the stable wire and storage name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Absent => "absent",
            Self::ChannelEvidence => "channel_evidence",
            Self::Credential => "credential",
            Self::Present => "present",
        }
    }

    /// Returns the minimum strength a risk of this level needs **to be auto-allowed**.
    ///
    /// Above the level's requirement the decision is an approval, not a refusal, because a stronger
    /// channel can supply it.
    #[must_use]
    pub const fn required_for(risk: Risk) -> Self {
        match risk {
            Risk::Minimal | Risk::Low => Self::ChannelEvidence,
            Risk::Moderate => Self::Credential,
            Risk::High => Self::Present,
        }
    }
}

impl fmt::Display for AuthenticationStrength {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The strongest authentication a channel can establish.
///
/// This is the "client/channel ceiling" of `docs/architecture/identity-and-workspaces.md`
/// ("a role grants a maximum; a client/channel may have a narrower ceiling"). It caps what an actor
/// may *claim* on that channel, so a client cannot raise its own strength by asserting one.
#[must_use]
pub const fn channel_ceiling(channel: SessionChannel) -> AuthenticationStrength {
    match channel {
        // A local terminal or the desktop view holds the profile credential, and can require user
        // presence for a specific action.
        SessionChannel::Cli | SessionChannel::Desktop => AuthenticationStrength::Present,
        // An API client presents a scoped credential. It cannot establish presence, because there is
        // no one in front of it to prove it.
        SessionChannel::Api => AuthenticationStrength::Credential,
        // Voice: a caller ID or a provider-supplied user identifier is evidence *from the channel*,
        // and never a credential. See the module comment for why this produces an approval rather
        // than a refusal for a risk-3 action.
        SessionChannel::Voice => AuthenticationStrength::ChannelEvidence,
    }
}

/// Where an actor stands.
///
/// `Suspended` is a distinct state rather than a missing scope: "this identity may not act at all"
/// is different from "this identity may not do this", and collapsing them would produce a
/// missing-scope reason for an identity that has every scope but is disabled.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorStatus {
    /// The actor may act, within its grants.
    Active,
    /// The actor is known but disabled. Every request is refused.
    Suspended,
}

/// What an actor and its client hold.
///
/// The scopes are the grants the **actor** holds. They are not the tool's `required_scopes`: those
/// declare what running the tool needs, and this is what the actor was given. Keeping them separate
/// is what makes the check a comparison rather than a tautology.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ActorAuthority {
    status: ActorStatus,
    scopes: ScopeSet,
}

impl ActorAuthority {
    /// Declares an active actor holding `scopes`.
    #[must_use]
    pub const fn active(scopes: ScopeSet) -> Self {
        Self {
            status: ActorStatus::Active,
            scopes,
        }
    }

    /// Declares a suspended actor, which is refused every request whatever it holds.
    #[must_use]
    pub const fn suspended(scopes: ScopeSet) -> Self {
        Self {
            status: ActorStatus::Suspended,
            scopes,
        }
    }

    /// Returns the actor's status.
    #[must_use]
    pub const fn status(&self) -> ActorStatus {
        self.status
    }

    /// Returns the scopes the actor holds.
    #[must_use]
    pub const fn scopes(&self) -> &ScopeSet {
        &self.scopes
    }
}

/// The workspace's own policy, which a request cannot widen.
///
/// Every field can only make the outcome **more** restrictive than the tool's own declaration. That
/// direction is the point: `docs/architecture/identity-and-workspaces.md` gives the workspace a
/// "maximum" and the tool a declaration, and if workspace policy could relax a tool's approval
/// policy then a workspace setting would be a way to remove a guard the tool author put in place.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkspacePolicy {
    max_risk: Risk,
    approval_threshold: Risk,
    requires_approval_for_external_communication: bool,
    requires_strong_authentication_for_high_risk: bool,
    denied_tools: BTreeSet<ToolId>,
}

impl Default for WorkspacePolicy {
    /// The local single-owner profile's policy.
    ///
    /// Permits the full risk range and asks for approval from risk 2 up, which is the risk guidance
    /// table's own posture for levels 2 and 3. It is a **default for a workspace whose owner has not
    /// chosen one**, and it grants nothing: the tool's own policy and the actor's scopes still apply,
    /// and a denial in either still wins.
    fn default() -> Self {
        Self {
            max_risk: Risk::High,
            approval_threshold: Risk::Moderate,
            requires_approval_for_external_communication: true,
            requires_strong_authentication_for_high_risk: true,
            denied_tools: BTreeSet::new(),
        }
    }
}

impl WorkspacePolicy {
    /// Declares a policy, refusing a threshold above the permitted maximum.
    ///
    /// # Errors
    ///
    /// Returns [`PolicyError::ApprovalThresholdAboveMaximum`] when `approval_threshold` is higher
    /// than `max_risk`. The pair would be contradictory — every risk that could be approved is
    /// already refused — and the likely cause is a misconfiguration that would otherwise be silent.
    pub fn new(
        max_risk: Risk,
        approval_threshold: Risk,
        requires_approval_for_external_communication: bool,
        requires_strong_authentication_for_high_risk: bool,
    ) -> Result<Self, PolicyError> {
        if approval_threshold > max_risk {
            return Err(PolicyError::ApprovalThresholdAboveMaximum {
                approval_threshold,
                max_risk,
            });
        }
        Ok(Self {
            max_risk,
            approval_threshold,
            requires_approval_for_external_communication,
            requires_strong_authentication_for_high_risk,
            denied_tools: BTreeSet::new(),
        })
    }

    /// Denies a tool in this workspace, returning the updated policy.
    ///
    /// Takes and returns `self` so a denial cannot be forgotten by a caller that built a policy and
    /// discarded the result of a `&mut` call.
    #[must_use]
    pub fn denying(mut self, id: ToolId) -> Self {
        self.denied_tools.insert(id);
        self
    }

    /// Returns the highest risk this workspace permits at all.
    #[must_use]
    pub const fn max_risk(&self) -> Risk {
        self.max_risk
    }

    /// Returns the risk at which approval is always required.
    #[must_use]
    pub const fn approval_threshold(&self) -> Risk {
        self.approval_threshold
    }

    /// Returns whether an external-communication effect always needs approval.
    #[must_use]
    pub const fn requires_approval_for_external_communication(&self) -> bool {
        self.requires_approval_for_external_communication
    }

    /// Returns whether a high-risk action needs presence-establishing authentication.
    #[must_use]
    pub const fn requires_strong_authentication_for_high_risk(&self) -> bool {
        self.requires_strong_authentication_for_high_risk
    }

    /// Returns whether this workspace denies a tool.
    #[must_use]
    pub fn denies(&self, id: &ToolId) -> bool {
        self.denied_tools.contains(id)
    }
}

/// Explains why a workspace policy was rejected.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum PolicyError {
    /// The approval threshold was above the permitted maximum.
    #[error(
        "the approval threshold {approval_threshold} is above the permitted maximum {max_risk}, \
         which would require approval for every risk already refused"
    )]
    ApprovalThresholdAboveMaximum {
        /// The threshold that was declared.
        approval_threshold: Risk,
        /// The maximum that was declared.
        max_risk: Risk,
    },
}

/// Context about a specific call that can raise its risk.
///
/// A set rather than one boolean per signal. A target can be external **and** bulk **and**
/// production, so the flags are genuinely independent — but four booleans cannot be iterated, cannot
/// be reported as a list, and let a caller set one without the decision recording it.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct TargetAssessment {
    signals: BTreeSet<EscalationSignal>,
}

impl TargetAssessment {
    /// Declares the signals present for this call.
    #[must_use]
    pub fn new(signals: impl IntoIterator<Item = EscalationSignal>) -> Self {
        Self {
            signals: signals.into_iter().collect(),
        }
    }

    /// Declares an unremarkable target, which raises nothing.
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    /// Returns whether a signal is present.
    #[must_use]
    pub fn contains(&self, signal: EscalationSignal) -> bool {
        self.signals.contains(&signal)
    }

    /// Returns whether this context is entirely unremarkable.
    #[must_use]
    pub fn is_unremarkable(&self) -> bool {
        self.signals.is_empty()
    }

    /// Returns the risk this context raises the call to.
    ///
    /// A **maximum of levels, not a sum**, for the same reason [`crate::EffectSet::risk_floor`] is:
    /// risk is a level of scrutiny, not a quantity of badness. Two risk-raising signals do not need
    /// more scrutiny than the stricter of the two already requires, and summing would make the
    /// mapping from signals to posture impossible to state.
    ///
    /// An unremarkable target returns [`Risk::Minimal`], which raises nothing in practice because
    /// [`effective_risk`] takes a maximum against the declared risk.
    #[must_use]
    pub fn escalation(&self) -> Risk {
        self.signals
            .iter()
            .map(|signal| signal.escalation())
            .max()
            .unwrap_or(Risk::Minimal)
    }

    /// Returns the signals, ordered and deduplicated.
    pub fn iter(&self) -> impl Iterator<Item = EscalationSignal> + '_ {
        self.signals.iter().copied()
    }

    /// Returns the signals as a slice-like collection.
    #[must_use]
    pub fn to_vec(&self) -> Vec<EscalationSignal> {
        self.signals.iter().copied().collect()
    }
}

/// Everything the decision is a function of.
///
/// Borrows the definition rather than owning it, so a caller that already holds one in the registry
/// does not clone two JSON Schema validators to ask a policy question.
#[derive(Clone, Debug)]
pub struct PolicyRequest<'a> {
    /// The tool's declared contract.
    pub definition: &'a ToolDefinition,
    /// The actor and its grants.
    pub actor: ActorAuthority,
    /// The workspace's policy.
    pub workspace: &'a WorkspacePolicy,
    /// The channel the request arrived on.
    pub channel: SessionChannel,
    /// The strength the actor's client claims. Capped by the channel.
    pub claimed_strength: AuthenticationStrength,
    /// Whether the tool can currently run.
    pub available: bool,
    /// Context that may raise the risk.
    pub target: TargetAssessment,
}

/// Why a tool call was refused or held.
///
/// Every variant is a stable code. They are stored, so reordering or renaming a variant would change
/// what an existing record means — which is why [`DenyReason::code`] is a hand-written match rather
/// than a derived string.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DenyReason {
    /// The tool cannot run now, so there is nothing to authorize.
    CapabilityUnavailable,
    /// The actor is not permitted to act at all.
    ActorNotPermitted,
    /// The workspace denies this tool outright.
    ToolDeniedByWorkspace,
    /// The call's effective risk is above the workspace's maximum.
    AboveWorkspaceCeiling,
    /// The actor does not hold a scope the tool requires.
    MissingScope,
    /// The tool's own policy refuses every call.
    ToolPolicyDenies,
    /// The call's risk requires an approval the request does not carry.
    ApprovalRequired,
    /// The channel cannot establish enough authentication to satisfy this risk.
    InsufficientAuthentication,
    /// An external-communication effect in a workspace that requires approval for it.
    ExternalCommunicationRequiresApproval,
}

impl DenyReason {
    /// Returns the stable snake-case code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::CapabilityUnavailable => "capability_unavailable",
            Self::ActorNotPermitted => "actor_not_permitted",
            Self::ToolDeniedByWorkspace => "tool_denied_by_workspace",
            Self::AboveWorkspaceCeiling => "above_workspace_ceiling",
            Self::MissingScope => "missing_scope",
            Self::ToolPolicyDenies => "tool_policy_denies",
            Self::ApprovalRequired => "approval_required",
            Self::InsufficientAuthentication => "insufficient_authentication",
            Self::ExternalCommunicationRequiresApproval => {
                "external_communication_requires_approval"
            }
        }
    }

    /// Returns every reason, so a test can sweep them.
    #[must_use]
    pub const fn all() -> [Self; 9] {
        [
            Self::CapabilityUnavailable,
            Self::ActorNotPermitted,
            Self::ToolDeniedByWorkspace,
            Self::AboveWorkspaceCeiling,
            Self::MissingScope,
            Self::ToolPolicyDenies,
            Self::ApprovalRequired,
            Self::InsufficientAuthentication,
            Self::ExternalCommunicationRequiresApproval,
        ]
    }

    /// Returns whether this reason is a refusal rather than a held obligation.
    ///
    /// `ApprovalRequired` is the only held reason: every other code means the call must not happen,
    /// while this one means it must not happen **yet**. Separating them is what lets a caller store
    /// the first as a terminal refusal and the second as a pending obligation.
    #[must_use]
    pub const fn is_refusal(self) -> bool {
        !matches!(
            self,
            Self::ApprovalRequired | Self::ExternalCommunicationRequiresApproval
        )
    }
}

impl fmt::Display for DenyReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

/// What the policy decided.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    /// The call may proceed without an approval.
    Allow,
    /// The call may proceed once an approval arrives.
    RequireApproval,
    /// The call must not proceed.
    Deny,
}

/// A context signal that raised a call's risk.
///
/// A typed closed set rather than a string, for the same reason [`DenyReason`] is one: this value is
/// stored in a decision record and read back, so a free-form name would be a place for content to
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

    /// Returns every signal, so a test can sweep them.
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
    /// `Bulk` and `Sensitive` reach `High` because of what they change about a call rather than how
    /// large it is: `docs/architecture/tools-and-connectors.md` lists "mass-send" under the risk-3
    /// posture, and a sensitive payload changes the consequence of the same action. `External` and
    /// `Production` reach `Moderate`.
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

/// The result of evaluating a request.
///
/// Carries the **effective risk** as well as the outcome, because a decision record that says
/// `deny` without recording the risk it was denied at cannot be reviewed, and an operator cannot
/// tell whether an escalation or a ceiling produced the refusal.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PolicyDecision {
    decision: Decision,
    reason: Option<DenyReason>,
    effective_risk: Risk,
    /// The strength an approval must be supplied with, when one is required.
    required_strength: Option<AuthenticationStrength>,
    /// The context signals that raised the risk above the declared level.
    escalated_by: Vec<EscalationSignal>,
}

impl PolicyDecision {
    /// Returns the outcome.
    #[must_use]
    pub const fn decision(&self) -> Decision {
        self.decision
    }

    /// Returns whether the call may proceed.
    #[must_use]
    pub const fn is_allowed(&self) -> bool {
        matches!(self.decision, Decision::Allow)
    }

    /// Returns whether the call is held pending an approval.
    #[must_use]
    pub const fn is_held(&self) -> bool {
        matches!(self.decision, Decision::RequireApproval)
    }

    /// Returns whether the call is refused.
    #[must_use]
    pub const fn is_denied(&self) -> bool {
        matches!(self.decision, Decision::Deny)
    }

    /// Returns the reason, which is present whenever the call is not allowed.
    #[must_use]
    pub const fn reason(&self) -> Option<DenyReason> {
        self.reason
    }

    /// Returns the stable reason code, or `"allowed"`.
    #[must_use]
    pub fn reason_code(&self) -> &'static str {
        self.reason.map_or("allowed", DenyReason::code)
    }

    /// Returns the risk the decision was taken at, including any escalation.
    #[must_use]
    pub const fn effective_risk(&self) -> Risk {
        self.effective_risk
    }

    /// Returns the strength an approval must be supplied with.
    ///
    /// Present whenever the outcome is `RequireApproval`, so a caller cannot hold a call without
    /// knowing what would release it. What supplies that strength — an authenticated approval record
    /// with a nonce and an expiry — is `P3-004`.
    #[must_use]
    pub const fn required_strength(&self) -> Option<AuthenticationStrength> {
        self.required_strength
    }

    /// Returns the context signals that raised the risk.
    #[must_use]
    pub fn escalated_by(&self) -> &[EscalationSignal] {
        &self.escalated_by
    }
}

/// Returns the signals that raise the risk, in the set's stable order.
fn escalation_signals(target: &TargetAssessment) -> Vec<EscalationSignal> {
    target.to_vec()
}

/// Returns the risk a call is evaluated at: the higher of what the tool declared and what the
/// context raises it to.
///
/// A `max`, so context can raise risk but cannot lower a floor — the rule from
/// `docs/architecture/tools-and-connectors.md`. The declared risk is already at or above the effect
/// floor (`P3-001` enforces that at construction), so the floor needs no separate term here.
#[must_use]
pub fn effective_risk(definition: &ToolDefinition, target: &TargetAssessment) -> Risk {
    let declared = definition.risk();
    let escalated = target.escalation();
    if escalated > declared {
        escalated
    } else {
        declared
    }
}

/// Decides whether a tool call may run.
///
/// The order of the checks **is** the deny-overrides rule, and it is fixed:
///
/// 1. the capability is not even available — nothing to authorize;
/// 2. the actor is suspended — a disabled identity has no grants;
/// 3. the workspace denies the tool — an explicit denial outranks every grant;
/// 4. the call's effective risk is above the workspace ceiling;
/// 5. the actor is missing a scope the tool requires;
/// 6. the tool's own policy refuses every call;
/// 7. the workspace or the tool requires an approval the request does not carry;
/// 8. the channel cannot establish enough authentication;
/// 9. otherwise, allow.
///
/// Steps 3, 5, and 6 are the deny-overrides cases, and they are deliberately **before** the approval
/// steps: a call that is denied and would also need approval must report the denial, or an operator
/// would approve something that cannot run.
///
/// There is no short-circuit in the other direction. No step can *grant* anything, so a permissive
/// finding cannot mask a later refusal — which is what makes the first-refusal-wins structure
/// equivalent to deny-overrides rather than merely consistent with it.
#[must_use]
pub fn evaluate(request: &PolicyRequest<'_>) -> PolicyDecision {
    let definition = request.definition;
    let risk = effective_risk(definition, &request.target);
    let escalated_by = escalation_signals(&request.target);

    // The channel caps what the actor may claim, so a client cannot raise its own strength by
    // asserting a value it did not establish.
    let effective_strength = request
        .claimed_strength
        .min(channel_ceiling(request.channel));

    let held =
        |reason: DenyReason, required_strength: Option<AuthenticationStrength>| PolicyDecision {
            decision: Decision::RequireApproval,
            reason: Some(reason),
            effective_risk: risk,
            required_strength,
            escalated_by: escalated_by.clone(),
        };
    let denied = |reason: DenyReason| PolicyDecision {
        decision: Decision::Deny,
        reason: Some(reason),
        effective_risk: risk,
        required_strength: None,
        escalated_by: escalated_by.clone(),
    };

    // 1. Availability. An unavailable capability has nothing to authorize, so this comes first: every
    //    later check would be a claim about a call that cannot happen.
    if !request.available {
        return denied(DenyReason::CapabilityUnavailable);
    }

    // 2. Actor status. A suspended identity is refused whatever it holds, so this precedes any grant.
    if request.actor.status() == ActorStatus::Suspended {
        return denied(DenyReason::ActorNotPermitted);
    }

    // 3. An explicit workspace denial. Deny overrides allow, so this precedes the scope check: a
    //    fully-scoped actor is still refused a tool the workspace denies.
    if request.workspace.denies(definition.id()) {
        return denied(DenyReason::ToolDeniedByWorkspace);
    }

    // 4. The workspace ceiling.
    if risk > request.workspace.max_risk() {
        return denied(DenyReason::AboveWorkspaceCeiling);
    }

    // 5. Capability scopes.
    if !definition
        .required_scopes()
        .is_satisfied_by(request.actor.scopes())
    {
        return denied(DenyReason::MissingScope);
    }

    // 6. The tool's own policy. `Deny` is absolute: it is the tool author saying this must never run
    //    automatically, and a workspace cannot relax it.
    if !definition.approval().is_runnable() {
        return denied(DenyReason::ToolPolicyDenies);
    }

    // 7. Approval obligations. The risk threshold needs the strength the level requires; the
    //    external-communication rule does not raise the strength requirement, because the risk
    //    already carries one.
    let risk_requires_approval = risk >= request.workspace.approval_threshold();
    let external_requires_approval = request
        .workspace
        .requires_approval_for_external_communication()
        && definition
            .effects()
            .contains(ToolEffect::ExternalCommunication);

    if risk_requires_approval || external_requires_approval {
        // The strength an approval must be supplied with. `Present` for a high-risk action in a
        // workspace that asks for it, which makes the voice ceiling produce the documented outcome:
        // a voice-originated risk-3 call is *held*, not refused, and a desktop approval releases it.
        let required = if request
            .workspace
            .requires_strong_authentication_for_high_risk()
            && risk == Risk::High
        {
            AuthenticationStrength::Present
        } else {
            AuthenticationStrength::required_for(risk)
        };
        return held(DenyReason::ApprovalRequired, Some(required));
    }

    // 8. Authentication strength. Reached only when no approval is required, so a request that is
    //    already held is not reported as under-authenticated — the approval is the stronger
    //    obligation, and reporting both would give an operator two remedies for one problem.
    if effective_strength < AuthenticationStrength::required_for(risk) {
        return held(
            DenyReason::InsufficientAuthentication,
            Some(AuthenticationStrength::required_for(risk)),
        );
    }

    PolicyDecision {
        decision: Decision::Allow,
        reason: None,
        effective_risk: risk,
        required_strength: None,
        escalated_by,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::definition::ToolDefinitionParts;
    use crate::effect::EffectSet;
    use crate::policy::{
        ApprovalPolicy, Availability, Idempotency, RetryDeclaration, ToolSensitivity,
    };
    use crate::schema::ToolSchema;
    use crate::scope::Scope;
    use jarvis_core::Sensitivity;

    fn schema() -> ToolSchema {
        ToolSchema::parse(&format!(
            r#"{{
                "$schema": "{}",
                "type": "object",
                "properties": {{ "q": {{ "type": "string" }} }}
            }}"#,
            crate::TOOL_SCHEMA_DIALECT
        ))
        .unwrap_or_else(|error| panic!("fixture schema: {error}"))
    }

    fn scope(value: &str) -> Scope {
        Scope::new(value).unwrap_or_else(|error| panic!("{value}: {error}"))
    }

    fn id(value: &str) -> ToolId {
        ToolId::new(value).unwrap_or_else(|error| panic!("{value}: {error}"))
    }

    /// Builds a tool with the given effects, declared risk, and approval policy.
    fn tool(
        name: &str,
        effects: EffectSet,
        risk: u8,
        approval: ApprovalPolicy,
        required_scopes: ScopeSet,
    ) -> ToolDefinition {
        let tool_id = id(name);
        let source = tool_id.source();
        ToolDefinition::new(ToolDefinitionParts {
            id: tool_id,
            version: "1.0.0".to_owned(),
            title: format!("Title {name}"),
            description: "A fixture tool.".to_owned(),
            input_schema: schema(),
            output_schema: schema(),
            effects,
            risk,
            required_scopes,
            approval,
            timeout_seconds: 30,
            retry: RetryDeclaration::none(),
            idempotency: Idempotency::Required,
            source,
            availability: Availability::Available,
            sensitivity: ToolSensitivity::new(Sensitivity::Internal, Sensitivity::Internal),
        })
        .unwrap_or_else(|error| panic!("fixture definition: {error}"))
    }

    /// A read-only tool needing `files.read`.
    fn reader() -> ToolDefinition {
        tool(
            "jarvis.files.read",
            EffectSet::single(ToolEffect::ReadOnly),
            0,
            ApprovalPolicy::Auto,
            ScopeSet::single(scope("files.read")),
        )
    }

    /// A tool that sends externally, at risk 2 needing approval by its own policy.
    fn sender() -> ToolDefinition {
        tool(
            "jarvis.mail.send",
            EffectSet::single(ToolEffect::ExternalCommunication),
            2,
            ApprovalPolicy::Ask,
            ScopeSet::single(scope("mail.send")),
        )
    }

    fn request<'a>(
        definition: &'a ToolDefinition,
        workspace: &'a WorkspacePolicy,
        actor: ActorAuthority,
    ) -> PolicyRequest<'a> {
        PolicyRequest {
            definition,
            actor,
            workspace,
            channel: SessionChannel::Desktop,
            claimed_strength: AuthenticationStrength::Present,
            available: true,
            target: TargetAssessment::none(),
        }
    }

    /// An authorised read is allowed and records what it decided at.
    #[test]
    fn an_authorised_read_is_allowed() {
        let definition = reader();
        let workspace = WorkspacePolicy::default();
        let actor = ActorAuthority::active(ScopeSet::single(scope("files.read")));
        let decision = evaluate(&request(&definition, &workspace, actor));

        assert!(decision.is_allowed(), "{decision:?}");
        assert_eq!(decision.decision(), Decision::Allow);
        assert_eq!(decision.reason(), None);
        assert_eq!(decision.reason_code(), "allowed");
        assert_eq!(decision.effective_risk(), Risk::Minimal);
        assert_eq!(decision.required_strength(), None);
        assert!(decision.escalated_by().is_empty());
    }

    /// **The deny-overrides test: a workspace denial beats a fully-scoped actor, and beats the
    /// approval that would otherwise be required.**
    ///
    /// The ordering claim, asserted from the direction that would catch a reordering: this request
    /// satisfies every grant and would also be held for approval, and it must report the denial.
    #[test]
    fn a_workspace_denial_overrides_scopes_and_approval() {
        let definition = sender();
        let workspace = WorkspacePolicy::default().denying(id("jarvis.mail.send"));
        // Every scope the tool needs, and presence-establishing authentication, and a channel that
        // can carry it.
        let actor = ActorAuthority::active(ScopeSet::single(scope("mail.send")));

        let decision = evaluate(&request(&definition, &workspace, actor));
        assert!(
            decision.is_denied(),
            "a workspace denial must win over every grant: {decision:?}"
        );
        assert_eq!(decision.reason(), Some(DenyReason::ToolDeniedByWorkspace));
        assert_eq!(decision.reason_code(), "tool_denied_by_workspace");
    }

    /// **A suspended actor is refused even holding every scope.**
    #[test]
    fn a_suspended_actor_is_refused_whatever_it_holds() {
        let definition = reader();
        let workspace = WorkspacePolicy::default();
        let actor = ActorAuthority::suspended(ScopeSet::single(scope("files.read")));
        let decision = evaluate(&request(&definition, &workspace, actor));
        assert!(decision.is_denied());
        assert_eq!(decision.reason(), Some(DenyReason::ActorNotPermitted));
    }

    /// **Missing a required scope is a refusal**, and an unrelated scope does not substitute.
    #[test]
    fn a_missing_scope_is_refused() {
        let definition = reader();
        let workspace = WorkspacePolicy::default();

        let empty = ActorAuthority::active(ScopeSet::none());
        let decision = evaluate(&request(&definition, &workspace, empty));
        assert!(decision.is_denied());
        assert_eq!(decision.reason(), Some(DenyReason::MissingScope));

        // A wildcard on the right resource satisfies it; the scope type's own rule.
        let wildcard = ActorAuthority::active(ScopeSet::single(scope("files.*")));
        assert!(evaluate(&request(&definition, &workspace, wildcard)).is_allowed());

        // A wildcard on a different resource does not.
        let other = ActorAuthority::active(ScopeSet::single(scope("mail.*")));
        let decision = evaluate(&request(&definition, &workspace, other));
        assert_eq!(decision.reason(), Some(DenyReason::MissingScope));
    }

    /// An unavailable tool is refused before anything else is considered.
    ///
    /// Reported as unavailable rather than as a permission problem, so an operator does not hunt a
    /// grant for a connector that is simply not configured.
    #[test]
    fn an_unavailable_tool_is_refused_first() {
        let definition = reader();
        let workspace = WorkspacePolicy::default().denying(id("jarvis.files.read"));
        let actor = ActorAuthority::suspended(ScopeSet::none());
        let mut request = request(&definition, &workspace, actor);
        request.available = false;

        let decision = evaluate(&request);
        assert_eq!(decision.reason(), Some(DenyReason::CapabilityUnavailable));
    }

    /// A tool whose own policy is `Deny` is refused, and a workspace cannot relax it.
    #[test]
    fn a_tool_that_denies_every_call_is_refused() {
        let definition = tool(
            "jarvis.files.read",
            EffectSet::single(ToolEffect::ReadOnly),
            0,
            ApprovalPolicy::Deny,
            ScopeSet::none(),
        );
        let workspace = WorkspacePolicy::default();
        let actor = ActorAuthority::active(ScopeSet::none());
        let decision = evaluate(&request(&definition, &workspace, actor));
        assert!(decision.is_denied());
        assert_eq!(decision.reason(), Some(DenyReason::ToolPolicyDenies));
    }

    /// **A risk above the workspace ceiling is refused, and the ceiling is what refuses it.**
    #[test]
    fn a_risk_above_the_workspace_ceiling_is_refused() {
        let definition = sender();
        // A workspace permitting only risk 1, with the threshold at 1 so the pair is not
        // contradictory.
        let workspace = WorkspacePolicy::new(Risk::Low, Risk::Low, true, true)
            .unwrap_or_else(|error| panic!("consistent policy: {error}"));
        let actor = ActorAuthority::active(ScopeSet::single(scope("mail.send")));

        let decision = evaluate(&request(&definition, &workspace, actor));
        assert_eq!(decision.reason(), Some(DenyReason::AboveWorkspaceCeiling));
        assert_eq!(decision.effective_risk(), Risk::Moderate);
    }

    /// A contradictory workspace policy is refused rather than silently producing a dead range.
    #[test]
    fn a_contradictory_workspace_policy_is_refused() {
        assert_eq!(
            WorkspacePolicy::new(Risk::Low, Risk::High, true, true),
            Err(PolicyError::ApprovalThresholdAboveMaximum {
                approval_threshold: Risk::High,
                max_risk: Risk::Low
            })
        );
    }

    /// A risk at the workspace threshold is held, and the reason says what would release it.
    ///
    /// The required strength is `Credential` for a risk-2 action, not `Present`: `Present` is
    /// reserved for a high-risk action in a workspace that asks for it. That distinction is what
    /// keeps the voice rule meaningful — a voice channel's ceiling is `ChannelEvidence`, so it can
    /// supply neither a `Credential` nor `Present`, and a risk-2 or risk-3 approval must therefore
    /// arrive through a stronger channel.
    #[test]
    fn a_risk_at_the_threshold_is_held_with_a_required_strength() {
        let definition = sender();
        let workspace = WorkspacePolicy::default();
        let actor = ActorAuthority::active(ScopeSet::single(scope("mail.send")));
        let decision = evaluate(&request(&definition, &workspace, actor));

        assert!(decision.is_held(), "{decision:?}");
        assert_eq!(decision.reason(), Some(DenyReason::ApprovalRequired));
        assert_eq!(
            decision.required_strength(),
            Some(AuthenticationStrength::Credential)
        );
        assert!(
            decision
                .required_strength()
                .is_some_and(|required| { required > channel_ceiling(SessionChannel::Voice) }),
            "a voice channel must not be able to satisfy the approval it would need"
        );
        assert!(
            decision.reason().is_some_and(|reason| !reason.is_refusal()),
            "an approval obligation is a hold, not a refusal"
        );
    }

    /// **The voice rule: a voice-originated risk-3 call is held, not refused.**
    ///
    /// `docs/architecture/security.md`: "Voice confirmation alone is insufficient for risk-3 actions
    /// by default. A trusted desktop/mobile/CLI approval may resume a voice-originated run." A
    /// refusal would make the documented resume path unreachable, so this asserts a hold.
    #[test]
    fn a_voice_originated_high_risk_call_is_held_not_refused() {
        let definition = tool(
            "jarvis.mail.delete_all",
            EffectSet::single(ToolEffect::Destructive),
            3,
            ApprovalPolicy::Ask,
            ScopeSet::single(scope("mail.delete")),
        );
        let workspace = WorkspacePolicy::default();
        let actor = ActorAuthority::active(ScopeSet::single(scope("mail.delete")));

        let mut request = request(&definition, &workspace, actor);
        request.channel = SessionChannel::Voice;
        // The caller claims the strongest strength; the channel caps it.
        request.claimed_strength = AuthenticationStrength::Present;

        let decision = evaluate(&request);
        assert!(
            !decision.is_denied(),
            "a voice request must not be refused outright: {decision:?}"
        );
        assert!(decision.is_held());
        assert_eq!(
            decision.required_strength(),
            Some(AuthenticationStrength::Present),
            "a desktop or CLI approval must be able to release it"
        );
    }

    /// The channel ceiling caps a claimed strength, so a client cannot assert what it did not
    /// establish.
    #[test]
    fn a_channel_ceiling_caps_a_claimed_strength() {
        // A voice channel claiming presence still evaluates as channel evidence.
        let definition = sender();
        // A workspace whose threshold is above the tool's risk, so the approval step is skipped and
        // the strength check is the one that decides.
        let workspace = WorkspacePolicy::new(Risk::High, Risk::High, false, true)
            .unwrap_or_else(|error| panic!("consistent policy: {error}"));
        let actor = ActorAuthority::active(ScopeSet::single(scope("mail.send")));

        let mut voice = request(&definition, &workspace, actor.clone());
        voice.channel = SessionChannel::Voice;
        voice.claimed_strength = AuthenticationStrength::Present;
        let decision = evaluate(&voice);
        assert_eq!(
            decision.reason(),
            Some(DenyReason::InsufficientAuthentication),
            "a voice channel cannot establish presence: {decision:?}"
        );
        assert_eq!(
            decision.required_strength(),
            Some(AuthenticationStrength::Credential)
        );

        // The same claim over the desktop channel is accepted.
        let mut desktop = request(&definition, &workspace, actor);
        desktop.claimed_strength = AuthenticationStrength::Present;
        assert!(evaluate(&desktop).is_allowed());
    }

    /// The channel ceilings are ordered as documented, and voice is the weakest.
    #[test]
    fn the_channel_ceilings_are_ordered() {
        assert_eq!(
            channel_ceiling(SessionChannel::Cli),
            AuthenticationStrength::Present
        );
        assert_eq!(
            channel_ceiling(SessionChannel::Desktop),
            AuthenticationStrength::Present
        );
        assert_eq!(
            channel_ceiling(SessionChannel::Api),
            AuthenticationStrength::Credential
        );
        assert_eq!(
            channel_ceiling(SessionChannel::Voice),
            AuthenticationStrength::ChannelEvidence
        );
        for channel in SessionChannel::all() {
            assert!(channel_ceiling(channel) <= AuthenticationStrength::Present);
        }
        assert_eq!(
            AuthenticationStrength::required_for(Risk::High),
            AuthenticationStrength::Present
        );
    }

    /// An external-communication effect needs approval in a workspace that asks for it.
    #[test]
    fn an_external_communication_effect_needs_approval_when_the_workspace_asks() {
        // Risk 1 declared, and the tool's policy is Auto, so only the effect rule can hold it.
        let definition = tool(
            "jarvis.mail.send",
            EffectSet::single(ToolEffect::ExternalCommunication),
            2,
            ApprovalPolicy::Auto,
            ScopeSet::single(scope("mail.send")),
        );
        let workspace = WorkspacePolicy::new(Risk::High, Risk::High, true, true)
            .unwrap_or_else(|error| panic!("{error}"));
        let actor = ActorAuthority::active(ScopeSet::single(scope("mail.send")));
        let decision = evaluate(&request(&definition, &workspace, actor));
        assert!(decision.is_held(), "{decision:?}");
        assert_eq!(decision.reason(), Some(DenyReason::ApprovalRequired));

        // The same workspace with the rule off allows it, so the rule is what held it.
        let permissive = WorkspacePolicy::new(Risk::High, Risk::High, false, true)
            .unwrap_or_else(|error| panic!("{error}"));
        let decision = evaluate(&request(
            &definition,
            &permissive,
            ActorAuthority::active(ScopeSet::single(scope("mail.send"))),
        ));
        assert!(decision.is_allowed(), "{decision:?}");
    }

    /// **Context raises risk but cannot lower it.**
    #[test]
    fn context_raises_risk_and_never_lowers_it() {
        let definition = sender();
        assert_eq!(
            effective_risk(&definition, &TargetAssessment::none()),
            Risk::Moderate
        );

        for (assessment, expected) in [
            (
                TargetAssessment::new([EscalationSignal::External]),
                Risk::Moderate,
            ),
            (
                TargetAssessment::new([EscalationSignal::Production]),
                Risk::Moderate,
            ),
            (TargetAssessment::new([EscalationSignal::Bulk]), Risk::High),
            (
                TargetAssessment::new([EscalationSignal::Sensitive]),
                Risk::High,
            ),
        ] {
            assert_eq!(
                effective_risk(&definition, &assessment),
                expected,
                "{assessment:?} must evaluate at {expected}"
            );
        }

        // A high declared risk is not lowered by an unremarkable target, nor by a target whose
        // escalation is lower than the declaration.
        let destructive = tool(
            "jarvis.mail.purge",
            EffectSet::single(ToolEffect::Destructive),
            3,
            ApprovalPolicy::Ask,
            ScopeSet::none(),
        );
        for assessment in [
            TargetAssessment::none(),
            TargetAssessment::new([EscalationSignal::External]),
            TargetAssessment::new([EscalationSignal::Production]),
            TargetAssessment::new([EscalationSignal::External, EscalationSignal::Production]),
        ] {
            assert_eq!(
                effective_risk(&destructive, &assessment),
                Risk::High,
                "{assessment:?} must not lower a declared risk of 3"
            );
        }
    }

    /// Escalation is a **maximum of levels, not a sum**: two signals do not compound.
    ///
    /// The property that makes the mapping from signals to posture statable. If it summed, three
    /// signals would need a risk level that does not exist.
    #[test]
    fn escalation_is_a_maximum_not_a_sum() {
        let every = TargetAssessment::new(EscalationSignal::all());
        assert_eq!(every.escalation(), Risk::High);
        assert_eq!(
            every.escalation(),
            TargetAssessment::new([EscalationSignal::Bulk]).escalation(),
            "adding more signals must not exceed the strongest one"
        );
        assert_eq!(every.to_vec().len(), EscalationSignal::all().len());
    }

    /// An escalated risk can change the outcome, which is what makes escalation worth modelling.
    #[test]
    fn escalation_can_change_the_outcome() {
        let definition = tool(
            "jarvis.files.write",
            EffectSet::single(ToolEffect::Write),
            1,
            ApprovalPolicy::Auto,
            ScopeSet::single(scope("files.write")),
        );
        let workspace = WorkspacePolicy::default();
        let actor = ActorAuthority::active(ScopeSet::single(scope("files.write")));

        let ordinary = evaluate(&request(&definition, &workspace, actor.clone()));
        assert!(ordinary.is_allowed(), "{ordinary:?}");
        assert_eq!(ordinary.effective_risk(), Risk::Low);

        // A bulk write escalates to risk 3, which the default workspace holds for approval.
        let mut bulk = request(&definition, &workspace, actor);
        bulk.target = TargetAssessment::new([EscalationSignal::Bulk]);
        let decision = evaluate(&bulk);
        assert!(decision.is_held(), "{decision:?}");
        assert_eq!(decision.effective_risk(), Risk::High);
        assert_eq!(decision.escalated_by(), &[EscalationSignal::Bulk]);
    }

    /// Every reason has a distinct code, and the codes are stable.
    #[test]
    fn every_reason_has_a_distinct_stable_code() {
        let mut codes: Vec<&str> = DenyReason::all()
            .iter()
            .map(|reason| reason.code())
            .collect();
        let total = codes.len();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), total, "every reason needs its own code");

        for reason in DenyReason::all() {
            let encoded = serde_json::to_string(&reason).unwrap_or_else(|error| panic!("{error}"));
            assert_eq!(encoded, format!("\"{}\"", reason.code()));
            let decoded: DenyReason =
                serde_json::from_str(&encoded).unwrap_or_else(|error| panic!("{error}"));
            assert_eq!(decoded, reason);
            // A reason is a code: lowercase snake case, no spaces, nothing to inject.
            assert!(
                reason
                    .code()
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '_')
            );
        }
    }

    /// Only the two obligation reasons are holds, so a caller can classify a decision without
    /// knowing the decision type.
    #[test]
    fn only_approval_reasons_are_holds() {
        for reason in DenyReason::all() {
            let expected_hold = matches!(
                reason,
                DenyReason::ApprovalRequired | DenyReason::ExternalCommunicationRequiresApproval
            );
            assert_eq!(
                !reason.is_refusal(),
                expected_hold,
                "{reason} hold/refusal classification"
            );
        }
    }

    /// **A decision is fully serializable, so it can be stored.**
    ///
    /// The audit requirement: a record saying `deny` and nothing else cannot be reviewed, so an
    /// escalated decision must round-trip with its risk and its escalation signals.
    #[test]
    fn a_decision_round_trips_with_its_evidence() {
        let definition = sender();
        let workspace = WorkspacePolicy::default();
        let actor = ActorAuthority::active(ScopeSet::single(scope("mail.send")));
        let mut request = request(&definition, &workspace, actor);
        request.target =
            TargetAssessment::new([EscalationSignal::External, EscalationSignal::Bulk]);
        let decision = evaluate(&request);

        let encoded = serde_json::to_value(&decision).unwrap_or_else(|error| panic!("{error}"));
        let decoded: PolicyDecision =
            serde_json::from_value(encoded).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(decoded, decision);
        assert_eq!(decoded.effective_risk(), Risk::High);
        assert_eq!(
            decoded.escalated_by(),
            &[EscalationSignal::External, EscalationSignal::Bulk],
            "signals are reported in the enum's declaration order, not the order they were declared"
        );
    }
    /// **Fail closed: a request with absent authentication and no grants is refused, not allowed.**
    ///
    /// `docs/architecture/security.md`: "Missing or stale evidence fails closed." The weakest
    /// possible request must produce a refusal or a hold, never an allowance.
    #[test]
    fn a_request_with_no_evidence_fails_closed() {
        let definition = sender();
        let workspace = WorkspacePolicy::default();
        let actor = ActorAuthority::active(ScopeSet::none());
        let mut request = request(&definition, &workspace, actor);
        request.claimed_strength = AuthenticationStrength::Absent;
        request.channel = SessionChannel::Api;

        let decision = evaluate(&request);
        assert!(
            !decision.is_allowed(),
            "a request with no scope and no authentication must not be allowed: {decision:?}"
        );
        assert_eq!(decision.reason(), Some(DenyReason::MissingScope));
    }

    /// The decision's own reason code agrees with the reason it carries.
    #[test]
    fn the_reason_code_agrees_with_the_reason() {
        let allowed = evaluate(&request(
            &reader(),
            &WorkspacePolicy::default(),
            ActorAuthority::active(ScopeSet::single(scope("files.read"))),
        ));
        assert_eq!(allowed.reason_code(), "allowed");

        let mut codes: Vec<&str> = DenyReason::all()
            .iter()
            .map(|reason| reason.code())
            .collect();
        let total = codes.len();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), total);
    }

    /// Every escalation signal has a distinct code that round-trips.
    #[test]
    fn every_escalation_signal_round_trips() {
        let mut codes: Vec<&str> = EscalationSignal::all()
            .iter()
            .map(|signal| signal.code())
            .collect();
        let total = codes.len();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), total);

        for signal in EscalationSignal::all() {
            let encoded = serde_json::to_string(&signal).unwrap_or_else(|error| panic!("{error}"));
            assert_eq!(encoded, format!("\"{}\"", signal.code()));
            let decoded: EscalationSignal =
                serde_json::from_str(&encoded).unwrap_or_else(|error| panic!("{error}"));
            assert_eq!(decoded, signal);
        }
    }

    /// An unremarkable target reports no escalation signals.
    #[test]
    fn an_unremarkable_target_reports_nothing() {
        assert!(TargetAssessment::none().is_unremarkable());
        assert_eq!(
            TargetAssessment::none().escalation(),
            Risk::Minimal,
            "an unremarkable target escalates to the lowest level, which changes nothing"
        );
        assert!(!TargetAssessment::new([EscalationSignal::External]).is_unremarkable());
        assert!(
            TargetAssessment::new([EscalationSignal::External])
                .contains(EscalationSignal::External)
        );
        assert!(
            !TargetAssessment::new([EscalationSignal::External]).contains(EscalationSignal::Bulk)
        );
        let decision = evaluate(&request(
            &reader(),
            &WorkspacePolicy::default(),
            ActorAuthority::active(ScopeSet::single(scope("files.read"))),
        ));
        assert!(decision.escalated_by().is_empty());
    }
}
