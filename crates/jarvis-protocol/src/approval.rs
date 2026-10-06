//! Request and reply bodies for the approval decision route.
//!
//! # Why the decision is its own document
//!
//! A decision is a different act from requesting or resuming a tool call: it is the owner's answer to a
//! plain yes/no question. Keeping it as its own body keeps "start this run", "resume this held call", and
//! "answer this approval" as three distinct contracts.
//!
//! # Why the approver is absent
//!
//! There is deliberately **no** `approver_id` field. The identity that answers is taken from the
//! authenticated session, exactly as a tool call's workspace is taken from the stored run rather than
//! from the request. `deny_unknown_fields` makes an attempt to supply one a `422` rather than a silently
//! ignored value — *an ignored field reads as an accepted one*, which is the same reasoning the tool call
//! body's absent `workspace_id` follows.

use jarvis_core::{ApprovalChannel, UtcTimestamp};
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
}

/// Request body for `POST /api/v1/approvals/{id}/decision`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalDecisionBody {
    /// Which way the decision went.
    pub decision: ApprovalDecisionRequest,
    /// Whether the daemon should release the approved call as part of the decision, from the arguments it held.
    ///
    /// Opt-in and defaulting to `false`, because clients that predate it decide and then resume the call
    /// themselves, and a resume that already happened is refused. Ignored for a denial.
    #[serde(default)]
    pub resume: bool,
    /// Which surface the owner answered on.
    ///
    /// Optional so older clients remain valid; the daemon treats an absent value as `api`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<ApprovalChannel>,
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
/// One pending approval, with what a person needs to decide it.
///
/// Returned by `GET /api/v1/approvals`. It carries the **arguments** the call was made with, which is the
/// reason this type exists beside [`ApprovalReply`]: a decision reply reports that a decision happened, while
/// this lets a person see **what they are about to approve**.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PendingApprovalReply {
    /// The approval's identifier, which a decision names.
    pub approval_id: String,
    /// `pending` while a person has yet to decide it, or `approved` when they have and the call has not run.
    ///
    /// An `approved` entry is what a daemon that died between the decision and the release leaves behind; it is
    /// finished with `POST /approvals/{id}/resume`.
    #[serde(default = "pending_state")]
    pub state: String,
    /// The run that is waiting on it.
    pub run_id: String,
    /// The held call, which is what a resume names.
    pub call_id: String,
    /// The tool the call would run.
    pub tool: String,
    /// The tool version the intent is bound to.
    pub tool_version: String,
    /// The risk level the call was held at.
    pub risk_level: u8,
    /// A short description of the action.
    pub preview: String,
    /// The arguments the call was made with, absent when they were too large to hold.
    ///
    /// When absent the approval is **not decidable from a client that must show what it approves**, which
    /// is the fail-closed direction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<serde_json::Value>,
    /// When the request was created.
    pub created_at: UtcTimestamp,
    /// When it lapses.
    pub expires_at: UtcTimestamp,
}

/// Response body for `GET /api/v1/approvals`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ApprovalListReply {
    /// How many approvals are pending.
    pub total: usize,
    /// The pending approvals, newest first.
    pub approvals: Vec<PendingApprovalReply>,
}

/// The default for [`PendingApprovalReply::state`], so a reply from a daemon that predates the field decodes.
fn pending_state() -> String {
    "pending".to_owned()
}
