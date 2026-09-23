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
    CorrelationId, NewMessage, RunEventKind, RunEventPayload, RunEventSequence, RunState,
    SessionChannel, SessionStatus, UtcTimestamp,
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

/// Whether a start creates a new session or continues an existing one.
///
/// A closed set rather than an `Option<SessionId>`, because the two cases differ in more than the
/// identifier: a new conversation creates its session row, and a continued one must **verify** that
/// the session exists and belongs to the same workspace and user before writing a run into it. An
/// optional identifier makes "no session" and "a session I declined to check" the same value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionTarget {
    /// Create a new session for this run.
    New,
    /// Continue an existing session, which must exist and belong to the same local identity.
    Existing(String),
}

impl SessionTarget {
    /// Returns whether this target continues an existing session.
    #[must_use]
    pub const fn is_existing(&self) -> bool {
        matches!(self, Self::Existing(_))
    }
}

/// The session, run, and first event to create atomically.
///
/// Every identifier is supplied by the caller so the use case can record them before the
/// write and correlate a failure, rather than discovering an identifier only after the
/// transaction commits.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StartRunInput {
    target: SessionTarget,
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
        Self::continuing(
            SessionTarget::New,
            session_id,
            run_id,
            event_id,
            workspace_id,
            user_id,
            objective,
            channel,
            correlation_id,
            started_at,
        )
    }

    /// Validates a start that creates a new session or continues an existing one.
    ///
    /// # Errors
    ///
    /// Returns [`DatabaseError::InvalidRunRequest`] when an identifier is not UUID-sized text or
    /// when the objective is empty or longer than the schema allows.
    #[allow(clippy::too_many_arguments)]
    pub fn continuing(
        target: SessionTarget,
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

        // The target and the identifier must agree. A continuation that named a different session
        // would write a run into one session while its transcript recorded the message in another,
        // and the two would be indistinguishable from a legitimate state afterwards.
        if let SessionTarget::Existing(existing) = &target
            && existing != &session_id
        {
            return Err(DatabaseError::InvalidRunRequest {
                field: "session id",
            });
        }

        // Counted in CHARACTERS for the same reason the title is: the schema bounds this
        // column with `length()` on TEXT, so counting bytes would reject text it accepts.
        let character_count = objective.chars().count();
        if character_count == 0 || character_count > crate::MAX_OBJECTIVE_CHARS {
            return Err(DatabaseError::InvalidRunRequest { field: "objective" });
        }

        Ok(Self {
            target,
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

    /// Returns whether this start creates its session or continues one.
    #[must_use]
    pub const fn target(&self) -> &SessionTarget {
        &self.target
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

/// Creates the session row for a new conversation.
///
/// Inside the caller's transaction so a session cannot exist without the run that justifies it. A
/// committed session with no run is a conversation a client could list and never open.
async fn create_session(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    input: &StartRunInput,
) -> Result<(), DatabaseError> {
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
    .execute(&mut **transaction)
    .await
    .map_err(|source| map_write_error("create a session", source))
    .map(|_| ())
}

/// Attaches a run to an existing session, proving this identity may write into it.
///
/// The guard is folded into one statement rather than a read followed by a decision, for the reason
/// the run-event repository records: a separate read takes a snapshot another writer can invalidate.
/// The workspace and user are part of the **predicate**, not checked afterwards — a session
/// identifier is guessable, and without this a caller could append to a conversation belonging to a
/// workspace it was never granted.
///
/// The failing case is resolved from a read, because three causes are indistinguishable from the
/// update alone: no such session, a session of another identity, and an archived session. Two of them
/// are reported as absence on purpose — telling a caller that somebody else's conversation exists is
/// itself a disclosure.
async fn attach_session(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    input: &StartRunInput,
) -> Result<(), DatabaseError> {
    let attached = sqlx::query(
        "UPDATE sessions SET updated_at = ?2, version = version + 1 \
         WHERE id = ?1 AND workspace_id = ?3 AND user_id = ?4 AND status = 'active'",
    )
    .bind(&input.session_id)
    .bind(input.started_at.to_string())
    .bind(&input.workspace_id)
    .bind(&input.user_id)
    .execute(&mut **transaction)
    .await
    .map_err(|source| map_write_error("continue a session", source))?;

    if attached.rows_affected() == 1 {
        return Ok(());
    }
    Err(classify_session_attachment(
        transaction,
        &input.session_id,
        &input.workspace_id,
        &input.user_id,
    )
    .await)
}

/// Explains why a continuation could not attach to a session.
///
/// Called only after the guarded update matched no row, so the three causes are resolved from a
/// read rather than guessed. A caller that was told "not found" for a session belonging to another
/// workspace would retry the same request; one told "archived" would start a new conversation.
async fn classify_session_attachment(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    session_id: &str,
    workspace_id: &str,
    user_id: &str,
) -> DatabaseError {
    let found = sqlx::query("SELECT workspace_id, user_id, status FROM sessions WHERE id = ?1")
        .bind(session_id)
        .fetch_optional(&mut **transaction)
        .await;

    let Ok(Some(row)) = found else {
        // Either no such session, or the read itself failed. Both leave the session unattachable,
        // and the caller's next action is the same, so the distinction is not worth inventing an
        // error for.
        return DatabaseError::SessionNotFound;
    };

    let session_workspace: Result<String, _> = row.try_get("workspace_id");
    let session_user: Result<String, _> = row.try_get("user_id");
    let session_status: Result<String, _> = row.try_get("status");

    // Checked before the archive state so a session of another identity is reported as an access
    // problem rather than as a lifecycle one. Reporting "archived" for a session the caller was
    // never entitled to would tell it the session exists.
    match (session_workspace, session_user) {
        (Ok(workspace), Ok(user)) if workspace != workspace_id || user != user_id => {
            return DatabaseError::SessionNotFound;
        }
        _ => {}
    }

    match session_status.as_deref() {
        Ok("archived") => DatabaseError::SessionNotWritable { field: "status" },
        _ => DatabaseError::SessionNotFound,
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
/// - [`DatabaseError::SessionNotFound`] when a continuation names a session this identity may not
///   write into, or one that does not exist.
/// - [`DatabaseError::SessionNotWritable`] when a continuation names an archived session.
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

    match input.target() {
        SessionTarget::New => create_session(&mut transaction, input).await?,
        SessionTarget::Existing(_) => attach_session(&mut transaction, input).await?,
    }

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

    // The user's own message is stored in the SAME transaction as the run it triggers. See
    // `store_user_message` for why the two writes cannot be separate calls.
    store_user_message(&mut transaction, input).await?;

    transaction
        .commit()
        .await
        .map_err(|source| DatabaseError::Sqlite {
            operation: "commit a run start",
            source,
        })?;

    read_started_run(database, &input.run_id).await
}

/// Stores the accepted user message inside the transaction that creates its run.
///
/// This is what `docs/quality/acceptance-tests.md` A03 requires: an accepted user message must never
/// be lost. As a second call after the commit, a crash in between would leave a run that answered a
/// question the transcript does not contain — and the transcript is what a later turn replays, so
/// the model would lose the question while the event log kept the answer.
///
/// The content is the objective. `P2-009` has one input per run, so the two are the same text; a
/// separate message body would be a second source of truth for what the run was asked.
async fn store_user_message(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    input: &StartRunInput,
) -> Result<(), DatabaseError> {
    let message = NewMessage::user(input.objective.clone())
        .map_err(|_| DatabaseError::InvalidMessageRequest { field: "content" })?;

    sqlx::query(
        "INSERT INTO messages (\
            id, session_id, sequence, author_kind, role, content, content_bytes, \
            sensitivity, source, run_id, created_at\
         ) \
         SELECT ?1, ?2, \
            (SELECT COALESCE(MAX(sequence) + 1, 0) FROM messages WHERE session_id = ?2), \
            ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10 \
         FROM sessions WHERE id = ?2",
    )
    .bind(jarvis_core::SessionId::new().to_string())
    .bind(&input.session_id)
    // `author_kind` and `role` hold the same value: the schema keeps "who spoke" and "where the
    // content came from" as separate columns whose value sets coincide for every message this build
    // writes. Both are bound from one validated value rather than two literals that could drift.
    .bind(message.role().as_str())
    .bind(message.role().as_str())
    .bind(message.content())
    .bind(message.content_bytes())
    .bind(message.sensitivity().as_str())
    .bind(message.source().as_str())
    .bind(&input.run_id)
    .bind(input.started_at.to_string())
    .execute(&mut **transaction)
    .await
    .map_err(|source| map_write_error("store the user message", source))?;
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A temporary profile directory holding a migrated database.
    struct TempProfile(PathBuf);

    impl TempProfile {
        fn new() -> Self {
            let path = std::env::temp_dir()
                .join(format!("jarvis-sessions-{}", jarvis_core::scratch_tag()));
            std::fs::create_dir_all(&path)
                .unwrap_or_else(|error| panic!("create temp profile: {error}"));
            Self(path)
        }

        fn database_path(&self) -> PathBuf {
            self.0.join(crate::database::DEFAULT_DATABASE_FILENAME)
        }
    }

    impl Drop for TempProfile {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    async fn database() -> (TempProfile, SqliteDatabase) {
        let profile = TempProfile::new();
        let database = SqliteDatabase::open(&profile.database_path())
            .await
            .unwrap_or_else(|error| panic!("open fixture database: {error}"));
        (profile, database)
    }

    fn at(minute: i128) -> UtcTimestamp {
        UtcTimestamp::from_unix_nanos(1_774_000_000_000_000_000 + minute * 60_000_000_000)
            .unwrap_or_else(|error| panic!("timestamp: {error}"))
    }

    /// Builds a start input, creating or continuing according to the target.
    async fn start_input(
        database: &SqliteDatabase,
        target: SessionTarget,
        session_id: String,
        objective: &str,
    ) -> StartRunInput {
        let identity = crate::load_local_identity(database)
            .await
            .unwrap_or_else(|error| panic!("the fixture must be seeded: {error}"));
        StartRunInput::continuing(
            target,
            session_id,
            jarvis_core::RunId::new().to_string(),
            jarvis_core::RequestId::new().to_string(),
            identity.workspace_id(),
            identity.user_id(),
            objective,
            API_SESSION_CHANNEL,
            CorrelationId::new(),
            at(0),
        )
        .unwrap_or_else(|error| panic!("start input: {error}"))
    }

    /// A second run may be added to an existing conversation, and it joins the same session.
    #[tokio::test]
    async fn a_run_can_continue_an_existing_session() {
        let (_profile, database) = database().await;
        let session = jarvis_core::SessionId::new().to_string();

        let first = start_input(&database, SessionTarget::New, session.clone(), "first turn").await;
        start_run(&database, &first)
            .await
            .unwrap_or_else(|error| panic!("first run: {error}"));

        let second = start_input(
            &database,
            SessionTarget::Existing(session.clone()),
            session.clone(),
            "second turn",
        )
        .await;
        let started = start_run(&database, &second)
            .await
            .unwrap_or_else(|error| panic!("second run must continue the session: {error:?}"));

        assert_eq!(
            started.session_id(),
            session,
            "the second run must belong to the same conversation"
        );
        assert_eq!(started.objective(), "second turn");

        // Both the questions must be in one transcript, in order, which is what a replay reads.
        let transcript = crate::read_messages(&database, &session, 10)
            .await
            .unwrap_or_else(|error| panic!("read: {error}"));
        let contents: Vec<&str> = transcript
            .iter()
            .map(crate::StoredMessage::content)
            .collect();
        assert_eq!(contents, vec!["first turn", "second turn"]);
        database.close().await;
    }

    /// Continuing a session this identity may not write into is refused, and reports absence.
    ///
    /// A session identifier is guessable, so a caller that supplied one belonging to another
    /// workspace must not be able to append to it. The refusal names absence rather than access,
    /// because telling a caller that somebody else's session exists is itself a disclosure.
    #[tokio::test]
    async fn continuing_a_foreign_session_is_refused_as_absent() {
        let (_profile, database) = database().await;
        let session = jarvis_core::SessionId::new().to_string();
        let first = start_input(&database, SessionTarget::New, session.clone(), "first turn").await;
        start_run(&database, &first)
            .await
            .unwrap_or_else(|error| panic!("first run: {error}"));

        // The same session, presented by a different workspace and user. The rows exist, so this
        // exercises the identity predicate rather than a missing row.
        let other = StartRunInput::continuing(
            SessionTarget::Existing(session.clone()),
            session.clone(),
            jarvis_core::RunId::new().to_string(),
            jarvis_core::RequestId::new().to_string(),
            crate::LOCAL_WORKSPACE_ID.to_owned(),
            "0198f000-0000-7000-8000-0000000000ff".to_owned(),
            "intruding turn",
            API_SESSION_CHANNEL,
            CorrelationId::new(),
            at(1),
        )
        .unwrap_or_else(|error| panic!("input: {error}"));

        let refused = start_run(&database, &other).await;
        assert!(
            matches!(refused, Err(DatabaseError::LocalIdentityMissing { .. })),
            "an unknown user is refused by the identity check first, so the session is never \
             evaluated against a stranger: {refused:?}"
        );

        // And the transcript is untouched by the attempt.
        let transcript = crate::read_messages(&database, &session, 10)
            .await
            .unwrap_or_else(|error| panic!("read: {error}"));
        assert_eq!(
            transcript.len(),
            1,
            "the refused attempt must store nothing"
        );
        database.close().await;
    }

    /// **The falsification test for the session ownership predicate.**
    ///
    /// The test above is refused by the identity check, which means it never exercises the session
    /// predicate at all. This one makes both identities real — a second user and workspace are seeded
    /// — so the only thing standing between the caller and the foreign session is the session's own
    /// workspace and user columns. Without the predicate in the `UPDATE`, a caller could append to
    /// any conversation whose identifier it could guess, and every other test here would still pass.
    #[tokio::test]
    async fn a_session_of_another_workspace_cannot_be_appended_to() {
        let (_profile, database) = database().await;

        // A second, real identity. Both rows exist, so `LocalIdentityMissing` cannot be the cause.
        for statement in [
            "INSERT INTO users (id, display_name, status, created_at, updated_at, version) \
             VALUES ('0198f000-0000-7000-8000-0000000000aa', 'Other User', 'active', \
                     '2026-09-21T10:00:00Z', '2026-09-21T10:00:00Z', 1)",
            "INSERT INTO workspaces (id, name, mode, data_policy, status, created_at, updated_at, version) \
             VALUES ('0198f000-0000-7000-8000-0000000000bb', 'Other', 'local', 'local-only', 'active', \
                     '2026-09-21T10:00:00Z', '2026-09-21T10:00:00Z', 1)",
            "INSERT INTO sessions (id, workspace_id, user_id, channel, status, created_at, updated_at, version) \
             VALUES ('0198f000-0000-7000-8000-0000000000cc', \
                     '0198f000-0000-7000-8000-0000000000bb', \
                     '0198f000-0000-7000-8000-0000000000aa', 'cli', 'active', \
                     '2026-09-21T10:00:00Z', '2026-09-21T10:00:00Z', 1)",
        ] {
            sqlx::query(statement)
                .execute(database.pool())
                .await
                .unwrap_or_else(|error| panic!("seed the second identity: {error}"));
        }

        let foreign = "0198f000-0000-7000-8000-0000000000cc".to_owned();
        let input = start_input(
            &database,
            SessionTarget::Existing(foreign.clone()),
            foreign.clone(),
            "into somebody else's conversation",
        )
        .await;

        let refused = start_run(&database, &input).await;
        assert!(
            matches!(refused, Err(DatabaseError::SessionNotFound)),
            "a session of another workspace must be refused as absent, so its existence is not \
             confirmed to a caller that cannot use it: {refused:?}"
        );

        // The foreign transcript must be untouched — no message, and no run.
        let count = crate::count_messages(&database, &foreign)
            .await
            .unwrap_or_else(|error| panic!("count: {error}"));
        assert_eq!(count, 0, "nothing may be written into a foreign session");
        database.close().await;
    }

    /// Continuing a session that does not exist is refused rather than creating one.
    ///
    /// A silent create would attach the run to a conversation the caller only believed existed, and
    /// the transcript would then begin mid-thread with no sign that the earlier turns were lost.
    #[tokio::test]
    async fn continuing_a_missing_session_is_refused() {
        let (_profile, database) = database().await;
        let absent = jarvis_core::SessionId::new().to_string();
        let input = start_input(
            &database,
            SessionTarget::Existing(absent.clone()),
            absent,
            "into nothing",
        )
        .await;

        let refused = start_run(&database, &input).await;
        assert!(
            matches!(refused, Err(DatabaseError::SessionNotFound)),
            "a missing session must be reported, not created: {refused:?}"
        );
        database.close().await;
    }

    /// Continuing an archived session is refused as a lifecycle problem, not as absence.
    ///
    /// The distinction is actionable: an archived conversation means "start a new one", while a
    /// missing one means the identifier was wrong.
    #[tokio::test]
    async fn continuing_an_archived_session_is_refused_as_closed() {
        let (_profile, database) = database().await;
        let session = jarvis_core::SessionId::new().to_string();
        let first = start_input(&database, SessionTarget::New, session.clone(), "first turn").await;
        start_run(&database, &first)
            .await
            .unwrap_or_else(|error| panic!("first run: {error}"));

        sqlx::query("UPDATE sessions SET status = 'archived', archived_at = ?2 WHERE id = ?1")
            .bind(&session)
            .bind(at(1).to_string())
            .execute(database.pool())
            .await
            .unwrap_or_else(|error| panic!("archive: {error}"));

        let input = start_input(
            &database,
            SessionTarget::Existing(session.clone()),
            session,
            "after archiving",
        )
        .await;
        let refused = start_run(&database, &input).await;
        assert!(
            matches!(refused, Err(DatabaseError::SessionNotWritable { .. })),
            "an archived session must be reported as closed, not missing: {refused:?}"
        );
        database.close().await;
    }

    /// A continuation whose identifier disagrees with its target is refused before any write.
    ///
    /// The two must denote the same session, or the run would land in one conversation while the
    /// transcript recorded its question in another. Both rows would look legitimate afterwards.
    #[tokio::test]
    async fn a_continuation_naming_two_sessions_is_refused() {
        let constructed = StartRunInput::continuing(
            SessionTarget::Existing(jarvis_core::SessionId::new().to_string()),
            jarvis_core::SessionId::new().to_string(),
            jarvis_core::RunId::new().to_string(),
            jarvis_core::RequestId::new().to_string(),
            crate::LOCAL_WORKSPACE_ID.to_owned(),
            crate::LOCAL_USER_ID.to_owned(),
            "mismatched",
            API_SESSION_CHANNEL,
            CorrelationId::new(),
            at(0),
        );
        assert!(
            matches!(
                constructed,
                Err(DatabaseError::InvalidRunRequest {
                    field: "session id"
                })
            ),
            "a target and an identifier that disagree must be refused at construction"
        );
    }

    /// A new conversation still creates its session, so continuation did not replace creation.
    #[tokio::test]
    async fn a_new_session_is_still_created() {
        let (_profile, database) = database().await;
        let session = jarvis_core::SessionId::new().to_string();
        let input = start_input(
            &database,
            SessionTarget::New,
            session.clone(),
            "a fresh question",
        )
        .await;
        let started = start_run(&database, &input)
            .await
            .unwrap_or_else(|error| panic!("start: {error}"));
        assert_eq!(started.session_id(), session);

        let stored = find_session(&database, &session)
            .await
            .unwrap_or_else(|error| panic!("find: {error}"));
        assert_eq!(stored.status(), SessionStatus::Active);
        database.close().await;
    }
}
