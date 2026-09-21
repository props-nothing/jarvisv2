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
    CorrelationId, ExpectedRunState, RunErrorCode, RunOutcome, RunState, RunTransition,
    UtcTimestamp,
};
use sqlx::Row;

use crate::database::{DatabaseError, SqliteDatabase, require_run_transition};

/// Maximum objective length, matching the migration's `CHECK`.
pub const MAX_OBJECTIVE_CHARS: usize = 4096;

/// One agent run as stored, limited to what the state machine reads and writes.
///
/// A full `agent_runs` read model belongs with the run queries (`P2-007`). This type exists
/// so a transition can return the state it produced without the caller re-reading the row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredRun {
    id: String,
    session_id: String,
    workspace_id: String,
    state: RunState,
    version: i64,
    cancellation_requested_at: Option<UtcTimestamp>,
    terminal_outcome: Option<RunOutcome>,
    completed_at: Option<UtcTimestamp>,
    error_code: Option<String>,
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

/// Reads one run by identifier.
///
/// # Errors
///
/// Returns [`DatabaseError::RunNotFound`] when no row has that identifier, or
/// [`DatabaseError::Sqlite`] when the read fails.
pub async fn find_run(database: &SqliteDatabase, id: &str) -> Result<StoredRun, DatabaseError> {
    let row = sqlx::query(
        "SELECT id, session_id, workspace_id, state, version, \
                cancellation_requested_at, terminal_outcome, completed_at, error_code \
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

/// Records that a client requested cancellation without settling the run yet.
///
/// This is deliberately not a transition. Cancellation is a *request*; the run settles as
/// `cancelled` only once the in-flight work has actually stopped, which is what
/// `docs/quality/acceptance-tests.md` A04 requires ("new work stops, state settles once").
/// Marking the run terminal here would report a stopped run before anything stopped.
///
/// # Errors
///
/// Returns [`DatabaseError::RunTransitionRefused`] for an already-settled run,
/// [`DatabaseError::RunConflict`] when the expected version is stale, and
/// [`DatabaseError::Sqlite`] for any other persistence failure.
pub async fn request_run_cancellation(
    database: &SqliteDatabase,
    id: &str,
    expected: ExpectedRunState,
    at: UtcTimestamp,
) -> Result<StoredRun, DatabaseError> {
    if expected.state().is_terminal() {
        return Err(DatabaseError::RunTransitionRefused {
            source: jarvis_core::RunTransitionError::TerminalStateImmutable {
                from: expected.state(),
            },
        });
    }

    // The `state NOT IN (...)` clause is not redundant with the expected-state comparison:
    // it keeps the guard correct even when a caller passes a non-terminal expected state
    // for a row that has already settled, which the version alone would not catch.
    let result = sqlx::query(
        "UPDATE agent_runs \
         SET cancellation_requested_at = COALESCE(cancellation_requested_at, ?3), \
             updated_at = ?4, \
             version = version + 1 \
         WHERE id = ?1 AND state = ?2 AND version = ?5 \
           AND state NOT IN ('completed', 'cancelled', 'failed')",
    )
    .bind(id)
    .bind(expected.state().as_str())
    .bind(at.to_string())
    .bind(at.to_string())
    .bind(expected.version())
    .execute(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "request agent run cancellation",
        source,
    })?;

    require_run_transition(result.rows_affected())?;
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
        let run = one_run(&database, RUN_A).await;

        // A request must not settle the run: acceptance test A04 requires the run to settle
        // once, after the in-flight work has actually stopped.
        let requested =
            must(request_run_cancellation(&database, RUN_A, run.expectation(), at(3)).await);
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
        let run = one_run(&database, RUN_A).await;

        let first =
            must(request_run_cancellation(&database, RUN_A, run.expectation(), at(3)).await);
        let second =
            must(request_run_cancellation(&database, RUN_A, first.expectation(), at(9)).await);

        assert_eq!(second.cancellation_requested_at(), Some(at(3)));
        assert_eq!(second.version(), 3);
        assert_eq!(second.state(), RunState::Received);
    }

    #[tokio::test]
    async fn cancelling_a_settled_run_is_refused() {
        let (_directory, database) = seeded_database().await;
        let run = one_run(&database, RUN_A).await;
        let failed = must(
            transition_run(
                &database,
                RUN_A,
                run.expectation(),
                &RunTransition::failed(code("cancelled_by_operator")),
                at(1),
            )
            .await,
        );

        let outcome = request_run_cancellation(&database, RUN_A, failed.expectation(), at(2)).await;
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
}
