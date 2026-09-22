//! Run use cases for the HTTP gateway: start, read, and cancel.
//!
//! This is the transaction boundary between the transport and storage. It exists so route
//! handlers contain no policy: a handler parses, delegates, and maps one error onto a status,
//! while every decision about identity, state, and correlation lives here.
//!
//! # Identity is resolved, never accepted
//!
//! The workspace and user come from the profile's **seeded local identity**, not from the
//! request. A client-supplied workspace identifier would let a caller name a workspace it was
//! never granted, which `docs/architecture/identity-and-workspaces.md` forbids: access is
//! established by authentication, and a free-standing workspace identifier is not proof of it.
//!
//! # Cancellation requests rather than settles
//!
//! [`cancel_run`] records a cancellation request and returns the run still in its current
//! state. Settling here would report a stopped run before anything stopped, which
//! `docs/quality/acceptance-tests.md` A04 rules out: cancellation stops **new** work, and the
//! in-flight step decides when the run actually settles.

use std::sync::Arc;

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use jarvis_core::{
    CorrelationId, ErrorCode, ExpectedRunState, RunId, SafeMessage, SessionId, SystemClock,
    UtcTimestamp,
};
use jarvis_protocol::{RunReply, StartRunRequest, rest_error, safe};
use jarvis_storage::{
    API_SESSION_CHANNEL, DatabaseError, SessionTarget, SqliteDatabase, StartRunInput, find_run,
    load_local_identity, request_run_cancellation, start_run,
};

/// A run use-case failure that maps onto a status and the shared error envelope.
#[derive(Debug)]
pub struct RunServiceError {
    status: StatusCode,
    code: ErrorCode,
    message: SafeMessage,
}

impl RunServiceError {
    fn new(status: StatusCode, code: ErrorCode, message: &str) -> Self {
        Self {
            status,
            code,
            message: safe(message),
        }
    }
}

impl IntoResponse for RunServiceError {
    fn into_response(self) -> Response {
        (self.status, Json(rest_error(self.code, self.message))).into_response()
    }
}

/// Starts, reads, and cancels runs.
///
/// Holds no state of its own beyond the database handle: run policy is in the storage
/// transaction and the state machine, so caching anything here would be a second copy that
/// can disagree.
#[derive(Clone)]
pub struct RunService {
    database: Arc<SqliteDatabase>,
}

impl RunService {
    /// Creates the service over the daemon's database.
    #[must_use]
    pub fn new(database: Arc<SqliteDatabase>) -> Self {
        Self { database }
    }

    /// Starts a run.
    ///
    /// # Errors
    ///
    /// Returns [`RunServiceError`] when the objective is invalid, the profile identity is
    /// missing, a request carries an idempotency key this build cannot honour, or persistence
    /// fails.
    pub fn start(
        &self,
        request: &StartRunRequest,
    ) -> impl std::future::Future<Output = Result<RunReply, RunServiceError>> + '_ {
        let objective = request.objective.trim().to_owned();
        let idempotency_key = request.idempotency_key.clone();
        // Bound before the `async move` so the future owns its input rather than borrowing the
        // request. The returned future's lifetime is already tied to `&self`, and adding a borrow of
        // the request to it as well would make the two outlive each other incorrectly.
        let requested_session = request.session_id.clone();
        async move {
            if objective.is_empty() {
                return Err(RunServiceError::new(
                    StatusCode::BAD_REQUEST,
                    ErrorCode::Validation,
                    "the objective must not be empty",
                ));
            }

            // An idempotency key is refused rather than ignored. Accepting the field and starting
            // a second run would tell a retrying client that its request was deduplicated when two
            // runs now exist, which is the failure the contract exists to prevent. Refusing until a
            // deduplication ledger exists keeps the contract honest: `Unsupported` tells the caller
            // exactly which guarantee is absent.
            if idempotency_key.is_some() {
                return Err(RunServiceError::new(
                    StatusCode::NOT_IMPLEMENTED,
                    ErrorCode::Unsupported,
                    "idempotency_key is not supported by this build; omit it to start a run",
                ));
            }

            let identity = load_local_identity(&self.database)
                .await
                .map_err(|error| map_identity_error(&error))?;
            let now = UtcTimestamp::now(&SystemClock);
            let correlation_id = CorrelationId::new();

            // The requested conversation, or a new one. The target is built from the request rather
            // than decided later, so "continue this session" and "start a conversation" stay
            // distinguishable all the way to the transaction that enforces it.
            let (target, session_id) = match requested_session {
                Some(existing) => (SessionTarget::Existing(existing.clone()), existing),
                None => (SessionTarget::New, SessionId::new().to_string()),
            };

            // The identifiers are generated before the write so a failure can be correlated against
            // the identifiers the client will be told about, rather than discovered only after the
            // transaction commits.
            let input = StartRunInput::continuing(
                target,
                session_id,
                RunId::new().to_string(),
                jarvis_core::RequestId::new().to_string(),
                identity.workspace_id(),
                identity.user_id(),
                &objective,
                API_SESSION_CHANNEL,
                correlation_id,
                now,
            )
            .map_err(|error| map_database_error(&error))?;

            let started = start_run(&self.database, &input)
                .await
                .map_err(|error| map_database_error(&error))?;
            Ok(reply::from_started(&started))
        }
    }

    /// Reads one run.
    ///
    /// # Errors
    ///
    /// Returns [`RunServiceError`] when no run has that identifier.
    pub async fn read(&self, id: &str) -> Result<RunReply, RunServiceError> {
        let run = find_run(&self.database, id)
            .await
            .map_err(|error| map_database_error(&error))?;
        Ok(reply::from_stored(&run))
    }

    /// Requests cancellation of one run.
    ///
    /// # Errors
    ///
    /// Returns [`RunServiceError`] when the run is absent, the supplied version is stale, or
    /// the run already settled.
    pub async fn cancel(
        &self,
        id: &str,
        expected_version: i64,
    ) -> Result<RunReply, RunServiceError> {
        let current = find_run(&self.database, id)
            .await
            .map_err(|error| map_database_error(&error))?;
        let expected = ExpectedRunState::new(current.state(), expected_version);

        let run = request_run_cancellation(
            &self.database,
            id,
            expected,
            UtcTimestamp::now(&SystemClock),
        )
        .await
        .map_err(|error| map_database_error(&error))?;
        Ok(reply::from_stored(&run))
    }
}

/// Builds the REST reply from a stored run.
pub mod reply {
    use jarvis_storage::{StartedRun, StoredRun};

    /// Converts a stored run into its wire form.
    #[must_use]
    pub fn from_stored(run: &StoredRun) -> jarvis_protocol::RunReply {
        jarvis_protocol::RunReply {
            run_id: run.id().to_owned(),
            session_id: run.session_id().to_owned(),
            workspace_id: run.workspace_id().to_owned(),
            objective: run.objective().to_owned(),
            state: run.state(),
            version: run.version(),
            outcome: run.terminal_outcome(),
            error_code: run.error_code().map(ToOwned::to_owned),
            started_at: run.started_at(),
            completed_at: run.completed_at(),
            cancellation_requested_at: run.cancellation_requested_at(),
        }
    }

    /// Converts the run produced by a start into its wire form.
    ///
    /// Built from the start result rather than by re-reading the run, because the start
    /// result is what the transaction actually committed.
    #[must_use]
    pub fn from_started(run: &StartedRun) -> jarvis_protocol::RunReply {
        jarvis_protocol::RunReply {
            run_id: run.run_id().to_owned(),
            session_id: run.session_id().to_owned(),
            workspace_id: run.workspace_id().to_owned(),
            objective: run.objective().to_owned(),
            state: run.state(),
            version: run.version(),
            outcome: None,
            error_code: None,
            started_at: run.started_at(),
            completed_at: None,
            cancellation_requested_at: None,
        }
    }
}

/// Maps an absent local identity onto its own response.
///
/// Distinct from the general storage mapping because the cause is this profile's state, not
/// the caller's request, and a client told "validation failed" would retry the same way.
fn map_identity_error(error: &DatabaseError) -> RunServiceError {
    match error {
        DatabaseError::LocalIdentityMissing { .. } => RunServiceError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::Internal,
            "this profile is missing its local identity",
        ),
        other => map_database_error(other),
    }
}

/// Maps a storage failure onto a status and the shared error envelope.
///
/// Written per variant rather than as a catch-all, so a new error kind cannot silently be
/// reported as internal. A missing run is `404` rather than `400`: the identifier is
/// well-formed and simply absent, so blaming the caller's content would be wrong.
fn map_database_error(error: &DatabaseError) -> RunServiceError {
    match error {
        DatabaseError::RunNotFound => RunServiceError::new(
            StatusCode::NOT_FOUND,
            ErrorCode::Validation,
            "no run exists for the requested identifier",
        ),
        // A continuation that named a session this identity may not write into is reported exactly
        // as one that does not exist, because a caller must not learn that somebody else's session
        // is there.
        DatabaseError::SessionNotFound => RunServiceError::new(
            StatusCode::NOT_FOUND,
            ErrorCode::Validation,
            "no session exists for the requested identifier",
        ),
        // An archived session is a state conflict, not a missing one: the conversation exists and
        // is closed to new work, which is what the caller needs to know to start another.
        DatabaseError::SessionNotWritable { .. } => RunServiceError::new(
            StatusCode::CONFLICT,
            ErrorCode::Conflict,
            "the session does not accept new work; start a new conversation",
        ),
        DatabaseError::RunConflict
        | DatabaseError::RunEventConflict
        | DatabaseError::RunTransitionRefused { .. }
        | DatabaseError::RunEventAfterSettlement => RunServiceError::new(
            StatusCode::CONFLICT,
            ErrorCode::Conflict,
            "the run was changed by another writer; re-read it and retry",
        ),
        DatabaseError::InvalidRunRequest { .. }
        | DatabaseError::InvalidRunEventRequest { .. }
        | DatabaseError::InvalidSessionRequest { .. } => RunServiceError::new(
            StatusCode::BAD_REQUEST,
            ErrorCode::Validation,
            "the request was not valid",
        ),
        DatabaseError::LocalIdentityMissing { .. } => RunServiceError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::Internal,
            "this profile is missing its local identity",
        ),
        DatabaseError::StoredRunInvalid { .. }
        | DatabaseError::StoredRunEventInvalid { .. }
        | DatabaseError::StoredSessionInvalid { .. } => RunServiceError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::Internal,
            "the stored state is not internally consistent",
        ),
        // Every remaining variant is an infrastructure failure whose text can name a path or
        // a database message, so the source is deliberately not echoed to the client.
        _ => RunServiceError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::Internal,
            "the local database is not available",
        ),
    }
}
