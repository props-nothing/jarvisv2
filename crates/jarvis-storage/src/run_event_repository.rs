//! Durable storage for the per-run event stream.
//!
//! `docs/architecture/protocols.md` requires stream events to carry a sequence and an
//! event ID and requires the server to "replay from durable records within retention or
//! return an explicit resync requirement". ADR-0011 decides that this record is a
//! per-run ordered log, and that it is **not** the Phase 6 event bus.
//!
//! # Appending is ONE statement, and it allocates its own sequence
//!
//! [`append_run_event`] neither accepts a sequence from the caller nor reads the stream
//! before writing it. The insert is a single `INSERT ... SELECT` that both verifies the
//! run is present and unsettled and allocates `MAX(sequence) + 1` in the same statement.
//!
//! This is deliberate, and the alternative was tried first. A separate `SELECT state`
//! followed by an `INSERT` runs inside the same deferred transaction and looks equivalent,
//! but SQLite takes a **read snapshot** at the first `SELECT`; if another connection
//! commits in between, upgrading to a write fails with `SQLITE_BUSY_SNAPSHOT` rather than
//! waiting. Two clients starting a run at once would therefore produce a spurious failure
//! that has nothing to do with the data. One statement removes the window entirely, so
//! losing a race is reported as a lost race instead of as a lock error.
//!
//! # A terminal event is the last event, enforced on read
//!
//! [`read_run_events`] reports a stored stream that contains any event after a terminal
//! one. The migration's `kind` constraint makes an *invalid* kind unstorable, but it
//! cannot express "nothing follows a settlement", because that is a property of the stream
//! rather than of a single row. A stream that continued after `run_completed` would mean
//! the run settled twice, which is precisely what this record exists to prove did not
//! happen.

use jarvis_core::{
    CorrelationId, EventSummary, MAX_EVENT_SUMMARY_CHARS, ReplayRequest, RunEventKind,
    RunEventPayload, RunEventSequence, UtcTimestamp,
};
use sqlx::Row;

use crate::database::{DatabaseError, SqliteDatabase};

/// One stored run event.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredRunEvent {
    id: String,
    run_id: String,
    sequence: RunEventSequence,
    kind: RunEventKind,
    summary: Option<String>,
    payload: String,
    correlation_id: CorrelationId,
    occurred_at: UtcTimestamp,
    recorded_at: UtcTimestamp,
}

impl StoredRunEvent {
    /// Returns the event identifier, which is also the SSE `id:` field.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the run this event belongs to.
    #[must_use]
    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    /// Returns this event's position in its run's stream.
    #[must_use]
    pub const fn sequence(&self) -> RunEventSequence {
        self.sequence
    }

    /// Returns the normalized event kind.
    #[must_use]
    pub const fn kind(&self) -> RunEventKind {
        self.kind
    }

    /// Returns the bounded operational summary, if this kind carries one.
    #[must_use]
    pub fn summary(&self) -> Option<&str> {
        self.summary.as_deref()
    }

    /// Returns the validated JSON payload.
    #[must_use]
    pub fn payload(&self) -> &str {
        &self.payload
    }

    /// Returns the correlation identifier shared with the requesting client.
    #[must_use]
    pub const fn correlation_id(&self) -> CorrelationId {
        self.correlation_id
    }

    /// Returns when the run produced the event.
    #[must_use]
    pub const fn occurred_at(&self) -> UtcTimestamp {
        self.occurred_at
    }

    /// Returns when the event row was written.
    ///
    /// Distinct from [`Self::occurred_at`] on purpose: the two differ when an event is
    /// persisted after a retry, and keeping both makes "the run emitted this late"
    /// distinguishable from "we stored it late".
    #[must_use]
    pub const fn recorded_at(&self) -> UtcTimestamp {
        self.recorded_at
    }
}

/// One event to append to a run's stream.
///
/// The sequence is deliberately absent: it is allocated by the writing statement, so a
/// caller cannot introduce a gap or a duplicate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewRunEvent {
    id: String,
    run_id: String,
    kind: RunEventKind,
    summary: Option<EventSummary>,
    payload: RunEventPayload,
    correlation_id: CorrelationId,
    occurred_at: UtcTimestamp,
}

impl NewRunEvent {
    /// Validates the fields required to append an event.
    ///
    /// # Errors
    ///
    /// Returns [`DatabaseError::InvalidRunEventRequest`] when the event or run identifier
    /// is not UUID-sized text.
    pub fn new(
        id: impl Into<String>,
        run_id: impl Into<String>,
        kind: RunEventKind,
        summary: Option<EventSummary>,
        payload: RunEventPayload,
        correlation_id: CorrelationId,
        occurred_at: UtcTimestamp,
    ) -> Result<Self, DatabaseError> {
        let id = id.into();
        let run_id = run_id.into();
        for (field, value) in [("id", &id), ("run id", &run_id)] {
            if value.len() != 36 {
                return Err(DatabaseError::InvalidRunEventRequest { field });
            }
        }
        Ok(Self {
            id,
            run_id,
            kind,
            summary,
            payload,
            correlation_id,
            occurred_at,
        })
    }

    /// Returns the event identifier.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the run this event belongs to.
    #[must_use]
    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    /// Returns the event kind.
    #[must_use]
    pub const fn kind(&self) -> RunEventKind {
        self.kind
    }

    /// Returns the bounded operational summary, when present.
    #[must_use]
    pub const fn summary(&self) -> Option<&EventSummary> {
        self.summary.as_ref()
    }

    /// Returns the validated JSON payload.
    #[must_use]
    pub const fn payload(&self) -> &RunEventPayload {
        &self.payload
    }

    /// Returns the correlation identity shared with the originating request.
    #[must_use]
    pub const fn correlation_id(&self) -> CorrelationId {
        self.correlation_id
    }

    /// Returns when the event occurred.
    #[must_use]
    pub const fn occurred_at(&self) -> UtcTimestamp {
        self.occurred_at
    }
}

/// Appends one event to its run's stream, allocating the next sequence.
///
/// # Errors
///
/// - [`DatabaseError::RunNotFound`] when no run has that identifier.
/// - [`DatabaseError::RunEventAfterSettlement`] when the run is already terminal, because
///   a settled run emits nothing more.
/// - [`DatabaseError::RunEventConflict`] when a concurrent writer allocated the same
///   sequence first, so the caller should re-read rather than assume corruption.
/// - [`DatabaseError::Sqlite`] for any other persistence failure.
pub async fn append_run_event(
    database: &SqliteDatabase,
    new: &NewRunEvent,
) -> Result<StoredRunEvent, DatabaseError> {
    // One statement. The `WHERE EXISTS` clause is the settlement guard and the scalar
    // subquery is the sequence allocation; both are evaluated under the same write lock,
    // so nothing can settle between them. A refused append affects zero rows, and the
    // reason is resolved on the failure path below rather than guessed at here.
    let result = sqlx::query(
        "INSERT INTO run_events (\
            id, run_id, sequence, kind, summary, payload, \
            correlation_id, occurred_at, recorded_at\
         ) \
         SELECT ?1, ?2, \
            (SELECT COALESCE(MAX(sequence), 0) + 1 FROM run_events WHERE run_id = ?2), \
            ?3, ?4, ?5, ?6, ?7, ?7 \
         WHERE EXISTS (\
            SELECT 1 FROM agent_runs \
            WHERE id = ?2 AND state NOT IN ('completed', 'cancelled', 'failed')\
         )",
    )
    .bind(&new.id)
    .bind(&new.run_id)
    .bind(new.kind.as_str())
    .bind(new.summary.as_ref().map(EventSummary::as_str))
    .bind(new.payload.as_str())
    .bind(new.correlation_id.to_string())
    .bind(new.occurred_at.to_string())
    .execute(database.pool())
    .await
    .map_err(map_event_insert_error)?;

    if result.rows_affected() == 0 {
        // Zero rows means the guard refused it. The two causes are a missing run and a
        // settled run, and they need different responses, so they are separated by
        // reading the reason rather than by reporting a generic conflict.
        return match find_run_state(database, &new.run_id).await? {
            None => Err(DatabaseError::RunNotFound),
            Some(state) if is_terminal_state(&state) => Err(DatabaseError::RunEventAfterSettlement),
            Some(_) => Err(DatabaseError::Sqlite {
                operation: "append a run event",
                source: sqlx::Error::RowNotFound,
            }),
        };
    }

    find_run_event(database, &new.id).await
}

/// Reads one event by identifier.
///
/// # Errors
///
/// Returns [`DatabaseError::RunEventNotFound`] when no event has that identifier, or
/// [`DatabaseError::Sqlite`] when the read fails.
pub async fn find_run_event(
    database: &SqliteDatabase,
    id: &str,
) -> Result<StoredRunEvent, DatabaseError> {
    let row = sqlx::query(
        "SELECT id, run_id, sequence, kind, summary, payload, \
                correlation_id, occurred_at, recorded_at \
         FROM run_events WHERE id = ?1",
    )
    .bind(id)
    .fetch_optional(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "read a run event",
        source,
    })?
    .ok_or(DatabaseError::RunEventNotFound)?;
    decode_event(&row)
}

/// Reads a bounded slice of one run's stream in sequence order.
///
/// # Errors
///
/// - [`DatabaseError::RunNotFound`] when no run has that identifier, so a stream for a
///   nonexistent run is distinguishable from an empty one.
/// - [`DatabaseError::StoredRunEventInvalid`] when a stored row cannot be decoded, or when
///   the stream contains an event after a terminal one.
/// - [`DatabaseError::Sqlite`] for any other read failure.
pub async fn read_run_events(
    database: &SqliteDatabase,
    run_id: &str,
    request: ReplayRequest,
) -> Result<Vec<StoredRunEvent>, DatabaseError> {
    if find_run_state(database, run_id).await?.is_none() {
        return Err(DatabaseError::RunNotFound);
    }

    // Checked across the WHOLE stream before paging, not just within the returned page.
    // A page that happens to end before the terminal event would otherwise hide a later
    // row and the reader would be told the stream is fine.
    if stream_continues_after_settlement(database, run_id).await? {
        return Err(DatabaseError::StoredRunEventInvalid {
            field: "terminal event position",
        });
    }

    let rows = sqlx::query(
        "SELECT id, run_id, sequence, kind, summary, payload, \
                correlation_id, occurred_at, recorded_at \
         FROM run_events \
         WHERE run_id = ?1 AND sequence >= ?2 \
         ORDER BY sequence ASC \
         LIMIT ?3",
    )
    .bind(run_id)
    .bind(i64::from(request.from().get()))
    .bind(i64::from(request.limit()))
    .fetch_all(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "read a run event stream",
        source,
    })?;

    let mut events = Vec::with_capacity(rows.len());
    for row in &rows {
        events.push(decode_event(row)?);
    }
    Ok(events)
}

/// Returns the highest stored sequence for a run, or `None` when it has no events.
///
/// A stream handler uses this to detect a client that supplied a last-event position
/// beyond what this daemon holds, which requires an explicit resync rather than a silent
/// empty replay. The two are indistinguishable to the client otherwise.
///
/// # Errors
///
/// Returns [`DatabaseError::Sqlite`] when the read fails, or
/// [`DatabaseError::StoredRunEventInvalid`] when a stored sequence is unusable.
pub async fn highest_run_event_sequence(
    database: &SqliteDatabase,
    run_id: &str,
) -> Result<Option<RunEventSequence>, DatabaseError> {
    let highest: Option<i64> =
        sqlx::query_scalar("SELECT MAX(sequence) FROM run_events WHERE run_id = ?1")
            .bind(run_id)
            .fetch_one(database.pool())
            .await
            .map_err(|source| DatabaseError::Sqlite {
                operation: "read the highest run event sequence",
                source,
            })?;
    let Some(highest) = highest else {
        return Ok(None);
    };
    u32::try_from(highest)
        .ok()
        .and_then(|value| RunEventSequence::new(value).ok())
        .map(Some)
        .ok_or(DatabaseError::StoredRunEventInvalid { field: "sequence" })
}

/// Reports whether any event follows a terminal event in one run's stream.
///
/// A self-join is used rather than comparing against the page in memory so that a
/// settlement is found even when the requested page ends before it.
async fn stream_continues_after_settlement(
    database: &SqliteDatabase,
    run_id: &str,
) -> Result<bool, DatabaseError> {
    let offender: Option<i64> = sqlx::query_scalar(
        "SELECT later.sequence \
         FROM run_events terminal \
         JOIN run_events later \
           ON later.run_id = terminal.run_id AND later.sequence > terminal.sequence \
         WHERE terminal.run_id = ?1 \
           AND terminal.kind IN ('run_completed', 'run_failed', 'run_cancelled') \
         LIMIT 1",
    )
    .bind(run_id)
    .fetch_optional(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "check a run event stream for a later settlement",
        source,
    })?;
    Ok(offender.is_some())
}

async fn find_run_state(
    database: &SqliteDatabase,
    run_id: &str,
) -> Result<Option<String>, DatabaseError> {
    sqlx::query_scalar("SELECT state FROM agent_runs WHERE id = ?1")
        .bind(run_id)
        .fetch_optional(database.pool())
        .await
        .map_err(|source| DatabaseError::Sqlite {
            operation: "read a run state",
            source,
        })
}

fn is_terminal_state(state: &str) -> bool {
    matches!(state, "completed" | "cancelled" | "failed")
}

fn decode_event(row: &sqlx::sqlite::SqliteRow) -> Result<StoredRunEvent, DatabaseError> {
    let kind = text_field(row, "kind")?
        .parse::<RunEventKind>()
        .map_err(|_| DatabaseError::StoredRunEventInvalid { field: "kind" })?;

    let sequence = row
        .try_get::<i64, _>("sequence")
        .map_err(|source| DatabaseError::Sqlite {
            operation: "decode a stored run event sequence",
            source,
        })
        .and_then(|value| {
            u32::try_from(value)
                .ok()
                .and_then(|value| RunEventSequence::new(value).ok())
                .ok_or(DatabaseError::StoredRunEventInvalid { field: "sequence" })
        })?;

    let summary = row
        .try_get::<Option<String>, _>("summary")
        .map_err(|source| DatabaseError::Sqlite {
            operation: "decode a stored run event summary",
            source,
        })?;
    if let Some(value) = &summary
        && value.chars().count() > MAX_EVENT_SUMMARY_CHARS
    {
        return Err(DatabaseError::StoredRunEventInvalid { field: "summary" });
    }

    let payload = text_field(row, "payload")?;
    // Re-validated rather than trusted. The migration bounds the byte length, but it
    // cannot prove the text parses, so a row written by another build could hold payload
    // text this build cannot interpret. Reporting that here is better than a stream
    // handler failing later with no context.
    if RunEventPayload::new(payload.clone()).is_err() {
        return Err(DatabaseError::StoredRunEventInvalid { field: "payload" });
    }

    let correlation_id = text_field(row, "correlation_id")?
        .parse::<CorrelationId>()
        .map_err(|_| DatabaseError::StoredRunEventInvalid {
            field: "correlation id",
        })?;

    Ok(StoredRunEvent {
        id: text_field(row, "id")?,
        run_id: text_field(row, "run_id")?,
        sequence,
        kind,
        summary,
        payload,
        correlation_id,
        occurred_at: decode_timestamp(row, "occurred_at")?,
        recorded_at: decode_timestamp(row, "recorded_at")?,
    })
}

fn decode_timestamp(
    row: &sqlx::sqlite::SqliteRow,
    column: &'static str,
) -> Result<UtcTimestamp, DatabaseError> {
    text_field(row, column)?
        .parse::<UtcTimestamp>()
        .map_err(|_| DatabaseError::StoredRunEventInvalid {
            field: "event timestamp",
        })
}

fn text_field(
    row: &sqlx::sqlite::SqliteRow,
    column: &'static str,
) -> Result<String, DatabaseError> {
    row.try_get(column).map_err(|source| DatabaseError::Sqlite {
        operation: "decode a stored run event field",
        source,
    })
}

/// Maps an event insertion failure onto the reason it represents.
///
/// Two distinct defects arrive as the same `sqlx::Error` and need different responses: a
/// unique violation on `(run_id, sequence)` means a concurrent writer took the sequence
/// and the caller should re-read, while a foreign-key violation means the run does not
/// exist. Separating them by the database's own code avoids reporting a lost race as a
/// missing parent.
fn map_event_insert_error(source: sqlx::Error) -> DatabaseError {
    if let sqlx::Error::Database(database_error) = &source {
        match database_error.code().as_deref() {
            // SQLITE_CONSTRAINT_UNIQUE / SQLITE_CONSTRAINT_PRIMARYKEY.
            Some("2067" | "1555") => return DatabaseError::RunEventConflict,
            // SQLITE_CONSTRAINT_FOREIGNKEY.
            Some("787") => return DatabaseError::RunNotFound,
            _ => {}
        }
    }
    DatabaseError::Sqlite {
        operation: "insert a run event",
        source,
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::*;
    use crate::{
        DEFAULT_DATABASE_FILENAME, SqliteDatabase,
        run_repository::{NewRun, StoredRun, create_run, transition_run},
    };

    static TEST_DIRECTORY_ID: AtomicU64 = AtomicU64::new(0);

    const USER: &str = "0198f000-0000-7000-8000-000000000001";
    const WORKSPACE: &str = "0198f000-0000-7000-8000-000000000002";
    const SESSION: &str = "0198f000-0000-7000-8000-000000000003";
    const RUN: &str = "0198f000-0000-7000-8000-0000000000c3";
    const MISSING: &str = "0198f000-0000-7000-8000-0000000000ff";

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let sequence = TEST_DIRECTORY_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "jarvis-run-events-{}-{sequence}",
                std::process::id()
            ));
            must(fs::create_dir_all(&path));
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn must<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
        match result {
            Ok(value) => value,
            Err(error) => panic!("expected success, got {error:?}"),
        }
    }

    /// A distinct instant per call, so a stored timestamp is attributable to its writer.
    fn at(minute: i128) -> UtcTimestamp {
        must(UtcTimestamp::from_unix_nanos(
            1_774_000_000_000_000_000 + minute * 60_000_000_000,
        ))
    }

    async fn seeded_database() -> (TestDirectory, SqliteDatabase) {
        let directory = TestDirectory::new();
        let path = directory.path().join(DEFAULT_DATABASE_FILENAME);
        let database = must(SqliteDatabase::open(&path).await);
        for statement in [
            "INSERT INTO users (id, display_name, status, created_at, updated_at, version) \
             VALUES ('0198f000-0000-7000-8000-000000000001', 'Local User', 'active', \
                     '2026-09-21T10:00:00Z', '2026-09-21T10:00:00Z', 1)",
            "INSERT INTO workspaces (id, name, mode, data_policy, status, created_at, updated_at, version) \
             VALUES ('0198f000-0000-7000-8000-000000000002', 'Local', 'local', 'local-only', 'active', \
                     '2026-09-21T10:00:00Z', '2026-09-21T10:00:00Z', 1)",
            "INSERT INTO sessions (id, workspace_id, user_id, channel, status, created_at, updated_at, version) \
             VALUES ('0198f000-0000-7000-8000-000000000003', \
                     '0198f000-0000-7000-8000-000000000002', \
                     '0198f000-0000-7000-8000-000000000001', 'cli', 'active', \
                     '2026-09-21T10:00:00Z', '2026-09-21T10:00:00Z', 1)",
        ] {
            must(
                sqlx::query(statement)
                    .execute(database.pool())
                    .await
                    .map(|_| ()),
            );
        }
        (directory, database)
    }

    async fn live_run(database: &SqliteDatabase) -> StoredRun {
        let new = must(NewRun::new(
            RUN,
            SESSION,
            WORKSPACE,
            USER,
            "Summarise today's calendar",
            CorrelationId::new(),
            at(0),
        ));
        must(create_run(database, &new).await)
    }

    fn event_id(value: u8) -> String {
        format!("0198f000-0000-7000-8000-0000000000{value:02x}")
    }

    fn delta(run: &StoredRun, id: u8, text: &str) -> NewRunEvent {
        must(NewRunEvent::new(
            event_id(id),
            run.id().to_owned(),
            RunEventKind::OutputDelta,
            None,
            must(RunEventPayload::new(format!(r#"{{"text":"{text}"}}"#))),
            CorrelationId::new(),
            at(1),
        ))
    }

    fn settlement(run: &StoredRun, id: u8, kind: RunEventKind) -> NewRunEvent {
        must(NewRunEvent::new(
            event_id(id),
            run.id().to_owned(),
            kind,
            Some(must(EventSummary::new("run failed"))),
            must(RunEventPayload::new(r#"{"code":"provider_failed"}"#)),
            CorrelationId::new(),
            at(2),
        ))
    }

    fn failed_transition() -> jarvis_core::RunTransition {
        must(jarvis_core::RunTransition::new(
            jarvis_core::RunState::Failed,
            Some(jarvis_core::RunOutcome::Failed),
            Some(must(jarvis_core::RunErrorCode::new("provider_failed"))),
        ))
    }

    fn replay(from: u32, limit: u32) -> ReplayRequest {
        must(ReplayRequest::new(must(RunEventSequence::new(from)), limit))
    }

    /// The first event of a run's stream is sequence 1, allocated by the writer.
    #[tokio::test]
    async fn the_first_appended_event_is_sequence_one() {
        let (_directory, database) = seeded_database().await;
        let run = live_run(&database).await;

        let stored = must(append_run_event(&database, &delta(&run, 1, "first")).await);
        assert_eq!(stored.sequence().get(), 1);
        assert_eq!(stored.run_id(), RUN);
        assert_eq!(stored.kind(), RunEventKind::OutputDelta);
        assert_eq!(stored.payload(), r#"{"text":"first"}"#);
        assert_eq!(stored.occurred_at(), at(1));
        assert_eq!(stored.recorded_at(), at(1));
        assert_eq!(
            stored.summary(),
            None,
            "a delta carries text and no operational summary"
        );
    }

    /// Sequence is allocated by the writing statement, so appends are gapless and ordered.
    #[tokio::test]
    async fn appends_are_gapless_and_ordered() {
        let (_directory, database) = seeded_database().await;
        let run = live_run(&database).await;

        for (id, text) in [(1_u8, "a"), (2, "b"), (3, "c")] {
            must(append_run_event(&database, &delta(&run, id, text)).await);
        }

        let events = must(read_run_events(&database, RUN, replay(1, 10)).await);
        let sequences: Vec<u32> = events.iter().map(|event| event.sequence().get()).collect();
        assert_eq!(sequences, vec![1, 2, 3]);
        assert_eq!(
            must(highest_run_event_sequence(&database, RUN).await).map(RunEventSequence::get),
            Some(3)
        );
    }

    /// A replay resumes inclusively from a client's last-seen position.
    #[tokio::test]
    async fn a_replay_resumes_after_the_last_seen_sequence() {
        let (_directory, database) = seeded_database().await;
        let run = live_run(&database).await;
        for (id, text) in [(1_u8, "a"), (2, "b"), (3, "c"), (4, "d")] {
            must(append_run_event(&database, &delta(&run, id, text)).await);
        }

        let events = must(read_run_events(&database, RUN, replay(3, 10)).await);
        let sequences: Vec<u32> = events.iter().map(|event| event.sequence().get()).collect();
        assert_eq!(sequences, vec![3, 4], "replay is inclusive of the cursor");
    }

    /// A page stops at its limit rather than returning the whole stream.
    #[tokio::test]
    async fn a_replay_is_bounded() {
        let (_directory, database) = seeded_database().await;
        let run = live_run(&database).await;
        for (id, text) in [(1_u8, "a"), (2, "b"), (3, "c")] {
            must(append_run_event(&database, &delta(&run, id, text)).await);
        }

        let events = must(read_run_events(&database, RUN, replay(1, 2)).await);
        assert_eq!(events.len(), 2);
        assert_eq!(
            events.last().map(StoredRunEvent::sequence),
            Some(must(RunEventSequence::new(2)))
        );
    }

    /// A stream for a nonexistent run is an error, not an empty page: the two are
    /// different answers and a client must be able to tell them apart.
    #[tokio::test]
    async fn a_stream_for_an_unknown_run_is_refused() {
        let (_directory, database) = seeded_database().await;

        assert!(matches!(
            read_run_events(&database, MISSING, replay(1, 10)).await,
            Err(DatabaseError::RunNotFound)
        ));
        assert!(matches!(
            highest_run_event_sequence(&database, MISSING).await,
            Ok(None)
        ));
    }

    /// A run with no events is an empty stream, which is distinct from a missing run.
    #[tokio::test]
    async fn a_run_with_no_events_has_no_highest_sequence() {
        let (_directory, database) = seeded_database().await;
        let run = live_run(&database).await;
        assert_eq!(run.id(), RUN);

        assert_eq!(must(highest_run_event_sequence(&database, RUN).await), None);
        assert!(must(read_run_events(&database, RUN, replay(1, 10)).await).is_empty());
    }

    /// **The guard that makes settlement final.** A settled run emits nothing more.
    #[tokio::test]
    async fn a_settled_run_cannot_emit_another_event() {
        let (_directory, database) = seeded_database().await;
        let run = live_run(&database).await;
        must(append_run_event(&database, &delta(&run, 1, "answer")).await);
        must(
            transition_run(
                &database,
                RUN,
                run.expectation(),
                &failed_transition(),
                at(2),
            )
            .await,
        );

        assert!(matches!(
            append_run_event(&database, &delta(&run, 2, "too late")).await,
            Err(DatabaseError::RunEventAfterSettlement)
        ));

        let events = must(read_run_events(&database, RUN, replay(1, 10)).await);
        assert_eq!(events.len(), 1, "the refused append must not be stored");
    }

    /// An append to a missing run is reported as a missing run, not as a lost race.
    #[tokio::test]
    async fn an_append_for_a_missing_run_is_refused() {
        let (_directory, database) = seeded_database().await;
        let new = must(NewRunEvent::new(
            event_id(9),
            MISSING,
            RunEventKind::Heartbeat,
            None,
            must(RunEventPayload::new(r#"{"at":"now"}"#)),
            CorrelationId::new(),
            at(1),
        ));

        assert!(matches!(
            append_run_event(&database, &new).await,
            Err(DatabaseError::RunNotFound)
        ));
    }

    /// **The reason the sequence is allocated inside the insert.** Two writers appending
    /// at once must both succeed with distinct sequences rather than one hitting a lock
    /// error or both claiming sequence 1.
    #[tokio::test]
    async fn concurrent_appends_do_not_reuse_a_sequence() {
        let (_directory, database) = seeded_database().await;
        let run = live_run(&database).await;

        // Bound to `let`s before the join: an inline temporary borrows the run and is
        // dropped at the end of the `join!` argument expression, which does not outlive
        // the joined future (E0716). Same trap the run-transition concurrency test hit.
        let first_event = delta(&run, 1, "one");
        let second_event = delta(&run, 2, "two");
        let (first, second) = tokio::join!(
            append_run_event(&database, &first_event),
            append_run_event(&database, &second_event),
        );
        let both = [first, second];
        let accepted = both.iter().filter(|result| result.is_ok()).count();
        assert_eq!(accepted, 2, "both appends must succeed: {both:?}");

        let events = must(read_run_events(&database, RUN, replay(1, 10)).await);
        let sequences: Vec<u32> = events.iter().map(|event| event.sequence().get()).collect();
        assert_eq!(
            sequences,
            vec![1, 2],
            "sequences must be gapless and distinct, not both 1"
        );
    }

    /// The unique constraint is the ordering guarantee, so a duplicate sequence cannot be
    /// stored even by a direct write that bypasses the repository.
    #[tokio::test]
    async fn a_duplicate_sequence_is_unrepresentable() {
        let (_directory, database) = seeded_database().await;
        let run = live_run(&database).await;
        must(append_run_event(&database, &delta(&run, 1, "first")).await);

        let duplicate =
            raw_insert(&database, &event_id(2), RUN, 1, "heartbeat", r#"{"a":1}"#).await;
        assert!(
            duplicate.is_err(),
            "UNIQUE (run_id, sequence) must reject a duplicate sequence"
        );
    }

    /// An unknown kind cannot be stored, so a reader never meets one it must guess at.
    #[tokio::test]
    async fn an_unknown_kind_is_unrepresentable() {
        let (_directory, database) = seeded_database().await;
        let run = live_run(&database).await;
        assert_eq!(run.id(), RUN);

        let unknown = raw_insert(
            &database,
            &event_id(3),
            RUN,
            1,
            "reasoning_trace",
            r#"{"a":1}"#,
        )
        .await;
        assert!(unknown.is_err(), "the CHECK must reject an unknown kind");
    }

    /// A zero sequence cannot be stored, so a stream cannot begin at an event a client
    /// starting from 1 would never request.
    #[tokio::test]
    async fn a_zero_sequence_is_unrepresentable() {
        let (_directory, database) = seeded_database().await;
        let run = live_run(&database).await;
        assert_eq!(run.id(), RUN);

        let zero = raw_insert(&database, &event_id(4), RUN, 0, "heartbeat", r#"{"a":1}"#).await;
        assert!(zero.is_err(), "the CHECK must reject sequence 0");
    }

    /// A payload of the wrong shape is storable text but not a valid event, so the domain
    /// refuses to read it back. Asserted from both sides so the split between what SQL
    /// enforces and what the domain enforces stays explicit.
    #[tokio::test]
    async fn a_non_json_payload_is_refused_on_read() {
        let (_directory, database) = seeded_database().await;
        let run = live_run(&database).await;
        assert_eq!(run.id(), RUN);

        must(raw_insert(&database, &event_id(5), RUN, 1, "heartbeat", "not json").await);
        assert!(matches!(
            read_run_events(&database, RUN, replay(1, 10)).await,
            Err(DatabaseError::StoredRunEventInvalid { field: "payload" })
        ));
    }

    /// **The stream cannot continue past settlement.** A row written after a terminal
    /// event is a stored defect, reported rather than replayed as if valid.
    #[tokio::test]
    async fn a_stream_continuing_after_settlement_is_reported() {
        let (_directory, database) = seeded_database().await;
        let run = live_run(&database).await;
        must(append_run_event(&database, &delta(&run, 1, "delta")).await);

        // Written directly, bypassing the repository guard, to reproduce another build or
        // an external writer -- exactly the case the migration cannot prevent.
        must(
            raw_insert(
                &database,
                &event_id(2),
                RUN,
                2,
                "run_failed",
                r#"{"code":"x"}"#,
            )
            .await,
        );
        must(
            raw_insert(
                &database,
                &event_id(3),
                RUN,
                3,
                "output_delta",
                r#"{"text":"zombie"}"#,
            )
            .await,
        );
        must(
            transition_run(
                &database,
                RUN,
                run.expectation(),
                &failed_transition(),
                at(3),
            )
            .await,
        );

        // Requesting only the FIRST event must still detect the later row, because the
        // check spans the stream rather than the page.
        assert!(matches!(
            read_run_events(&database, RUN, replay(1, 1)).await,
            Err(DatabaseError::StoredRunEventInvalid {
                field: "terminal event position"
            })
        ));
    }

    /// An in-progress stream must NOT be reported as continuing past settlement, because
    /// an open run legitimately has more events to come. This is the false positive a
    /// naive "last event is not terminal" check produces.
    #[tokio::test]
    async fn an_open_stream_is_not_reported_as_continuing_past_settlement() {
        let (_directory, database) = seeded_database().await;
        let run = live_run(&database).await;
        for (id, text) in [(1_u8, "a"), (2, "b"), (3, "c")] {
            must(append_run_event(&database, &delta(&run, id, text)).await);
        }

        let page = must(read_run_events(&database, RUN, replay(1, 1)).await);
        assert_eq!(page.len(), 1, "an in-progress run reads normally");
    }

    /// A run that settles with a terminal event and nothing after it reads cleanly.
    #[tokio::test]
    async fn a_stream_ending_on_its_settlement_reads_cleanly() {
        let (_directory, database) = seeded_database().await;
        let run = live_run(&database).await;
        must(append_run_event(&database, &delta(&run, 1, "answer")).await);
        must(append_run_event(&database, &settlement(&run, 2, RunEventKind::RunFailed)).await);
        must(
            transition_run(
                &database,
                RUN,
                run.expectation(),
                &failed_transition(),
                at(3),
            )
            .await,
        );

        let events = must(read_run_events(&database, RUN, replay(1, 10)).await);
        assert_eq!(events.len(), 2);
        assert_eq!(
            events.last().map(StoredRunEvent::kind),
            Some(RunEventKind::RunFailed)
        );
        assert!(
            events
                .last()
                .is_some_and(|event| event.kind().is_terminal()),
            "the terminal event must be last, which is what makes the stream readable"
        );
        assert_eq!(
            events.last().and_then(StoredRunEvent::summary),
            Some("run failed"),
            "a settlement carries an operational summary"
        );
    }

    /// Deleting a run removes its stream, so an orphaned event cannot outlive the run it
    /// describes and become unreadable.
    #[tokio::test]
    async fn deleting_a_run_removes_its_stream() {
        let (_directory, database) = seeded_database().await;
        let run = live_run(&database).await;
        must(append_run_event(&database, &delta(&run, 1, "answer")).await);

        must(
            sqlx::query("DELETE FROM agent_runs WHERE id = ?1")
                .bind(RUN)
                .execute(database.pool())
                .await
                .map(|_| ()),
        );

        let remaining: i64 = must(
            sqlx::query_scalar("SELECT COUNT(*) FROM run_events WHERE run_id = ?1")
                .bind(RUN)
                .fetch_one(database.pool())
                .await,
        );
        assert_eq!(remaining, 0, "ON DELETE CASCADE must remove the stream");
    }

    /// The stream survives a reopen, which is what makes a reconnect after a daemon
    /// restart answerable at all.
    #[tokio::test]
    async fn a_stream_survives_a_reopen() {
        let directory = TestDirectory::new();
        let path = directory.path().join(DEFAULT_DATABASE_FILENAME);
        let database = must(SqliteDatabase::open(&path).await);
        for statement in [
            "INSERT INTO users (id, display_name, status, created_at, updated_at, version) \
             VALUES ('0198f000-0000-7000-8000-000000000001', 'Local User', 'active', \
                     '2026-09-21T10:00:00Z', '2026-09-21T10:00:00Z', 1)",
            "INSERT INTO workspaces (id, name, mode, data_policy, status, created_at, updated_at, version) \
             VALUES ('0198f000-0000-7000-8000-000000000002', 'Local', 'local', 'local-only', 'active', \
                     '2026-09-21T10:00:00Z', '2026-09-21T10:00:00Z', 1)",
            "INSERT INTO sessions (id, workspace_id, user_id, channel, status, created_at, updated_at, version) \
             VALUES ('0198f000-0000-7000-8000-000000000003', \
                     '0198f000-0000-7000-8000-000000000002', \
                     '0198f000-0000-7000-8000-000000000001', 'cli', 'active', \
                     '2026-09-21T10:00:00Z', '2026-09-21T10:00:00Z', 1)",
        ] {
            must(
                sqlx::query(statement)
                    .execute(database.pool())
                    .await
                    .map(|_| ()),
            );
        }
        let run = live_run(&database).await;
        must(append_run_event(&database, &delta(&run, 1, "before restart")).await);
        database.close().await;

        let reopened = must(SqliteDatabase::open(&path).await);
        let events = must(read_run_events(&reopened, RUN, replay(1, 10)).await);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].payload(), r#"{"text":"before restart"}"#);
        assert_eq!(events[0].sequence().get(), 1);
    }

    /// Inserts a run event directly, bypassing the repository, so a test can reproduce a
    /// row this build would never write.
    async fn raw_insert(
        database: &SqliteDatabase,
        id: &str,
        run_id: &str,
        sequence: i64,
        kind: &str,
        payload: &str,
    ) -> Result<sqlx::sqlite::SqliteQueryResult, sqlx::Error> {
        sqlx::query(
            "INSERT INTO run_events \
             (id, run_id, sequence, kind, summary, payload, correlation_id, occurred_at, recorded_at) \
             VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?6, ?7, ?7)",
        )
        .bind(id)
        .bind(run_id)
        .bind(sequence)
        .bind(kind)
        .bind(payload)
        .bind(CorrelationId::new().to_string())
        .bind(at(1).to_string())
        .execute(database.pool())
        .await
    }
}
