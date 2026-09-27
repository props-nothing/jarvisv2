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
    ApprovalChannel, ApprovalDecision, ApprovalDecisionOutcome, AuthenticationStrength,
    CorrelationId, ErrorCode, RunId, SafeMessage, SessionId, SystemClock, UtcTimestamp,
};
use jarvis_protocol::{
    ApprovalDecisionBody, ApprovalDecisionRequest, ApprovalReply, RunReply, StartRunRequest,
    rest_error, safe,
};
use jarvis_storage::{
    API_SESSION_CHANNEL, DatabaseError, SecretStore, SessionTarget, SqliteDatabase, StartRunInput,
    find_approval, find_run, load_local_identity, record_decision, request_run_cancellation,
    start_run,
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
    /// The store the plaintext decision nonce is taken from.
    ///
    /// Held here rather than passed per call so the daemon's own profile state directory is the only
    /// place a nonce is ever read from: a caller-supplied path would let a client decide an approval
    /// against a secret of its own choosing.
    secrets: SecretStore,
}

impl RunService {
    /// Creates the service over the daemon's database and nonce store.
    #[must_use]
    pub fn new(database: Arc<SqliteDatabase>, secrets: SecretStore) -> Self {
        Self { database, secrets }
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
        let run = request_run_cancellation(&self.database, id, UtcTimestamp::now(&SystemClock))
            .await
            .map_err(|error| map_database_error(&error))?;
        Ok(reply::from_stored(&run))
    }

    /// Decides a pending approval, consuming its one-time nonce.
    ///
    /// # The approver is the local identity, taken here and never from the request
    ///
    /// `load_local_identity` is the same source the gateway already uses to attribute a run, and it is
    /// the identity that is **not** the requester. A tool call's requester is the run (`P3-012a`), so a
    /// decision recorded under the local user is precisely "the human answered the agent's request" — the
    /// only shape `security.md`'s confused-deputy control recognises.
    ///
    /// # The nonce is verified, not taken from the file
    ///
    /// The daemon **presents** the caller's nonce to `record_decision`, which compares it against the
    /// stored digest and — only when the write lands — rotates that digest to the digest of the empty
    /// string. That rotation is the one-time mechanism (`ADR-0018`, `ADR-0042`), and it is what makes a
    /// replay fail whether or not any file still exists.
    ///
    /// An earlier shape had the daemon `take` the nonce from the file and pass *that*. It was wrong in a
    /// way worth recording: the file is a **delivery channel**, so a client that has already read it — the
    /// operator's client, which is the whole point of the channel — would find the daemon consuming a file
    /// the client had legitimately used, and the decision would be refused as "no nonce pending". Reading
    /// the file here would also make the route's answer depend on filesystem state rather than on the
    /// caller's credential, which is not a check at all.
    ///
    /// The delivered file is **discarded after a successful decision** so a presentable secret does not sit
    /// on disk for the remainder of the approval's lifetime. That is defence in depth and not the
    /// control: a failure to remove it does not make the decision replayable, because the digest has
    /// already rotated.
    ///
    /// # A refused decision does not consume anything
    ///
    /// The domain refuses *after* the digest matches, and the guarded `UPDATE` rotates only when the write
    /// lands, so an honest mistake — deciding as the wrong identity, a lapsed approval — leaves the nonce
    /// valid and the operator can try again. Burning an approval on a retryable error would turn a typo
    /// into a re-request.
    ///
    /// # Errors
    ///
    /// Returns [`RunServiceError`] for an unknown approval, a nonce that does not match, an already-decided
    /// approval, or a decision the domain refuses. Each maps to its own status so a client can tell "you
    /// are too late" from "you may not answer that".
    pub async fn decide(
        &self,
        id: &str,
        body: &ApprovalDecisionBody,
    ) -> Result<ApprovalReply, RunServiceError> {
        // Read first, so an unknown identifier never reaches the nonce store. Otherwise a caller could
        // learn whether an approval exists by observing whether a file was consumed. Mapped through the
        // decision mapper so an unknown identifier is a `404` rather than the generic storage answer.
        // The value is otherwise unused: `record_decision` re-reads the row inside its own guarded update,
        // so binding here would be a second copy that can go stale between the read and the write.
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
                ApprovalDecisionRequest::Cancel => ApprovalDecisionOutcome::Cancel,
            },
            ApprovalChannel::Cli,
            // The loopback credential is what this transport actually established, which is the strongest
            // thing a local API client can claim. It is a **claim** the domain then compares against the
            // required strength, so a hold needing presence still refuses it — the check is not bypassed by
            // the claim being high.
            AuthenticationStrength::Present,
            now,
            // The approver is the profile's local identity, and it is recorded **on the decision** rather
            // than passed beside it, so the identity the self-approval guard checks and the identity the row
            // stores cannot be different values. There is deliberately no request field for it: see
            // `jarvis_protocol::ApprovalDecisionBody`.
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

        let decided = record_decision(&self.database, id, &body.nonce, &decision)
            .await
            .map_err(|error| map_decision_error(&error))?;

        // The decision is recorded, so the delivered secret has served its purpose. Removing it is
        // defence in depth: the digest already rotated, so a surviving file cannot decide anything. A
        // failure to remove is therefore not reported as a failed decision — that would tell an operator a
        // decision *did not happen* when it did, which is a worse answer than a stale file. It is logged
        // instead, naming only the approval identifier the caller already supplied in the URL, so the log
        // cannot become a directory listing for the nonce store.
        if let Err(error) = self.secrets.discard(id) {
            tracing::warn!(
                approval_id = %id,
                error = %error,
                "a decided approval's nonce file could not be removed; the decision stands"
            );
        }

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

/// Maps an approval-decision failure onto a status and the shared error envelope.
///
/// # Each rule gets its own answer, and the reason is what the caller does next
///
/// - A **forged or wrong nonce** is `403`, not `400`: the request was well formed and the caller is not
///   permitted to answer for this approval. Reporting it as a content error would invite a retry with a
///   different nonce, which is what a guessing attack looks like.
/// - An **expired** approval is `409`: the caller was permitted and is now too late. Telling them to fix
///   their request would send them hunting for a mistake they did not make.
/// - An **already-decided** approval is `409` too, and that is the property that makes duplicate
///   delivery safe: a second decision cannot overwrite the first, whatever it says.
/// - A **self-approval** is `403`: the requester cannot answer for itself, which is the confused-deputy
///   refusal `security.md` names.
/// - **Insufficient strength** is `403`: a weaker channel than the action requires did not answer for it.
///
/// Every message is a fixed phrase. None of them carries the required strength, the presented value, or
/// a field's content, so a wrong answer does not teach a caller what the right one would look like.
fn map_decision_error(error: &DatabaseError) -> RunServiceError {
    match error {
        DatabaseError::ApprovalNotFound => RunServiceError::new(
            StatusCode::NOT_FOUND,
            ErrorCode::Validation,
            "no approval exists for the requested identifier",
        ),
        DatabaseError::ApprovalNonceMismatch => RunServiceError::new(
            StatusCode::FORBIDDEN,
            ErrorCode::Authorization,
            // A `403` rather than a `400`: the request was well formed and the caller is not permitted to
            // answer for this approval. Reporting it as a content error would invite a retry with a
            // different nonce, which is what a guessing attack looks like.
            "the decision nonce does not match this approval",
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
            // The three refusals a *caller* can act on are separated from the rest, because "your
            // decision was already too late" and "you may not answer this" are different next steps.
            "expires_at" => RunServiceError::new(
                StatusCode::CONFLICT,
                ErrorCode::Conflict,
                "this approval expired before the decision arrived",
            ),
            "decided_by" => RunServiceError::new(
                StatusCode::FORBIDDEN,
                ErrorCode::Authorization,
                "the requester cannot decide its own approval",
            ),
            "decision_strength" => RunServiceError::new(
                StatusCode::FORBIDDEN,
                ErrorCode::Authorization,
                "this approval requires stronger authentication than this channel establishes",
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
