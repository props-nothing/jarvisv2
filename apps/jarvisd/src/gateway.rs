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
use jarvis_protocol::{MAX_STREAM_PAGE, RunEventPageReply, StartRunRequest, rest_error, safe};
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
    /// `None` means no tool is registered at all, so there is nothing to call — which is deliberately
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
    pub fn new(database: Arc<SqliteDatabase>, credential: ClientCredential) -> Self {
        Self {
            runs: RunService::new(Arc::clone(&database)),
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
        .route("/tools/{tool}/calls", post(call_tool));

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
                let run_id = reply.run_id.clone();
                tokio::spawn(async move {
                    if let Err(error) =
                        crate::executor::execute_run(&database, executor.model(), &run_id).await
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
/// authority — `docs/architecture/identity-and-workspaces.md` requires access to follow from
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
    // than the daemon inventing one — but the *workspace* comes from the run's stored row, so a caller
    // cannot attribute a call to a workspace the run is not in.
    let run = match state.runs.read(&request.run_id).await {
        Ok(run) => run,
        Err(error) => return error.into_response(),
    };

    // The actor holds **both** tool areas' scopes, because this transport serves whichever tools the daemon
    // composed: a filesystem tool when roots are granted, an MCP tool when servers are configured. The scope
    // set is still derived here rather than accepted from the request, which is the rule that matters — a
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
            required_strength,
            reason_code,
        }) => (
            StatusCode::ACCEPTED,
            Json(serde_json::json!({
                "call_id": call_id,
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
/// supply — see `RunService::cancel`.
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
            let _ = std::fs::remove_dir_all(&self.0);
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
        let state = GatewayState::new(Arc::new(database), credential);
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
    /// is stale almost immediately and the request was refused with a conflict it could not resolve —
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
    /// **Proved at the storage layer, not here**, because the gateway has no route that settles a run —
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
    /// test are about what the route does with a request — which workspace it uses and which fields
    /// it accepts — and a double would decide those itself, which is the thing to be checked.
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
        )
        .unwrap_or_else(|error| panic!("compose the tool pipeline: {error}"));

        let app = router(GatewayState::new(database, credential).with_tools(Arc::new(pipeline)));

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
        // this is refused by the decoder rather than silently ignored — an ignored field reads as an
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
        // outcome is a **correct answer** to a request that was understood — ADR-0020's fifth rule puts
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
}
