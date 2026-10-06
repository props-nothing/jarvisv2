//! Versioned REST DTOs for the tool control-plane surface.
//!
//! `docs/architecture/tools-and-connectors.md` gives every capability a typed contract with effects,
//! risk, and an approval policy, and `P3-025` made those configurable per tool. This module is the
//! **wire** form of that surface, so a CLI or a control-plane UI can answer the two questions an
//! operator actually asks: *what tools does this daemon have*, and *what would happen if one were
//! called right now*.
//!
//! # Why an inventory needs its own reply rather than reusing discovery
//!
//! `ToolRegistry::discover()` is the **model-facing** surface, and `ToolSummary` is deliberately
//! narrow: it omits `required_scopes`, `approval`, and the schemas because a model selects on what a
//! tool does and must not act on authorization. An operator needs the opposite — the approval policy,
//! the required scopes, and *why* a tool is not callable. Reusing the summary would either leak
//! authorization vocabulary into a model-facing shape or leave an operator unable to see the posture
//! they just configured.
//!
//! # The closed sets are `jarvis-core`'s types, not `String`s
//!
//! `Risk`, `ApprovalPolicy`, and `EscalationSignal` are domain vocabulary that this crate can name
//! directly, so a mistyped policy or an unknown escalation signal is a **`422` naming the field**
//! rather than a silently defaulted value — and a default here would be the *most permissive* reading
//! of a value a caller mistyped, which is the one direction this surface must never fail toward.
//!
//! This is the payoff of `ADR-0122`: those types belong to `jarvis-core` precisely because a wire
//! contract needs them and may not depend on the adapter crate that used to define them. A `String`
//! field plus a hand-written membership check would have been a second statement of each set, which is
//! the defect the move removed.
//!
//! # Unknown fields are tolerated on response
//!
//! Every type here except [`ToolPreviewRequest`] is a response, so the rule from `rest.rs` applies
//! unchanged: a newer daemon's additive field must not break an older client.

use jarvis_core::{ApprovalPolicy, EscalationSignal, Risk};
use serde::{Deserialize, Serialize};

/// One tool as an operator sees it.
///
/// Carries the authorization facts `ToolSummary` omits, because this is the shape a person uses to
/// decide whether a tool's posture is right — not the shape a model uses to choose a tool.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "a wire shape: each flag is an independent fact a client reads by name"
)]
pub struct ToolReply {
    /// The canonical identifier, which is also the name a policy override addresses.
    pub id: String,
    /// The operator- and model-facing title.
    pub title: String,
    /// The declared behaviour/schema version.
    pub version: String,
    /// Which class of source the tool came from (`native`, `connector`, `mcp`, ...).
    pub source: String,
    /// The declared baseline risk.
    pub risk: Risk,
    /// The effects the tool may have, as stable names.
    ///
    /// Names rather than a typed set because the effect vocabulary lives in `jarvis-tools`, which this
    /// crate may not depend on. The names are this crate's contract with a client, and the daemon renders
    /// them from the adapter's own `as_str` — so there is one spelling, in the crate that owns the type.
    pub effects: Vec<String>,
    /// The capability scopes the tool requires.
    pub required_scopes: Vec<String>,
    /// The approval policy the **tool itself** declares.
    ///
    /// Distinct from [`Self::effective_approval`], and both are reported for the same reason a
    /// decision reports its effective risk beside the declared one: an operator who sees only the
    /// effective value cannot tell whether they configured it or the tool's author did.
    pub declared_approval: ApprovalPolicy,
    /// The approval policy actually in force for this tool, after any workspace override.
    pub effective_approval: ApprovalPolicy,
    /// Whether this tool's approval is currently raised by a workspace override.
    ///
    /// Reported explicitly rather than left to a client comparing the two fields, because "why is
    /// this asking when the tool says auto" is the question the override exists to answer, and a
    /// client-side diff would be a second implementation of the direction rule.
    pub overridden: bool,
    /// Whether the tool is refused outright by the workspace.
    ///
    /// A denial outranks every grant and every approval, so an operator must be able to see it
    /// without inferring it from a call that was refused.
    pub denied: bool,
    /// Whether the tool can currently run.
    pub callable: bool,
    /// Why it cannot, when it cannot.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
    /// Whether a call is held for the owner's yes or no before it runs, under the policy now in force.
    #[serde(default)]
    pub asks_first: bool,
}

/// Response body for `GET /api/v1/tools`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolListReply {
    /// The tools, in stable identifier order.
    pub tools: Vec<ToolReply>,
    /// How many tools are registered at all.
    ///
    /// Equal to `tools.len()` today, and carried anyway so a later bound or filter does not silently
    /// change what a client believes it received — the same reasoning `DiscoveryReport` gives for its
    /// own totals.
    pub total: usize,
    /// The highest risk the workspace permits at all, as a policy-level setting.
    ///
    /// Reported beside the per-tool entries because a per-tool decision is only readable against it: the
    /// same tool may be callable in one workspace and refused in another, and an operator looking at a
    /// single tool cannot tell which ceiling they are under. This is the value `evaluate` step 4 compares
    /// against, read from the same policy the tools were projected from.
    pub max_risk: Risk,
    /// The risk at which an approval is always required.
    ///
    /// Also a workspace-level setting, and reported for the same reason: `declared_approval` says what the
    /// tool's author asked for, while this says what the workspace asks for regardless — and a tool held by
    /// the *threshold* looks identical to one held by its own declaration unless both are visible.
    pub approval_threshold: Risk,
}

/// Request body for `POST /api/v1/tools/{tool}/preview`.
///
/// # What a caller may and may not supply
///
/// Only the **context signals** are supplied, because they are properties of the call being previewed
/// that only the caller knows. The actor's scopes, the workspace policy, and the tool definition are
/// **not** — they come from the daemon's own state, and accepting them would make the preview a way to
/// ask "what if I had different permissions", which is a question an authorization surface must not
/// answer.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ToolPreviewRequest {
    /// Context signals that would raise the risk.
    ///
    /// Supplied because a preview of "send this to a bulk external list" is a different question from
    /// a preview of "send this to one internal address", and only the caller knows which it is asking
    /// about.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub escalation: Vec<EscalationSignal>,
}

/// Response body for `POST /api/v1/tools/{tool}/preview`.
///
/// # Why the reason code is reported and not just the outcome
///
/// A decision that returns only "deny" is unusable for the operator this surface exists for: they
/// cannot answer *why* without reproducing the decision by hand, and the remedy differs per reason —
/// a missing scope is a grant to add, a denial is a configuration line to remove, an approval is a
/// human to find. The code is the same stable snake-case vocabulary `PolicyDecision` stores.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolPreviewReply {
    /// The tool the preview was computed for.
    pub tool: String,
    /// The decision, as its stable snake-case name (`allow`, `require_approval`, `deny`).
    ///
    /// A name rather than a typed value because `Decision` lives in `jarvis-tools`. The daemon renders
    /// it from the decision's own accessor, so a reordered variant cannot change what a stored or
    /// transmitted value means.
    pub decision: String,
    /// The stable reason code, or `allowed`.
    pub reason: String,
    /// The risk the decision was taken at, including any escalation the context applied.
    pub effective_risk: Risk,
    /// The risk the tool declares, before any context escalation.
    ///
    /// Reported beside [`Self::effective_risk`] so an operator can see that a call was held *because
    /// of the context they described* rather than because of the tool's own declaration.
    pub declared_risk: Risk,
    /// The context signals that raised the risk.
    pub escalated_by: Vec<EscalationSignal>,
}
