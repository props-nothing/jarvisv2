//! Durable storage for scheduled tasks, and the one operation that must not be repeated: claiming a fire.
//!
//! # Claiming is the guarded write
//!
//! [`claim_due_schedules`] selects what is due and advances each task's next fire in **one guarded `UPDATE` per
//! task** (`WHERE id = ? AND next_run_nanos = <the value it just read>`). A task whose row changed between the
//! read and the write is not claimed, so two scheduler passes — or a restart racing a slow tick — cannot both fire
//! it. The advance happens **before** the run starts, which makes the guarantee *at most once*: a crash after the
//! claim and before the run loses one fire and never repeats one. That is the right direction for a task that may
//! send, fetch, or compute: a missed morning briefing is an annoyance, a doubled action is an incident.
//!
//! # Missed fires are skipped
//!
//! The next fire is computed from **now** (`Cadence::next_after`), never from the time that was due, so a machine
//! that slept through ten fires owes one, not ten.

use jarvis_core::{Cadence, MAX_SCHEDULES_PER_WORKSPACE, ScheduleId, UtcTimestamp};
use sqlx::Row;

use crate::database::{DatabaseError, SqliteDatabase};

/// One stored scheduled task.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredSchedule {
    id: String,
    workspace_id: String,
    objective: String,
    cadence: Cadence,
    enabled: bool,
    next_run: UtcTimestamp,
    session_id: Option<String>,
    last_run_id: Option<String>,
    last_fired_at: Option<UtcTimestamp>,
    fire_count: u32,
    skipped_count: u32,
    created_at: UtcTimestamp,
}

impl StoredSchedule {
    /// The task's identifier.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The workspace that owns it.
    #[must_use]
    pub fn workspace_id(&self) -> &str {
        &self.workspace_id
    }

    /// What the run is asked to do.
    #[must_use]
    pub fn objective(&self) -> &str {
        &self.objective
    }

    /// When and how often it fires.
    #[must_use]
    pub const fn cadence(&self) -> Cadence {
        self.cadence
    }

    /// Whether it is active; a paused or finished task never fires.
    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    /// The next time it fires. Meaningful only while [`Self::enabled`].
    #[must_use]
    pub const fn next_run(&self) -> UtcTimestamp {
        self.next_run
    }

    /// The conversation its runs share, once the first has started.
    #[must_use]
    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    /// The most recent run it started.
    #[must_use]
    pub fn last_run_id(&self) -> Option<&str> {
        self.last_run_id.as_deref()
    }

    /// When it last started a run.
    #[must_use]
    pub const fn last_fired_at(&self) -> Option<UtcTimestamp> {
        self.last_fired_at
    }

    /// How many runs it has started.
    #[must_use]
    pub const fn fire_count(&self) -> u32 {
        self.fire_count
    }

    /// How many fires it skipped because the previous run was still going.
    #[must_use]
    pub const fn skipped_count(&self) -> u32 {
        self.skipped_count
    }

    /// When it was created.
    #[must_use]
    pub const fn created_at(&self) -> UtcTimestamp {
        self.created_at
    }
}

/// The column list, as a macro so a query can `concat!` it into a `&'static str` — `sqlx` refuses a built string
/// as a possible injection, and a constant list is exactly what it is safe to splice.
macro_rules! columns {
    () => {
        "id, workspace_id, objective, cadence, interval_seconds, enabled, next_run_nanos, \
         session_id, last_run_id, last_fired_at, fire_count, skipped_count, created_at"
    };
}

/// Creates a scheduled task.
///
/// # Errors
///
/// Returns [`DatabaseError::ScheduleLimit`] when the workspace holds the maximum, and [`DatabaseError::Sqlite`]
/// when the write fails.
pub async fn create_schedule(
    database: &SqliteDatabase,
    workspace_id: &str,
    objective: &str,
    cadence: Cadence,
    now: UtcTimestamp,
) -> Result<StoredSchedule, DatabaseError> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM schedules WHERE workspace_id = ?1")
        .bind(workspace_id)
        .fetch_one(database.pool())
        .await
        .map_err(|source| DatabaseError::Sqlite {
            operation: "count a workspace's schedules",
            source,
        })?;
    if usize::try_from(count).unwrap_or(usize::MAX) >= MAX_SCHEDULES_PER_WORKSPACE {
        return Err(DatabaseError::ScheduleLimit);
    }

    let id = ScheduleId::new().to_string();
    let (kind, interval) = match cadence {
        Cadence::Every(seconds) => ("every", Some(i64::try_from(seconds).unwrap_or(i64::MAX))),
        Cadence::Once(_) => ("once", None),
    };
    let first = cadence.first_fire(now);
    sqlx::query(
        "INSERT INTO schedules (id, workspace_id, objective, cadence, interval_seconds, enabled, \
            next_run_nanos, fire_count, skipped_count, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, 0, 0, ?7)",
    )
    .bind(&id)
    .bind(workspace_id)
    .bind(objective)
    .bind(kind)
    .bind(interval)
    .bind(nanos_to_i64(first))
    .bind(now.to_string())
    .execute(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "create a schedule",
        source,
    })?;
    find_schedule(database, workspace_id, &id).await
}

/// Reads one task, scoped to its workspace.
///
/// # Errors
///
/// Returns [`DatabaseError::ScheduleNotFound`] when no task has that identifier **in this workspace** — a task in
/// another workspace is indistinguishable from one that does not exist.
pub async fn find_schedule(
    database: &SqliteDatabase,
    workspace_id: &str,
    id: &str,
) -> Result<StoredSchedule, DatabaseError> {
    let row = sqlx::query(concat!(
        "SELECT ",
        columns!(),
        " FROM schedules WHERE id = ?1 AND workspace_id = ?2"
    ))
    .bind(id)
    .bind(workspace_id)
    .fetch_optional(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "read a schedule",
        source,
    })?
    .ok_or(DatabaseError::ScheduleNotFound)?;
    decode(&row)
}

/// Lists a workspace's tasks, oldest first.
///
/// # Errors
///
/// Returns [`DatabaseError::StoredScheduleInvalid`] when a stored row cannot be decoded.
pub async fn list_schedules(
    database: &SqliteDatabase,
    workspace_id: &str,
) -> Result<Vec<StoredSchedule>, DatabaseError> {
    let rows = sqlx::query(concat!(
        "SELECT ",
        columns!(),
        " FROM schedules WHERE workspace_id = ?1 ORDER BY created_at ASC, id ASC"
    ))
    .bind(workspace_id)
    .fetch_all(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "list schedules",
        source,
    })?;
    rows.iter().map(decode).collect()
}

/// Removes a task.
///
/// # Errors
///
/// Returns [`DatabaseError::ScheduleNotFound`] when there is no such task in this workspace.
pub async fn delete_schedule(
    database: &SqliteDatabase,
    workspace_id: &str,
    id: &str,
) -> Result<(), DatabaseError> {
    let result = sqlx::query("DELETE FROM schedules WHERE id = ?1 AND workspace_id = ?2")
        .bind(id)
        .bind(workspace_id)
        .execute(database.pool())
        .await
        .map_err(|source| DatabaseError::Sqlite {
            operation: "delete a schedule",
            source,
        })?;
    if result.rows_affected() == 0 {
        return Err(DatabaseError::ScheduleNotFound);
    }
    Ok(())
}

/// Pauses or resumes a task.
///
/// Resuming a repeating task schedules its next fire a full interval from **now**, not from before it was paused:
/// a task paused for a week does not fire the moment it is resumed, for the reason a backlog is never replayed.
///
/// # Errors
///
/// Returns [`DatabaseError::ScheduleNotFound`], or [`DatabaseError::ScheduleFinished`] for a one-off that has
/// already fired.
pub async fn set_schedule_enabled(
    database: &SqliteDatabase,
    workspace_id: &str,
    id: &str,
    enabled: bool,
    now: UtcTimestamp,
) -> Result<StoredSchedule, DatabaseError> {
    let current = find_schedule(database, workspace_id, id).await?;
    if enabled && !current.enabled {
        let next = match current.cadence {
            Cadence::Every(_) => current.cadence.first_fire(now),
            Cadence::Once(at) => {
                if current.fire_count > 0 || at <= now {
                    return Err(DatabaseError::ScheduleFinished);
                }
                at
            }
        };
        sqlx::query("UPDATE schedules SET enabled = 1, next_run_nanos = ?3 WHERE id = ?1 AND workspace_id = ?2")
            .bind(id)
            .bind(workspace_id)
            .bind(nanos_to_i64(next))
            .execute(database.pool())
            .await
            .map_err(|source| DatabaseError::Sqlite {
                operation: "resume a schedule",
                source,
            })?;
    } else if !enabled {
        sqlx::query("UPDATE schedules SET enabled = 0 WHERE id = ?1 AND workspace_id = ?2")
            .bind(id)
            .bind(workspace_id)
            .execute(database.pool())
            .await
            .map_err(|source| DatabaseError::Sqlite {
                operation: "pause a schedule",
                source,
            })?;
    }
    find_schedule(database, workspace_id, id).await
}

/// Claims every task that is due, advancing each one's next fire, and returns the claimed tasks.
///
/// Returned tasks are as they were **before** the advance (so `next_run` is the time they were due), and each is
/// claimed by exactly one caller however many passes race: see the module header.
///
/// # Errors
///
/// Returns [`DatabaseError::StoredScheduleInvalid`] when a due row cannot be decoded, and
/// [`DatabaseError::Sqlite`] when a write fails.
pub async fn claim_due_schedules(
    database: &SqliteDatabase,
    now: UtcTimestamp,
    limit: u32,
) -> Result<Vec<StoredSchedule>, DatabaseError> {
    let rows = sqlx::query(concat!(
        "SELECT ",
        columns!(),
        " FROM schedules WHERE enabled = 1 AND next_run_nanos <= ?1 \
         ORDER BY next_run_nanos ASC, id ASC LIMIT ?2"
    ))
    .bind(nanos_to_i64(now))
    .bind(i64::from(limit))
    .fetch_all(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "find due schedules",
        source,
    })?;

    let mut claimed = Vec::with_capacity(rows.len());
    for row in &rows {
        let schedule = decode(row)?;
        // A repeating task is rescheduled from now; a finished one-off is disabled, which is how it is "done".
        let (enabled, next) = match schedule.cadence.next_after(now) {
            Some(next) => (1_i64, next),
            None => (0_i64, schedule.next_run),
        };
        let result = sqlx::query(
            "UPDATE schedules SET enabled = ?3, next_run_nanos = ?4 \
             WHERE id = ?1 AND next_run_nanos = ?2 AND enabled = 1",
        )
        .bind(&schedule.id)
        .bind(nanos_to_i64(schedule.next_run))
        .bind(enabled)
        .bind(nanos_to_i64(next))
        .execute(database.pool())
        .await
        .map_err(|source| DatabaseError::Sqlite {
            operation: "claim a schedule fire",
            source,
        })?;
        // Zero rows means another pass claimed it first (or it was paused in between): not ours to fire.
        if result.rows_affected() == 1 {
            claimed.push(schedule);
        }
    }
    Ok(claimed)
}

/// Records that a claimed fire started a run.
///
/// # Errors
///
/// Returns [`DatabaseError::Sqlite`] when the write fails. A task deleted in between is not an error.
pub async fn record_schedule_run(
    database: &SqliteDatabase,
    id: &str,
    run_id: &str,
    session_id: &str,
    now: UtcTimestamp,
) -> Result<(), DatabaseError> {
    sqlx::query(
        "UPDATE schedules SET last_run_id = ?2, session_id = ?3, last_fired_at = ?4, \
            fire_count = fire_count + 1 WHERE id = ?1",
    )
    .bind(id)
    .bind(run_id)
    .bind(session_id)
    .bind(now.to_string())
    .execute(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "record a schedule's run",
        source,
    })?;
    Ok(())
}

/// Records that a claimed fire was skipped because the previous run was still going.
///
/// # Errors
///
/// Returns [`DatabaseError::Sqlite`] when the write fails.
pub async fn record_schedule_skip(
    database: &SqliteDatabase,
    id: &str,
) -> Result<(), DatabaseError> {
    sqlx::query("UPDATE schedules SET skipped_count = skipped_count + 1 WHERE id = ?1")
        .bind(id)
        .execute(database.pool())
        .await
        .map_err(|source| DatabaseError::Sqlite {
            operation: "record a skipped schedule fire",
            source,
        })?;
    Ok(())
}

fn nanos_to_i64(value: UtcTimestamp) -> i64 {
    i64::try_from(value.unix_nanos()).unwrap_or(i64::MAX)
}

fn decode(row: &sqlx::sqlite::SqliteRow) -> Result<StoredSchedule, DatabaseError> {
    let text = |field: &'static str| -> Result<String, DatabaseError> {
        row.try_get::<String, _>(field)
            .map_err(|_| DatabaseError::StoredScheduleInvalid { field })
    };
    let optional = |field: &'static str| -> Result<Option<String>, DatabaseError> {
        row.try_get::<Option<String>, _>(field)
            .map_err(|_| DatabaseError::StoredScheduleInvalid { field })
    };
    let integer = |field: &'static str| -> Result<i64, DatabaseError> {
        row.try_get::<i64, _>(field)
            .map_err(|_| DatabaseError::StoredScheduleInvalid { field })
    };

    let next_run =
        UtcTimestamp::from_unix_nanos(i128::from(integer("next_run_nanos")?)).map_err(|_| {
            DatabaseError::StoredScheduleInvalid {
                field: "next_run_nanos",
            }
        })?;
    let interval = row
        .try_get::<Option<i64>, _>("interval_seconds")
        .map_err(|_| DatabaseError::StoredScheduleInvalid {
            field: "interval_seconds",
        })?;
    let cadence = match (text("cadence")?.as_str(), interval) {
        ("every", Some(seconds)) => Cadence::every_seconds(u64::try_from(seconds).unwrap_or(0))
            .map_err(|_| DatabaseError::StoredScheduleInvalid {
                field: "interval_seconds",
            })?,
        // A one-off's instant is its next fire; once fired it is disabled, and the instant stays as history.
        ("once", None) => Cadence::Once(next_run),
        _ => return Err(DatabaseError::StoredScheduleInvalid { field: "cadence" }),
    };
    let timestamp = |value: &str, field: &'static str| {
        value
            .parse::<UtcTimestamp>()
            .map_err(|_| DatabaseError::StoredScheduleInvalid { field })
    };
    Ok(StoredSchedule {
        id: text("id")?,
        workspace_id: text("workspace_id")?,
        objective: text("objective")?,
        cadence,
        enabled: integer("enabled")? == 1,
        next_run,
        session_id: optional("session_id")?,
        last_run_id: optional("last_run_id")?,
        last_fired_at: optional("last_fired_at")?
            .map(|value| timestamp(&value, "last_fired_at"))
            .transpose()?,
        fire_count: u32::try_from(integer("fire_count")?).map_err(|_| {
            DatabaseError::StoredScheduleInvalid {
                field: "fire_count",
            }
        })?,
        skipped_count: u32::try_from(integer("skipped_count")?).map_err(|_| {
            DatabaseError::StoredScheduleInvalid {
                field: "skipped_count",
            }
        })?,
        created_at: timestamp(&text("created_at")?, "created_at")?,
    })
}

#[cfg(test)]
#[path = "schedule_tests.rs"]
mod tests;
