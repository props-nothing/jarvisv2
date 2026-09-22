//! The SQLite agent-run repository.
//!
//! This adapter makes the transition table in `jarvis_core::run` durable. It owns SQL; the
//! domain owns legality.
//!
//! # The concurrency decision
//!
//! [`transition_run`] performs **one** `UPDATE` whose `WHERE` clause contains both the
//! expected state and the expected version, and treats a match count other than one as a
//! conflict. Reading the row and then writing it would be a check-then-act race that two
//! connections in the same pool can lose, and the row would then be advanced twice.
//! Putting the expectation in the writing statement makes the comparison and the write a
//! single atomic operation, so a lost update is not representable rather than merely
//! unlikely.
//!
//! # Why the domain is consulted before the database
//!
//! [`RunTransition::apply`] runs first so an out-of-order request gets a precise
//! `IllegalTransition` naming both states rather than a generic conflict. Both checks are
//! required and neither is redundant: the domain check rejects an edge that is illegal *in
//! the abstract*, and the `WHERE` clause rejects an edge that was legal when read but stale
//! when written.
//!
//! # Scope
//!
//! `P2-005` covers the run state machine. `state`, `terminal_outcome`, `error_code`,
//! `completed_at`, and `cancellation_requested_at` are the only columns written here.
//! Starting a run from a client request and writing usage totals are the run and model-call
//! paths (`P2-007`), so they are deliberately absent rather than stubbed.

use jarvis_core::{
    CorrelationId, EventSummary, ExpectedRunState, RunErrorCode, RunEventKind, RunEventPayload,
    RunOutcome, RunState, RunTransition, UtcTimestamp,
};
use sqlx::Row;

use crate::database::{DatabaseError, SqliteDatabase, require_run_transition};
use crate::run_event_repository::NewRunEvent;

/// Maximum objective length, matching the migration's `CHECK`.
pub const MAX_OBJECTIVE_CHARS: usize = 4096;

/// One agent run as stored, limited to what the state machine reads and writes.
///
/// `objective` and `started_at` were added for the REST read model (`P2-007`), which must
/// report what a run was asked to do and when it began. They are read-only here: nothing in
/// this module writes them after creation, because a run's objective is its accepted input
/// and silently rewriting it would make the stored record disagree with what the model was
/// actually given.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredRun {
    id: String,
    session_id: String,
    workspace_id: String,
    objective: String,
    state: RunState,
    version: i64,
    cancellation_requested_at: Option<UtcTimestamp>,
    terminal_outcome: Option<RunOutcome>,
    completed_at: Option<UtcTimestamp>,
    error_code: Option<String>,
    started_at: UtcTimestamp,
}

impl StoredRun {
    /// Returns the run identifier.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the owning session identifier.
    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Returns the owning workspace identifier.
    #[must_use]
    pub fn workspace_id(&self) -> &str {
        &self.workspace_id
    }

    /// Returns the objective the run was created with.
    ///
    /// Read-only for this struct's lifetime: an objective is the run's accepted input, and
    /// a writer that rewrote it would leave the stored record disagreeing with the text the
    /// model was actually given.
    #[must_use]
    pub fn objective(&self) -> &str {
        &self.objective
    }

    /// Returns when the run was created.
    #[must_use]
    pub const fn started_at(&self) -> UtcTimestamp {
        self.started_at
    }

    /// Returns the current lifecycle state.
    #[must_use]
    pub const fn state(&self) -> RunState {
        self.state
    }

    /// Returns the current row version.
    #[must_use]
    pub const fn version(&self) -> i64 {
        self.version
    }

    /// Returns when cancellation was requested, if it was.
    #[must_use]
    pub const fn cancellation_requested_at(&self) -> Option<UtcTimestamp> {
        self.cancellation_requested_at
    }

    /// Returns the terminal outcome, present exactly when the run settled.
    #[must_use]
    pub const fn terminal_outcome(&self) -> Option<RunOutcome> {
        self.terminal_outcome
    }

    /// Returns when the run settled, if it did.
    #[must_use]
    pub const fn completed_at(&self) -> Option<UtcTimestamp> {
        self.completed_at
    }

    /// Returns the bounded failure reason, present exactly for a failed run.
    #[must_use]
    pub fn error_code(&self) -> Option<&str> {
        self.error_code.as_deref()
    }

    /// Returns the expectation a caller must supply to advance this run.
    ///
    /// Exists so a caller cannot pair a state read from one row with a version read from
    /// another.
    #[must_use]
    pub const fn expectation(&self) -> ExpectedRunState {
        ExpectedRunState::new(self.state, self.version)
    }
}

/// The fields required to create a run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewRun {
    id: String,
    session_id: String,
    workspace_id: String,
    user_id: String,
    objective: String,
    correlation_id: CorrelationId,
    started_at: UtcTimestamp,
}

impl NewRun {
    /// Validates the fields required to create a run.
    ///
    /// # Errors
    ///
    /// Returns [`DatabaseError::InvalidRunRequest`] when an identifier field is not
    /// UUID-sized text or the objective is empty or longer than 4096 characters.
    pub fn new(
        id: impl Into<String>,
        session_id: impl Into<String>,
        workspace_id: impl Into<String>,
        user_id: impl Into<String>,
        objective: impl Into<String>,
        correlation_id: CorrelationId,
        started_at: UtcTimestamp,
    ) -> Result<Self, DatabaseError> {
        let id = id.into();
        let session_id = session_id.into();
        let workspace_id = workspace_id.into();
        let user_id = user_id.into();
        let objective = objective.into();

        for (field, value) in [
            ("id", &id),
            ("session id", &session_id),
            ("workspace id", &workspace_id),
            ("user id", &user_id),
        ] {
            if value.len() != 36 {
                return Err(DatabaseError::InvalidRunRequest { field });
            }
        }

        // Counted in CHARACTERS because the migration bounds this column with `length()` on
        // a TEXT value. An objective is arbitrary user text, so it can contain non-ASCII
        // characters and counting bytes here would reject text the schema accepts,
        // presenting a unit mismatch as a caller validation error.
        let objective_length = objective.chars().count();
        if objective_length == 0 || objective_length > MAX_OBJECTIVE_CHARS {
            return Err(DatabaseError::InvalidRunRequest { field: "objective" });
        }

        Ok(Self {
            id,
            session_id,
            workspace_id,
            user_id,
            objective,
            correlation_id,
            started_at,
        })
    }
}

/// Creates a run in [`RunState::Received`].
///
/// A run always begins in `Received`, so no initial state is accepted. Accepting one would
/// let a caller create a run that is already `completed` and bypass every transition rule.
///
/// # Errors
///
/// Returns [`DatabaseError::RunConflict`] when the identifier already exists, or
/// [`DatabaseError::Sqlite`] when insertion fails. A foreign-key violation is reported as
/// `Sqlite` because it means the caller named a session, workspace, or user that does not
/// exist, which is a different bug from a duplicate identifier.
pub async fn create_run(
    database: &SqliteDatabase,
    new: &NewRun,
) -> Result<StoredRun, DatabaseError> {
    sqlx::query(
        "INSERT INTO agent_runs (\
            id, session_id, workspace_id, user_id, objective, state, \
            started_at, updated_at, correlation_id, version\
         ) VALUES (?1, ?2, ?3, ?4, ?5, 'received', ?6, ?6, ?7, 1)",
    )
    .bind(&new.id)
    .bind(&new.session_id)
    .bind(&new.workspace_id)
    .bind(&new.user_id)
    .bind(&new.objective)
    .bind(new.started_at.to_string())
    .bind(new.correlation_id.to_string())
    .execute(database.pool())
    .await
    .map_err(map_insert_error)?;

    let run = find_run(database, &new.id).await?;
    debug_assert_eq!(run.state, RunState::Received);
    Ok(run)
}

/// The failure code stored on a run that a process restart interrupted.
pub const INTERRUPTED_ERROR_CODE: &str = "interrupted_by_restart";

/// Settles every run that a process restart left in flight.
///
/// Returns the identifiers it settled, so the caller can log what it recovered rather than reporting
/// a count with no evidence.
///
/// # Why this exists, and why recovery is not a resume
///
/// A run's progress lives in memory: the executor holds the model stream and the answer text while
/// it works. A daemon that dies mid-run therefore leaves a row in a non-terminal state with no
/// process that will ever advance it. `FR-RUN-003` requires "crash recovery at documented
/// boundaries", and the documented boundary this build can honor is **truthfulness**: the run is
/// settled as failed with a code that says a restart interrupted it.
///
/// Resuming was the alternative and is rejected here on purpose. The daemon cannot know whether the
/// provider accepted the in-flight call, so a resume could answer a question that was already being
/// answered — and the model stream, not the run row, is where that partial answer lives. Reporting
/// the interruption is honest; a `completed` run whose answer was never produced would not be, and
/// neither would a `failed` run with no explanation.
///
/// A run whose cancellation was requested before the restart is settled as **cancelled** instead,
/// because the operator's intent outranks the restart: the work was already meant to stop.
///
/// # Errors
///
/// Returns [`DatabaseError`] when the scan or a settlement cannot be persisted. Both writes share one
/// transaction per run, so a failure here leaves that run's state and its event stream consistent.
pub async fn recover_interrupted_runs(
    database: &SqliteDatabase,
    now: UtcTimestamp,
) -> Result<Vec<String>, DatabaseError> {
    // Non-terminal states are exactly the ones a `NOT IN` list describes, and the index
    // `agent_runs_active_idx` covers that predicate, so this scan does not walk the table.
    let rows = sqlx::query(
        "SELECT id, session_id, state, version, cancellation_requested_at \
         FROM agent_runs \
         WHERE state NOT IN ('completed', 'cancelled', 'failed')",
    )
    .fetch_all(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "scan for interrupted runs",
        source,
    })?;

    let mut settled = Vec::with_capacity(rows.len());
    for row in &rows {
        let id: String = row.try_get("id").map_err(|source| DatabaseError::Sqlite {
            operation: "decode an interrupted run id",
            source,
        })?;
        let state_text: String = row
            .try_get("state")
            .map_err(|source| DatabaseError::Sqlite {
                operation: "decode an interrupted run state",
                source,
            })?;
        let state = state_text
            .parse::<RunState>()
            .map_err(|_| DatabaseError::StoredRunInvalid { field: "state" })?;
        let version: i64 = row
            .try_get("version")
            .map_err(|source| DatabaseError::Sqlite {
                operation: "decode an interrupted run version",
                source,
            })?;
        let cancellation_requested: Option<String> = row
            .try_get("cancellation_requested_at")
            .map_err(|source| DatabaseError::Sqlite {
                operation: "decode an interrupted run cancellation",
                source,
            })?;
        let session_id: String =
            row.try_get("session_id")
                .map_err(|source| DatabaseError::Sqlite {
                    operation: "decode an interrupted run session",
                    source,
                })?;

        let cancelled = cancellation_requested.is_some();
        let (transition, kind, summary, payload) = if cancelled {
            (
                RunTransition::cancelled(),
                RunEventKind::RunCancelled,
                "the run was cancelled before the daemon restarted",
                r#"{"outcome":"cancelled","recovered":true}"#,
            )
        } else {
            (
                RunTransition::failed(RunErrorCode::new(INTERRUPTED_ERROR_CODE).map_err(|_| {
                    DatabaseError::InvalidRunRequest {
                        field: "error_code",
                    }
                })?),
                RunEventKind::RunFailed,
                "the run was interrupted by a daemon restart",
                r#"{"outcome":"failed","error_code":"interrupted_by_restart","recovered":true}"#,
            )
        };

        // The correlation is generated here rather than reconstructed: the original is on the run's
        // earlier events, and inventing a link to it would claim a relation this write cannot prove.
        let correlation_id = CorrelationId::new();
        let event = NewRunEvent::new(
            jarvis_core::RunId::new().to_string(),
            &id,
            kind,
            Some(
                EventSummary::new(summary)
                    .map_err(|_| DatabaseError::InvalidRunEventRequest { field: "summary" })?,
            ),
            RunEventPayload::new(payload)
                .map_err(|_| DatabaseError::InvalidRunEventRequest { field: "payload" })?,
            correlation_id,
            now,
        )?;
        let expected = ExpectedRunState::new(state, version);
        let settlement = TerminalTransition::new(expected, transition, &event)?;
        settle_run(database, &settlement).await?;

        // The question is already in the transcript because it is written with the run, so recovery
        // needs no message write. Nothing records an answer, because none was produced.
        let _ = session_id;
        settled.push(id);
    }

    Ok(settled)
}

/// Reads one run by identifier.
///
/// # Errors
///
/// Returns [`DatabaseError::RunNotFound`] when no row has that identifier, or
/// [`DatabaseError::Sqlite`] when the read fails.
pub async fn find_run(database: &SqliteDatabase, id: &str) -> Result<StoredRun, DatabaseError> {
    let row = sqlx::query(
        "SELECT id, session_id, workspace_id, objective, state, version, \
                cancellation_requested_at, terminal_outcome, completed_at, error_code, \
                started_at \
         FROM agent_runs WHERE id = ?1",
    )
    .bind(id)
    .fetch_optional(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "read an agent run",
        source,
    })?
    .ok_or(DatabaseError::RunNotFound)?;

    decode_run(&row)
}

/// Advances one run through the state machine.
///
/// # Errors
///
/// - [`DatabaseError::RunTransitionRefused`] when the domain rejects the edge, which
///   includes every request made against an already-settled run.
/// - [`DatabaseError::RunConflict`] when the stored state or version no longer matches the
///   caller's expectation, meaning another writer advanced the run first.
/// - [`DatabaseError::Sqlite`] for any other persistence failure.
pub async fn transition_run(
    database: &SqliteDatabase,
    id: &str,
    expected: ExpectedRunState,
    transition: &RunTransition,
    at: UtcTimestamp,
) -> Result<StoredRun, DatabaseError> {
    transition
        .apply(expected)
        .map_err(|source| DatabaseError::RunTransitionRefused { source })?;

    let target = transition.to();
    let outcome = transition.outcome().map(RunOutcome::as_str);
    let error_code = transition.error_code().map(RunErrorCode::as_str);
    // A terminal write stamps `completed_at`; a non-terminal write must leave it null,
    // which the migration's CHECK enforces independently of this decision.
    let completed_at = target.is_terminal().then(|| at.to_string());
    // `COALESCE` keeps an earlier cancellation request visible. The settlement is not the
    // moment cancellation was asked for, and overwriting the request time would erase the
    // interval between the two, which is the number that shows whether cancellation
    // actually interrupted fast.
    let cancellation_requested_at = (target == RunState::Cancelled).then(|| at.to_string());

    let result = sqlx::query(
        "UPDATE agent_runs \
         SET state = ?3, \
             terminal_outcome = ?4, \
             error_code = ?5, \
             completed_at = ?6, \
             cancellation_requested_at = COALESCE(cancellation_requested_at, ?7), \
             updated_at = ?8, \
             version = version + 1 \
         WHERE id = ?1 AND state = ?2 AND version = ?9",
    )
    .bind(id)
    .bind(expected.state().as_str())
    .bind(target.as_str())
    .bind(outcome)
    .bind(error_code)
    .bind(completed_at)
    .bind(cancellation_requested_at)
    .bind(at.to_string())
    .bind(expected.version())
    .execute(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "advance an agent run",
        source,
    })?;

    require_run_transition(result.rows_affected())?;
    find_run(database, id).await
}

/// One terminal transition together with the settlement event that must accompany it.
///
/// Both halves are required together because they describe one settlement, and splitting them
/// across two calls is what makes the ordering defect below possible.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalTransition {
    id: String,
    run_id: String,
    expected: ExpectedRunState,
    transition: RunTransition,
    summary: Option<EventSummary>,
    payload: RunEventPayload,
    correlation_id: CorrelationId,
    occurred_at: UtcTimestamp,
}

impl TerminalTransition {
    /// Validates a terminal transition and its event.
    ///
    /// The event fields are taken as a [`NewRunEvent`] so the settlement uses the same validated
    /// event shape as an ordinary append. Building them separately here would create a second
    /// constructor for the same row, and the two would drift.
    ///
    /// # Errors
    ///
    /// Returns [`DatabaseError::RunTransitionRefused`] when the transition's target is not
    /// terminal, because this function exists to settle a run and a non-terminal write here would
    /// leave the run open beside an event that claims it settled. Returns
    /// [`DatabaseError::InvalidRunEventRequest`] when the event's kind does not match the target
    /// state, so a run cannot settle with an event that describes a different outcome.
    pub fn new(
        expected: ExpectedRunState,
        transition: RunTransition,
        event: &NewRunEvent,
    ) -> Result<Self, DatabaseError> {
        if !transition.to().is_terminal() {
            return Err(DatabaseError::RunTransitionRefused {
                // The reported state is the one the caller believed they were moving from, which is
                // the only state the evidence actually contains.
                source: jarvis_core::RunTransitionError::TerminalStateImmutable {
                    from: expected.state(),
                },
            });
        }
        // The kind must match the target state, so a `Completed` transition cannot be recorded as
        // a `RunCancelled` event. `settle_run` derives the stored kind from the target state as
        // well; this check catches the mistake at construction, where the diagnosis is clearest.
        let Some(expected_kind) = transition.to().terminal_event_kind() else {
            return Err(DatabaseError::InvalidRunEventRequest { field: "kind" });
        };
        if event.kind() != expected_kind {
            return Err(DatabaseError::InvalidRunEventRequest { field: "kind" });
        }
        Ok(Self {
            id: event.id().to_owned(),
            run_id: event.run_id().to_owned(),
            expected,
            transition,
            summary: event.summary().cloned(),
            payload: event.payload().clone(),
            correlation_id: event.correlation_id(),
            occurred_at: event.occurred_at(),
        })
    }
}

/// Settles a run and appends its terminal event in ONE transaction.
///
/// # Why the ordering matters, and why two calls cannot express it
///
/// [`append_run_event`] refuses to write to a run that is already terminal, because a settled
/// run emits nothing more. `transition_run` writes the terminal state. So the two calls are
/// **inherently ordered**: transitioning first makes the event append fail with
/// `RunEventAfterSettlement`, and appending first writes a settlement event for a run that has
/// not settled.
///
/// Neither order can be fixed by the caller, because the guard and the transition are correct
/// individually — only their combination is impossible. The event therefore has to be written
/// inside the transaction that performs the transition, and that is what this function does
/// rather than what its documentation asks a caller to remember.
///
/// # Errors
///
/// - [`DatabaseError::RunTransitionRefused`] when the domain rejects the edge, which includes
///   every request made against an already-settled run.
/// - [`DatabaseError::RunConflict`] when the stored state or version no longer matches the
///   caller's expectation. Nothing is written in that case, so the losing writer leaves no event.
/// - [`DatabaseError::RunNotFound`] when no run has that identifier.
/// - [`DatabaseError::Sqlite`] for any other persistence failure.
pub async fn settle_run(
    database: &SqliteDatabase,
    settlement: &TerminalTransition,
) -> Result<StoredRun, DatabaseError> {
    // Consulted before the transaction so an illegal edge is reported precisely, matching
    // `transition_run`. The `WHERE` clause still decides, because this check cannot see a
    // concurrent writer.
    settlement
        .transition
        .apply(settlement.expected)
        .map_err(|source| DatabaseError::RunTransitionRefused { source })?;

    let target = settlement.transition.to();
    // The kind is derived from the target state rather than passed in, so the event cannot
    // describe a different state than the row now holds.
    let kind = target
        .terminal_event_kind()
        .ok_or(DatabaseError::InvalidRunEventRequest { field: "kind" })?;

    let mut transaction =
        database
            .pool()
            .begin()
            .await
            .map_err(|source| DatabaseError::Sqlite {
                operation: "begin a run settlement",
                source,
            })?;

    let outcome = settlement.transition.outcome().map(RunOutcome::as_str);
    let error_code = settlement.transition.error_code().map(RunErrorCode::as_str);
    let at = settlement.occurred_at.to_string();

    let result = sqlx::query(
        "UPDATE agent_runs \
         SET state = ?3, \
             terminal_outcome = ?4, \
             error_code = ?5, \
             completed_at = ?6, \
             cancellation_requested_at = COALESCE(cancellation_requested_at, ?7), \
             updated_at = ?6, \
             version = version + 1 \
         WHERE id = ?1 AND state = ?2 AND version = ?8 \
           AND state NOT IN ('completed', 'cancelled', 'failed')",
    )
    .bind(&settlement.run_id)
    .bind(settlement.expected.state().as_str())
    .bind(target.as_str())
    .bind(outcome)
    .bind(error_code)
    .bind(&at)
    .bind((target == RunState::Cancelled).then_some(at.as_str()))
    .bind(settlement.expected.version())
    .execute(&mut *transaction)
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "settle an agent run",
        source,
    })?;

    if result.rows_affected() != 1 {
        // The transaction is dropped rather than committed, so a lost race leaves nothing
        // behind. The reason is resolved by the caller's follow-up read, which knows identity.
        return Err(DatabaseError::RunConflict);
    }

    sqlx::query(
        "INSERT INTO run_events (\
            id, run_id, sequence, kind, summary, payload, \
            correlation_id, occurred_at, recorded_at\
         ) \
         SELECT ?1, ?2, \
            (SELECT COALESCE(MAX(sequence), 0) + 1 FROM run_events WHERE run_id = ?2), \
            ?3, ?4, ?5, ?6, ?7, ?7",
    )
    .bind(&settlement.id)
    .bind(&settlement.run_id)
    .bind(kind.as_str())
    .bind(settlement.summary.as_ref().map(EventSummary::as_str))
    .bind(settlement.payload.as_str())
    .bind(settlement.correlation_id.to_string())
    .bind(&at)
    .execute(&mut *transaction)
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "append a settlement run event",
        source,
    })?;

    transaction
        .commit()
        .await
        .map_err(|source| DatabaseError::Sqlite {
            operation: "commit a run settlement",
            source,
        })?;

    find_run(database, &settlement.run_id).await
}

/// Records that a client requested cancellation without settling the run yet.
///
/// This is deliberately not a transition. Cancellation is a *request*; the run settles as
/// `cancelled` only once the in-flight work has actually stopped, which is what
/// `docs/quality/acceptance-tests.md` A04 requires ("new work stops, state settles once").
/// Marking the run terminal here would report a stopped run before anything stopped.
///
/// # Why there is no expected version
///
/// Cancellation is **operator intent**, and `docs/adr/0013-restart-settles-interrupted-runs.md`
/// establishes that such intent "already outranks the interruption". A version guard on this write was
/// therefore wrong in two ways, and both were observed rather than theorised:
///
/// 1. **A cancel could be lost to the system's own activity.** Every progress write advances the run's
///    version while the run executes, so a client's version is stale almost immediately. The request
///    then failed with [`DatabaseError::RunConflict`], which a client cannot act on and cannot resolve —
///    the version it needs is changing several times a second. The user asked to stop a run and was
///    refused because the run was running.
/// 2. **A concurrent-cancel test was flaky for the same reason**, failing only under load and only
///    intermittently, because the window between reading the version and writing it is where the
///    executor's next write lands.
///
/// The guard that *is* meaningful is kept: a **settled** run refuses a cancellation request, because
/// there is no work left to stop. That is a fact about the run rather than about who is asking, so it
/// cannot go stale.
///
/// A repeat request is idempotent: `COALESCE` keeps the first request time, so the interval between
/// asking and stopping stays measurable.
///
/// # Errors
///
/// Returns [`DatabaseError::RunNotFound`] when no run has that identifier,
/// [`DatabaseError::RunTransitionRefused`] for an already-settled run, and
/// [`DatabaseError::Sqlite`] for any other persistence failure.
pub async fn request_run_cancellation(
    database: &SqliteDatabase,
    id: &str,
    at: UtcTimestamp,
) -> Result<StoredRun, DatabaseError> {
    let result = sqlx::query(
        "UPDATE agent_runs \
         SET cancellation_requested_at = COALESCE(cancellation_requested_at, ?2), \
             updated_at = ?2, \
             version = version + 1 \
         WHERE id = ?1 \
           AND state NOT IN ('completed', 'cancelled', 'failed')",
    )
    .bind(id)
    .bind(at.to_string())
    .execute(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "request agent run cancellation",
        source,
    })?;

    if result.rows_affected() == 0 {
        // Either the run is absent or it has settled, and the two are different answers for a caller,
        // so which one it is comes from reading rather than from guessing. A `RunNotFound` propagates
        // from the read, which is where identity is actually known.
        let current = find_run(database, id).await?;
        if current.state().is_terminal() {
            return Err(DatabaseError::RunTransitionRefused {
                source: jarvis_core::RunTransitionError::TerminalStateImmutable {
                    from: current.state(),
                },
            });
        }
        // Non-terminal and unmatched is not reachable through the statement above, so reaching it means
        // a racing writer changed the row between the write and this read. A conflict is the honest
        // report; silently succeeding would claim a request that was not recorded.
        return Err(DatabaseError::RunConflict);
    }

    find_run(database, id).await
}

fn decode_run(row: &sqlx::sqlite::SqliteRow) -> Result<StoredRun, DatabaseError> {
    let state = row
        .try_get::<String, _>("state")
        .map_err(|source| DatabaseError::Sqlite {
            operation: "decode a stored run state",
            source,
        })?
        .parse::<RunState>()
        .map_err(|_| DatabaseError::StoredRunInvalid { field: "state" })?;

    let terminal_outcome = match row
        .try_get::<Option<String>, _>("terminal_outcome")
        .map_err(|source| DatabaseError::Sqlite {
            operation: "decode a stored run outcome",
            source,
        })? {
        Some(value) => {
            Some(
                value
                    .parse::<RunOutcome>()
                    .map_err(|_| DatabaseError::StoredRunInvalid {
                        field: "terminal outcome",
                    })?,
            )
        }
        None => None,
    };

    let run = StoredRun {
        id: text_field(row, "id")?,
        session_id: text_field(row, "session_id")?,
        workspace_id: text_field(row, "workspace_id")?,
        objective: text_field(row, "objective")?,
        state,
        version: row
            .try_get("version")
            .map_err(|source| DatabaseError::Sqlite {
                operation: "decode a stored run version",
                source,
            })?,
        cancellation_requested_at: decode_optional_timestamp(row, "cancellation_requested_at")?,
        terminal_outcome,
        completed_at: decode_optional_timestamp(row, "completed_at")?,
        error_code: row
            .try_get::<Option<String>, _>("error_code")
            .map_err(|source| DatabaseError::Sqlite {
                operation: "decode a stored run error code",
                source,
            })?,
        started_at: text_field(row, "started_at")?
            .parse::<UtcTimestamp>()
            .map_err(|_| DatabaseError::StoredRunInvalid {
                field: "start timestamp",
            })?,
    };

    // Verified rather than trusted. SQLite's CHECK constraints prove these invariants for
    // rows this build wrote, but a row written by another build, restored from a backup, or
    // edited outside JARVIS is not covered by that argument. A `completed` run with no
    // outcome would otherwise be read back as a settled success.
    check_stored_consistency(&run)?;
    Ok(run)
}

/// Rejects a stored row whose state and terminal fields disagree.
fn check_stored_consistency(run: &StoredRun) -> Result<(), DatabaseError> {
    if run.terminal_outcome != run.state.required_outcome() {
        return Err(DatabaseError::StoredRunInvalid {
            field: "terminal outcome",
        });
    }
    if run.completed_at.is_some() != run.state.is_terminal() {
        return Err(DatabaseError::StoredRunInvalid {
            field: "completion timestamp",
        });
    }
    if run.error_code.is_some() != (run.state == RunState::Failed) {
        return Err(DatabaseError::StoredRunInvalid {
            field: "failure code",
        });
    }
    Ok(())
}

fn decode_optional_timestamp(
    row: &sqlx::sqlite::SqliteRow,
    column: &'static str,
) -> Result<Option<UtcTimestamp>, DatabaseError> {
    match row
        .try_get::<Option<String>, _>(column)
        .map_err(|source| DatabaseError::Sqlite {
            operation: "decode a stored run timestamp",
            source,
        })? {
        Some(value) => value
            .parse::<UtcTimestamp>()
            .map(Some)
            .map_err(|_| DatabaseError::StoredRunInvalid { field: "timestamp" }),
        None => Ok(None),
    }
}

fn text_field(
    row: &sqlx::sqlite::SqliteRow,
    column: &'static str,
) -> Result<String, DatabaseError> {
    row.try_get(column).map_err(|source| DatabaseError::Sqlite {
        operation: "decode a stored run field",
        source,
    })
}

/// Maps an insertion failure onto the reason it represents.
///
/// A duplicate identifier is a caller mistake with a known remedy; a foreign-key violation
/// means a parent row is missing, which is a different bug. Both arrive as `sqlx::Error`, so
/// they are separated by the database's own code rather than by assuming every insert
/// failure is alike. The identifier is deliberately not echoed into the error: a diagnostic
/// should describe the failure without restating input.
fn map_insert_error(source: sqlx::Error) -> DatabaseError {
    if let sqlx::Error::Database(database_error) = &source {
        // SQLITE_CONSTRAINT_PRIMARYKEY: a run with this identifier already exists.
        if database_error.code().as_deref() == Some("1555") {
            return DatabaseError::RunConflict;
        }
    }
    DatabaseError::Sqlite {
        operation: "insert an agent run",
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
    use crate::DEFAULT_DATABASE_FILENAME;
    use jarvis_core::RunTransitionError;

    static TEST_DIRECTORY_ID: AtomicU64 = AtomicU64::new(0);

    const USER: &str = "0198f000-0000-7000-8000-000000000001";
    const WORKSPACE: &str = "0198f000-0000-7000-8000-000000000002";
    const SESSION: &str = "0198f000-0000-7000-8000-000000000003";
    const RUN_A: &str = "0198f000-0000-7000-8000-0000000000a1";
    const RUN_B: &str = "0198f000-0000-7000-8000-0000000000b2";

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let sequence = TEST_DIRECTORY_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "jarvis-run-state-machine-{}-{sequence}",
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

    async fn seed_identity(database: &SqliteDatabase) {
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
    }

    async fn seeded_database() -> (TestDirectory, SqliteDatabase) {
        let directory = TestDirectory::new();
        let path = directory.path().join(DEFAULT_DATABASE_FILENAME);
        let database = must(SqliteDatabase::open(&path).await);
        seed_identity(&database).await;
        (directory, database)
    }

    /// A distinct instant per call, so a stored timestamp is attributable to its writer
    /// rather than merely present.
    fn at(minute: i128) -> UtcTimestamp {
        must(UtcTimestamp::from_unix_nanos(
            1_774_000_000_000_000_000 + minute * 60_000_000_000,
        ))
    }

    fn transition(to: RunState) -> RunTransition {
        must(RunTransition::new(to, to.required_outcome(), None))
    }

    fn code(value: &str) -> RunErrorCode {
        must(RunErrorCode::new(value))
    }

    /// Builds a settlement event for the tests below, so each call site states only what it varies.
    fn settlement_event(
        id: &str,
        run_id: &str,
        kind: jarvis_core::RunEventKind,
        payload: &str,
        minute: i128,
    ) -> crate::NewRunEvent {
        must(crate::NewRunEvent::new(
            id,
            run_id,
            kind,
            None,
            must(jarvis_core::RunEventPayload::new(payload)),
            CorrelationId::new(),
            at(minute),
        ))
    }

    async fn one_run(database: &SqliteDatabase, id: &str) -> StoredRun {
        let new = must(NewRun::new(
            id,
            SESSION,
            WORKSPACE,
            USER,
            "Summarise today's calendar",
            CorrelationId::new(),
            at(0),
        ));
        must(create_run(database, &new).await)
    }

    async fn advance(database: &SqliteDatabase, run: StoredRun, to: RunState) -> StoredRun {
        must(
            transition_run(
                database,
                run.id(),
                run.expectation(),
                &transition(to),
                at(1),
            )
            .await,
        )
    }

    /// Walks a run to `Responding` through the documented path.
    async fn at_responding(database: &SqliteDatabase) -> StoredRun {
        let mut run = one_run(database, RUN_A).await;
        for next in [
            RunState::ContextBuilding,
            RunState::Planning,
            RunState::Executing,
            RunState::Observing,
            RunState::Responding,
        ] {
            run = advance(database, run, next).await;
        }
        run
    }

    #[tokio::test]
    async fn a_new_run_starts_received_at_version_one() {
        let (_directory, database) = seeded_database().await;
        let run = one_run(&database, RUN_A).await;

        assert_eq!(run.state(), RunState::Received);
        assert_eq!(run.version(), 1);
        assert_eq!(run.terminal_outcome(), None);
        assert_eq!(run.completed_at(), None);
        assert_eq!(run.error_code(), None);
        assert_eq!(run.cancellation_requested_at(), None);
        assert_eq!(run.session_id(), SESSION);
        assert_eq!(run.workspace_id(), WORKSPACE);
    }

    #[tokio::test]
    async fn a_documented_path_advances_step_by_step_and_settles() {
        let (_directory, database) = seeded_database().await;
        let mut run = one_run(&database, RUN_A).await;

        // Exactly one version per transition. A version that did not move, or moved by more
        // than one, would let a second writer holding the previous version match and advance
        // the run again. The expected version is stated per step rather than counted, so no
        // cast can hide a wrap.
        for (next, expected_version) in [
            (RunState::ContextBuilding, 2_i64),
            (RunState::Planning, 3),
            (RunState::Executing, 4),
            (RunState::Observing, 5),
            (RunState::Responding, 6),
            (RunState::Completed, 7),
        ] {
            run = advance(&database, run, next).await;
            assert_eq!(run.state(), next);
            assert_eq!(run.version(), expected_version, "version after {next}");
        }

        assert_eq!(run.terminal_outcome(), Some(RunOutcome::Succeeded));
        assert_eq!(run.completed_at(), Some(at(1)));
        assert_eq!(run.error_code(), None);
    }

    #[tokio::test]
    async fn an_illegal_edge_is_refused_by_name_and_leaves_the_row_untouched() {
        let (_directory, database) = seeded_database().await;
        let run = one_run(&database, RUN_A).await;

        // `received -> executing` skips context building and planning. It must be refused
        // for the *ordering* reason, not a concurrency reason, or a real ordering bug would
        // read as a race and be retried.
        let outcome = transition_run(
            &database,
            RUN_A,
            run.expectation(),
            &transition(RunState::Executing),
            at(1),
        )
        .await;
        match outcome {
            Err(DatabaseError::RunTransitionRefused { source }) => assert_eq!(
                source,
                RunTransitionError::IllegalTransition {
                    from: RunState::Received,
                    to: RunState::Executing,
                }
            ),
            other => panic!("expected an illegal-transition refusal, got {other:?}"),
        }

        let reloaded = must(find_run(&database, RUN_A).await);
        assert_eq!(reloaded.state(), RunState::Received);
        assert_eq!(
            reloaded.version(),
            1,
            "a refused transition must not consume a version"
        );
    }

    #[tokio::test]
    async fn a_stale_expectation_is_a_conflict_and_does_not_double_advance() {
        let (_directory, database) = seeded_database().await;
        let run = one_run(&database, RUN_A).await;
        let stale = run.expectation();

        let advanced = advance(&database, run, RunState::ContextBuilding).await;
        assert_eq!(advanced.version(), 2);

        // Same legal edge and same expectation, but the version has moved. This is the
        // guard that stops two writers both advancing one run.
        let second = transition_run(
            &database,
            RUN_A,
            stale,
            &transition(RunState::ContextBuilding),
            at(2),
        )
        .await;
        assert!(
            matches!(second, Err(DatabaseError::RunConflict)),
            "a stale expectation must be a conflict, got {second:?}"
        );

        let reloaded = must(find_run(&database, RUN_A).await);
        assert_eq!(reloaded.state(), RunState::ContextBuilding);
        assert_eq!(reloaded.version(), 2);
    }

    /// The falsification test for the concurrency guard.
    ///
    /// Both callers hold the *same* expectation and both pass the domain's transition check:
    /// `received -> context_building` is legal for both, and the state each believes is
    /// stored really is stored. Only the `WHERE` clause separates them. Implemented as a
    /// read-then-write, both would succeed and the run would advance twice.
    #[tokio::test]
    async fn concurrent_callers_with_one_expectation_advance_a_run_exactly_once() {
        let (_directory, database) = seeded_database().await;
        let run = one_run(&database, RUN_A).await;
        let shared = run.expectation();

        // Bound before the join because both futures borrow the transition: an inline
        // temporary would be dropped at the end of the statement while still borrowed.
        let first_attempt = transition(RunState::ContextBuilding);
        let second_attempt = transition(RunState::ContextBuilding);

        let (first, second) = tokio::join!(
            transition_run(&database, RUN_A, shared, &first_attempt, at(1)),
            transition_run(&database, RUN_A, shared, &second_attempt, at(2)),
        );

        let successes = [&first, &second]
            .into_iter()
            .filter(|outcome| outcome.is_ok())
            .count();
        assert_eq!(
            successes, 1,
            "exactly one writer must win: {first:?} / {second:?}"
        );

        let reloaded = must(find_run(&database, RUN_A).await);
        assert_eq!(reloaded.state(), RunState::ContextBuilding);
        assert_eq!(reloaded.version(), 2, "the run advanced more than once");
    }

    #[tokio::test]
    async fn a_settled_run_cannot_move_again_even_with_the_current_version() {
        let (_directory, database) = seeded_database().await;
        let mut run = one_run(&database, RUN_A).await;
        for next in [
            RunState::ContextBuilding,
            RunState::Planning,
            RunState::Executing,
            RunState::Observing,
            RunState::Responding,
            RunState::Completed,
        ] {
            run = advance(&database, run, next).await;
        }

        // The expectation is current, so this cannot be a version conflict. It must be
        // refused for terminal immutability, which is a different defect from a stale read
        // and has a different remedy.
        let outcome = transition_run(
            &database,
            RUN_A,
            run.expectation(),
            &transition(RunState::Cancelled),
            at(2),
        )
        .await;
        match outcome {
            Err(DatabaseError::RunTransitionRefused { source }) => assert_eq!(
                source,
                RunTransitionError::TerminalStateImmutable {
                    from: RunState::Completed
                }
            ),
            other => panic!("expected terminal immutability, got {other:?}"),
        }

        let reloaded = must(find_run(&database, RUN_A).await);
        assert_eq!(reloaded.state(), RunState::Completed);
        assert_eq!(reloaded.version(), run.version());
    }

    #[tokio::test]
    async fn a_failure_stores_an_outcome_a_code_and_a_completion_time() {
        let (_directory, database) = seeded_database().await;
        let run = one_run(&database, RUN_A).await;

        let failed = must(
            transition_run(
                &database,
                RUN_A,
                run.expectation(),
                &RunTransition::failed(code("provider_timeout")),
                at(7),
            )
            .await,
        );

        assert_eq!(failed.state(), RunState::Failed);
        assert_eq!(failed.terminal_outcome(), Some(RunOutcome::Failed));
        assert_eq!(failed.error_code(), Some("provider_timeout"));
        assert_eq!(failed.completed_at(), Some(at(7)));
    }

    #[tokio::test]
    async fn a_run_waiting_on_approval_can_only_be_cancelled() {
        let (_directory, database) = seeded_database().await;
        let run = one_run(&database, RUN_A).await;
        let context = advance(&database, run, RunState::ContextBuilding).await;
        let planning = advance(&database, context, RunState::Planning).await;
        let waiting = must(
            transition_run(
                &database,
                RUN_A,
                planning.expectation(),
                &transition(RunState::AwaitingApproval),
                at(1),
            )
            .await,
        );
        assert_eq!(waiting.state(), RunState::AwaitingApproval);

        // No machine work is in progress while a human decides, so there is no process to
        // attribute a failure to. Refusing it here is what stops an unrelated infrastructure
        // error from settling a run that is waiting on a person.
        let failed = transition_run(
            &database,
            RUN_A,
            waiting.expectation(),
            &RunTransition::failed(code("provider_timeout")),
            at(2),
        )
        .await;
        assert!(
            matches!(
                failed,
                Err(DatabaseError::RunTransitionRefused {
                    source: RunTransitionError::IllegalTransition {
                        from: RunState::AwaitingApproval,
                        to: RunState::Failed
                    }
                })
            ),
            "awaiting approval must not be failable, got {failed:?}"
        );

        // Cancellation is the documented way an approval ends without a decision.
        let cancelled = advance(&database, waiting, RunState::Cancelled).await;
        assert_eq!(cancelled.terminal_outcome(), Some(RunOutcome::Cancelled));
    }

    #[tokio::test]
    async fn a_generation_failure_is_recorded_as_failed_not_completed() {
        let (_directory, database) = seeded_database().await;
        let run = at_responding(&database).await;

        // Without the `responding -> failed` edge the only legal successor would be
        // `completed`, recording a failed answer as a success.
        let failed = must(
            transition_run(
                &database,
                RUN_A,
                run.expectation(),
                &RunTransition::failed(code("generation_failed")),
                at(5),
            )
            .await,
        );
        assert_eq!(failed.state(), RunState::Failed);
        assert_eq!(failed.error_code(), Some("generation_failed"));
        assert_eq!(failed.terminal_outcome(), Some(RunOutcome::Failed));
    }

    #[tokio::test]
    async fn cancellation_is_recorded_as_a_request_before_the_run_settles() {
        let (_directory, database) = seeded_database().await;
        // A run must exist; its version is deliberately NOT read, because a cancellation carries no
        // expectation for the same reason a client cannot supply a current one: the run's own progress
        // writes invalidate it.
        one_run(&database, RUN_A).await;

        // A request must not settle the run: acceptance test A04 requires the run to settle
        // once, after the in-flight work has actually stopped.
        let requested = must(request_run_cancellation(&database, RUN_A, at(3)).await);
        assert_eq!(requested.state(), RunState::Received);
        assert_eq!(requested.terminal_outcome(), None);
        assert_eq!(requested.cancellation_requested_at(), Some(at(3)));
        assert_eq!(requested.version(), 2);

        let settled = advance(&database, requested, RunState::Cancelled).await;
        assert_eq!(settled.state(), RunState::Cancelled);
        assert_eq!(settled.terminal_outcome(), Some(RunOutcome::Cancelled));
        // The request time is preserved rather than overwritten by the settlement, so the
        // interval between asking and stopping stays measurable.
        assert_eq!(settled.cancellation_requested_at(), Some(at(3)));
        assert_ne!(settled.completed_at(), Some(at(3)));
    }

    #[tokio::test]
    async fn a_second_cancellation_request_keeps_the_first_request_time() {
        let (_directory, database) = seeded_database().await;
        one_run(&database, RUN_A).await;

        let first = must(request_run_cancellation(&database, RUN_A, at(3)).await);
        assert_eq!(
            first.cancellation_requested_at(),
            Some(at(3)),
            "the first request records the time"
        );
        // The second request is made **without** the version the first one returned, which is the
        // property that matters: a cancellation carries no expectation, so a caller whose version was
        // invalidated by the run's own progress writes is still able to ask.
        let second = must(request_run_cancellation(&database, RUN_A, at(9)).await);

        assert_eq!(second.cancellation_requested_at(), Some(at(3)));
        assert_eq!(second.version(), 3);
        assert_eq!(second.state(), RunState::Received);
    }

    #[tokio::test]
    async fn cancelling_a_settled_run_is_refused() {
        let (_directory, database) = seeded_database().await;
        let run = one_run(&database, RUN_A).await;
        must(
            transition_run(
                &database,
                RUN_A,
                run.expectation(),
                &RunTransition::failed(code("cancelled_by_operator")),
                at(1),
            )
            .await,
        );

        let outcome = request_run_cancellation(&database, RUN_A, at(2)).await;
        assert!(
            matches!(
                outcome,
                Err(DatabaseError::RunTransitionRefused {
                    source: RunTransitionError::TerminalStateImmutable {
                        from: RunState::Failed
                    }
                })
            ),
            "a settled run must not accept a cancellation request, got {outcome:?}"
        );
    }
    #[tokio::test]
    async fn a_missing_run_is_reported_and_never_created_by_a_transition() {
        let (_directory, database) = seeded_database().await;

        assert!(matches!(
            find_run(&database, RUN_B).await,
            Err(DatabaseError::RunNotFound)
        ));

        let outcome = transition_run(
            &database,
            RUN_B,
            ExpectedRunState::new(RunState::Received, 1),
            &transition(RunState::ContextBuilding),
            at(1),
        )
        .await;
        assert!(
            matches!(outcome, Err(DatabaseError::RunConflict)),
            "an UPDATE that matched nothing is a conflict, got {outcome:?}"
        );
        assert!(matches!(
            find_run(&database, RUN_B).await,
            Err(DatabaseError::RunNotFound)
        ));
    }

    #[tokio::test]
    async fn a_duplicate_run_identifier_is_refused() {
        let (_directory, database) = seeded_database().await;
        let _first = one_run(&database, RUN_A).await;

        let duplicate = must(NewRun::new(
            RUN_A,
            SESSION,
            WORKSPACE,
            USER,
            "A second objective",
            CorrelationId::new(),
            at(1),
        ));
        assert!(matches!(
            create_run(&database, &duplicate).await,
            Err(DatabaseError::RunConflict)
        ));
    }

    #[tokio::test]
    async fn a_run_for_a_missing_session_is_refused_by_the_foreign_key() {
        let (_directory, database) = seeded_database().await;
        let orphan = must(NewRun::new(
            RUN_A,
            // No such session. A dangling run would make the run graph unresolvable and
            // would never be reachable as a child of any session.
            "0198f000-0000-7000-8000-0000000000ff",
            WORKSPACE,
            USER,
            "Orphan run",
            CorrelationId::new(),
            at(0),
        ));

        assert!(create_run(&database, &orphan).await.is_err());
    }

    /// An objective is user text, so its bound must be counted in characters.
    #[tokio::test]
    async fn an_objective_is_bounded_in_characters_not_bytes() {
        let (_directory, database) = seeded_database().await;

        // Exactly 4096 characters, but 8192 bytes (`é` is two UTF-8 bytes). Byte-counting
        // would reject this even though the schema accepts it.
        let multibyte = "é".repeat(MAX_OBJECTIVE_CHARS);
        assert_eq!(multibyte.chars().count(), MAX_OBJECTIVE_CHARS);
        assert_eq!(multibyte.len(), MAX_OBJECTIVE_CHARS * 2);

        let accepted = must(NewRun::new(
            RUN_A,
            SESSION,
            WORKSPACE,
            USER,
            multibyte,
            CorrelationId::new(),
            at(0),
        ));
        let stored = must(create_run(&database, &accepted).await);
        assert_eq!(stored.state(), RunState::Received);

        // One character more is refused before SQLite sees it.
        let too_long = "é".repeat(MAX_OBJECTIVE_CHARS + 1);
        assert!(matches!(
            NewRun::new(
                RUN_B,
                SESSION,
                WORKSPACE,
                USER,
                too_long,
                CorrelationId::new(),
                at(0)
            ),
            Err(DatabaseError::InvalidRunRequest { field: "objective" })
        ));
    }

    #[tokio::test]
    async fn an_empty_objective_or_malformed_identifier_is_refused() {
        let (_directory, _database) = seeded_database().await;

        assert!(matches!(
            NewRun::new(
                RUN_A,
                SESSION,
                WORKSPACE,
                USER,
                "",
                CorrelationId::new(),
                at(0)
            ),
            Err(DatabaseError::InvalidRunRequest { field: "objective" })
        ));

        assert!(matches!(
            NewRun::new(
                "not-a-uuid",
                SESSION,
                WORKSPACE,
                USER,
                "Objective",
                CorrelationId::new(),
                at(0)
            ),
            Err(DatabaseError::InvalidRunRequest { field: "id" })
        ));

        assert!(matches!(
            NewRun::new(
                RUN_A,
                "short",
                WORKSPACE,
                USER,
                "Objective",
                CorrelationId::new(),
                at(0)
            ),
            Err(DatabaseError::InvalidRunRequest {
                field: "session id"
            })
        ));
    }

    /// A run row that contradicts the state machine is reported, not trusted.
    #[tokio::test]
    async fn a_stored_row_that_contradicts_the_state_machine_is_reported() {
        let (_directory, database) = seeded_database().await;
        let _run = one_run(&database, RUN_A).await;

        // `completed` with no outcome must already be unrepresentable through the schema...
        let direct = sqlx::query("UPDATE agent_runs SET state = 'completed' WHERE id = ?1")
            .bind(RUN_A)
            .execute(database.pool())
            .await;
        assert!(
            direct.is_err(),
            "the migration must make this row unrepresentable"
        );

        // ...so producing it requires reaching around the constraint, which is what a
        // restored backup or an external tool effectively does. Disabling check constraints
        // on one connection reproduces exactly that without editing the schema.
        let mut connection = must(database.pool().acquire().await);
        must(
            sqlx::query("PRAGMA ignore_check_constraints = ON")
                .execute(&mut *connection)
                .await
                .map(|_| ()),
        );
        must(
            sqlx::query("UPDATE agent_runs SET state = 'completed' WHERE id = ?1")
                .bind(RUN_A)
                .execute(&mut *connection)
                .await
                .map(|_| ()),
        );
        must(connection.close().await);

        match find_run(&database, RUN_A).await {
            Err(DatabaseError::StoredRunInvalid { field }) => {
                assert_eq!(field, "terminal outcome");
            }
            other => panic!("a contradictory row must be reported, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn the_state_machine_survives_a_reopen() {
        let directory = TestDirectory::new();
        let path = directory.path().join(DEFAULT_DATABASE_FILENAME);

        let (state, version) = {
            let database = must(SqliteDatabase::open(&path).await);
            seed_identity(&database).await;
            let run = one_run(&database, RUN_A).await;
            let advanced = advance(&database, run, RunState::ContextBuilding).await;
            database.close().await;
            (advanced.state(), advanced.version())
        };

        let reopened = must(SqliteDatabase::open(&path).await);
        let reloaded = must(find_run(&reopened, RUN_A).await);
        assert_eq!((reloaded.state(), reloaded.version()), (state, version));

        // The optimistic-concurrency guard still holds after the reopen: the persisted
        // version is what it was written as, not reset. The requested edge must be legal
        // from the state the caller *believes* is stored, or the domain would refuse it as
        // an ordering error and this would no longer be testing the SQL guard.
        let from_previous_version = ExpectedRunState::new(RunState::Received, 1);
        assert!(matches!(
            transition_run(
                &reopened,
                RUN_A,
                from_previous_version,
                &transition(RunState::ContextBuilding),
                at(2)
            )
            .await,
            Err(DatabaseError::RunConflict)
        ));
        reopened.close().await;
    }

    /// The defect this primitive exists to prevent, asserted against the two-call form.
    ///
    /// A caller that settles a run and then appends its terminal event finds the append
    /// **refused**, because a settled run emits nothing more. The transition and the guard are
    /// each right; only their combination is impossible. This test writes out the two-call form
    /// so the reason `settle_run` exists is falsifiable rather than a claim in a comment.
    #[tokio::test]
    async fn two_calls_cannot_settle_a_run_and_record_its_event() {
        let (_directory, database) = seeded_database().await;
        let run = at_responding(&database).await;

        // Call one: transition to the terminal state.
        let settled = must(
            transition_run(
                &database,
                run.id(),
                run.expectation(),
                &transition(RunState::Completed),
                at(2),
            )
            .await,
        );
        assert!(settled.state().is_terminal());

        // Call two: append the settlement event. This is what a caller would naturally do next,
        // and it cannot succeed.
        let event = settlement_event(
            "0198f000-0000-7000-8000-0000000000e1",
            run.id(),
            jarvis_core::RunEventKind::RunCompleted,
            r#"{"outcome":"succeeded"}"#,
            2,
        );
        assert!(
            matches!(
                crate::append_run_event(&database, &event).await,
                Err(DatabaseError::RunEventAfterSettlement)
            ),
            "a settled run must refuse a later event, which is why the event is written with the transition"
        );
        database.close().await;
    }

    /// A settlement event whose kind disagrees with the target state is refused, so a run cannot
    /// settle as `completed` while its stream records that it was cancelled.
    #[test]
    fn a_settlement_event_that_disagrees_with_the_target_state_is_refused() {
        let expected = ExpectedRunState::new(RunState::Responding, 5);
        let mismatched = settlement_event(
            "0198f000-0000-7000-8000-0000000000e6",
            RUN_A,
            jarvis_core::RunEventKind::RunCancelled,
            r#"{"outcome":"cancelled"}"#,
            2,
        );
        assert!(
            TerminalTransition::new(expected, transition(RunState::Completed), &mismatched)
                .is_err(),
            "a completed settlement must not record a cancellation event"
        );
    }

    /// The primitive writes both rows, and the event describes the state the row now holds.
    #[tokio::test]
    async fn settling_writes_the_terminal_state_and_its_event_together() {
        let (_directory, database) = seeded_database().await;
        let run = at_responding(&database).await;

        let settlement = must(TerminalTransition::new(
            run.expectation(),
            transition(RunState::Completed),
            &settlement_event(
                "0198f000-0000-7000-8000-0000000000e2",
                run.id(),
                jarvis_core::RunEventKind::RunCompleted,
                r#"{"outcome":"succeeded"}"#,
                2,
            ),
        ));
        let settled = must(settle_run(&database, &settlement).await);

        assert_eq!(settled.state(), RunState::Completed);
        assert_eq!(settled.terminal_outcome(), Some(RunOutcome::Succeeded));

        // The event exists, is last, and its kind matches the settled state. A kind derived
        // from anywhere but the target state is how a run settles with an event describing a
        // different state.
        let events = must(
            crate::read_run_events(
                &database,
                run.id(),
                must(jarvis_core::ReplayRequest::new(
                    jarvis_core::RunEventSequence::first(),
                    100,
                )),
            )
            .await,
        );
        let last = events
            .last()
            .unwrap_or_else(|| panic!("the settlement event must exist"));
        assert_eq!(last.kind(), jarvis_core::RunEventKind::RunCompleted);
        assert!(last.kind().is_terminal());
        database.close().await;
    }

    /// A settlement that loses a race must leave NO event behind, because the event is written
    /// inside the transaction that the losing writer never commits.
    ///
    /// This is the property that makes writing both rows in one transaction necessary rather
    /// than merely tidy: a committed event for a transition that did not happen would be a
    /// stream describing a settlement the run does not have.
    #[tokio::test]
    async fn a_lost_settlement_race_leaves_no_event_behind() {
        let (_directory, database) = seeded_database().await;
        let run = at_responding(&database).await;
        let stale = ExpectedRunState::new(run.state(), run.version());

        // Another writer settles first.
        let settlement = must(TerminalTransition::new(
            stale,
            transition(RunState::Completed),
            &settlement_event(
                "0198f000-0000-7000-8000-0000000000e3",
                run.id(),
                jarvis_core::RunEventKind::RunCompleted,
                r#"{"outcome":"succeeded"}"#,
                2,
            ),
        ));
        must(settle_run(&database, &settlement).await);

        // A second writer holds the same expectation, so only its version guard separates them.
        // Its edge is legal from the state it believes is stored, so the domain refuses nothing
        // and the SQL guard is what decides.
        let loser = must(TerminalTransition::new(
            stale,
            transition(RunState::Cancelled),
            &settlement_event(
                "0198f000-0000-7000-8000-0000000000e4",
                run.id(),
                jarvis_core::RunEventKind::RunCancelled,
                r#"{"outcome":"cancelled"}"#,
                3,
            ),
        ));
        assert!(matches!(
            settle_run(&database, &loser).await,
            Err(DatabaseError::RunConflict)
        ));

        let events = must(
            crate::read_run_events(
                &database,
                run.id(),
                must(jarvis_core::ReplayRequest::new(
                    jarvis_core::RunEventSequence::first(),
                    100,
                )),
            )
            .await,
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event.kind().is_terminal())
                .count(),
            1,
            "a settlement that lost its race must not have written an event"
        );
        database.close().await;
    }

    /// `settle_run` refuses a non-terminal target, so the settlement path cannot be used to
    /// write an ordinary transition and leave the run open beside an event that says otherwise.
    #[test]
    fn a_non_terminal_settlement_is_refused() {
        let expected = ExpectedRunState::new(RunState::Planning, 3);
        assert!(
            TerminalTransition::new(
                expected,
                transition(RunState::Executing),
                &settlement_event(
                    "0198f000-0000-7000-8000-0000000000e5",
                    RUN_A,
                    jarvis_core::RunEventKind::StateChanged,
                    r#"{"state":"executing"}"#,
                    2,
                ),
            )
            .is_err(),
            "a non-terminal target must not be accepted by the settlement path"
        );
    }

    /// A run left in flight by a dead process is settled truthfully, not left looking active.
    ///
    /// This is the boundary `FR-RUN-003` calls "crash recovery": the state a client sees after a
    /// restart must be one a process is actually in, and a run whose executor is gone is not active.
    #[tokio::test]
    async fn an_interrupted_run_is_settled_as_failed() {
        let (_directory, database) = seeded_database().await;
        let run = at_responding(&database).await;

        // Nothing is running: the run row is exactly what a killed process would leave behind.
        let recovered = must(recover_interrupted_runs(&database, at(3)).await);
        assert_eq!(recovered, vec![run.id().to_owned()]);

        let settled = must(find_run(&database, run.id()).await);
        assert_eq!(settled.state(), RunState::Failed);
        assert_eq!(settled.terminal_outcome(), Some(RunOutcome::Failed));
        assert_eq!(
            settled.error_code(),
            Some(INTERRUPTED_ERROR_CODE),
            "the stored code must say why, not just that it failed"
        );

        let kinds = must(
            crate::read_run_events(
                &database,
                run.id(),
                must(jarvis_core::ReplayRequest::new(
                    jarvis_core::RunEventSequence::first(),
                    100,
                )),
            )
            .await,
        );
        assert_eq!(
            kinds.last().map(crate::StoredRunEvent::kind),
            Some(jarvis_core::RunEventKind::RunFailed),
            "recovery must record the settlement in the stream a client replays"
        );
        database.close().await;
    }

    /// A run whose cancellation was requested before the restart settles **cancelled**, because the
    /// operator's intent outranks the interruption. Settling it `failed` would report a daemon fault
    /// for work that was already meant to stop.
    #[tokio::test]
    async fn an_interrupted_run_that_was_already_cancelled_settles_as_cancelled() {
        let (_directory, database) = seeded_database().await;
        let run = at_responding(&database).await;
        must(request_run_cancellation(&database, run.id(), at(2)).await);

        let recovered = must(recover_interrupted_runs(&database, at(3)).await);
        assert_eq!(recovered, vec![run.id().to_owned()]);

        let settled = must(find_run(&database, run.id()).await);
        assert_eq!(settled.state(), RunState::Cancelled);
        assert_eq!(
            settled.error_code(),
            None,
            "a cancellation is not a failure"
        );
        database.close().await;
    }

    /// Recovery is safe to run again. A daemon that crashed during recovery must not corrupt a run it
    /// already settled, and a second startup must not re-settle anything.
    #[tokio::test]
    async fn recovery_is_idempotent() {
        let (_directory, database) = seeded_database().await;
        let run = at_responding(&database).await;
        must(recover_interrupted_runs(&database, at(3)).await);

        let before = must(
            crate::read_run_events(
                &database,
                run.id(),
                must(jarvis_core::ReplayRequest::new(
                    jarvis_core::RunEventSequence::first(),
                    100,
                )),
            )
            .await,
        )
        .len();

        let second = must(recover_interrupted_runs(&database, at(4)).await);
        assert!(second.is_empty(), "nothing is left in flight to recover");

        let after = must(
            crate::read_run_events(
                &database,
                run.id(),
                must(jarvis_core::ReplayRequest::new(
                    jarvis_core::RunEventSequence::first(),
                    100,
                )),
            )
            .await,
        )
        .len();
        assert_eq!(
            after, before,
            "a settled run must not gain an event from a second recovery pass"
        );
        database.close().await;
    }

    /// The profile with nothing in flight recovers nothing, which is the common case on a clean start.
    #[tokio::test]
    async fn recovery_on_a_clean_profile_reports_nothing() {
        let (_directory, database) = seeded_database().await;
        let recovered = must(recover_interrupted_runs(&database, at(3)).await);
        assert!(recovered.is_empty());
        database.close().await;
    }

    /// Recovery holds at **every** non-terminal boundary, not just the one a single test happens to
    /// pick. A process can die after any committed transition, so a gap that only appears in one
    /// state would be invisible to a test written against another.
    #[tokio::test]
    async fn every_non_terminal_boundary_recovers_truthfully() {
        let (_directory, database) = seeded_database().await;

        // The documented path, in order. Each entry is a boundary a process can die at.
        let boundaries = [
            RunState::Received,
            RunState::ContextBuilding,
            RunState::Planning,
            RunState::Executing,
            RunState::Observing,
            RunState::Responding,
        ];

        for boundary in boundaries {
            let id = jarvis_core::RunId::new().to_string();
            let mut run = one_run(&database, &id).await;
            // Walked rather than written directly, so the run reaches the boundary through the
            // same guarded transitions a live executor uses.
            for next in boundaries.into_iter().skip(1) {
                if run.state() == boundary {
                    break;
                }
                run = advance(&database, run, next).await;
            }
            assert_eq!(run.state(), boundary, "the run must reach its boundary");

            let recovered = must(recover_interrupted_runs(&database, at(3)).await);
            assert_eq!(
                recovered,
                vec![id.clone()],
                "a run left at {boundary} must be recovered"
            );

            let settled = must(find_run(&database, &id).await);
            assert_eq!(settled.state(), RunState::Failed, "at {boundary}");
            assert_eq!(
                settled.error_code(),
                Some(INTERRUPTED_ERROR_CODE),
                "at {boundary}"
            );
        }

        database.close().await;
    }
}
