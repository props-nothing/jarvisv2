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
    ApprovalChannel, ApprovalDecision, ApprovalDecisionOutcome, CorrelationId, ErrorCode, RunId,
    SafeMessage, SessionId, SystemClock, UtcTimestamp,
};
use jarvis_protocol::{
    ApprovalDecisionBody, ApprovalDecisionRequest, ApprovalReply, RunReply, StartRunRequest,
    rest_error, safe,
};
use jarvis_storage::{
    API_SESSION_CHANNEL, DatabaseError, LinkKind, SessionTarget, SqliteDatabase, StartRunInput,
    find_approval, find_project, find_run, link_project, load_local_identity, project_for,
    record_decision, request_run_cancellation, settle_parked_run_cancelled, start_run,
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
        let requested_project = request.project_id.clone();
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

            // A named project is resolved, and a conversation that already belongs to another one refused, before anything is
            // written, so a refusal never leaves a run behind.
            let project = match requested_project {
                Some(name) => {
                    let project = find_project(&self.database, identity.workspace_id(), &name)
                        .await
                        .map_err(|error| map_database_error(&error))?;
                    if let Some(session) = &requested_session {
                        let current = project_for(&self.database, LinkKind::Session, session)
                            .await
                            .map_err(|error| map_database_error(&error))?;
                        if current.is_some_and(|other| other.id != project.id) {
                            return Err(map_database_error(&DatabaseError::ProjectConflict));
                        }
                    }
                    Some(project)
                }
                None => None,
            };

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
            if let Some(project) = project {
                link_project(
                    &self.database,
                    LinkKind::Session,
                    started.session_id(),
                    &project.id,
                )
                .await
                .map_err(|error| map_database_error(&error))?;
            }
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
    /// Takes no expected version, deliberately, and `request_run_cancellation` documents why: a
    /// cancellation is **operator intent**, and every progress write advances a running run's version,
    /// so a client's version is stale immediately and the request would be refused with a conflict the
    /// client cannot resolve. The guard that remains is the one that cannot go stale — a **settled** run
    /// refuses a cancellation, because there is no work left to stop.
    ///
    /// A repeat request is idempotent: the first request time is preserved.
    ///
    /// # Errors
    ///
    /// Returns [`RunServiceError`] when the run is absent or has already settled.
    pub async fn cancel(&self, id: &str) -> Result<RunReply, RunServiceError> {
        let now = UtcTimestamp::now(&SystemClock);
        let run = request_run_cancellation(&self.database, id, now)
            .await
            .map_err(|error| map_database_error(&error))?;
        // A run waiting for a person has no driver to notice a request, so it is settled here and what it was
        // waiting on is withdrawn. Without this the kill switch reported success for a run that never stopped.
        if run.state() == jarvis_core::RunState::AwaitingApproval {
            let settled = settle_parked_run_cancelled(&self.database, &run, now)
                .await
                .map_err(|error| map_database_error(&error))?;
            return Ok(reply::from_stored(&settled));
        }
        Ok(reply::from_stored(&run))
    }

    /// Decides a pending approval.
    ///
    /// # The approver is the local identity, taken here and never from the request
    ///
    /// `load_local_identity` is the same source the gateway already uses to attribute a run. There is
    /// deliberately no request field for "who approved this": the authenticated caller answers for the
    /// local owner, and the approval row stores that audit label with the chosen channel.
    ///
    /// # Errors
    ///
    /// Returns [`RunServiceError`] for an unknown approval, an already-decided approval, or a decision the
    /// domain refuses. Each maps to its own status so a client can tell "you are too late" from "you may
    /// not answer that".
    pub async fn decide(
        &self,
        id: &str,
        body: &ApprovalDecisionBody,
    ) -> Result<ApprovalReply, RunServiceError> {
        // Read first so an unknown identifier is a `404` before the guarded write path runs. The value is
        // otherwise unused: `record_decision` re-reads the row inside its own guarded update, so binding
        // here would be a second copy that can go stale between the read and the write.
        let _existing = find_approval(&self.database, id)
            .await
            .map_err(|error| map_decision_error(&error))?;

        let identity = load_local_identity(&self.database)
            .await
            .map_err(|error| map_identity_error(&error))?;

        let now = UtcTimestamp::now(&SystemClock);
        let decision = ApprovalDecision::new(
            match body.decision {
                ApprovalDecisionRequest::Approve => ApprovalDecisionOutcome::Approve,
                ApprovalDecisionRequest::Deny => ApprovalDecisionOutcome::Deny,
            },
            body.channel.unwrap_or(ApprovalChannel::Api),
            now,
            identity.user_id(),
        )
        // The only field this constructor can refuse is the approver, and the approver here is the
        // profile's own seeded identity — which the schema already bounds at the same length. So this arm
        // is unreachable for a profile that could start; it is mapped rather than unwrapped because
        // "unreachable" is a claim about the seeded row, and a panic would turn a corrupted profile into a
        // crash instead of an answer.
        .map_err(|_| {
            map_decision_error(&DatabaseError::InvalidApprovalRequest {
                field: "decided_by",
            })
        })?;

        let decided = record_decision(&self.database, id, &decision)
            .await
            .map_err(|error| map_decision_error(&error))?;

        Ok(ApprovalReply {
            approval_id: decided.id().to_string(),
            run_id: decided.run_id().to_string(),
            tool: decided.tool().to_owned(),
            tool_version: decided.tool_version().to_owned(),
            // The **effective** state at the instant of answering, so an approval decided within its
            // lifetime but read after the expiry reports `expired` rather than an authority that lapsed.
            state: decided.state_at(now).as_str().to_owned(),
            outcome: decided
                .decision()
                .map(|decision| decision.outcome().as_str().to_owned()),
            created_at: decided.created_at(),
            expires_at: decided.expires_at(),
        })
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
        DatabaseError::ProjectNotFound => RunServiceError::new(
            StatusCode::NOT_FOUND,
            ErrorCode::Validation,
            "no project has that name or identifier",
        ),
        DatabaseError::ProjectConflict => RunServiceError::new(
            StatusCode::CONFLICT,
            ErrorCode::Conflict,
            "that conversation already belongs to a different project",
        ),
        DatabaseError::ProjectNameTaken => RunServiceError::new(
            StatusCode::CONFLICT,
            ErrorCode::Conflict,
            "a project with that name already exists",
        ),
        DatabaseError::InvalidProject { .. } => RunServiceError::new(
            StatusCode::BAD_REQUEST,
            ErrorCode::Validation,
            "the project was not valid",
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
        // a database message, so the source is deliberately not echoed to the client, only logged.
        other => {
            tracing::warn!(error = %other, "a run request could not be served by the database");
            RunServiceError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                ErrorCode::Internal,
                "the local database is not available",
            )
        }
    }
}

/// Maps an approval-decision failure onto a status and the shared error envelope.
///
/// # Each rule gets its own answer, and the reason is what the caller does next
///
/// - An **expired** approval is `409`: the caller was permitted and is now too late. Telling them to fix
///   their request would send them hunting for a mistake they did not make.
/// - An **already-decided** approval is `409` too, and that is the property that makes duplicate
///   delivery safe: a second decision cannot overwrite the first, whatever it says.
///
/// Every message is a fixed phrase. None of them carries a field's content, so a wrong answer does not
/// teach a caller what the right one would look like.
fn map_decision_error(error: &DatabaseError) -> RunServiceError {
    match error {
        DatabaseError::ApprovalNotFound => RunServiceError::new(
            StatusCode::NOT_FOUND,
            ErrorCode::Validation,
            "no approval exists for the requested identifier",
        ),
        DatabaseError::ApprovalAlreadyDecided => RunServiceError::new(
            StatusCode::CONFLICT,
            ErrorCode::Conflict,
            "this approval has already been decided",
        ),
        DatabaseError::ApprovalConflict => RunServiceError::new(
            StatusCode::CONFLICT,
            ErrorCode::Conflict,
            "another writer decided this approval; re-read it",
        ),
        DatabaseError::InvalidApprovalRequest { field } => match *field {
            // The refusal a caller can act on is separated from the rest, because "your decision was
            // already too late" and "the request was malformed" are different next steps.
            "expires_at" => RunServiceError::new(
                StatusCode::CONFLICT,
                ErrorCode::Conflict,
                "this approval expired before the decision arrived",
            ),
            _ => RunServiceError::new(
                StatusCode::BAD_REQUEST,
                ErrorCode::Validation,
                "the decision was not accepted",
            ),
        },
        other => map_database_error(other),
    }
}
