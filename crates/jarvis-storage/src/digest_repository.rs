//! What happened over a stretch of time, read from the runs and the project journals (`ADR-0155`).
//!
//! A person who comes back after a day wants the answer to "what did it do while I was away" without opening every
//! conversation. Everything here is a read over rows that already exist: no new record is kept for it.

use jarvis_core::UtcTimestamp;
use sqlx::Row;

use crate::database::{DatabaseError, SqliteDatabase};
use crate::project_repository::StoredProjectNote;

/// How many runs of each kind a window held, and what they cost.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunTotals {
    /// Every run that started in the window.
    pub runs: u32,
    /// Runs that finished with an answer.
    pub succeeded: u32,
    /// Runs that failed.
    pub failed: u32,
    /// Runs that were cancelled.
    pub cancelled: u32,
    /// Runs parked on an approval, which only the owner can move.
    pub waiting: u32,
    /// Prompt tokens the model was sent.
    pub input_tokens: u64,
    /// Tokens the model wrote.
    pub output_tokens: u64,
}

/// One project's share of a window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectActivity {
    /// The project's identifier.
    pub project_id: String,
    /// Its name.
    pub name: String,
    /// Runs its conversations started in the window.
    pub runs: u32,
    /// Its newest decisions, results, blockers and owner notes written in the window.
    pub highlights: Vec<StoredProjectNote>,
}

/// A failed run, with why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailedRun {
    /// The run.
    pub run_id: String,
    /// What it was asked, as stored.
    pub objective: String,
    /// The stored failure code.
    pub error_code: String,
}

/// A window's worth of activity.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StoredDigest {
    /// Counts and tokens over every run.
    pub totals: RunTotals,
    /// Projects that had runs, busiest first.
    pub projects: Vec<ProjectActivity>,
    /// The newest failed runs.
    pub problems: Vec<FailedRun>,
}

/// The most projects, highlights per project and failed runs a digest lists.
const MAX_PROJECTS: i64 = 20;
const MAX_HIGHLIGHTS: i64 = 5;
const MAX_PROBLEMS: i64 = 5;

fn sqlite(operation: &'static str) -> impl FnOnce(sqlx::Error) -> DatabaseError {
    move |source| DatabaseError::Sqlite { operation, source }
}

fn count(row: &sqlx::sqlite::SqliteRow, column: &'static str) -> Result<u64, DatabaseError> {
    let value: i64 = row
        .try_get(column)
        .map_err(sqlite("decode a digest count"))?;
    Ok(u64::try_from(value).unwrap_or(0))
}

/// The newest failed runs that started in the window.
async fn failed_runs(
    database: &SqliteDatabase,
    workspace_id: &str,
    since: &str,
) -> Result<Vec<FailedRun>, DatabaseError> {
    let rows = sqlx::query(
        "SELECT id, objective, error_code FROM agent_runs \
         WHERE workspace_id = ?1 AND state = 'failed' AND started_at >= ?2 ORDER BY id DESC LIMIT ?3",
    )
    .bind(workspace_id)
    .bind(since)
    .bind(MAX_PROBLEMS)
    .fetch_all(database.pool())
    .await
    .map_err(sqlite("list the failed runs of a digest"))?;
    let mut problems = Vec::with_capacity(rows.len());
    for row in &rows {
        problems.push(FailedRun {
            run_id: row.try_get("id").map_err(sqlite("decode a failed run"))?,
            objective: row
                .try_get("objective")
                .map_err(sqlite("decode a failed run"))?,
            error_code: row
                .try_get("error_code")
                .map_err(sqlite("decode a failed run"))?,
        });
    }
    Ok(problems)
}
/// Summarises the runs that started at or after `since`.
///
/// # Errors
///
/// Returns [`DatabaseError`] when a read fails.
pub async fn digest_since(
    database: &SqliteDatabase,
    workspace_id: &str,
    since: UtcTimestamp,
) -> Result<StoredDigest, DatabaseError> {
    let since = since.to_string();
    let row = sqlx::query(
        "SELECT COUNT(*) AS runs, \
                COALESCE(SUM(CASE WHEN state = 'completed' THEN 1 ELSE 0 END), 0) AS succeeded, \
                COALESCE(SUM(CASE WHEN state = 'failed' THEN 1 ELSE 0 END), 0) AS failed, \
                COALESCE(SUM(CASE WHEN state = 'cancelled' THEN 1 ELSE 0 END), 0) AS cancelled, \
                COALESCE(SUM(CASE WHEN state = 'awaiting_approval' THEN 1 ELSE 0 END), 0) AS waiting \
         FROM agent_runs WHERE workspace_id = ?1 AND started_at >= ?2",
    )
    .bind(workspace_id)
    .bind(&since)
    .fetch_one(database.pool())
    .await
    .map_err(sqlite("total the runs of a digest"))?;
    // The run rows do not carry token counts; every model call records its own `usage_updated` event, so the cost of a window is
    // the sum of those.
    let usage = sqlx::query(
        "SELECT COALESCE(SUM(json_extract(event.payload, '$.input_tokens')), 0) AS input_tokens, \
                COALESCE(SUM(json_extract(event.payload, '$.output_tokens')), 0) AS output_tokens \
         FROM run_events AS event JOIN agent_runs AS run ON run.id = event.run_id \
         WHERE run.workspace_id = ?1 AND run.started_at >= ?2 AND event.kind = 'usage_updated'",
    )
    .bind(workspace_id)
    .bind(&since)
    .fetch_one(database.pool())
    .await
    .map_err(sqlite("total the tokens of a digest"))?;
    let small = |column| count(&row, column).map(|value| u32::try_from(value).unwrap_or(u32::MAX));
    let totals = RunTotals {
        runs: small("runs")?,
        succeeded: small("succeeded")?,
        failed: small("failed")?,
        cancelled: small("cancelled")?,
        waiting: small("waiting")?,
        input_tokens: count(&usage, "input_tokens")?,
        output_tokens: count(&usage, "output_tokens")?,
    };

    let rows = sqlx::query(
        "SELECT p.id AS id, p.name AS name, COUNT(r.id) AS runs \
         FROM projects AS p \
         JOIN project_links AS l ON l.project_id = p.id AND l.kind = 'session' \
         JOIN agent_runs AS r ON r.session_id = l.ref_id \
         WHERE p.workspace_id = ?1 AND r.started_at >= ?2 \
         GROUP BY p.id, p.name ORDER BY COUNT(r.id) DESC, p.name LIMIT ?3",
    )
    .bind(workspace_id)
    .bind(&since)
    .bind(MAX_PROJECTS)
    .fetch_all(database.pool())
    .await
    .map_err(sqlite("total the projects of a digest"))?;
    let mut projects = Vec::with_capacity(rows.len());
    for row in &rows {
        let project_id: String = row
            .try_get("id")
            .map_err(sqlite("decode a digest project"))?;
        let highlights = sqlx::query(
            "SELECT id, project_id, kind, body, run_id, created_at FROM project_notes \
             WHERE project_id = ?1 AND created_at >= ?2 AND kind IN ('decision', 'result', 'blocker', 'owner') \
             ORDER BY id DESC LIMIT ?3",
        )
        .bind(&project_id)
        .bind(&since)
        .bind(MAX_HIGHLIGHTS)
        .fetch_all(database.pool())
        .await
        .map_err(sqlite("read a digest's highlights"))?;
        let mut highlights = crate::project_repository::decode_notes(&highlights)?;
        highlights.reverse();
        projects.push(ProjectActivity {
            name: row
                .try_get("name")
                .map_err(sqlite("decode a digest project"))?,
            runs: u32::try_from(count(row, "runs")?).unwrap_or(u32::MAX),
            project_id,
            highlights,
        });
    }

    let problems = failed_runs(database, workspace_id, &since).await?;
    Ok(StoredDigest {
        totals,
        projects,
        problems,
    })
}
