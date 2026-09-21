//! Durable storage for conversation sessions and the transactional start of a run.
//!
//! `docs/architecture/identity-and-workspaces.md` separates a **session** (a conversation)
//! from a **run** (one piece of work inside it), and `agent_runs` carries a foreign key to
//! `sessions`, so a run cannot be stored before its conversation exists.
//!
//! # Why this module exists rather than the use case writing SQL
//!
//! `SqliteDatabase::pool` is crate-private, deliberately: SQL is not allowed to escape an
//! adapter and reach a use case. Every session statement therefore lives here and the
//! gateway calls a function.
//!
//! # Why starting a run is ONE transaction
//!
//! [`start_run`] writes the session, the run, and the run's first event inside a single
//! transaction. As three independent calls it could leave a run with no events, or a
//! session with no run, if the process stopped between them. Both outcomes are worse than a
//! failed start, and one of them is specifically the condition this record exists to make
//! impossible: a run whose stream is empty is indistinguishable from a run whose events
//! were lost.
//!
//! # Why the first sequence is computed rather than hard-coded
//!
//! The first event's sequence is `MAX(sequence) + 1` for the run, evaluated in the same
//! statement as its insert. A new run has no events, so the value is `1` â€” but writing the
//! constant would bake in an assumption that stops holding the moment another writer
//! appends in between. The subquery cannot make that assumption, which is the same reasoning
//! [`crate::run_event_repository`] records for its append.

use jarvis_core::{
    CorrelationId, RunEventKind, RunEventPayload, RunEventSequence, RunState, SessionChannel,
    SessionStatus, UtcTimestamp,
};
use sqlx::Row;

use crate::database::{DatabaseError, SqliteDatabase};

/// The exact identifier length the schema's `CHECK` requires.
const ID_LENGTH: usize = 36;

/// The summary recorded for a run's first event.
const RUN_ACCEPTED_SUMMARY: &str = "run accepted";

/// How an HTTP-originated session is recorded.
///
/// A named constant rather than a literal at the call site, so the value the schema's
/// `channel` constraint allows and the value this module writes cannot drift apart.
pub const API_SESSION_CHANNEL: SessionChannel = SessionChannel::Api;

/// One session to create.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewSession {
    id: String,
    workspace_id: String,
    user_id: String,
    channel: SessionChannel,
    title: Option<String>,
}

impl NewSession {
    /// Validates the fields required to create a session.
    ///
    /// # Errors
    ///
    /// Returns [`DatabaseError::InvalidSessionRequest`] when an identifier is not
    /// UUID-sized text or the title violates the schema's bounds. The title is checked
    /// here as well as by the schema because a rejected insert otherwise surfaces as a
    /// generic constraint failure naming no field.
    pub fn new(
        id: impl Into<String>,
        workspace_id: impl Into<String>,
        user_id: impl Into<String>,
        channel: SessionChannel,
        title: Option<impl Into<String>>,
    ) -> Result<Self, DatabaseError> {
        let id = id.into();
        let workspace_id = workspace_id.into();
        let user_id = user_id.into();
        let title = title.map(Into::into);

        for (field, value) in [
            ("id", &id),
            ("workspace id", &workspace_id),
            ("user id", &user_id),
        ] {
            if value.len() != ID_LENGTH {
                return Err(DatabaseError::InvalidSessionRequest { field });
            }
        }

        if let Some(title) = title.as_deref() {
            let trimmed = title.trim();
            if trimmed.is_empty() {
                return Err(DatabaseError::InvalidSessionRequest { field: "title" });
            }
            // Counted in CHARACTERS to match the migration's `length()` on a TEXT value, so
            // a multi-byte title the schema accepts is not rejected here.
            if trimmed.chars().count() > jarvis_core::MAX_SESSION_TITLE_CHARS {
                return Err(DatabaseError::InvalidSessionRequest { field: "title" });
            }
        }

        Ok(Self {
            id,
            workspace_id,
            user_id,
            channel,
            title,
        })
    }

    /// Returns the session identifier that will be created.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }
}

/// The session, run, and first event to create atomically.
///
/// Every identifier is supplied by the caller so the use case can record them before the
/// write and correlate a failure, rather than discovering an identifier only after the
/// transaction commits.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StartRunInput {
    session_id: String,
    run_id: String,
    event_id: String,
    workspace_id: String,
    user_id: String,
    objective: String,
    channel: SessionChannel,
    correlation_id: CorrelationId,
    started_at: UtcTimestamp,
}

impl StartRunInput {
    /// Validates the identifiers, the objective, and the session channel.
    ///
    /// # Errors
    ///
    /// Returns [`DatabaseError::InvalidRunRequest`] when an identifier is not UUID-sized
    /// text, or when the objective is empty or longer than the schema allows.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        session_id: impl Into<String>,
        run_id: impl Into<String>,
        event_id: impl Into<String>,
        workspace_id: impl Into<String>,
        user_id: impl Into<String>,
        objective: impl Into<String>,
        channel: SessionChannel,
        correlation_id: CorrelationId,
        started_at: UtcTimestamp,
    ) -> Result<Self, DatabaseError> {
        let session_id = session_id.into();
        let run_id = run_id.into();
        let event_id = event_id.into();
        let workspace_id = workspace_id.into();
        let user_id = user_id.into();
        let objective = objective.into();

        for (field, value) in [
            ("session id", &session_id),
            ("id", &run_id),
            ("event id", &event_id),
            ("workspace id", &workspace_id),
            ("user id", &user_id),
        ] {
            if value.len() != ID_LENGTH {
                return Err(DatabaseError::InvalidRunRequest { field });
            }
        }

        // Counted in CHARACTERS for the same reason the title is: the schema bounds this
        // column with `length()` on TEXT, so counting bytes would reject text it accepts.
        let character_count = objective.chars().count();
        if character_count == 0 || character_count > crate::MAX_OBJECTIVE_CHARS {
            return Err(DatabaseError::InvalidRunRequest { field: "objective" });
        }

        Ok(Self {
            session_id,
            run_id,
            event_id,
            workspace_id,
            user_id,
            objective,
            channel,
            correlation_id,
            started_at,
        })
    }

    /// Returns the run identifier that will be created.
    #[must_use]
    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    /// Returns the session identifier the run belongs to.
    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
}

/// One stored session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredSession {
    id: String,
    workspace_id: String,
    user_id: String,
    channel: SessionChannel,
    status: SessionStatus,
    version: i64,
}

impl StoredSession {
    /// Returns the session identifier.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the owning workspace identifier.
    #[must_use]
    pub fn workspace_id(&self) -> &str {
        &self.workspace_id
    }

    /// Returns the owning user identifier.
    #[must_use]
    pub fn user_id(&self) -> &str {
        &self.user_id
    }

    /// Returns how the conversation reached the daemon.
    #[must_use]
    pub const fn channel(&self) -> SessionChannel {
        self.channel
    }

    /// Returns the session lifecycle status.
    #[must_use]
    pub const fn status(&self) -> SessionStatus {
        self.status
    }

    /// Returns the row version.
    #[must_use]
    pub const fn version(&self) -> i64 {
        self.version
    }
}

/// The run produced by [`start_run`], together with its first event's position.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StartedRun {
    session_id: String,
    run_id: String,
    workspace_id: String,
    objective: String,
    state: RunState,
    version: i64,
    correlation_id: CorrelationId,
    started_at: UtcTimestamp,
    first_sequence: RunEventSequence,
}

impl StartedRun {
    /// Returns the session the run belongs to.
    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Returns the run identifier.
    #[must_use]
    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    /// Returns the owning workspace identifier.
    #[must_use]
    pub fn workspace_id(&self) -> &str {
        &self.workspace_id
    }

    /// Returns the accepted objective.
    #[must_use]
    pub fn objective(&self) -> &str {
        &self.objective
    }

    /// Returns the run's lifecycle state, which is `received` for a new run.
    #[must_use]
    pub const fn state(&self) -> RunState {
        self.state
    }

    /// Returns the row version.
    #[must_use]
    pub const fn version(&self) -> i64 {
        self.version
    }

    /// Returns the correlation identifier shared with the requesting client.
    #[must_use]
    pub const fn correlation_id(&self) -> CorrelationId {
        self.correlation_id
    }

    /// Returns when the run started.
    #[must_use]
    pub const fn started_at(&self) -> UtcTimestamp {
        self.started_at
    }

    /// Returns the sequence of the run's first event, which is always one.
    ///
    /// Returned rather than assumed so a caller reporting the stream position reads the
    /// value that was written instead of restating a constant.
    #[must_use]
    pub const fn first_sequence(&self) -> RunEventSequence {
        self.first_sequence
    }
}

/// Creates a session, the run it holds, and the run's first event in one transaction.
///
/// The run is created in `received` and its first event is `state_changed` describing that,
/// so a run's stream always explains how the run began.
///
/// # Errors
///
/// - [`DatabaseError::InvalidRunRequest`] when the input violates a schema bound.
/// - [`DatabaseError::LocalIdentityMissing`] when the workspace or user row is absent, so a
///   start against an unseeded profile names its cause rather than surfacing a foreign-key
///   failure that names no field.
/// - [`DatabaseError::Sqlite`] for any other persistence failure.
pub async fn start_run(
    database: &SqliteDatabase,
    input: &StartRunInput,
) -> Result<StartedRun, DatabaseError> {
    let mut transaction =
        database
            .pool()
            .begin()
            .await
            .map_err(|source| DatabaseError::Sqlite {
                operation: "begin a run start",
                source,
            })?;

    // The identity is checked first so an unseeded profile reports its specific cause. The
    // foreign keys would refuse the insert anyway, but as an opaque constraint failure that
    // names neither the missing table nor the direction of the problem.
    let identity_count = sqlx::query_scalar::<_, i64>(
        "SELECT (SELECT COUNT(*) FROM users WHERE id = ?1) \
              + (SELECT COUNT(*) FROM workspaces WHERE id = ?2)",
    )
    .bind(&input.user_id)
    .bind(&input.workspace_id)
    .fetch_one(&mut *transaction)
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "check the local identity",
        source,
    })?;
    if identity_count != 2 {
        return Err(DatabaseError::LocalIdentityMissing {
            field: "user or workspace",
        });
    }

    sqlx::query(
        "INSERT INTO sessions \
         (id, workspace_id, user_id, channel, status, created_at, updated_at, version) \
         VALUES (?1, ?2, ?3, ?4, 'active', ?5, ?5, 1)",
    )
    .bind(&input.session_id)
    .bind(&input.workspace_id)
    .bind(&input.user_id)
    .bind(input.channel.as_str())
    .bind(input.started_at.to_string())
    .execute(&mut *transaction)
    .await
    .map_err(|source| map_write_error("create a session", source))?;

    sqlx::query(
        "INSERT INTO agent_runs (\
            id, session_id, workspace_id, user_id, objective, state, \
            started_at, updated_at, correlation_id, version\
         ) VALUES (?1, ?2, ?3, ?4, ?5, 'received', ?6, ?6, ?7, 1)",
    )
    .bind(&input.run_id)
    .bind(&input.session_id)
    .bind(&input.workspace_id)
    .bind(&input.user_id)
    .bind(&input.objective)
    .bind(input.started_at.to_string())
    .bind(input.correlation_id.to_string())
    .execute(&mut *transaction)
    .await
    .map_err(|source| map_write_error("create a run", source))?;

    // Sequence allocation and the insert are one statement, for the reason the run-event
    // repository records: a separate read would take a snapshot another writer can invalidate.
    let payload = initial_state_payload()?;
    sqlx::query(
        "INSERT INTO run_events (\
            id, run_id, sequence, kind, summary, payload, \
            correlation_id, occurred_at, recorded_at\
         ) \
         SELECT ?1, ?2, \
            (SELECT COALESCE(MAX(sequence), 0) + 1 FROM run_events WHERE run_id = ?2), \
            ?3, ?4, ?5, ?6, ?7, ?7 \
         FROM agent_runs WHERE id = ?2",
    )
    .bind(&input.event_id)
    .bind(&input.run_id)
    .bind(RunEventKind::StateChanged.as_str())
    .bind(RUN_ACCEPTED_SUMMARY)
    .bind(payload.as_str())
    .bind(input.correlation_id.to_string())
    .bind(input.started_at.to_string())
    .execute(&mut *transaction)
    .await
    .map_err(|source| map_write_error("append the first run event", source))?;

    transaction
        .commit()
        .await
        .map_err(|source| DatabaseError::Sqlite {
            operation: "commit a run start",
            source,
        })?;

    read_started_run(database, &input.run_id).await
}

/// Reads one session by identifier.
///
/// # Errors
///
/// Returns [`DatabaseError::SessionNotFound`] when no session has that identifier, or
/// [`DatabaseError::StoredSessionInvalid`] when a stored row holds a value outside the closed
/// channel or status sets.
pub async fn find_session(
    database: &SqliteDatabase,
    id: &str,
) -> Result<StoredSession, DatabaseError> {
    let row = sqlx::query(
        "SELECT id, workspace_id, user_id, channel, status, version \
         FROM sessions WHERE id = ?1",
    )
    .bind(id)
    .fetch_optional(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "read a session",
        source,
    })?
    .ok_or(DatabaseError::SessionNotFound)?;

    let channel_text: String = row.get("channel");
    let channel = SessionChannel::parse(&channel_text)
        .map_err(|_| DatabaseError::StoredSessionInvalid { field: "channel" })?;
    let status_text: String = row.get("status");
    let status = SessionStatus::parse(&status_text)
        .map_err(|_| DatabaseError::StoredSessionInvalid { field: "status" })?;

    Ok(StoredSession {
        id: row.get("id"),
        workspace_id: row.get("workspace_id"),
        user_id: row.get("user_id"),
        channel,
        status,
        version: row.get("version"),
    })
}

/// Reads back the run written by [`start_run`].
async fn read_started_run(
    database: &SqliteDatabase,
    run_id: &str,
) -> Result<StartedRun, DatabaseError> {
    let row = sqlx::query(
        "SELECT r.id, r.session_id, r.workspace_id, r.objective, r.state, r.version, \
                r.correlation_id, r.started_at, \
                (SELECT MIN(sequence) FROM run_events WHERE run_id = r.id) AS first_sequence \
         FROM agent_runs AS r WHERE r.id = ?1",
    )
    .bind(run_id)
    .fetch_optional(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "read a started run",
        source,
    })?
    .ok_or(DatabaseError::RunNotFound)?;

    let state_text: String = row
        .try_get("state")
        .map_err(|source| DatabaseError::Sqlite {
            operation: "decode a stored run state",
            source,
        })?;
    let state = state_text
        .parse::<RunState>()
        .map_err(|_| DatabaseError::StoredRunInvalid { field: "state" })?;

    let correlation_text: String = row.get("correlation_id");
    let correlation_id =
        correlation_text
            .parse::<CorrelationId>()
            .map_err(|_| DatabaseError::StoredRunInvalid {
                field: "correlation_id",
            })?;

    let started_text: String = row.get("started_at");
    let started_at =
        started_text
            .parse::<UtcTimestamp>()
            .map_err(|_| DatabaseError::StoredRunInvalid {
                field: "started_at",
            })?;

    // A run with no event would mean the transaction half-applied, so its absence is a
    // storage-integrity finding rather than a `first()` default that hides it.
    let sequence: Option<i64> =
        row.try_get("first_sequence")
            .map_err(|source| DatabaseError::Sqlite {
                operation: "read a run's first event sequence",
                source,
            })?;
    let first_sequence = u32::try_from(sequence.ok_or(DatabaseError::StoredRunInvalid {
        field: "first event sequence",
    })?)
    .ok()
    .and_then(|value| RunEventSequence::new(value).ok())
    .ok_or(DatabaseError::StoredRunInvalid {
        field: "first event sequence",
    })?;

    Ok(StartedRun {
        session_id: row.get("session_id"),
        run_id: row.get("id"),
        workspace_id: row.get("workspace_id"),
        objective: row.get("objective"),
        state,
        version: row.get("version"),
        correlation_id,
        started_at,
        first_sequence,
    })
}

/// The payload for a run's first event.
///
/// Routed through [`RunEventPayload::new`] rather than bound as a literal so this write path
/// obeys the same validation as an ordinary append. Binding raw text here would make the
/// first event the one event that could slip a malformed payload past the domain check.
fn initial_state_payload() -> Result<RunEventPayload, DatabaseError> {
    RunEventPayload::new(format!(
        r#"{{"state":"{}","version":1}}"#,
        RunState::Received.as_str()
    ))
    .map_err(|_| DatabaseError::InvalidRunEventRequest { field: "payload" })
}

/// Maps an insert failure to a bounded, non-echoing reason.
fn map_write_error(operation: &'static str, source: sqlx::Error) -> DatabaseError {
    let text = source.to_string();
    // The foreign-key check above already reported a missing identity with its own cause, so
    // reaching here with one means the row disappeared mid-transaction. Reporting it as the
    // identity failure keeps the attribution honest instead of blaming the caller's input.
    if text.contains("FOREIGN KEY") {
        return DatabaseError::LocalIdentityMissing {
            field: "user or workspace",
        };
    }
    DatabaseError::Sqlite { operation, source }
}
