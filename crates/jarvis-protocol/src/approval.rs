//! Request and reply bodies for the approval decision route.
//!
//! # Why the decision carries a nonce and the request does not
//!
//! An approval exists to stop the party that *asked* for an action from *answering* for it
//! (`docs/architecture/security.md` names the threat as model self-approval / confused deputy). So the
//! one-time decision nonce must reach the **human** and must not reach the **requester**. It travels to
//! the human through a profile-private file — the `ADR-0042` channel — and the human's own client
//! presents it here.
//!
//! That is also why this type is a **separate document** from `StartRunRequest` and the tool-call body
//! rather than a field on either: both of those are produced on a path the agent drives, and a nonce in
//! either one would be handed to the party it exists to exclude.
//!
//! # Why the approver is absent
//!
//! There is deliberately **no** `approver_id` field. The identity that answers is taken from the
//! authenticated session, exactly as a tool call's workspace is taken from the stored run rather than
//! from the request. A caller that could name its own approver would defeat the self-approval refusal in
//! one field, and `deny_unknown_fields` makes an attempt to supply one a `422` rather than a silently
//! ignored value — *an ignored field reads as an accepted one*, which is the same reasoning the tool
//! call body's absent `workspace_id` follows.

use jarvis_core::UtcTimestamp;
use serde::{Deserialize, Serialize};

/// The stable wire name of a decision, so a client and the daemon cannot disagree about the spelling.
///
/// Mirrors `jarvis_core::ApprovalDecisionOutcome` rather than re-exporting it, because the wire form is a
/// contract with clients while the domain enum is a closed set the daemon owns. The two are converted in
/// one place (the decision service), so a difference is a compile error rather than a wire surprise.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecisionRequest {
    /// Allow the action.
    Approve,
    /// Refuse the action.
    Deny,
    /// Withdraw the request without deciding it.
    Cancel,
}

/// Request body for `POST /api/v1/approvals/{id}/decision`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalDecisionBody {
    /// Which way the decision went.
    pub decision: ApprovalDecisionRequest,
    /// The one-time nonce delivered through the profile's private nonce file.
    ///
    /// Required for every outcome, including a denial. Skipping the check for a denial would let
    /// anything that can reach this route cancel a pending action — a denial-of-service on the approval
    /// rather than a decision about it — and "a denial is the safe outcome" is exactly the argument that
    /// would make a forged one indistinguishable from an ordinary refusal.
    pub nonce: String,
}

/// Response body for `POST /api/v1/approvals/{id}/decision`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ApprovalReply {
    /// The approval that was decided.
    pub approval_id: String,
    /// The run the approval belongs to.
    pub run_id: String,
    /// The tool the decision authorizes or refuses.
    pub tool: String,
    /// The tool version the intent was built against, so a client can see the decision bound to a
    /// specific contract rather than to a name.
    pub tool_version: String,
    /// The state at the instant the daemon answered.
    ///
    /// Reported as the **effective** state (`state_at`), not the stored one: an approval decided within
    /// its lifetime but read after the expiry is `expired`, and reporting the stored `approved` would
    /// tell a client an authority exists that no longer does.
    pub state: String,
    /// The decision's outcome, absent while the approval is undecided.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    /// When the request was created.
    pub created_at: UtcTimestamp,
    /// When it lapses.
    pub expires_at: UtcTimestamp,
}
