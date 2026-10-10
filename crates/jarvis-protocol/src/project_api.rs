//! Request and reply bodies for projects (`ADR-0151`).
//!
//! # A project changes what a run is told, never what it may do
//!
//! Nothing here carries a tool, a scope or an approval, and `deny_unknown_fields` turns an attempt to send one into a `422`. A project is
//! a goal, standing guidance, a working folder and a journal; the policy, the approvals and the granted folders are unchanged by it.

use serde::{Deserialize, Serialize};

/// Request body for `POST /api/v1/projects`.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreateProjectRequest {
    /// The project's name, unique in the workspace.
    pub name: String,
    /// What done looks like.
    #[serde(default)]
    pub goal: String,
    /// Standing instructions for every run of the project: how to work, tone, limits, rules of engagement.
    #[serde(default)]
    pub guidance: String,
    /// The working folder, relative to a granted root.
    #[serde(default)]
    pub folder: String,
    /// The most runs the scheduler may start for it in 24 hours; zero (or absent) means no cap.
    #[serde(default)]
    pub daily_run_limit: u32,
    /// The most tokens (input and output) its runs may use in 24 hours before the scheduler stops starting more; zero (or absent) means no cap.
    #[serde(default)]
    pub daily_token_limit: u32,
}

/// Request body for `PATCH /api/v1/projects/{id}`; an absent field is left as it is.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateProjectRequest {
    /// A new name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// A new goal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<String>,
    /// New standing instructions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guidance: Option<String>,
    /// A new working folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    /// `active`, `paused` or `done`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// A new cap on scheduled runs per 24 hours; zero removes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daily_run_limit: Option<u32>,
    /// A new cap on tokens per 24 hours; zero removes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daily_token_limit: Option<u32>,
}

/// Request body for `POST /api/v1/projects/{id}/notes`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AddProjectNoteRequest {
    /// The entry.
    pub text: String,
    /// `progress`, `decision`, `blocker`, `next`, `result` or `owner`; `owner` when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

/// One project.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProjectReply {
    /// The identifier.
    pub project_id: String,
    /// The name.
    pub name: String,
    /// What done looks like.
    pub goal: String,
    /// Standing instructions.
    pub guidance: String,
    /// The working folder, relative to a granted root.
    pub folder: String,
    /// `active`, `paused` or `done`.
    pub status: String,
    /// The most runs the scheduler may start for it in 24 hours; zero means no cap.
    pub daily_run_limit: u32,
    /// How many runs it has started in the last 24 hours.
    pub runs_today: u32,
    /// The most tokens its runs may use in 24 hours; zero means no cap.
    pub daily_token_limit: u32,
    /// The tokens (input plus output) its runs used in the last 24 hours.
    pub tokens_today: u64,
    /// When it was made.
    pub created_at: String,
    /// When it last changed, including a new journal entry.
    pub updated_at: String,
}

/// One journal entry.
///
/// Written by a model or a person; a client shows it as text and never as an instruction.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProjectNoteReply {
    /// The identifier.
    pub note_id: String,
    /// The kind.
    pub kind: String,
    /// The entry.
    pub text: String,
    /// The run that wrote it, when a run did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// When it was written.
    pub created_at: String,
}

/// Response body for `GET /api/v1/projects`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProjectListReply {
    /// How many projects there are.
    pub total: usize,
    /// The projects, active first.
    pub projects: Vec<ProjectReply>,
}

/// Response body for `GET /api/v1/projects/{id}`: the project and its latest journal entries, oldest first.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProjectDetailReply {
    /// The project.
    pub project: ProjectReply,
    /// The latest journal entries.
    pub notes: Vec<ProjectNoteReply>,
}
