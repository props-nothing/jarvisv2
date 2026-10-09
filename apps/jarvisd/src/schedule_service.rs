//! Scheduled tasks: the routes that manage them, and the scheduler that fires them.
//!
//! # A fire starts an ordinary run
//!
//! When a task is due the scheduler calls [`GatewayState::start_and_drive`] — the same function the REST route
//! uses. A scheduled run therefore has no authority an interactive one lacks: a tool the policy holds for a person
//! is held for a person, and the run parks until they decide. The scheduler never approves anything and cannot be
//! asked to.
//!
//! # Three decisions that make an unattended task safe to leave on
//!
//! 1. **At most once.** A fire is claimed (the next time advanced) before the run starts. A crash in between loses
//!    one fire and never repeats one, which is the right direction for a task that may fetch, compute, or write.
//! 2. **No backlog.** A machine that slept through ten fires owes one. The next is measured from when it woke.
//! 3. **No pile-up.** A fire is skipped while the previous run is still going — typically parked on an approval
//!    nobody has answered yet — and the skip is counted and shown, so a person returns to *one* request to decide,
//!    not forty identical ones.
//!
//! # What the model is told
//!
//! The objective is prefixed with a line saying the user is not present, because a scheduled run that ends with a
//! question has asked it of nobody. It is told to report briefly instead.

use std::time::Duration;

use axum::{
    Json,
    extract::{Path, RawQuery, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use jarvis_core::{
    Cadence, ErrorCode, InvalidSchedule, SystemClock, UtcTimestamp, parse_interval,
    validate_objective,
};
use jarvis_protocol::{
    CreateScheduleRequest, RunListReply, RunSummaryReply, ScheduleListReply, ScheduleReply,
    StartRunRequest,
};
use jarvis_storage::{DatabaseError, StoredSchedule};

use crate::gateway::{GatewayState, error_response};

/// How often the scheduler looks for due tasks.
pub const TICK: Duration = Duration::from_secs(5);

/// The most tasks one pass will fire, so a pass after a long absence cannot start an unbounded burst of runs.
const MAX_FIRES_PER_PASS: u32 = 20;

/// The most runs a list returns.
const MAX_RUN_LIST: u32 = 50;

/// The longest answer carried in a run list, in characters.
const MAX_LISTED_ANSWER_CHARS: usize = 4000;

/// Prefixed to a scheduled objective so the model knows nobody is there to answer a question.
const UNATTENDED_NOTICE: &str = "This is a scheduled task running while the user is away, so there is no one to \
ask. Do the work, make reasonable assumptions, and report the result briefly.\n\nTask: ";

/// What one scheduler pass did, for the log and for tests.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TickReport {
    /// Runs started.
    pub started: u32,
    /// Fires skipped because the previous run was still going.
    pub skipped: u32,
    /// Fires that could not start a run.
    pub failed: u32,
}

/// Starts the scheduler on its own task, which ends when the returned handle is aborted.
///
/// Only meaningful when the daemon drives runs; the caller checks.
pub fn spawn(state: GatewayState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(TICK);
        // A tick that overran is not made up for: the scheduler's job is "what is due now".
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut watcher = ResultWatcher::default();
        loop {
            interval.tick().await;
            watcher.pass(&state).await;
            match tick(&state, UtcTimestamp::now(&SystemClock)).await {
                Ok(report) if report != TickReport::default() => {
                    tracing::info!(
                        started = report.started,
                        skipped = report.skipped,
                        failed = report.failed,
                        "scheduler pass"
                    );
                }
                Ok(_) => {}
                Err(error) => tracing::warn!(%error, "a scheduler pass could not run"),
            }
        }
    })
}

/// When a console last asked for the schedules (the console polls every second or two while it is open), in Unix seconds.
static LAST_CONSOLE_POLL: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

/// A console polled this recently is open, and shows a scheduled result itself.
const CONSOLE_OPEN_WITHIN_SECONDS: i64 = 15;

fn unix_seconds() -> i64 {
    i64::try_from(UtcTimestamp::now(&SystemClock).unix_nanos() / 1_000_000_000).unwrap_or(0)
}

/// Notices scheduled runs that finish and, when no console is open to show them, says so with a desktop notification.
///
/// It watches the schedules rather than hooking the executor: a schedule's `last_run_id` changes exactly when a run was started
/// for it, and what existed when the daemon started is recorded first so a restart does not re-announce old results.
#[derive(Default)]
struct ResultWatcher {
    seen: Option<std::collections::HashMap<String, String>>,
    pending: Vec<(String, String)>,
}

impl ResultWatcher {
    async fn pass(&mut self, state: &GatewayState) {
        let Ok(identity) = jarvis_storage::load_local_identity(state.database()).await else {
            return;
        };
        let Ok(schedules) =
            jarvis_storage::list_schedules(state.database(), identity.workspace_id()).await
        else {
            return;
        };
        let first_pass = self.seen.is_none();
        let seen = self.seen.get_or_insert_with(std::collections::HashMap::new);
        for schedule in &schedules {
            let Some(run) = schedule.last_run_id() else {
                continue;
            };
            if seen.get(schedule.id()).map(String::as_str) != Some(run) {
                seen.insert(schedule.id().to_owned(), run.to_owned());
                if !first_pass {
                    self.pending
                        .push((run.to_owned(), schedule.objective().to_owned()));
                }
            }
        }
        let mut still_running = Vec::new();
        for (run_id, task) in std::mem::take(&mut self.pending) {
            match jarvis_storage::find_run(state.database(), &run_id).await {
                Ok(run) if run.state().is_terminal() => announce(state, &run, &task).await,
                Ok(_) => still_running.push((run_id, task)),
                Err(_) => {}
            }
        }
        self.pending = still_running;
    }
}

async fn announce(state: &GatewayState, run: &jarvis_storage::StoredRun, task: &str) {
    let console_open = unix_seconds()
        - LAST_CONSOLE_POLL.load(std::sync::atomic::Ordering::Relaxed)
        < CONSOLE_OPEN_WITHIN_SECONDS;
    let enabled = state.settings().is_none_or(|context| {
        jarvis_storage::ConfigStore::from_paths(context.paths())
            .load()
            .map_or(true, |loaded| {
                loaded.config().daemon().notifications_enabled()
            })
    });
    if console_open || !enabled {
        return;
    }
    let answer = if run.terminal_outcome() == Some(jarvis_core::RunOutcome::Succeeded) {
        crate::executor::last_answer(&state.database_handle(), run)
            .await
            .ok()
            .flatten()
    } else {
        None
    };
    let body = answer.unwrap_or_else(|| format!("This scheduled task did not finish: {task}"));
    tracing::info!(
        "a scheduled task finished with no console open, so it was shown as a desktop notification"
    );
    crate::notify::show("JARVIS", &body);
}

/// Runs one scheduler pass at `now`.
///
/// # Errors
///
/// Returns [`DatabaseError`] when due tasks cannot be claimed. A single task that cannot start is counted in the
/// report and does not stop the others.
pub async fn tick(state: &GatewayState, now: UtcTimestamp) -> Result<TickReport, DatabaseError> {
    let mut report = TickReport::default();
    let due =
        jarvis_storage::claim_due_schedules(state.database(), now, MAX_FIRES_PER_PASS).await?;
    for schedule in due {
        // The previous run is still going — most likely parked on an approval — so this fire would only queue a
        // second identical request behind the first. Skipped, counted, and visible in `jarvis schedule list`.
        if previous_run_is_active(state, &schedule).await {
            if let Err(error) =
                jarvis_storage::record_schedule_skip(state.database(), schedule.id()).await
            {
                tracing::warn!(%error, "a skipped fire could not be recorded");
            }
            report.skipped += 1;
            continue;
        }
        // A paused or finished project does not get fires: the task stays, waiting, and is counted as skipped.
        let project = jarvis_storage::project_for(
            state.database(),
            jarvis_storage::LinkKind::Schedule,
            schedule.id(),
        )
        .await
        .ok()
        .flatten();
        if project
            .as_ref()
            .is_some_and(|project| project.status != jarvis_storage::ProjectStatus::Active)
        {
            if let Err(error) =
                jarvis_storage::record_schedule_skip(state.database(), schedule.id()).await
            {
                tracing::warn!(%error, "a skipped fire could not be recorded");
            }
            report.skipped += 1;
            continue;
        }
        let request = StartRunRequest {
            objective: format!("{UNATTENDED_NOTICE}{}", schedule.objective()),
            session_id: schedule.session_id().map(str::to_owned),
            idempotency_key: None,
            project_id: project.map(|project| project.id),
        };
        if let Ok(reply) = state.start_and_drive(&request).await {
            if let Err(error) = jarvis_storage::record_schedule_run(
                state.database(),
                schedule.id(),
                &reply.run_id,
                &reply.session_id,
                now,
            )
            .await
            {
                tracing::warn!(%error, "a scheduled run could not be recorded against its task");
            }
            report.started += 1;
        } else {
            // The fire was claimed, so it is lost rather than retried: at most once. Logged with the task so an
            // operator can see which one.
            tracing::warn!(
                schedule_id = schedule.id(),
                "a scheduled run could not be started"
            );
            report.failed += 1;
        }
    }
    Ok(report)
}

async fn previous_run_is_active(state: &GatewayState, schedule: &StoredSchedule) -> bool {
    let Some(last) = schedule.last_run_id() else {
        return false;
    };
    match jarvis_storage::find_run(state.database(), last).await {
        Ok(run) => !run.state().is_terminal(),
        // A run that cannot be read is not a reason to block the task forever.
        Err(_) => false,
    }
}

/// The objective as a person would recognise it: a scheduled or delegated run's framing line is for the model, not
/// for a list.
fn shown_objective(objective: &str) -> String {
    if let Some(task) = objective.strip_prefix(UNATTENDED_NOTICE) {
        return format!("[scheduled] {task}");
    }
    if let Some(task) = objective.strip_prefix(crate::delegate::DELEGATED_NOTICE) {
        return format!("[sub-agent] {task}");
    }
    objective.to_owned()
}

/// The name of the project a schedule belongs to, if any.
async fn project_name(state: &GatewayState, schedule: &StoredSchedule) -> Option<String> {
    jarvis_storage::project_for(
        state.database(),
        jarvis_storage::LinkKind::Schedule,
        schedule.id(),
    )
    .await
    .ok()
    .flatten()
    .map(|project| project.name)
}

fn schedule_reply(schedule: &StoredSchedule, project: Option<String>) -> ScheduleReply {
    let (cadence, interval) = match schedule.cadence() {
        Cadence::Every(seconds) => ("every", Some(seconds)),
        Cadence::Once(_) => ("once", None),
    };
    ScheduleReply {
        schedule_id: schedule.id().to_owned(),
        objective: schedule.objective().to_owned(),
        cadence: cadence.to_owned(),
        interval_seconds: interval,
        enabled: schedule.enabled(),
        project,
        next_run_at: schedule.enabled().then_some(schedule.next_run()),
        session_id: schedule.session_id().map(str::to_owned),
        last_run_id: schedule.last_run_id().map(str::to_owned),
        last_fired_at: schedule.last_fired_at(),
        fire_count: schedule.fire_count(),
        skipped_count: schedule.skipped_count(),
        created_at: schedule.created_at(),
    }
}

/// Maps a storage error to the answer a client gets, never echoing a database's own text.
fn storage_response(error: &DatabaseError) -> Response {
    match error {
        DatabaseError::ScheduleNotFound => error_response(
            StatusCode::NOT_FOUND,
            ErrorCode::Validation,
            "no scheduled task exists for the requested identifier",
        ),
        DatabaseError::ScheduleLimit => error_response(
            StatusCode::CONFLICT,
            ErrorCode::Conflict,
            "this workspace already holds the maximum number of scheduled tasks",
        ),
        DatabaseError::ProjectNotFound => error_response(
            StatusCode::NOT_FOUND,
            ErrorCode::Validation,
            "no project has that name or identifier",
        ),
        DatabaseError::ScheduleFinished => error_response(
            StatusCode::CONFLICT,
            ErrorCode::Conflict,
            "a one-off task that has already fired cannot be resumed; create a new one",
        ),
        other => {
            tracing::warn!(error = %other, "a schedule request could not be served");
            error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                ErrorCode::Internal,
                "the local database is not available",
            )
        }
    }
}

fn invalid_response(error: InvalidSchedule) -> Response {
    error_response(
        StatusCode::UNPROCESSABLE_ENTITY,
        ErrorCode::Validation,
        &error.to_string(),
    )
}

// The error is the response itself, as every handler here returns it; boxing it would only add an unwrap at each use.
#[allow(clippy::result_large_err)]
async fn workspace(state: &GatewayState) -> Result<String, Response> {
    jarvis_storage::load_local_identity(state.database())
        .await
        .map(|identity| identity.workspace_id().to_owned())
        .map_err(|_| {
            error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                ErrorCode::Internal,
                "the local database is not available",
            )
        })
}

/// `POST /api/v1/schedules`
pub async fn create(
    State(state): State<GatewayState>,
    Json(request): Json<CreateScheduleRequest>,
) -> Response {
    let objective = match validate_objective(&request.objective) {
        Ok(objective) => objective,
        Err(error) => return invalid_response(error),
    };
    let now = UtcTimestamp::now(&SystemClock);
    let cadence = match (&request.every, request.at) {
        (Some(every), None) => parse_interval(every).and_then(Cadence::every_seconds),
        (None, Some(at)) => Cadence::once_at(at, now),
        _ => Err(InvalidSchedule::ExactlyOneCadence),
    };
    let cadence = match cadence {
        Ok(cadence) => cadence,
        Err(error) => return invalid_response(error),
    };
    let workspace_id = match workspace(&state).await {
        Ok(id) => id,
        Err(response) => return response,
    };
    let project = match &request.project_id {
        Some(name) => {
            match jarvis_storage::find_project(state.database(), &workspace_id, name).await {
                Ok(project) => Some(project),
                Err(error) => return storage_response(&error),
            }
        }
        None => None,
    };
    match jarvis_storage::create_schedule(state.database(), &workspace_id, &objective, cadence, now)
        .await
    {
        Ok(schedule) => {
            if let Some(project) = &project {
                let linked = jarvis_storage::link_project(
                    state.database(),
                    jarvis_storage::LinkKind::Schedule,
                    schedule.id(),
                    &project.id,
                )
                .await;
                if let Err(error) = linked {
                    return storage_response(&error);
                }
            }
            let reply = schedule_reply(&schedule, project.map(|project| project.name));
            (StatusCode::CREATED, Json(reply)).into_response()
        }
        Err(error) => storage_response(&error),
    }
}

/// `GET /api/v1/schedules`
pub async fn list(State(state): State<GatewayState>) -> Response {
    LAST_CONSOLE_POLL.store(unix_seconds(), std::sync::atomic::Ordering::Relaxed);
    let workspace_id = match workspace(&state).await {
        Ok(id) => id,
        Err(response) => return response,
    };
    match jarvis_storage::list_schedules(state.database(), &workspace_id).await {
        Ok(schedules) => {
            let mut replies = Vec::with_capacity(schedules.len());
            for schedule in &schedules {
                replies.push(schedule_reply(
                    schedule,
                    project_name(&state, schedule).await,
                ));
            }
            let reply = ScheduleListReply {
                total: replies.len(),
                schedules: replies,
            };
            (StatusCode::OK, Json(reply)).into_response()
        }
        Err(error) => storage_response(&error),
    }
}

/// `DELETE /api/v1/schedules/{id}`
pub async fn remove(State(state): State<GatewayState>, Path(id): Path<String>) -> Response {
    let workspace_id = match workspace(&state).await {
        Ok(id) => id,
        Err(response) => return response,
    };
    match jarvis_storage::delete_schedule(state.database(), &workspace_id, &id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => storage_response(&error),
    }
}

async fn set_enabled(state: &GatewayState, id: &str, enabled: bool) -> Response {
    let workspace_id = match workspace(state).await {
        Ok(id) => id,
        Err(response) => return response,
    };
    match jarvis_storage::set_schedule_enabled(
        state.database(),
        &workspace_id,
        id,
        enabled,
        UtcTimestamp::now(&SystemClock),
    )
    .await
    {
        Ok(schedule) => {
            let project = project_name(state, &schedule).await;
            (StatusCode::OK, Json(schedule_reply(&schedule, project))).into_response()
        }
        Err(error) => storage_response(&error),
    }
}

/// `POST /api/v1/schedules/{id}/pause`
pub async fn pause(State(state): State<GatewayState>, Path(id): Path<String>) -> Response {
    set_enabled(&state, &id, false).await
}

/// `POST /api/v1/schedules/{id}/resume`
pub async fn resume(State(state): State<GatewayState>, Path(id): Path<String>) -> Response {
    set_enabled(&state, &id, true).await
}

/// `GET /api/v1/runs?limit=N`
///
/// The recent runs with what each answered. A scheduled run has nobody watching its stream, so this is how a
/// person finds out what it said.
pub async fn list_runs(State(state): State<GatewayState>, RawQuery(raw): RawQuery) -> Response {
    let limit = match crate::gateway::parse_limit(raw.as_deref(), 10) {
        Ok(limit) if (1..=MAX_RUN_LIST).contains(&limit) => limit,
        Ok(_) => {
            return error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                ErrorCode::Validation,
                "the limit must be between 1 and 50",
            );
        }
        Err(message) => {
            return error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                ErrorCode::Validation,
                message,
            );
        }
    };
    let workspace_id = match workspace(&state).await {
        Ok(id) => id,
        Err(response) => return response,
    };
    let runs = match jarvis_storage::read_recent_runs(state.database(), &workspace_id, limit).await
    {
        Ok(runs) => runs,
        Err(error) => return storage_response(&error),
    };
    let mut summaries = Vec::with_capacity(runs.len());
    for run in &runs {
        // Only a run that has produced an answer has one to show; an unreadable stream is "no answer", not a
        // failure of the whole list.
        let database = std::sync::Arc::clone(&state.database_handle());
        let answer = crate::executor::last_answer(&database, run)
            .await
            .ok()
            .flatten()
            .map(|text| {
                text.chars()
                    .take(MAX_LISTED_ANSWER_CHARS)
                    .collect::<String>()
            });
        summaries.push(RunSummaryReply {
            run_id: run.id().to_owned(),
            session_id: run.session_id().to_owned(),
            objective: shown_objective(run.objective()),
            state: run.state(),
            outcome: run.terminal_outcome(),
            error_code: run.error_code().map(str::to_owned),
            started_at: run.started_at(),
            completed_at: run.completed_at(),
            answer,
        });
    }
    let reply = RunListReply {
        total: summaries.len(),
        runs: summaries,
    };
    (StatusCode::OK, Json(reply)).into_response()
}

#[cfg(test)]
#[path = "schedule_service_tests.rs"]
mod tests;
