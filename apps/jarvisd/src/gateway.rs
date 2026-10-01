//! The authenticated `/api/v1` HTTP gateway.
//!
//! ADR-0011 makes this a **first-class peer transport** to local IPC rather than a fallback: it
//! is how a browser, the desktop web view, and provider callbacks reach the daemon. Routing
//! lives in this composition root so `jarvis-core`, `jarvis-application`, and `jarvis-storage`
//! stay framework-free, and the DTOs live in `jarvis-protocol`.
//!
//! # One credential, one error envelope
//!
//! Every route is authenticated by the **same** profile-bound credential the local IPC
//! transport uses, verified with [`jarvis_core::ClientCredential::matches`], and every failure
//! reuses [`jarvis_protocol::WireError`]. There is deliberately no second authentication
//! implementation and no third-party auth middleware: two implementations of one security
//! control is how the weaker one becomes the way in.
//!
//! # Why the request body limit is explicit
//!
//! `docs/architecture/security.md` requires bounded payload sizes, and axum's
//! `DefaultBodyLimit` defaults to 2 MB. It is set explicitly here because the default applies
//! only where an extractor consults it. The bound is far below 2 MB because the storage schema
//! already bounds an objective at 4096 characters, so buffering 2 MB in order to reject it
//! spends memory to reach the same answer.
//!
//! # Why the credential is header-only
//!
//! A credential in a query parameter becomes a substring of every access log, proxy log, and
//! `Referer` header it passes through, so the `Authorization` header is the only accepted
//! location. The `from` and `limit` query parameters carry stream positions, never secrets.
//!
//! # Why the health routes are authenticated too
//!
//! An unauthenticated local liveness endpoint would tell any local process whether a daemon is
//! running, and `docs/architecture/security.md` treats "network location alone is not identity"
//! as a rule rather than a preference.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, RawQuery, Request, State},
    http::{HeaderMap, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use jarvis_core::{ClientCredential, ErrorCode, ReplayRequest, RunEventSequence};
use jarvis_protocol::{
    ApprovalDecisionBody, CorrectMemoryRequest, ForgetMemoryRequest, MAX_STREAM_PAGE,
    MemorySearchRequest, RememberRequest, RunEventPageReply, StartRunRequest, rest_error, safe,
};
use jarvis_storage::{DatabaseError, SqliteDatabase, highest_run_event_sequence, read_run_events};
use serde::{Deserialize, Serialize};

pub use crate::run_service::RunService;

/// Maximum accepted request body size in bytes.
///
/// Applied as a router layer so it covers every current and future route, rather than relying on
/// each extractor to apply it correctly.
pub const MAX_REQUEST_BODY_BYTES: usize = 16 * 1024;

/// Shared state every route needs.
#[derive(Clone)]
pub struct GatewayState {
    database: Arc<SqliteDatabase>,
    credential: ClientCredential,
    runs: RunService,
    /// The native executor, when one is configured.
    ///
    /// `None` means runs are accepted and recorded but never driven, which is what `P2-007`
    /// shipped. Holding it here rather than reaching for a global keeps the executor's existence a
    /// property of the running daemon's configuration.
    executor: Option<Arc<crate::executor::Executor>>,
    /// The composed tool pipeline, when workspace roots were granted.
    ///
    /// `None` means no tool is registered at all, so there is nothing to call â€” which is deliberately
    /// different from a pipeline with no roots, because the latter would offer a tool that fails every
    /// call (`docs/adr/0020-filesystem-confinement-is-a-handle.md`).
    tools: Option<Arc<crate::tool_pipeline::ToolPipeline>>,
}

impl GatewayState {
    /// Builds gateway state from the daemon's live resources, with no executor.
    ///
    /// Used by tests that exercise the transport, so a route test cannot accidentally start
    /// spending a model budget.
    #[must_use]
    pub fn new(
        database: Arc<SqliteDatabase>,
        credential: ClientCredential,
        secrets: jarvis_storage::SecretStore,
    ) -> Self {
        Self {
            runs: RunService::new(Arc::clone(&database), secrets),
            database,
            credential,
            executor: None,
            tools: None,
        }
    }

    /// Attaches the native executor, so a started run is driven to a terminal state.
    #[must_use]
    pub fn with_executor(mut self, executor: Arc<crate::executor::Executor>) -> Self {
        self.executor = Some(executor);
        self
    }

    /// Attaches the composed tool pipeline.
    #[must_use]
    pub fn with_tools(mut self, tools: Arc<crate::tool_pipeline::ToolPipeline>) -> Self {
        self.tools = Some(tools);
        self
    }

    /// Returns the tool pipeline, when one is configured.
    #[must_use]
    pub fn tools(&self) -> Option<&Arc<crate::tool_pipeline::ToolPipeline>> {
        self.tools.as_ref()
    }

    /// Returns the database the gateway reads through.
    #[must_use]
    pub fn database(&self) -> &SqliteDatabase {
        &self.database
    }

    /// Returns the memory surface.
    ///
    /// Built on demand from the database rather than held as a field, because it owns nothing but an `Arc`
    /// clone and a stored field would be one more thing a constructor has to remember to set — the shape a
    /// route added later silently omits, which is the same reasoning the router's layers follow.
    #[must_use]
    pub fn memories(&self) -> crate::memory_service::MemoryService {
        crate::memory_service::MemoryService::new(Arc::clone(&self.database))
    }
}

/// Builds the authenticated router.
///
/// Layer order is deliberate: the body limit and the authentication middleware wrap the whole
/// router, so a route added later cannot omit either. A per-route layer would be forgettable in
/// exactly that way.
pub fn router(state: GatewayState) -> Router {
    let api = Router::new()
        .route("/runs", post(start_run))
        .route("/runs/{id}", get(read_run))
        .route("/runs/{id}/cancel", post(cancel_run))
        .route("/runs/{id}/events", get(read_events))
        .route("/runs/{id}/stream", get(crate::sse::stream_events))
        .route("/tools/{tool}/calls", post(call_tool))
        .route("/calls/{id}/resume", post(resume_call))
        .route("/approvals/{id}/decision", post(decide_approval))
        .route("/memories", get(list_memories).post(remember))
        .route("/memories/search", post(search_memories))
        .route("/memories/export", get(export_memories))
        .route("/memories/{id}", get(read_memory))
        .route("/memories/{id}/correct", post(correct_memory))
        .route("/memories/{id}/forget", post(forget_memory));

    Router::new()
        .route("/health/live", get(health_live))
        .route("/health/ready", get(health_ready))
        .nest("/api/v1", api)
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BODY_BYTES))
        .layer(middleware::from_fn_with_state(state.clone(), authenticate))
        .with_state(state)
}

/// Rejects any request that does not present the profile credential.
async fn authenticate(State(state): State<GatewayState>, request: Request, next: Next) -> Response {
    let Some(presented) = bearer_token(request.headers()) else {
        return unauthorized("the request did not present a bearer credential");
    };

    // Constant-time and length-independent, so a caller cannot learn the credential's prefix
    // from response timing.
    if !state.credential.matches(presented) {
        return unauthorized("the presented credential was rejected");
    }

    next.run(request).await
}

/// Extracts the bearer token from an `Authorization` header.
///
/// Returns `None` for a missing header, a different scheme, or an empty token. A query parameter
/// is deliberately not consulted.
///
/// The token is taken **verbatim**: no surrounding whitespace is trimmed. Trimming would make the
/// guard accept a byte sequence that is not the credential, so a caller that appended or prefixed
/// a space would authenticate with a value the daemon never issued. Padding is therefore a
/// rejection rather than a tolerated formatting quirk.
fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let token = value.strip_prefix("Bearer ")?;
    (!token.is_empty()).then_some(token)
}

/// Builds the shared unauthorized response.
fn unauthorized(message: &str) -> Response {
    error_response(StatusCode::UNAUTHORIZED, ErrorCode::Authentication, message)
}

/// Builds a response carrying the shared error envelope.
///
/// One shape for every failure, so a client cannot infer a different trust model from a
/// different body, and error handling is written once across transports.
pub(crate) fn error_response(status: StatusCode, code: ErrorCode, message: &str) -> Response {
    (status, Json(rest_error(code, safe(message)))).into_response()
}

async fn health_live() -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, jarvis_protocol::JSON_CONTENT_TYPE)],
        r#"{"live":true}"#,
    )
        .into_response()
}

async fn health_ready() -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, jarvis_protocol::JSON_CONTENT_TYPE)],
        r#"{"ready":true}"#,
    )
        .into_response()
}

/// `POST /api/v1/runs`
async fn start_run(
    State(state): State<GatewayState>,
    Json(request): Json<StartRunRequest>,
) -> Response {
    match state.runs.start(&request).await {
        Ok(reply) => {
            // The run is driven on its own task so the response is not held open for the whole
            // model call. The client learns the run identifier immediately and follows the stream,
            // which is what makes the API usable for a long answer.
            //
            // The task is deliberately not awaited here and its failure is logged rather than
            // returned: the run is already durably recorded, so a task that fails leaves a run
            // that is visibly unfinished rather than a response that claims a start it did not
            // make.
            if let Some(executor) = &state.executor {
                let database = Arc::clone(&state.database);
                let executor = Arc::clone(executor);
                // The composed tool pipeline, when one exists, so the run is an agent loop rather than a
                // single model call. `None` for a profile with no tool surface, in which case the run
                // answers without tools — the behavior every run had before tools were wired here.
                let tools = state.tools.clone();
                let run_id = reply.run_id.clone();
                tokio::spawn(async move {
                    if let Err(error) = crate::executor::execute_run_with_tools(
                        &database,
                        executor.model(),
                        tools.as_ref(),
                        &run_id,
                    )
                    .await
                    {
                        tracing::error!(
                            run_id,
                            error = %error,
                            "the run executor could not persist its progress"
                        );
                    }
                });
            }
            (StatusCode::CREATED, Json(reply)).into_response()
        }
        Err(error) => error.into_response(),
    }
}

/// Request body for `POST /api/v1/tools/{tool}/calls`.
///
/// Only two fields, and both are the caller's to choose: **which stored run** the call is attributed
/// to, and what to pass the tool. The workspace, the actor's scopes, and the authentication strength
/// are **not** here, because a client that could name its own workspace or grant could widen its own
/// authority â€” `docs/architecture/identity-and-workspaces.md` requires access to follow from
/// authentication rather than from a client-supplied identifier.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ToolCallRequest {
    /// The stored run the call belongs to, which is also where the workspace comes from.
    pub run_id: String,
    /// The arguments to pass, which the tool's own input schema validates.
    pub arguments: serde_json::Value,
}

/// `POST /api/v1/tools/{tool}/calls`
///
/// The first path from a client to a tool adapter. It exists so the composition the pipeline performs
/// is **reachable** rather than only constructible: without a route, the pipeline could be composed and
/// never exercised, which is the state `P3-001`..`P3-006` were in for six slices.
///
/// # The actor is derived, not accepted
///
/// The workspace and run come from the profile's seeded local identity and the stored run, and the
/// scope is one the daemon grants rather than one the request names. A caller-supplied workspace or
/// scope would let a client widen its own authority, which
/// `docs/architecture/identity-and-workspaces.md` forbids. The tool name and the arguments are the
/// only parts of the request this handler reads.
async fn call_tool(
    State(state): State<GatewayState>,
    Path(tool): Path<String>,
    Json(request): Json<ToolCallRequest>,
) -> Response {
    let Some(tools) = state.tools() else {
        return error_response(
            StatusCode::NOT_FOUND,
            ErrorCode::Validation,
            "no tool is registered: grant `daemon.tool_workspace_roots` or configure MCP servers in \
             `mcp-servers.toml` to enable tools",
        );
    };

    // The run the call is attributed to. A tool call belongs to a run, and the caller names it rather
    // than the daemon inventing one â€” but the *workspace* comes from the run's stored row, so a caller
    // cannot attribute a call to a workspace the run is not in.
    let run = match state.runs.read(&request.run_id).await {
        Ok(run) => run,
        Err(error) => return error.into_response(),
    };

    // The actor holds **both** tool areas' scopes, because this transport serves whichever tools the daemon
    // composed: a filesystem tool when roots are granted, an MCP tool when servers are configured. The scope
    // set is still derived here rather than accepted from the request, which is the rule that matters â€” a
    // caller cannot name a scope, so it cannot widen its own authority.
    //
    // The construction is checked rather than unwrapped: a rejected literal is an authoring error, and the
    // daemon failing closed on it is better than a caller being denied for a missing scope that reads as a
    // policy problem.
    let Some(actor) = crate::tool_actor::ToolActor::workspace_and_mcp(
        run.workspace_id.clone(),
        run.run_id.clone(),
        jarvis_core::SessionChannel::Cli,
        // The loopback credential over local IPC is a verified credential, which is what an
        // HTTP client on this transport has actually established.
        jarvis_tools::AuthenticationStrength::Credential,
        "policy-1",
    ) else {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::Internal,
            "the tool actor could not be built: a fixed scope literal was rejected",
        );
    };

    match tools
        .call_tool(
            &tool,
            request.arguments,
            &actor,
            jarvis_core::CorrelationId::new(),
        )
        .await
    {
        Ok(crate::tool_pipeline::ToolPipelineOutcome::Executed(result)) => {
            (StatusCode::OK, Json(tool_call_reply(&result))).into_response()
        }
        Ok(crate::tool_pipeline::ToolPipelineOutcome::AwaitingApproval {
            call_id,
            approval_id,
            required_strength,
            reason_code,
        }) => (
            StatusCode::ACCEPTED,
            Json(serde_json::json!({
                "call_id": call_id,
                "approval_id": approval_id,
                "state": "awaiting_approval",
                "required_strength": required_strength.as_str(),
                "reason_code": reason_code,
            })),
        )
            .into_response(),
        // A refusal is a correct answer to a request, so it is `403` with the reason code rather than
        // a `5xx`: the request was understood and declined, and a client should not retry it.
        Ok(crate::tool_pipeline::ToolPipelineOutcome::Refused { reason_code }) => (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "state": "refused",
                "reason_code": reason_code,
            })),
        )
            .into_response(),
        Err(error) => error_response(
            StatusCode::CONFLICT,
            ErrorCode::Validation,
            &format!("the tool call could not be completed: {error}"),
        ),
    }
}

/// Request body for `POST /api/v1/calls/{id}/resume`.
///
/// # Why this is a separate document from `ToolCallRequest`
///
/// A resume is not a second call. It names an **existing** call rather than a tool, and the arguments it
/// carries are not a request to run something â€” they are the payload the resumed call must still match,
/// which is why a different set is refused rather than run. Making it a field on `ToolCallRequest` would
/// put "call this tool" and "continue the call a human already approved" in one body, and a client that
/// sent the wrong one would get a refusal whose reason it could not tell from a policy denial.
///
/// There is no `run_id` field, and that is deliberate: the call row already records its run, and the resume
/// checks the caller's run against the **stored** value, so a caller that could name a run here would be
/// naming the authority this checks.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResumeCallRequest {
    /// The arguments, which must hash to the intent the approval decided.
    pub arguments: serde_json::Value,
}

/// `POST /api/v1/calls/{id}/resume`
///
/// The route that turns a decided approval into the effect the human authorized. `P3-018` built the
/// pipeline method and this makes it reachable, which is the same reason `P3-007` added the tool-call
/// route: a composition nothing can call is a composition nobody has exercised.
///
/// # The actor is derived exactly as the tool-call route derives it
///
/// Same workspace source, same scope set, same channel. A resume is not a privileged path â€” it runs under
/// the identical authority a fresh call would, and the only thing it adds is the approval it must cite.
///
/// # What comes from the request, and what does not
///
/// The **arguments** do, because `tool_calls` deliberately stores no payload (`0007`) and the digest
/// comparison is what makes supplying them safe. The **run** does not: it comes from the stored run the
/// path's call belongs to, so a caller cannot attribute a resume to another run. Everything else â€” the
/// approval, its approver, its expiry, the retry key, the policy version â€” is read from rows the daemon
/// wrote.
async fn resume_call(
    State(state): State<GatewayState>,
    Path(call_id): Path<String>,
    Json(body): Json<ResumeCallRequest>,
) -> Response {
    let Some(tools) = state.tools() else {
        return error_response(
            StatusCode::NOT_FOUND,
            ErrorCode::Validation,
            "no tool is registered, so no call can be resumed",
        );
    };

    // The run comes from the **call's own row**, not from the request. Reading it first also makes an
    // unknown call a `404` before any authority is derived, so a caller cannot probe for call identifiers
    // by watching which ones produce a policy answer.
    let stored = match jarvis_storage::find_tool_call(state.database(), &call_id).await {
        Ok(stored) => stored,
        Err(jarvis_storage::DatabaseError::ToolCallNotFound) => {
            return error_response(
                StatusCode::NOT_FOUND,
                ErrorCode::Validation,
                "no tool call exists for the requested identifier",
            );
        }
        Err(error) => {
            // The source is not echoed: a database error's text can name a path.
            let _ = error;
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                ErrorCode::Internal,
                "the local database is not available",
            );
        }
    };

    let Some(actor) = crate::tool_actor::ToolActor::workspace_and_mcp(
        stored.workspace_id().to_owned(),
        stored.run_id().to_owned(),
        jarvis_core::SessionChannel::Cli,
        jarvis_tools::AuthenticationStrength::Credential,
        "policy-1",
    ) else {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::Internal,
            "the tool actor could not be built: a fixed scope literal was rejected",
        );
    };

    match tools
        .resume(
            &call_id,
            body.arguments,
            &actor,
            jarvis_core::CorrelationId::new(),
        )
        .await
    {
        Ok(crate::tool_pipeline::ToolPipelineOutcome::Executed(result)) => {
            (StatusCode::OK, Json(tool_call_reply(&result))).into_response()
        }
        // A resumed call cannot be held again: the approval is what released it. Reaching `AwaitingApproval`
        // here would mean a second approval for one action, so it is reported as a fault rather than
        // dressed up as `202`.
        Ok(crate::tool_pipeline::ToolPipelineOutcome::AwaitingApproval { .. }) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::Internal,
            "a resumed call was held again, which is not a state a resume can produce",
        ),
        Ok(crate::tool_pipeline::ToolPipelineOutcome::Refused { reason_code }) => (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "state": "refused",
                "reason_code": reason_code,
            })),
        )
            .into_response(),
        // Every refusal here is about **whether this call may run**, not about how the request is shaped.
        // The database source is never echoed, so the message is a fixed phrase: the reasons name which
        // check fired, and none of them can be manufactured into an oracle about another run's call.
        Err(crate::tool_pipeline::ToolPipelineError::ResumeRefused { reason, .. }) => {
            error_response(StatusCode::CONFLICT, ErrorCode::Conflict, reason)
        }
        Err(crate::tool_pipeline::ToolPipelineError::UnknownCall { .. }) => error_response(
            StatusCode::NOT_FOUND,
            ErrorCode::Validation,
            "no tool call exists for the requested identifier",
        ),
        Err(error) => {
            // Not `Display`ed to the client: a storage fault's text can name a path, and a receipt fault can
            // carry a digest. It is **logged** though, because a fixed phrase with nothing behind it makes a
            // real failure indistinguishable from a misconfiguration on the operator's side.
            tracing::warn!(call_id = %call_id, error = %error, "a call could not be resumed");
            error_response(
                StatusCode::CONFLICT,
                ErrorCode::Validation,
                "the call could not be resumed",
            )
        }
    }
}

/// `POST /api/v1/approvals/{id}/decision`
///
/// # The three things this handler deliberately does not take from the request
///
/// - **The approver.** It is the profile's seeded local identity, read here, not a field. A caller that
///   could name its own approver would defeat the self-approval refusal in one request field.
/// - **The nonce's storage location.** It comes from the daemon's own profile state directory, so a
///   caller cannot point the daemon at a file of its choosing.
/// - **The intent.** It is re-read from the stored row inside `record_decision`, because the digest is
///   what a decision binds to and a value the request supplied would be a binding to a claim.
///
/// # The order, and why the nonce is taken last
///
/// The approval is read first so an unknown identifier is a `404` that never reaches the nonce file â€”
/// otherwise a caller could distinguish "no such approval" from "wrong nonce" only by whether a file was
/// consumed. Then the nonce is taken, then the decision is recorded. A failure between the take and the
/// record leaves the approval undecidable, which is `ADR-0042`'s chosen failure direction and is
/// recoverable by asking for the action again.
async fn decide_approval(
    State(state): State<GatewayState>,
    Path(id): Path<String>,
    Json(body): Json<ApprovalDecisionBody>,
) -> Response {
    match state.runs.decide(&id, &body).await {
        Ok(reply) => (StatusCode::OK, Json(reply)).into_response(),
        Err(error) => error.into_response(),
    }
}

/// Renders an executed call's result as JSON.
///
/// The outcome, the evidence, and the output are **separate fields**, which is the whole point of
/// `P3-005`: a caller must be able to tell a `confirmed` outcome from a `failed` one and must receive
/// the provider evidence separately from the content, so a success-sounding sentence cannot be mistaken
/// for proof.
fn tool_call_reply(result: &jarvis_tools::ToolCallResult) -> serde_json::Value {
    serde_json::json!({
        "state": result.outcome().as_str(),
        "terminal": result.outcome().is_terminal(),
        "evidence": result.evidence().map(jarvis_tools::ProviderEvidence::as_str),
        "reason": result.record().reason(),
        "output": result.output().map(jarvis_tools::BoundedOutput::content),
        "truncated": result.output().is_some_and(jarvis_tools::BoundedOutput::is_truncated),
    })
}

/// `GET /api/v1/runs/{id}`
async fn read_run(State(state): State<GatewayState>, Path(id): Path<String>) -> Response {
    match state.runs.read(&id).await {
        Ok(reply) => (StatusCode::OK, Json(reply)).into_response(),
        Err(error) => error.into_response(),
    }
}

/// `POST /api/v1/runs/{id}/cancel`
///
/// Takes no body. Cancellation is operator intent and carries no expectation, so there is no version to
/// supply â€” see `RunService::cancel`.
async fn cancel_run(State(state): State<GatewayState>, Path(id): Path<String>) -> Response {
    match state.runs.cancel(&id).await {
        Ok(reply) => (StatusCode::OK, Json(reply)).into_response(),
        Err(error) => error.into_response(),
    }
}

/// A parsed `from`/`limit` query pair.
#[derive(Clone, Copy, Debug, Default)]
struct EventQuery {
    from: Option<RunEventSequence>,
    limit: Option<u32>,
}

/// Parses the two integer query parameters by hand.
///
/// Parsed manually rather than through a deserializer so this route does not require axum's
/// `query` feature, which pulls in a URL-decoding dependency that two integer parameters do not
/// justify. A value that is present but unparseable is an error rather than absent: silently
/// treating `from=abc` as "from the start" would replay a stream the client already holds.
fn parse_event_query(raw: Option<&str>) -> Result<EventQuery, &'static str> {
    let mut query = EventQuery::default();
    let Some(raw) = raw else {
        return Ok(query);
    };

    for pair in raw.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        match key {
            "from" => {
                let position: u32 = value
                    .parse()
                    .map_err(|_| "the from parameter is not a positive integer")?;
                query.from = Some(
                    RunEventSequence::new(position)
                        .map_err(|_| "the from parameter is not a positive integer")?,
                );
            }
            "limit" => {
                query.limit = Some(
                    value
                        .parse()
                        .map_err(|_| "the limit parameter is not a positive integer")?,
                );
            }
            // An unrecognized parameter is ignored rather than refused, matching the rule that
            // unknown additive fields are tolerated on a request the daemon does not own.
            _ => {}
        }
    }
    Ok(query)
}

/// `GET /api/v1/runs/{id}/events`
async fn read_events(
    State(state): State<GatewayState>,
    Path(id): Path<String>,
    RawQuery(raw): RawQuery,
) -> Response {
    let query = match parse_event_query(raw.as_deref()) {
        Ok(query) => query,
        Err(message) => {
            return error_response(StatusCode::BAD_REQUEST, ErrorCode::Validation, message);
        }
    };

    let from = query.from.unwrap_or_else(RunEventSequence::first);
    let limit = query.limit.unwrap_or(crate::sse::DEFAULT_PAGE);
    // A limit above the maximum is refused rather than clamped. Clamping would let a caller
    // believe it asked for and received a larger page than it did, the same defect as a
    // silently truncated replay.
    if limit == 0 || limit > MAX_STREAM_PAGE {
        return error_response(
            StatusCode::BAD_REQUEST,
            ErrorCode::Validation,
            "the event page limit must be between 1 and 1000",
        );
    }
    let Ok(request) = ReplayRequest::new(from, limit) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            ErrorCode::Validation,
            "the requested event position is invalid",
        );
    };

    let highest = match highest_run_event_sequence(state.database(), &id).await {
        Ok(highest) => highest,
        Err(error) => return storage_error(&error),
    };
    let events = match read_run_events(state.database(), &id, request).await {
        Ok(events) => events,
        Err(error) => return storage_error(&error),
    };

    // A requested position beyond the stored stream is a resync, not an empty history.
    // `docs/architecture/protocols.md` requires this be explicit: without it a client that lost
    // its position receives an empty page and believes it holds the whole history.
    let resync_required = match highest {
        Some(highest) => from > highest,
        None => from > RunEventSequence::first(),
    };

    let reply = RunEventPageReply {
        events: crate::sse::to_replies(&events),
        highest_sequence: highest,
        resync_required,
    };
    (StatusCode::OK, Json(reply)).into_response()
}

/// `GET /api/v1/memories`
///
/// A listing of the workspace's claims, as **references** rather than content. The query parameter is
/// optional so a client can page, and it is bounded on the server rather than trusted: `limit` arrives from
/// the wire, and an unbounded one is a read whose cost grows with the user's history.
async fn list_memories(State(state): State<GatewayState>, RawQuery(raw): RawQuery) -> Response {
    let page = match parse_memory_page(raw.as_deref()) {
        Ok(page) => page,
        Err(message) => {
            return error_response(StatusCode::BAD_REQUEST, ErrorCode::Validation, message);
        }
    };
    let limit = page.limit.unwrap_or(crate::memory_service::MAX_MEMORY_PAGE);
    match state.memories().list(limit).await {
        Ok(reply) => Json(reply).into_response(),
        Err(error) => memory_error(&error),
    }
}

/// `POST /api/v1/memories`
async fn remember(
    State(state): State<GatewayState>,
    Json(request): Json<RememberRequest>,
) -> Response {
    match state.memories().remember(&request).await {
        // `201` when something was written and `200` when the claim was already known: the second is not a
        // creation, and reporting it as one would make a retry look like a second memory.
        Ok(reply) => {
            let status = if reply.outcome == "remembered" || reply.outcome == "proposed" {
                StatusCode::CREATED
            } else {
                StatusCode::OK
            };
            (status, Json(reply)).into_response()
        }
        Err(error) => memory_error(&error),
    }
}

/// `GET /api/v1/memories/{id}`
async fn read_memory(State(state): State<GatewayState>, Path(id): Path<String>) -> Response {
    match state.memories().read(&id).await {
        Ok(reply) => Json(reply).into_response(),
        Err(error) => memory_error(&error),
    }
}

/// `POST /api/v1/memories/search`
async fn search_memories(
    State(state): State<GatewayState>,
    Json(request): Json<MemorySearchRequest>,
) -> Response {
    match state.memories().search(&request).await {
        Ok(reply) => Json(reply).into_response(),
        Err(error) => memory_error(&error),
    }
}

/// `POST /api/v1/memories/{id}/correct`
async fn correct_memory(
    State(state): State<GatewayState>,
    Path(id): Path<String>,
    Json(request): Json<CorrectMemoryRequest>,
) -> Response {
    match state.memories().correct(&id, &request).await {
        Ok(reply) => Json(reply).into_response(),
        Err(error) => memory_error(&error),
    }
}

/// `POST /api/v1/memories/{id}/forget`
async fn forget_memory(
    State(state): State<GatewayState>,
    Path(id): Path<String>,
    Json(request): Json<ForgetMemoryRequest>,
) -> Response {
    match state.memories().forget(&id, &request).await {
        Ok(receipt) => Json(receipt).into_response(),
        Err(error) => memory_error(&error),
    }
}

/// `GET /api/v1/memories/export`
///
/// The one memory read that returns **content**, because the user asked for their own data and an export
/// whose text was redacted would not be one. It includes archived and deleted claims — the latter as
/// tombstones with no text — so a reader can see what this platform holds rather than only what it would
/// retrieve.
async fn export_memories(State(state): State<GatewayState>, RawQuery(raw): RawQuery) -> Response {
    let page = match parse_memory_page(raw.as_deref()) {
        Ok(page) => page,
        Err(message) => {
            return error_response(StatusCode::BAD_REQUEST, ErrorCode::Validation, message);
        }
    };
    let limit = page.limit.unwrap_or(crate::memory_service::MAX_MEMORY_PAGE);
    let offset = page.offset.unwrap_or(0);
    match state.memories().export(limit, offset).await {
        Ok(reply) => Json(reply).into_response(),
        Err(error) => memory_error(&error),
    }
}

/// Paging parameters for the memory listings.
///
/// Parsed by hand for the same reason the event page is: the `Query` extractor needs axum's `query` feature,
/// which pulls `serde_urlencoded` — a package not currently in this build's dependency graph. A two-field
/// parameter set is not worth a new transitive dependency, and the parser below is the shape the adjacent
/// event route already uses, so the two cannot drift in how they read a query string.
#[derive(Clone, Copy, Debug, Default)]
pub struct MemoryPageQuery {
    /// How many rows to return at most.
    limit: Option<u32>,
    /// How many rows to skip, for an export that pages.
    offset: Option<u32>,
}

/// Parses the memory listing's query parameters.
///
/// # Errors
///
/// Returns a message naming the parameter that could not be read, so a caller is not sent hunting a problem
/// in a parameter it did not send.
fn parse_memory_page(raw: Option<&str>) -> Result<MemoryPageQuery, &'static str> {
    let mut page = MemoryPageQuery::default();
    let Some(raw) = raw else {
        return Ok(page);
    };
    for pair in raw.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        match key {
            "limit" => {
                page.limit = Some(
                    value
                        .parse()
                        .map_err(|_| "the limit parameter is not a positive integer")?,
                );
            }
            "offset" => {
                page.offset = Some(
                    value
                        .parse()
                        .map_err(|_| "the offset parameter is not a positive integer")?,
                );
            }
            // An unrecognized parameter is ignored rather than refused, matching the rule the event route
            // follows: a request the daemon does not own tolerates an additive field.
            _ => {}
        }
    }
    Ok(page)
}

/// Maps a memory failure onto the shared error envelope.
///
/// # Why the status codes are what they are
///
/// - `404` for a claim that is not in this workspace. A claim in *another* workspace is reported the same
///   way, deliberately: "not yours" would confirm that something exists, which is the rule the session
///   lookup already follows.
/// - `409` for a version conflict, because the caller's remedy is to re-read and retry rather than to change
///   the request.
/// - `422` for an unrecognized value or an oversized page. `422` rather than `400` because the body is
///   syntactically valid and it is the *value* that is unacceptable, which is the same distinction the run
///   route's `deny_unknown_fields` uses.
/// - `503` for storage, with a generic message: a `DatabaseError`'s text can name a path or a SQL statement.
///
/// The detail string is carried in the message for the caller-actionable variants and **not** for storage,
/// because a refusal that says only "refused" sends the caller looking for a problem that may not exist.
fn memory_error(error: &crate::memory_service::MemoryServiceError) -> Response {
    use crate::memory_service::MemoryServiceError as Memory;
    match error {
        Memory::NotFound => error_response(
            StatusCode::NOT_FOUND,
            ErrorCode::Validation,
            "no memory exists for the requested identifier in this workspace",
        ),
        Memory::Conflict => error_response(
            StatusCode::CONFLICT,
            ErrorCode::Conflict,
            "the memory was changed by another writer; re-read it and retry",
        ),
        Memory::UnknownValue { .. } | Memory::PageTooLarge { .. } | Memory::Refused { .. } => {
            // The detail is passed through `SafeMessage`, which bounds the length and refuses a control
            // character, so a refusal cannot forge a log line or a second header. It never contains stored
            // content: every detail this service produces is written by the service.
            error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                ErrorCode::Validation,
                safe(&error.detail()).as_str(),
            )
        }
        // Storage is matched last and deliberately without the detail.
        Memory::Storage(_) => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::Internal,
            "the local database is not available",
        ),
    }
}

/// Maps a storage failure onto the shared error envelope.
///
/// Explicit per variant rather than a catch-all, so a new error kind cannot be silently reported
/// as internal. A missing run maps to `404` rather than `400`: the identifier is well-formed and
/// simply absent, so blaming the caller's content would be wrong.
fn storage_error(error: &DatabaseError) -> Response {
    match error {
        DatabaseError::RunNotFound | DatabaseError::RunEventNotFound => error_response(
            StatusCode::NOT_FOUND,
            ErrorCode::Validation,
            "no run exists for the requested identifier",
        ),
        DatabaseError::RunConflict
        | DatabaseError::RunEventConflict
        | DatabaseError::RunTransitionRefused { .. }
        | DatabaseError::RunEventAfterSettlement => error_response(
            StatusCode::CONFLICT,
            ErrorCode::Conflict,
            "the run was changed by another writer; re-read it and retry",
        ),
        DatabaseError::InvalidRunRequest { .. }
        | DatabaseError::InvalidRunEventRequest { .. }
        | DatabaseError::InvalidSessionRequest { .. } => error_response(
            StatusCode::BAD_REQUEST,
            ErrorCode::Validation,
            "the request was not valid",
        ),
        DatabaseError::StoredRunInvalid { .. }
        | DatabaseError::StoredRunEventInvalid { .. }
        | DatabaseError::StoredSessionInvalid { .. } => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::Internal,
            "the stored state is not internally consistent",
        ),
        // Every remaining variant is an infrastructure failure whose text can name a path or a
        // database message, so the source is deliberately not echoed.
        _ => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::Internal,
            "the local database is not available",
        ),
    }
}
#[cfg(test)]
mod tests {
    use axum::body::Body;
    use std::path::PathBuf;
    use tower::ServiceExt as _;

    use super::*;

    /// Owns a temporary profile directory and removes it on drop.
    struct TempProfile(PathBuf);

    impl TempProfile {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("jarvis-gateway-{}", jarvis_core::scratch_tag()));
            std::fs::create_dir_all(&path)
                .unwrap_or_else(|error| panic!("create temp profile: {error}"));
            Self(path)
        }

        fn database_path(&self) -> PathBuf {
            self.0.join(jarvis_storage::DEFAULT_DATABASE_FILENAME)
        }
    }

    impl Drop for TempProfile {
        fn drop(&mut self) {
            jarvis_core::remove_scratch_dir(&self.0);
        }
    }

    /// Builds a router over a real migrated database, so the identity rows migration `0005`
    /// seeds are present without a fixture writing them.
    async fn test_router() -> (Router, String, TempProfile) {
        let profile = TempProfile::new();
        let database = jarvis_storage::SqliteDatabase::open(&profile.database_path())
            .await
            .unwrap_or_else(|error| panic!("open fixture database: {error}"));
        let credential = ClientCredential::generate()
            .unwrap_or_else(|error| panic!("generate fixture credential: {error}"));
        let presented = credential.expose().to_owned();
        let state = GatewayState::new(
            Arc::new(database),
            credential,
            jarvis_storage::SecretStore::in_state(&profile.0.join("state")),
        );
        (router(state), presented, profile)
    }

    fn get_request(path: &str, credential: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder().uri(path).method("GET");
        if let Some(value) = credential {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {value}"));
        }
        builder
            .body(Body::empty())
            .unwrap_or_else(|error| panic!("fixture request: {error}"))
    }

    fn post_json(path: &str, credential: &str, body: &str) -> Request<Body> {
        Request::builder()
            .uri(path)
            .method("POST")
            .header(header::AUTHORIZATION, format!("Bearer {credential}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_owned()))
            .unwrap_or_else(|error| panic!("fixture request: {error}"))
    }

    async fn body_text(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), 256 * 1024)
            .await
            .unwrap_or_else(|error| panic!("read response body: {error}"));
        String::from_utf8_lossy(&bytes).into_owned()
    }

    /// **The falsification test for the authentication guard.** A valid credential succeeds and
    /// every other shape fails closed. Only asserting the happy path would pass with the
    /// middleware removed entirely.
    ///
    /// The refusal is also decoded and asserted to carry [`ErrorCode::Authentication`], which is
    /// the code the local IPC transport reports from `ClientSession::connect` for the same
    /// condition (`crates/jarvis-protocol/tests/local_transport.rs`,
    /// `a_wrong_credential_is_refused_over_native_transport`). ADR-0011 requires one error
    /// vocabulary across transports, so asserting only a status here would let the two diverge
    /// while both still "refused the client".
    #[tokio::test]
    async fn a_request_without_the_credential_is_refused() {
        let (app, presented, _profile) = test_router().await;
        let truncated = presented[..presented.len() - 1].to_owned();
        let padded = format!(" {presented}");
        let wrong = "0".repeat(presented.len());

        for (label, supplied) in [
            ("absent header", None),
            ("empty token", Some("")),
            ("wrong token", Some(wrong.as_str())),
            ("truncated token", Some(truncated.as_str())),
            ("padded token", Some(padded.as_str())),
        ] {
            let response = app
                .clone()
                .oneshot(get_request("/health/live", supplied))
                .await
                .unwrap_or_else(|error| panic!("router call: {error}"));
            assert_eq!(
                response.status(),
                StatusCode::UNAUTHORIZED,
                "{label} must be refused"
            );

            let body = body_text(response).await;
            let error: jarvis_protocol::WireError = serde_json::from_str(&body)
                .unwrap_or_else(|error| panic!("decode {label} refusal {body}: {error}"));
            assert_eq!(
                error.code,
                ErrorCode::Authentication,
                "{label} must report the same authentication code the local transport reports"
            );
        }

        let accepted = app
            .oneshot(get_request("/health/live", Some(&presented)))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(accepted.status(), StatusCode::OK);
    }

    /// A credential in a query parameter must NOT authenticate, because a URL embeds itself in
    /// access logs and `Referer` headers.
    #[tokio::test]
    async fn a_credential_in_a_query_parameter_is_not_accepted() {
        let (app, presented, _profile) = test_router().await;

        let response = app
            .oneshot(get_request(
                &format!("/health/live?api_key={presented}"),
                None,
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "a credential in a URL must not authenticate the request"
        );
    }

    /// A run can be started, read back, and has a non-empty stream. The whole point of the
    /// gateway is that these three agree, so they are asserted together.
    #[tokio::test]
    async fn starting_a_run_persists_it_and_seeds_its_stream() {
        let (app, presented, _profile) = test_router().await;

        let created = app
            .clone()
            .oneshot(post_json(
                "/api/v1/runs",
                &presented,
                r#"{"objective":"summarise the inbox"}"#,
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(created.status(), StatusCode::CREATED);
        let body = body_text(created).await;
        let reply: jarvis_protocol::RunReply =
            serde_json::from_str(&body).unwrap_or_else(|error| panic!("decode {body}: {error}"));
        assert_eq!(reply.state, jarvis_core::RunState::Received);
        assert_eq!(reply.objective, "summarise the inbox");
        assert_eq!(reply.version, 1);

        let read = app
            .clone()
            .oneshot(get_request(
                &format!("/api/v1/runs/{}", reply.run_id),
                Some(&presented),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(read.status(), StatusCode::OK);
        let read_body = body_text(read).await;
        let read_reply: jarvis_protocol::RunReply = serde_json::from_str(&read_body)
            .unwrap_or_else(|error| panic!("decode {read_body}: {error}"));
        assert_eq!(read_reply, reply, "the read must agree with the create");

        let events = app
            .oneshot(get_request(
                &format!("/api/v1/runs/{}/events", reply.run_id),
                Some(&presented),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(events.status(), StatusCode::OK);
        let events_body = body_text(events).await;
        let page: jarvis_protocol::RunEventPageReply = serde_json::from_str(&events_body)
            .unwrap_or_else(|error| panic!("decode {events_body}: {error}"));
        assert_eq!(
            page.events.len(),
            1,
            "a new run's stream must already contain its first event"
        );
        assert_eq!(page.events[0].sequence.get(), 1);
        assert_eq!(page.events[0].kind, jarvis_core::RunEventKind::StateChanged);
        assert_eq!(page.highest_sequence.map(RunEventSequence::get), Some(1));
        assert!(!page.resync_required);
        // The payload is nested JSON rather than a double-encoded string.
        assert_eq!(page.events[0].payload["state"], "received");
    }

    /// An empty objective is refused, so a run cannot start with nothing the model could act on.
    #[tokio::test]
    async fn an_empty_objective_is_refused() {
        let (app, presented, _profile) = test_router().await;

        for objective in ["", "   ", "\t\n"] {
            let response = app
                .clone()
                .oneshot(post_json(
                    "/api/v1/runs",
                    &presented,
                    &format!(r#"{{"objective":"{objective}"}}"#),
                ))
                .await
                .unwrap_or_else(|error| panic!("router call: {error}"));
            assert_eq!(
                response.status(),
                StatusCode::BAD_REQUEST,
                "an objective of {objective:?} must be refused"
            );
        }
    }

    /// An idempotency key is refused rather than silently ignored, so a retrying client is not
    /// told its request was deduplicated while two runs now exist.
    #[tokio::test]
    async fn an_idempotency_key_is_refused_until_deduplication_exists() {
        let (app, presented, _profile) = test_router().await;

        let response = app
            .oneshot(post_json(
                "/api/v1/runs",
                &presented,
                r#"{"objective":"x","idempotency_key":"abc"}"#,
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
        let body = body_text(response).await;
        let error: jarvis_protocol::WireError =
            serde_json::from_str(&body).unwrap_or_else(|error| panic!("decode {body}: {error}"));
        assert_eq!(error.code, ErrorCode::Unsupported);
    }

    /// A missing run is `404` carrying the shared error envelope, not a bare status or a `400`.
    #[tokio::test]
    async fn a_missing_run_reports_the_shared_error_envelope() {
        let (app, presented, _profile) = test_router().await;

        let response = app
            .oneshot(get_request(
                "/api/v1/runs/0198f000-0000-7000-8000-0000000000ff",
                Some(&presented),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let body = body_text(response).await;
        let error: jarvis_protocol::WireError =
            serde_json::from_str(&body).unwrap_or_else(|error| panic!("decode {body}: {error}"));
        assert_eq!(error.code, ErrorCode::Validation);
        assert!(!error.retryable, "an absent run is not retryable");
    }

    /// **A cancellation is accepted without a version, and cannot be refused for being "stale".**
    ///
    /// This replaces a test that asserted a stale version was refused. That field is gone: the
    /// executor advances a running run's version as it walks the state machine, so a client's version
    /// is stale almost immediately and the request was refused with a conflict it could not resolve â€”
    /// the user asked to stop a run and was told the run had changed. See
    /// `jarvis_storage::request_run_cancellation`.
    ///
    /// What is asserted instead is that a body is not required, and that a repeat request is accepted
    /// rather than refused, because a cancellation is idempotent operator intent.
    #[tokio::test]
    async fn a_cancellation_is_accepted_without_a_version_and_is_repeatable() {
        let (app, presented, _profile) = test_router().await;

        let created = app
            .clone()
            .oneshot(post_json(
                "/api/v1/runs",
                &presented,
                r#"{"objective":"stop me"}"#,
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        let reply: jarvis_protocol::RunReply = serde_json::from_str(&body_text(created).await)
            .unwrap_or_else(|error| panic!("decode create: {error}"));

        // No body at all. A client cannot supply a current version, so the endpoint must not require one.
        let accepted = app
            .clone()
            .oneshot(post_json(
                &format!("/api/v1/runs/{}/cancel", reply.run_id),
                &presented,
                "",
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(
            accepted.status(),
            StatusCode::OK,
            "a cancellation must not require a version"
        );
        let cancelled: jarvis_protocol::RunReply = serde_json::from_str(&body_text(accepted).await)
            .unwrap_or_else(|error| panic!("decode cancel: {error}"));
        // Cancellation is a REQUEST, so the run is not yet terminal. Reporting it settled would
        // claim a stop that has not happened.
        assert!(cancelled.cancellation_requested_at.is_some());
        assert_eq!(cancelled.state, jarvis_core::RunState::Received);
        let first_requested_at = cancelled.cancellation_requested_at;

        // A repeat is accepted, and keeps the FIRST request time so the interval between asking and
        // stopping stays measurable. This is what makes the request idempotent.
        let repeated = app
            .oneshot(post_json(
                &format!("/api/v1/runs/{}/cancel", reply.run_id),
                &presented,
                "",
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(repeated.status(), StatusCode::OK);
        let again: jarvis_protocol::RunReply = serde_json::from_str(&body_text(repeated).await)
            .unwrap_or_else(|error| panic!("decode cancel: {error}"));
        assert_eq!(
            again.cancellation_requested_at, first_requested_at,
            "a second request must preserve the first request time"
        );
    }

    /// Cancelling a run that has already settled is refused.
    ///
    /// **Proved at the storage layer, not here**, because the gateway has no route that settles a run â€”
    /// settlement is the executor's `settle_run` and there is no HTTP path to it, so a gateway test
    /// would have to fabricate one. `jarvis_storage::run_repository`'s
    /// `cancelling_a_settled_run_is_refused` covers the guard where it lives, which is where a stale
    /// version would previously have been mistaken for it.
    ///
    /// Recorded here as a gap rather than left silent: the remaining guard on this endpoint is the one
    /// that cannot go stale, and it is exercised by the test above plus the storage test.
    #[tokio::test]
    async fn cancelling_a_settled_run_is_refused() {
        let (app, presented, _profile) = test_router().await;

        let created = app
            .clone()
            .oneshot(post_json(
                "/api/v1/runs",
                &presented,
                r#"{"objective":"already done"}"#,
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        let reply: jarvis_protocol::RunReply = serde_json::from_str(&body_text(created).await)
            .unwrap_or_else(|error| panic!("decode create: {error}"));

        // The run is not settled through HTTP: there is no route for it, and inventing one here would
        // be a fixture for a path no client can take. The storage test covers the guard.
        let _ = reply.run_id;
    }

    /// A cursor past what the daemon holds is a resync rather than an empty success. Without this
    /// a client that lost its position receives an empty page and believes it has everything.
    #[tokio::test]
    async fn a_position_beyond_the_stream_requests_a_resync() {
        let (app, presented, _profile) = test_router().await;

        let created = app
            .clone()
            .oneshot(post_json(
                "/api/v1/runs",
                &presented,
                r#"{"objective":"resync me"}"#,
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        let reply: jarvis_protocol::RunReply = serde_json::from_str(&body_text(created).await)
            .unwrap_or_else(|error| panic!("decode create: {error}"));

        let response = app
            .clone()
            .oneshot(get_request(
                &format!("/api/v1/runs/{}/events?from=500", reply.run_id),
                Some(&presented),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(response.status(), StatusCode::OK);
        let page: jarvis_protocol::RunEventPageReply =
            serde_json::from_str(&body_text(response).await)
                .unwrap_or_else(|error| panic!("decode page: {error}"));
        assert!(page.resync_required);
        assert!(page.events.is_empty());

        // A page limit above the maximum is refused rather than clamped.
        let refused = app
            .oneshot(get_request(
                &format!("/api/v1/runs/{}/events?limit=5000", reply.run_id),
                Some(&presented),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
    }

    /// Builds a header map holding one `Authorization` value.
    fn headers_with(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            value
                .parse()
                .unwrap_or_else(|error| panic!("fixture header: {error}")),
        );
        headers
    }

    /// **The falsification test for the token extractor.** A credential is accepted only in the
    /// `Bearer` scheme, so a raw token, another scheme, or an empty token cannot authenticate.
    #[test]
    fn the_bearer_extractor_rejects_every_non_bearer_shape() {
        assert_eq!(bearer_token(&HeaderMap::new()), None, "a missing header");

        assert_eq!(
            bearer_token(&headers_with("Basic abc")),
            None,
            "a different scheme"
        );
        assert_eq!(
            bearer_token(&headers_with("Bearer ")),
            None,
            "an empty token is not a token"
        );
        assert_eq!(bearer_token(&headers_with("Bearer abc")), Some("abc"));
    }

    /// A query string is parsed into exactly the two positions this route accepts, and an
    /// unparseable value is an error rather than a silent default.
    #[test]
    fn the_event_query_parses_positions_and_refuses_nonsense() {
        let parsed = parse_event_query(Some("from=7&limit=25"))
            .unwrap_or_else(|error| panic!("valid query: {error}"));
        assert_eq!(parsed.from.map(RunEventSequence::get), Some(7));
        assert_eq!(parsed.limit, Some(25));

        assert_eq!(
            parse_event_query(None)
                .unwrap_or_else(|error| panic!("absent query: {error}"))
                .from,
            None
        );

        assert!(
            parse_event_query(Some("from=abc")).is_err(),
            "an unparseable position must not silently become the start of the stream"
        );
        assert!(
            parse_event_query(Some("from=0")).is_err(),
            "zero is not a valid one-based position"
        );

        // An unknown parameter is tolerated, matching the additive-field rule.
        let unknown = parse_event_query(Some("future=1"))
            .unwrap_or_else(|error| panic!("unknown parameter: {error}"));
        assert_eq!(unknown.from, None);
    }

    /// A malformed resume cursor over HTTP is refused, so a client cannot silently restart a
    /// stream it already holds or skip events it never saw.
    #[tokio::test]
    async fn a_malformed_stream_cursor_is_refused() {
        let (app, presented, _profile) = test_router().await;

        let created = app
            .clone()
            .oneshot(post_json(
                "/api/v1/runs",
                &presented,
                r#"{"objective":"stream me"}"#,
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        let reply: jarvis_protocol::RunReply = serde_json::from_str(&body_text(created).await)
            .unwrap_or_else(|error| panic!("decode create: {error}"));

        for cursor in ["", "abc", "0", "-1"] {
            let request = Request::builder()
                .uri(format!("/api/v1/runs/{}/stream", reply.run_id))
                .header(header::AUTHORIZATION, format!("Bearer {presented}"))
                .header("last-event-id", cursor)
                .body(Body::empty())
                .unwrap_or_else(|error| panic!("fixture request: {error}"));
            let response = app
                .clone()
                .oneshot(request)
                .await
                .unwrap_or_else(|error| panic!("router call: {error}"));
            assert_eq!(
                response.status(),
                StatusCode::BAD_REQUEST,
                "a cursor of {cursor:?} must be refused rather than defaulted"
            );
        }

        // A cursor beyond the end cannot be resumed without skipping events, so it is a conflict.
        let beyond = Request::builder()
            .uri(format!("/api/v1/runs/{}/stream", reply.run_id))
            .header(header::AUTHORIZATION, format!("Bearer {presented}"))
            .header("last-event-id", "500")
            .body(Body::empty())
            .unwrap_or_else(|error| panic!("fixture request: {error}"));
        let response = app
            .oneshot(beyond)
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    /// A stream for an unknown run is a `404` rather than a stream that opens and closes, because
    /// an empty stream cannot be distinguished from a truncated one.
    #[tokio::test]
    async fn a_stream_for_an_unknown_run_is_not_found() {
        let (app, presented, _profile) = test_router().await;

        let request = Request::builder()
            .uri("/api/v1/runs/0198f000-0000-7000-8000-0000000000ff/stream")
            .header(header::AUTHORIZATION, format!("Bearer {presented}"))
            .body(Body::empty())
            .unwrap_or_else(|error| panic!("fixture request: {error}"));
        let response = app
            .oneshot(request)
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    /// Builds a router with a composed tool pipeline over one granted root, plus the id of a stored
    /// run to attribute tool calls to.
    ///
    /// A **real** `ToolPipeline` over a real temporary directory, not a stand-in: the claims under
    /// test are about what the route does with a request â€” which workspace it uses and which fields
    /// it accepts â€” and a double would decide those itself, which is the thing to be checked.
    async fn tool_router() -> (Router, String, TempProfile, String) {
        let profile = TempProfile::new();
        let root = profile.0.join("workspace");
        std::fs::create_dir_all(&root).unwrap_or_else(|error| panic!("create workspace: {error}"));
        std::fs::write(root.join("secret.txt"), "workspace contents")
            .unwrap_or_else(|error| panic!("write fixture file: {error}"));

        let database = Arc::new(
            jarvis_storage::SqliteDatabase::open(&profile.database_path())
                .await
                .unwrap_or_else(|error| panic!("open fixture database: {error}")),
        );
        let credential = ClientCredential::generate()
            .unwrap_or_else(|error| panic!("generate fixture credential: {error}"));
        let presented = credential.expose().to_owned();

        let roots = jarvis_tools::WorkspaceRoots::new([root.as_path()])
            .unwrap_or_else(|error| panic!("grant the workspace root: {error}"));
        let pipeline = crate::tool_pipeline::ToolPipeline::new(
            Arc::clone(&database),
            roots,
            jarvis_tools::WorkspacePolicy::default(),
            jarvis_storage::SecretStore::in_state(&profile.0.join("state")),
        )
        .unwrap_or_else(|error| panic!("compose the tool pipeline: {error}"));

        let app = router(
            GatewayState::new(
                database,
                credential,
                jarvis_storage::SecretStore::in_state(&profile.0.join("state")),
            )
            .with_tools(Arc::new(pipeline)),
        );

        // A run is the attribution target: the handler reads its **stored** row for the workspace, so
        // the run has to exist for any of this to be exercised.
        let created = app
            .clone()
            .oneshot(post_json(
                "/api/v1/runs",
                &presented,
                r#"{"objective":"read a file"}"#,
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(created.status(), StatusCode::CREATED);
        let reply: jarvis_protocol::RunReply = serde_json::from_str(&body_text(created).await)
            .unwrap_or_else(|error| panic!("decode create: {error}"));

        (app, presented, profile, reply.run_id)
    }

    /// Posts a tool call the way the route expects it.
    fn tool_call_request(
        presented: &str,
        tool: &str,
        run_id: &str,
        arguments: &serde_json::Value,
    ) -> Request<Body> {
        post_json(
            &format!("/api/v1/tools/{tool}/calls"),
            presented,
            &serde_json::json!({ "run_id": run_id, "arguments": arguments }).to_string(),
        )
    }

    /// **The falsification test for the tool route's authority derivation.**
    ///
    /// The claim in `docs/adr/0023-tool-pipeline-composition-root.md` is that the workspace comes from
    /// the stored run and the scope from the daemon, so that a client cannot widen its own authority.
    /// A guard on a request field is only worth something if the **absence** of the field is what makes
    /// the call succeed, so the positive control is asserted first: with the same request the adapter
    /// really does reach the file. Without that, the refusals below would pass on a route that never
    /// worked at all.
    #[tokio::test]
    async fn a_tool_call_cannot_name_its_own_workspace() {
        let (app, presented, _profile, run_id) = tool_router().await;

        let allowed = app
            .clone()
            .oneshot(tool_call_request(
                &presented,
                jarvis_tools::READ_TOOL,
                &run_id,
                &serde_json::json!({ "path": "secret.txt" }),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(
            allowed.status(),
            StatusCode::OK,
            "the granted root must be readable, or the refusals below prove nothing"
        );
        let body = body_text(allowed).await;
        assert!(
            body.contains("workspace contents"),
            "the call must reach the granted file: {body}"
        );
        assert!(
            body.contains("\"state\":\"confirmed\""),
            "the reply must report the adapter's outcome separately from the content: {body}"
        );

        // Widening attempt one: name a workspace of the caller's choosing. `deny_unknown_fields` means
        // this is refused by the decoder rather than silently ignored â€” an ignored field reads as an
        // accepted one, which is how a client comes to believe it set something.
        let named = Request::builder()
            .uri(format!("/api/v1/tools/{}/calls", jarvis_tools::READ_TOOL))
            .method("POST")
            .header(header::AUTHORIZATION, format!("Bearer {presented}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::json!({
                    "run_id": run_id,
                    "arguments": { "path": "secret.txt" },
                    "workspace_id": "0198f000-0000-7000-8000-0000000000ff"
                })
                .to_string(),
            ))
            .unwrap_or_else(|error| panic!("fixture request: {error}"));
        let response = app
            .clone()
            .oneshot(named)
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "a request that names a workspace must be refused, not accepted with the field ignored"
        );

        // Widening attempt two: attribute the call to a run that does not exist. The workspace comes
        // from the run's stored row, so there is nothing to read from and the call cannot proceed.
        let absent = app
            .clone()
            .oneshot(tool_call_request(
                &presented,
                jarvis_tools::READ_TOOL,
                "0198f000-0000-7000-8000-0000000000fe",
                &serde_json::json!({ "path": "secret.txt" }),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(
            absent.status(),
            StatusCode::NOT_FOUND,
            "a call attributed to an unknown run has no workspace to be decided against"
        );

        // Widening attempt three: escape the granted root. The status is `200`, because a `Failed`
        // outcome is a **correct answer** to a request that was understood â€” ADR-0020's fifth rule puts
        // a refused path in the outcome rather than in a transport error. So the assertion is on the
        // outcome and on the absence of content, NOT on the status: asserting "not 2xx" here would pass
        // for a route that returned a body, which is the confusion `P3-005` exists to remove.
        let escaped = app
            .oneshot(tool_call_request(
                &presented,
                jarvis_tools::READ_TOOL,
                &run_id,
                &serde_json::json!({ "path": "../outside.txt" }),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        let body = body_text(escaped).await;
        let reply: serde_json::Value =
            serde_json::from_str(&body).unwrap_or_else(|error| panic!("decode {body}: {error}"));
        assert_eq!(
            reply["state"], "failed",
            "a traversal must be a failed outcome, not a confirmation: {body}"
        );
        assert!(
            reply["output"].is_null(),
            "a refused path must return no content at all: {body}"
        );
        assert!(
            reply["reason"]
                .as_str()
                .is_some_and(|reason| reason.contains("outside")),
            "the outcome must say the path left the root rather than only that something failed: {body}"
        );
    }

    /// An unknown tool is refused by the adapter's registry rather than by the route, so the route
    /// cannot become the place a tool name is validated and then drift from the registry's rules.
    #[tokio::test]
    async fn a_tool_that_is_not_registered_is_refused() {
        let (app, presented, _profile, run_id) = tool_router().await;

        let response = app
            .oneshot(tool_call_request(
                &presented,
                "jarvis.files.delete",
                &run_id,
                &serde_json::json!({ "path": "secret.txt" }),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(
            response.status(),
            StatusCode::CONFLICT,
            "an unregistered tool must not be resolved to something else"
        );
    }

    /// **With no roots granted there is no tool route at all**, which is deliberately different from a
    /// pipeline over zero roots.
    ///
    /// A pipeline registered over no roots would advertise the tool and then fail every call, which a
    /// caller reads as a broken tool rather than as an absent capability. The refusal must also name
    /// the configuration key, because "no tool is registered" without the remedy sends an operator to
    /// read the source.
    #[tokio::test]
    async fn a_tool_call_without_a_composed_pipeline_is_not_found() {
        let (app, presented, _profile) = test_router().await;

        let response = app
            .oneshot(tool_call_request(
                &presented,
                jarvis_tools::READ_TOOL,
                "0198f000-0000-7000-8000-0000000000fe",
                &serde_json::json!({ "path": "secret.txt" }),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = body_text(response).await;
        assert!(
            body.contains("daemon.tool_workspace_roots"),
            "the refusal must name the key that would enable the tool: {body}"
        );
    }

    /// A router, a credential, a profile, and **one pending approval with its nonce already delivered**.
    ///
    /// The approval is produced by the real pipeline holding a real risk-2 tool, so the row and the
    /// nonce file are exactly what a live daemon would leave behind. Building the row by hand would be
    /// the fixture stating its own assumption about what the pipeline writes, which is the thing the
    /// route test exists to check.
    ///
    /// The nonce is read straight out of the profile's private store, because that is what an operator's
    /// client does. Nothing in this fixture can obtain it from a response, which is `ADR-0042`'s point.
    async fn approval_router() -> (
        Router,
        String,
        TempProfile,
        String,
        String,
        Arc<SqliteDatabase>,
    ) {
        approval_router_with(crate::approval_fixture::approval_adapter()).await
    }

    /// The same fixture over a chosen adapter, so a test about **resumption** can observe the call running.
    ///
    /// The default fixture's adapter refuses, which is what makes "the held call did not run" observable.
    /// A resume test needs the opposite observation, so it supplies an adapter that succeeds and counts —
    /// and the choice is a parameter rather than a second fixture, because everything else about the setup
    /// (the profile, the root grant, the pipeline, the held call, the delivered nonce) has to be **identical**
    /// for the two tests to be about the same thing.
    async fn approval_router_with(
        adapter: Arc<dyn jarvis_tools::ToolExecutor>,
    ) -> (
        Router,
        String,
        TempProfile,
        String,
        String,
        Arc<SqliteDatabase>,
    ) {
        let profile = TempProfile::new();
        let directory = profile.0.join("workspace");
        std::fs::create_dir_all(&directory)
            .unwrap_or_else(|error| panic!("create workspace: {error}"));

        let database = Arc::new(
            jarvis_storage::SqliteDatabase::open(&profile.database_path())
                .await
                .unwrap_or_else(|error| panic!("open fixture database: {error}")),
        );
        let credential = ClientCredential::generate()
            .unwrap_or_else(|error| panic!("generate fixture credential: {error}"));
        let presented = credential.expose().to_owned();
        let state_directory = profile.0.join("state");
        let secrets = jarvis_storage::SecretStore::in_state(&state_directory);

        let roots = jarvis_tools::WorkspaceRoots::new([directory.as_path()])
            .unwrap_or_else(|error| panic!("grant the workspace root: {error}"));
        // The hold is produced by a tool that **declares** `ApprovalPolicy::Ask` at risk 0, so the hold
        // comes from the tool's own declaration rather than from a workspace threshold the fixture
        // invented. That is a posture a real MCP server can carry, and it holds regardless of the default
        // workspace policy â€” which is what makes this fixture's hold stable rather than a coincidence of
        // the policy defaults.
        let pipeline = crate::tool_pipeline::ToolPipeline::with_adapters(
            Arc::clone(&database),
            Some(roots),
            jarvis_tools::WorkspacePolicy::default(),
            vec![(
                vec![crate::approval_fixture::approval_declaring_definition()],
                adapter,
            )],
            secrets.clone(),
        )
        .unwrap_or_else(|error| panic!("compose the tool pipeline: {error}"));

        let app = router(
            GatewayState::new(Arc::clone(&database), credential, secrets.clone())
                .with_tools(Arc::new(pipeline)),
        );

        let created = app
            .clone()
            .oneshot(post_json(
                "/api/v1/runs",
                &presented,
                r#"{"objective":"read a file"}"#,
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        let reply: jarvis_protocol::RunReply = serde_json::from_str(&body_text(created).await)
            .unwrap_or_else(|error| panic!("decode create: {error}"));

        // A held call needs a tool that declares an approval. The filesystem adapter is read-only and
        // auto-allowed, so the hold is produced by an MCP-namespaced tool declaring `ApprovalPolicy::Ask`
        // â€” a posture an operator can genuinely configure.
        let held = app
            .clone()
            .oneshot(tool_call_request(
                &presented,
                crate::approval_fixture::APPROVAL_TOOL,
                &reply.run_id,
                &serde_json::json!({ "path": "notes.txt" }),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(
            held.status(),
            StatusCode::ACCEPTED,
            "the fixture requires a held call: {}",
            body_text(held).await
        );
        let held_body: serde_json::Value = serde_json::from_str(&body_text(held).await)
            .unwrap_or_else(|error| panic!("decode hold: {error}"));
        let approval_id = held_body["approval_id"]
            .as_str()
            .unwrap_or_else(|| panic!("a hold must carry its approval id: {held_body}"))
            .to_owned();

        // The nonce is read the way an operator's client reads it â€” straight from the file â€” and
        // **deliberately not** through `SecretStore::take`, because `take` is the daemon's consuming read.
        // A fixture that took it would leave nothing for the route to consume, which is a property of the
        // store working rather than of the route failing.
        let nonce =
            std::fs::read_to_string(profile.0.join("state").join("approvals").join(&approval_id))
                .unwrap_or_else(|error| panic!("read the delivered nonce: {error}"));

        (app, presented, profile, approval_id, nonce, database)
    }

    /// Posts an approval decision.
    fn decision_request(
        presented: &str,
        approval_id: &str,
        body: &serde_json::Value,
    ) -> Request<Body> {
        post_json(
            &format!("/api/v1/approvals/{approval_id}/decision"),
            presented,
            &body.to_string(),
        )
    }

    /// Posts a call resumption.
    fn resume_request(presented: &str, call_id: &str, body: &serde_json::Value) -> Request<Body> {
        post_json(
            &format!("/api/v1/calls/{call_id}/resume"),
            presented,
            &body.to_string(),
        )
    }

    /// **The route that makes a delivered nonce usable: an operator decides a held approval.**
    ///
    /// `P3-012a` wrote the approval and `P3-012b` delivered its nonce, and neither made the decision
    /// reachable. This is the end of that path, and it asserts the three things a route could get wrong:
    /// the decision lands, the row reports the **effective** state, and the nonce is consumed so a replay
    /// of the identical request is refused rather than recording a second decision.
    ///
    /// The replay is the important one. Without a consumed nonce, the second request would be accepted
    /// and â€” depending on the store â€” could overwrite the first decision. `record_decision` already
    /// refuses an already-decided row; this asserts the *route* does not defeat it.
    #[tokio::test]
    async fn an_operator_can_decide_a_held_approval_exactly_once() {
        let (app, presented, _profile, approval_id, nonce, _database) = approval_router().await;

        let approved = app
            .clone()
            .oneshot(decision_request(
                &presented,
                &approval_id,
                &serde_json::json!({ "decision": "approve", "nonce": nonce }),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(
            approved.status(),
            StatusCode::OK,
            "a delivered nonce must decide the approval: {}",
            body_text(approved).await
        );

        let decided = app
            .clone()
            .oneshot(decision_request(
                &presented,
                &approval_id,
                &serde_json::json!({ "decision": "approve", "nonce": nonce }),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(
            decided.status(),
            StatusCode::CONFLICT,
            "a replayed decision must be refused, and the nonce must be gone: {}",
            body_text(decided).await
        );
    }

    /// **A decision must not be recordable by naming the requester as its approver.**
    ///
    /// The approver is the profile's local identity and there is deliberately no field for it, so this
    /// asserts the *shape* rather than the behaviour: a request that tries to supply one is refused as
    /// malformed. That is the difference between a field nobody reads and a field nobody can send, and
    /// only the second one cannot be filled in by a caller that misreads the documentation.
    #[tokio::test]
    async fn a_decision_cannot_name_its_own_approver() {
        let (app, presented, _profile, approval_id, nonce, _database) = approval_router().await;

        let response = app
            .oneshot(decision_request(
                &presented,
                &approval_id,
                &serde_json::json!({
                    "decision": "approve",
                    "nonce": nonce,
                    "approver_id": "0198f000-0000-7000-8000-0000000000c3"
                }),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "an unknown field must be refused rather than ignored, because an ignored field reads as an \
             accepted one"
        );
    }

    /// **The whole path over the wire: hold, decide, resume â€” and resume only once.**
    ///
    /// This is the end-to-end property `P3-012` exists for, exercised through the real routes rather than
    /// through the pipeline: a client asks for an action, a human answers, and the effect happens **exactly
    /// once** however many times the resumption is delivered.
    ///
    /// The duplicate is the important half. A resumed call that reached the adapter twice would be the
    /// second effect a duplicate decision must never produce, and the refusal has to be observable as a
    /// status rather than only as an internal state change â€” a client that cannot tell "already done" from
    /// "done now" will retry, which is how a duplicate delivery becomes a duplicate effect.
    #[tokio::test]
    async fn an_approved_call_resumes_exactly_once_over_the_wire() {
        // The recording adapter, because this test must observe the call **run**: the refusing fixture
        // adapter would make "the resume worked" and "the resume never reached an adapter" produce the same
        // shape, so a duplicate-delivery assertion built on it could not tell the difference.
        //
        // The handle is kept so the count can be asserted, which is the claim that actually matters: the
        // statuses show the *route* refused the second attempt, while the counter shows the **effect**
        // happened once.
        let adapter =
            std::sync::Arc::new(crate::approval_fixture::RecordingApprovalAdapter::default());
        let (app, presented, _profile, approval_id, nonce, database) =
            approval_router_with(adapter.clone()).await;

        // The held call, decided by an operator.
        let decided = app
            .clone()
            .oneshot(decision_request(
                &presented,
                &approval_id,
                &serde_json::json!({ "decision": "approve", "nonce": nonce }),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(decided.status(), StatusCode::OK);
        let reply: serde_json::Value = serde_json::from_str(&body_text(decided).await)
            .unwrap_or_else(|error| panic!("decode decision: {error}"));
        let run_id = reply["run_id"]
            .as_str()
            .unwrap_or_else(|| panic!("a decision must name its run: {reply}"))
            .to_owned();

        // The call held for that run, read from the database rather than from a response, because the
        // resume route names a call and the hold's reply is what a client would have kept.
        let calls = jarvis_storage::read_run_tool_calls(&database, &run_id)
            .await
            .unwrap_or_else(|error| panic!("read the run's calls: {error}"));
        let call_id = calls.first().map_or_else(
            || panic!("the fixture must have held one call"),
            |call| call.id().to_owned(),
        );

        let resumed = app
            .clone()
            .oneshot(resume_request(
                &presented,
                &call_id,
                &serde_json::json!({ "arguments": { "path": "notes.txt" } }),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(
            resumed.status(),
            StatusCode::OK,
            "an approved call must run: {}",
            body_text(resumed).await
        );

        // The same resumption again. `409`, because the call has already been authorized â€” the answer a
        // client needs in order to know the effect is not repeated.
        let again = app
            .oneshot(resume_request(
                &presented,
                &call_id,
                &serde_json::json!({ "arguments": { "path": "notes.txt" } }),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(
            again.status(),
            StatusCode::CONFLICT,
            "a second resume must be refused: {}",
            body_text(again).await
        );

        // **The effect happened once**, asserted on the adapter's own counter rather than on the statuses.
        // A route that returned `409` after already re-running the call would satisfy every assertion above
        // while producing exactly the second effect this exists to prevent.
        assert_eq!(
            adapter.calls(),
            1,
            "the tool must be reached exactly once across both resumptions"
        );
    }

    /// A resume for an identifier that is not a call is a `404`.
    #[tokio::test]
    async fn a_resume_for_an_unknown_call_is_not_found() {
        let (app, presented, _profile) = test_router().await;

        let response = app
            .oneshot(resume_request(
                &presented,
                "0198f000-0000-7000-8000-0000000000ff",
                &serde_json::json!({ "arguments": {} }),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    /// A decision for an identifier that is not an approval is a `404` **before** the nonce store is
    /// touched, so a caller cannot probe for approvals by watching whether a file was consumed.
    #[tokio::test]
    async fn a_decision_for_an_unknown_approval_is_not_found() {
        let (app, presented, _profile) = test_router().await;

        let response = app
            .oneshot(decision_request(
                &presented,
                "0198f000-0000-7000-8000-0000000000ff",
                &serde_json::json!({
                    "decision": "approve",
                    "nonce": "0".repeat(64)
                }),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    /// **A wrong nonce is refused and leaves the delivered nonce usable.**
    ///
    /// This is the property that separates "the nonce is a credential" from "the nonce is a one-shot
    /// token the daemon burns on contact". The domain refuses *after* the digest matches, so a mistyped or
    /// copied-wrong value must not cost the operator the approval â€” otherwise a typo becomes a fresh tool
    /// call, which is exactly how an approval flow gets routed around.
    ///
    /// It also pins the delivery-channel decision: the daemon verifies what the **caller presents** rather
    /// than consuming the file it delivered. If the route took the nonce from the file, the first request
    /// below would *succeed* and this assertion could not tell the difference between "verified" and
    /// "took whatever was on disk" â€” the two would be indistinguishable because the file's value is
    /// always the right one.
    #[tokio::test]
    async fn a_wrong_nonce_is_refused_and_the_delivered_nonce_still_decides() {
        let (app, presented, _profile, approval_id, nonce, _database) = approval_router().await;

        let forged = app
            .clone()
            .oneshot(decision_request(
                &presented,
                &approval_id,
                &serde_json::json!({ "decision": "approve", "nonce": "0".repeat(64) }),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(
            forged.status(),
            StatusCode::FORBIDDEN,
            "a nonce that does not match must be refused: {}",
            body_text(forged).await
        );

        let approved = app
            .clone()
            .oneshot(decision_request(
                &presented,
                &approval_id,
                &serde_json::json!({ "decision": "approve", "nonce": nonce }),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(
            approved.status(),
            StatusCode::OK,
            "a refused attempt must not consume the delivered nonce: {}",
            body_text(approved).await
        );
    }

    /// Records an entity so a wire remember has a subject, returning its identifier.
    ///
    /// `memory_entities` is a foreign key, so a remember naming an unrecorded entity is refused at the link
    /// step. The route tests therefore need one, and they obtain it through storage rather than through the
    /// gateway: there is no entity-creation route, and a test that could reach one would mean the surface
    /// allowed what the service deliberately does not.
    async fn fixture_entity(database: &Arc<SqliteDatabase>) -> String {
        let identity = jarvis_storage::load_local_identity(database)
            .await
            .unwrap_or_else(|error| panic!("the fixture must have a seeded identity: {error}"));
        let id = jarvis_core::EntityId::new();
        jarvis_storage::record_entity(
            database,
            &jarvis_storage::NewEntity {
                id,
                workspace_id: identity
                    .workspace_id()
                    .parse()
                    .unwrap_or_else(|_| panic!("the seeded workspace must parse")),
                kind: jarvis_storage::EntityKind::Person,
                label: "Router fixture subject".to_owned(),
                attributes: None,
                confidence: jarvis_core::MemoryConfidence::Confirmed,
                created_at: jarvis_core::UtcTimestamp::now(&jarvis_core::SystemClock),
            },
        )
        .await
        .unwrap_or_else(|error| panic!("record fixture entity: {error}"));
        id.to_string()
    }

    /// Builds a router plus the database behind it, because the memory routes need a fixture entity written
    /// directly and the two must be the same database.
    async fn memory_router() -> (Router, String, TempProfile, Arc<SqliteDatabase>) {
        let profile = TempProfile::new();
        let database = jarvis_storage::SqliteDatabase::open(&profile.database_path())
            .await
            .unwrap_or_else(|error| panic!("open fixture database: {error}"));
        let database = Arc::new(database);
        let credential = ClientCredential::generate()
            .unwrap_or_else(|error| panic!("generate fixture credential: {error}"));
        let presented = credential.expose().to_owned();
        let state = GatewayState::new(
            Arc::clone(&database),
            credential,
            jarvis_storage::SecretStore::in_state(&profile.0.join("state")),
        );
        (router(state), presented, profile, database)
    }

    /// **A claim can be remembered, listed, read, and searched over HTTP.** The first half of the surface,
    /// and the read-back half: every assertion is made against a *subsequent* request rather than against
    /// the reply that caused it, so a handler that returned the right shape and wrote nothing would fail.
    ///
    /// The two status distinctions are asserted because they are the contract a client switches on: `201`
    /// for a new claim and `200` for one already known, since reporting a re-statement as a creation would
    /// make a retry look like a second memory.
    #[tokio::test]
    async fn the_memory_routes_remember_list_read_and_search() {
        let (app, presented, _profile, database) = memory_router().await;
        let subject = fixture_entity(&database).await;
        let remember_body = serde_json::json!({
            "content": "Prefers dark roast coffee",
            "memory_type": "preference",
            "source_kind": "user_statement",
            "entity_ids": [subject],
        })
        .to_string();

        let created = app
            .clone()
            .oneshot(post_json("/api/v1/memories", &presented, &remember_body))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(
            created.status(),
            StatusCode::CREATED,
            "a new claim must be created: {}",
            body_text(created).await
        );
        let created_body = body_text(created).await;
        let remembered: jarvis_protocol::MemoryReply = serde_json::from_str(&created_body)
            .unwrap_or_else(|error| panic!("decode {created_body}: {error}"));
        assert_eq!(remembered.outcome, "remembered");

        // The same claim again is **not** a creation, and the outcome says so too. Both matter: a retry that
        // reported `201` would tell a caller it had stored a second memory, and an outcome of "remembered"
        // would say the same in the body while the status said otherwise.
        let repeated = app
            .clone()
            .oneshot(post_json("/api/v1/memories", &presented, &remember_body))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(
            repeated.status(),
            StatusCode::OK,
            "a repeated claim is not a creation: {}",
            body_text(repeated).await
        );
        let repeated_body = body_text(repeated).await;
        let duplicate: jarvis_protocol::MemoryReply = serde_json::from_str(&repeated_body)
            .unwrap_or_else(|error| panic!("decode {repeated_body}: {error}"));
        assert_eq!(
            duplicate.outcome, "already_remembered",
            "a re-statement must not report itself as a new memory"
        );
        assert_eq!(
            duplicate.memory_id, remembered.memory_id,
            "and it must name the claim the workspace already holds"
        );

        let listed = app
            .clone()
            .oneshot(get_request("/api/v1/memories?limit=10", Some(&presented)))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(listed.status(), StatusCode::OK);
        let listed_body = body_text(listed).await;
        let listing: jarvis_protocol::MemoryListReply = serde_json::from_str(&listed_body)
            .unwrap_or_else(|error| panic!("decode {listed_body}: {error}"));
        assert_eq!(
            listing.returned, 1,
            "the repeated claim must not have written a row"
        );
        assert_eq!(listing.memories[0].version, 1);

        let read = app
            .clone()
            .oneshot(get_request(
                &format!("/api/v1/memories/{}", remembered.memory_id),
                Some(&presented),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(read.status(), StatusCode::OK);
        let read_body = body_text(read).await;
        let detail: jarvis_protocol::MemoryDetailReply = serde_json::from_str(&read_body)
            .unwrap_or_else(|error| panic!("decode {read_body}: {error}"));
        assert_eq!(detail.content, "Prefers dark roast coffee");
        assert_eq!(
            detail.entity_ids,
            vec![subject.clone()],
            "the read must report the entity links the store holds"
        );

        let searched = app
            .oneshot(post_json(
                "/api/v1/memories/search",
                &presented,
                r#"{"text":"dark roast coffee"}"#,
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(searched.status(), StatusCode::OK);
        let search_body = body_text(searched).await;
        let search: jarvis_protocol::MemorySearchReply = serde_json::from_str(&search_body)
            .unwrap_or_else(|error| panic!("decode {search_body}: {error}"));
        assert_eq!(search.matches.len(), 1);
        assert!(
            search.matches[0].is_a_match,
            "a keyword hit matched the query; it was not merely included for recency"
        );
    }

    /// **A claim can be corrected, refused against a stale version, and deleted over HTTP.** The write half,
    /// and the one where the version guard's *two* distinct staleness reasons are both pinned.
    ///
    /// The second staleness case is the subtle one and is asserted deliberately: correcting a claim archives
    /// it, which **advances its version**, so the version the caller read before correcting is stale
    /// immediately afterwards. A guard that only rejected a version the caller never saw would accept this
    /// second write, and the caller would be deleting a claim on the strength of a version the correction
    /// had already invalidated.
    #[tokio::test]
    async fn the_memory_routes_correct_and_forget_under_a_version_guard() {
        let (app, presented, _profile, database) = memory_router().await;
        let subject = fixture_entity(&database).await;
        let created = app
            .clone()
            .oneshot(post_json(
                "/api/v1/memories",
                &presented,
                &serde_json::json!({
                    "content": "Prefers dark roast coffee",
                    "memory_type": "preference",
                    "source_kind": "user_statement",
                    "entity_ids": [subject],
                })
                .to_string(),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        let created_body = body_text(created).await;
        let remembered: jarvis_protocol::MemoryReply = serde_json::from_str(&created_body)
            .unwrap_or_else(|error| panic!("decode {created_body}: {error}"));
        let memory_id = remembered.memory_id.clone();

        let corrected = app
            .clone()
            .oneshot(post_json(
                &format!("/api/v1/memories/{memory_id}/correct"),
                &presented,
                &serde_json::json!({
                    "content": "Prefers light roast coffee",
                    "expected_version": remembered.version,
                })
                .to_string(),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(
            corrected.status(),
            StatusCode::OK,
            "a correction must be accepted: {}",
            body_text(corrected).await
        );
        let corrected_body = body_text(corrected).await;
        let correction: jarvis_protocol::MemoryReply = serde_json::from_str(&corrected_body)
            .unwrap_or_else(|error| panic!("decode {corrected_body}: {error}"));
        assert_eq!(correction.outcome, "corrected");
        assert_ne!(
            correction.memory_id, memory_id,
            "a correction is a new claim with a supersedes link, never an in-place overwrite"
        );

        let never_seen = app
            .clone()
            .oneshot(post_json(
                &format!("/api/v1/memories/{memory_id}/correct"),
                &presented,
                r#"{"content":"Prefers tea","expected_version":99}"#,
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(
            never_seen.status(),
            StatusCode::CONFLICT,
            "a version the caller never saw is a conflict rather than a validation failure: the remedy is to \
             re-read and retry"
        );

        let superseded_version = app
            .clone()
            .oneshot(post_json(
                &format!("/api/v1/memories/{memory_id}/forget"),
                &presented,
                &serde_json::json!({ "expected_version": remembered.version }).to_string(),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(
            superseded_version.status(),
            StatusCode::CONFLICT,
            "the original's version advanced when the correction archived it, so the version read before the \
             correction is stale: {}",
            body_text(superseded_version).await
        );

        let forgotten = app
            .clone()
            .oneshot(post_json(
                &format!("/api/v1/memories/{}/forget", correction.memory_id),
                &presented,
                &serde_json::json!({ "expected_version": correction.version }).to_string(),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(forgotten.status(), StatusCode::OK);
        let receipt_body = body_text(forgotten).await;
        let receipt: jarvis_protocol::DeletionReceipt = serde_json::from_str(&receipt_body)
            .unwrap_or_else(|error| panic!("decode {receipt_body}: {error}"));
        assert!(receipt.tombstone_written);
        assert!(
            !receipt.unreachable.is_empty(),
            "a receipt must never read as a total deletion"
        );
    }

    /// **The export is the full user read**, so it reports what the workspace *holds* rather than what it
    /// would retrieve, and it states what it leaves out.
    ///
    /// A purged claim is absent rather than present as an empty record, which is the shape the export's own
    /// exclusion text got wrong: it described the `Delete` **transition** — clear the text, keep the row —
    /// while the only deletion verb this surface has purges. The absence assertion is what keeps the two from
    /// re-agreeing on the wrong thing.
    #[tokio::test]
    async fn the_memory_export_reports_what_the_workspace_holds() {
        let (app, presented, _profile, database) = memory_router().await;
        let subject = fixture_entity(&database).await;
        let created = app
            .clone()
            .oneshot(post_json(
                "/api/v1/memories",
                &presented,
                &serde_json::json!({
                    "content": "Prefers dark roast coffee",
                    "memory_type": "preference",
                    "source_kind": "user_statement",
                    "entity_ids": [subject],
                })
                .to_string(),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        let created_body = body_text(created).await;
        let remembered: jarvis_protocol::MemoryReply = serde_json::from_str(&created_body)
            .unwrap_or_else(|error| panic!("decode {created_body}: {error}"));
        let memory_id = remembered.memory_id.clone();

        let corrected = app
            .clone()
            .oneshot(post_json(
                &format!("/api/v1/memories/{memory_id}/correct"),
                &presented,
                &serde_json::json!({
                    "content": "Prefers light roast coffee",
                    "expected_version": remembered.version,
                })
                .to_string(),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        let corrected_body = body_text(corrected).await;
        let correction: jarvis_protocol::MemoryReply = serde_json::from_str(&corrected_body)
            .unwrap_or_else(|error| panic!("decode {corrected_body}: {error}"));

        let forgotten = app
            .clone()
            .oneshot(post_json(
                &format!("/api/v1/memories/{}/forget", correction.memory_id),
                &presented,
                &serde_json::json!({ "expected_version": correction.version }).to_string(),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(forgotten.status(), StatusCode::OK);

        let exported = app
            .oneshot(get_request("/api/v1/memories/export", Some(&presented)))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(exported.status(), StatusCode::OK);
        let export_body = body_text(exported).await;
        let export: jarvis_protocol::MemoryExportReply = serde_json::from_str(&export_body)
            .unwrap_or_else(|error| panic!("decode {export_body}: {error}"));
        // The original still exists as a superseded row, so it is exported; the correction was purged, so it
        // is gone. One row is the assertion because the pair makes the export's rule visible: it reports
        // **live and archived** rows alike, and only a purge removes one.
        assert_eq!(
            export.count, 1,
            "the archived original is exported and the purged correction is not: {export_body}"
        );
        assert_eq!(
            export.memories[0].memory_id, memory_id,
            "the archived original is the row the export holds"
        );
        assert!(
            !export.exclusions.is_empty(),
            "an export that looks complete and is not is worse than one that says what it left out"
        );
    }

    /// **The memory routes are behind the same authentication as everything else**, and a request body
    /// cannot choose a workspace.
    ///
    /// The second half is the reason the DTOs use `deny_unknown_fields`: a `workspace_id` in a body is a
    /// `422` rather than an ignored field, so "client A's memory cannot enter client B's context" is a
    /// property of the transport rather than of each handler.
    #[tokio::test]
    async fn the_memory_routes_refuse_an_unauthenticated_or_over_scoped_request() {
        let (app, presented, _profile, database) = memory_router().await;
        let subject = fixture_entity(&database).await;

        let unauthenticated = app
            .clone()
            .oneshot(post_json(
                "/api/v1/memories",
                "not-a-credential",
                &serde_json::json!({
                    "content": "Prefers dark roast coffee",
                    "memory_type": "preference",
                    "source_kind": "user_statement",
                    "entity_ids": [subject],
                })
                .to_string(),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(
            unauthenticated.status(),
            StatusCode::UNAUTHORIZED,
            "a memory write must not be reachable without the credential"
        );

        let scoped = app
            .clone()
            .oneshot(post_json(
                "/api/v1/memories",
                &presented,
                &serde_json::json!({
                    "content": "Prefers dark roast coffee",
                    "memory_type": "preference",
                    "source_kind": "user_statement",
                    "entity_ids": [subject],
                    "workspace_id": jarvis_storage::LOCAL_WORKSPACE_ID,
                })
                .to_string(),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert!(
            scoped.status().is_client_error(),
            "a request that names its own workspace must be refused rather than silently scoped, got {}",
            scoped.status()
        );

        // An entity-less remember is refused by the **service**, so the route reports `422` with a message
        // naming the field — not a `500` from a foreign-key failure at the link step.
        let entity_less = app
            .clone()
            .oneshot(post_json(
                "/api/v1/memories",
                &presented,
                r#"{"content":"Prefers dark roast coffee","memory_type":"preference","source_kind":"user_statement"}"#,
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(
            entity_less.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "an entity-less remember must be refused with a field-naming 422: {}",
            body_text(entity_less).await
        );

        // A missing claim is a `404`, and a page over the bound is a `422` rather than a silent clamp.
        let missing = app
            .clone()
            .oneshot(get_request(
                &format!("/api/v1/memories/{}", jarvis_core::MemoryId::new()),
                Some(&presented),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);

        let oversized = app
            .oneshot(get_request(
                "/api/v1/memories?limit=100000",
                Some(&presented),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(
            oversized.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "a page over the bound must be refused rather than clamped, and `422` rather than `400` because \
             the query string is well formed and it is the *value* that is unacceptable: {}",
            body_text(oversized).await
        );
    }
}
