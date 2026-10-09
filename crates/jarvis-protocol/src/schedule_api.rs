//! Request and reply bodies for scheduled tasks and the run list.
//!
//! # A schedule asks for a run, it does not carry authority
//!
//! The request names an **objective** and **when**, and nothing else: no tool, no scope, no approval. When a task
//! fires the daemon starts an ordinary run, so everything an ordinary run needs permission for is still asked of a
//! person. A client therefore cannot use this surface to widen what an unattended run may do, and `deny_unknown_fields`
//! turns an attempt to send such a field into a `422` rather than a silently ignored value.

use jarvis_core::{RunOutcome, RunState, UtcTimestamp};
use serde::{Deserialize, Serialize};

/// Request body for `POST /api/v1/schedules`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreateScheduleRequest {
    /// What each run is asked to do.
    pub objective: String,
    /// A repeat interval such as `30m`, `6h` or `2d`. Exactly one of `every` and `at`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub every: Option<String>,
    /// A single future instant (RFC 3339). Exactly one of `every` and `at`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<UtcTimestamp>,
    /// The project every run of this task belongs to, by identifier or name (ADR-0151).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
}

/// One scheduled task.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ScheduleReply {
    /// The task's identifier.
    pub schedule_id: String,
    /// What each run is asked to do.
    pub objective: String,
    /// `every` or `once`.
    pub cadence: String,
    /// The repeat interval in seconds, present for `every`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval_seconds: Option<u64>,
    /// Whether it is active. A paused task, and a one-off that has fired, are not.
    pub enabled: bool,
    /// The name of the project it belongs to, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// When it next fires, present only while it is active.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_run_at: Option<UtcTimestamp>,
    /// The conversation its runs share, once the first has started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// The most recent run it started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_id: Option<String>,
    /// When it last started a run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_fired_at: Option<UtcTimestamp>,
    /// How many runs it has started.
    pub fire_count: u32,
    /// How many fires it skipped because the previous run was still going.
    pub skipped_count: u32,
    /// When it was created.
    pub created_at: UtcTimestamp,
}

/// Response body for `GET /api/v1/schedules`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ScheduleListReply {
    /// How many tasks there are.
    pub total: usize,
    /// The tasks, oldest first.
    pub schedules: Vec<ScheduleReply>,
}

/// One run in the recent-runs list, with what it answered.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunSummaryReply {
    /// The run's identifier.
    pub run_id: String,
    /// The conversation it belongs to.
    pub session_id: String,
    /// What it was asked to do.
    pub objective: String,
    /// Its lifecycle state.
    pub state: RunState,
    /// How it ended, once it has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<RunOutcome>,
    /// The bounded failure code, for a failed run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    /// When it started.
    pub started_at: UtcTimestamp,
    /// When it settled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<UtcTimestamp>,
    /// The answer it produced, bounded, once it has produced one.
    ///
    /// Model output: a client shows it as text and never as an instruction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
}

/// Response body for `GET /api/v1/runs`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunListReply {
    /// How many runs are listed.
    pub total: usize,
    /// The runs, newest first.
    pub runs: Vec<RunSummaryReply>,
}
