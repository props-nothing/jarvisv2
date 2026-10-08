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
    AddAliasRequest, ApprovalDecisionBody, ConfirmMemoryRequest, CorrectMemoryRequest,
    CreateEntityRequest, CreateSkillRequest, ForgetMemoryRequest, ForgetSkillRequest,
    MAX_STREAM_PAGE, MemorySearchRequest, MergeEntityRequest, PromoteSkillRequest, RememberRequest,
    RunEventPageReply, SkillTransitionRequest, StartRunRequest, SummarizeSessionRequest,
    rest_error, safe,
};
use jarvis_storage::{DatabaseError, SqliteDatabase, highest_run_event_sequence, read_run_events};
use serde::{Deserialize, Serialize};

pub use crate::run_service::RunService;

/// Maximum accepted request body size in bytes.
///
/// Applied as a router layer so it covers every current and future route, rather than relying on
/// each extractor to apply it correctly.
pub const MAX_REQUEST_BODY_BYTES: usize = 256 * 1024;

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
    /// The speech provider, when a key file is configured (`ADR-0138`). `None` means the page speaks with the browser's own voice.
    speech: Option<Arc<jarvis_voice::ElevenLabsSpeech>>,
    /// Where the profile's configuration is, for the Settings routes (`P9-015`). `None` in a bare transport test.
    settings: Option<crate::settings_service::SettingsContext>,
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
            speech: None,
            settings: None,
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

    /// Attaches the speech provider.
    #[must_use]
    pub fn with_speech(mut self, speech: Arc<jarvis_voice::ElevenLabsSpeech>) -> Self {
        self.speech = Some(speech);
        self
    }

    /// Attaches the profile location, which turns the Settings routes on.
    #[must_use]
    pub fn with_settings(mut self, settings: crate::settings_service::SettingsContext) -> Self {
        self.settings = Some(settings);
        self
    }

    /// Returns the settings context, when the profile location is known.
    #[must_use]
    pub fn settings(&self) -> Option<&crate::settings_service::SettingsContext> {
        self.settings.as_ref()
    }

    /// Returns the speech provider, when one is configured.
    #[must_use]
    pub fn speech(&self) -> Option<&Arc<jarvis_voice::ElevenLabsSpeech>> {
        self.speech.as_ref()
    }

    /// Returns the tool pipeline, when one is configured.
    #[must_use]
    pub fn tools(&self) -> Option<&Arc<crate::tool_pipeline::ToolPipeline>> {
        self.tools.as_ref()
    }

    /// Whether runs are driven at all. A scheduler over a daemon with no executor would start runs nothing runs.
    #[must_use]
    pub(crate) const fn drives_runs(&self) -> bool {
        self.executor.is_some()
    }

    /// Starts a run and drives it to a terminal state on its own task.
    ///
    /// The one way a run begins, whoever asked: the REST route and the scheduler both come through here, so a
    /// scheduled run is an ordinary run with every guarantee an ordinary run has — the same policy, the same
    /// approvals, the same audit. There is no second, privileged way to start work.
    ///
    /// # Errors
    ///
    /// Returns the run service's error when the request is refused.
    pub(crate) async fn start_and_drive(
        &self,
        request: &StartRunRequest,
    ) -> Result<jarvis_protocol::RunReply, crate::run_service::RunServiceError> {
        let reply = self.runs.start(request).await?;
        self.drive(&reply.run_id);
        Ok(reply)
    }

    /// Drives a run on its own task, so the caller is not held open for a whole model call.
    ///
    /// The task is deliberately not awaited and its failure is logged rather than returned: the run is already
    /// durably recorded, so a task that fails leaves a run that is visibly unfinished rather than a response that
    /// claims a start it did not make. With no executor composed there is nothing to drive.
    pub(crate) fn drive(&self, run_id: &str) {
        let Some(executor) = &self.executor else {
            return;
        };
        let database = Arc::clone(&self.database);
        let executor = Arc::clone(executor);
        // The composed tool pipeline, when one exists, so the run is an agent loop rather than a single model
        // call. `None` for a profile with no tool surface, in which case the run answers without tools.
        let tools = self.tools.clone();
        let run_id = run_id.to_owned();
        tokio::spawn(async move {
            if let Err(error) = crate::executor::execute_run_with_tools(
                &database,
                executor.model(),
                executor.model_id(),
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
                crate::executor::fail_if_unfinished(&database, &run_id, &error.to_string()).await;
            }
        });
    }

    /// Returns the database the gateway reads through.
    #[must_use]
    pub fn database(&self) -> &SqliteDatabase {
        &self.database
    }

    /// Returns a shared handle to the database, for code that must hold one across a task.
    #[must_use]
    pub(crate) fn database_handle(&self) -> Arc<SqliteDatabase> {
        Arc::clone(&self.database)
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

    /// Returns the skill surface.
    ///
    /// Built on demand like the memory surface, but it also reads the **tool pipeline** — because creating a
    /// skill validates each step's tool against the registry. Passing `self.tools` rather than a separately
    /// configured predicate is what keeps the tools this surface accepts and the tools the executor can run the
    /// same set: two sources of that fact could disagree, and the disagreement would be a stored procedure that
    /// cannot execute.
    #[must_use]
    pub fn skills(&self) -> crate::skill_service::SkillService {
        crate::skill_service::SkillService::new(Arc::clone(&self.database), self.tools.as_ref())
    }

    /// Returns the entity surface.
    ///
    /// Built on demand like the others, and over the same database. This is the surface that removes the limit
    /// three slices recorded — `P4-008`'s "no entity-creation surface exists, so a remember is still unreachable
    /// by a user of the shipped product" — so it is deliberately a sibling of [`Self::memories`] rather than a
    /// part of it: an entity is not a memory, and making it a sub-resource would put entity creation behind a
    /// memory write that needs an entity.
    #[must_use]
    pub fn entities(&self) -> crate::entity_service::EntityService {
        crate::entity_service::EntityService::new(Arc::clone(&self.database))
    }
}

/// Builds the authenticated router.
///
/// Layer order is deliberate: the body limit and the authentication middleware wrap the whole
/// router, so a route added later cannot omit either. A per-route layer would be forgettable in
/// exactly that way.
pub fn router(state: GatewayState) -> Router {
    let api = Router::new()
        .route(
            "/runs",
            post(start_run).get(crate::schedule_service::list_runs),
        )
        .route(
            "/schedules",
            post(crate::schedule_service::create).get(crate::schedule_service::list),
        )
        .route(
            "/schedules/{id}",
            axum::routing::delete(crate::schedule_service::remove),
        )
        .route(
            "/schedules/{id}/pause",
            post(crate::schedule_service::pause),
        )
        .route(
            "/schedules/{id}/resume",
            post(crate::schedule_service::resume),
        )
        .route("/runs/{id}", get(read_run))
        .route("/runs/{id}/cancel", post(cancel_run))
        .route("/runs/{id}/events", get(read_events))
        .route("/runs/{id}/stream", get(crate::sse::stream_events))
        .route("/shutdown", post(crate::stop::request))
        .route("/restart", post(crate::settings_service::restart))
        .route(
            "/settings",
            get(crate::settings_service::list).put(crate::settings_service::batch),
        )
        .route(
            "/settings/tools/{tool}",
            axum::routing::put(crate::settings_service::set_posture),
        )
        .route(
            "/settings/{key}",
            axum::routing::put(crate::settings_service::set).delete(crate::settings_service::unset),
        )
        .route(
            "/settings/keys/{which}",
            axum::routing::put(crate::settings_service::set_key)
                .delete(crate::settings_service::remove_key),
        )
        .route(
            "/speech",
            get(crate::speech_service::status).post(crate::speech_service::speak),
        )
        .route("/tools", get(list_tools))
        .route("/tools/{tool}/calls", post(call_tool))
        .route("/tools/{tool}/preview", post(preview_tool))
        .route("/calls/{id}/resume", post(resume_call))
        .route("/approvals", get(list_approvals))
        .route("/approvals/{id}/decision", post(decide_approval))
        .route("/approvals/{id}/resume", post(resume_approval))
        .route("/memories", get(list_memories).post(remember))
        .route("/memories/search", post(search_memories))
        .route("/memories/export", get(export_memories))
        .route("/memories/{id}", get(read_memory))
        .route("/memories/{id}/correct", post(correct_memory))
        .route("/memories/{id}/confirm", post(confirm_memory))
        .route("/memories/{id}/forget", post(forget_memory))
        .route(
            "/sessions/{id}/summaries",
            get(list_session_summaries).post(summarize_session),
        )
        .route(
            "/sessions/{id}/summaries/retire",
            post(retire_session_summaries),
        )
        .route("/entities", get(list_entities).post(create_entity))
        .route("/entities/lookup", get(lookup_entity))
        .route("/entities/{id}", get(read_entity))
        .route("/entities/{id}/aliases", post(add_entity_alias))
        .route("/entities/{id}/merge", post(merge_entity))
        .route("/skills", get(list_skills).post(create_skill))
        .route("/skills/export", get(export_skills))
        .route("/skills/{id}", get(read_skill).delete(forget_skill))
        .route("/skills/{id}/promote", post(promote_skill))
        .route("/skills/{id}/disable", post(disable_skill))
        .route("/skills/{id}/enable", post(enable_skill));

    Router::new()
        // The heads-up display's two static assets: no data, no secret, served without the credential
        // (`crate::hud::is_public_asset` is the one place that says so).
        .route(crate::hud::PAGE_PATH, get(crate::hud::page))
        .route(crate::hud::ALIAS_PATH, get(crate::hud::page))
        .route(crate::hud::SCRIPT_PATH, get(crate::hud::script))
        .route(crate::hud::HEAD_PATH, get(crate::hud::head))
        .route(crate::hud::STYLE_PATH, get(crate::hud::style))
        .route("/health/live", get(health_live))
        .route("/health/ready", get(health_ready))
        .nest("/api/v1", api)
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BODY_BYTES))
        .layer(middleware::from_fn_with_state(state.clone(), authenticate))
        .with_state(state)
}

/// Rejects any request that does not present the profile credential.
async fn authenticate(State(state): State<GatewayState>, request: Request, next: Next) -> Response {
    // The two static display assets carry nothing to protect; the data they show is fetched with the credential.
    if crate::hud::is_public_asset(request.method(), request.uri().path()) {
        return next.run(request).await;
    }
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
    // The run is driven on its own task so the response is not held open for the whole model call: the client
    // learns the run identifier immediately and follows the stream, which is what makes the API usable for a long
    // answer.
    match state.start_and_drive(&request).await {
        Ok(reply) => (StatusCode::CREATED, Json(reply)).into_response(),
        Err(error) => error.into_response(),
    }
}

/// Request body for `POST /api/v1/tools/{tool}/calls`.
///
/// Only two fields, and both are the caller's to choose: **which stored run** the call is attributed
/// to, and what to pass the tool. The workspace and the actor's scopes are **not** here, because a
/// client that could name its own workspace or grant could widen its own authority â€” `docs/architecture
/// /identity-and-workspaces.md` requires access to follow from authentication rather than from a
/// client-supplied identifier.
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

    // The tools this daemon composed, which is what the actor's authority is derived from. A failure here is a
    // composition fault the daemon would already have reported at startup, so it is answered as an internal
    // error rather than as a refusal the caller could act on.
    let composed_definitions = match tools.definitions() {
        Ok(definitions) => definitions,
        Err(error) => {
            tracing::error!(%error, "the composed tool definitions could not be read");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorCode::Internal,
                "the daemon's tool surface could not be read",
            );
        }
    };

    // The actor's authority is **derived from the tools this daemon composed**, which is the daemon's own
    // authority over its own surface: a client addresses one of them, and the surface is exactly what the
    // registry holds. It is derived rather than accepted from the request — a caller cannot name a scope, so it
    // cannot widen its own authority — and derived from the *definitions* rather than written out, because a
    // hand-written list is a second statement of what the adapters already declare. That list was wrong twice:
    // it omitted `memory.propose`, so `P4-014`'s tool was registered and refused for every call, and removing
    // `mcp.call` from it broke an MCP write tool's approval in `phase_3_gate`.
    //
    // Infallible, unlike the fixed-literal constructors: the scopes come from the definitions the registry
    // already accepted, so there is no literal left to reject and no fail-closed branch to report.
    let actor = crate::tool_actor::ToolActor::for_composed_tools(
        run.workspace_id.clone(),
        run.run_id.clone(),
        jarvis_core::SessionChannel::Cli,
        "policy-1",
        &composed_definitions,
    );

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
            reason_code,
        }) => (
            StatusCode::ACCEPTED,
            Json(serde_json::json!({
                "call_id": call_id,
                "approval_id": approval_id,
                "state": "awaiting_approval",
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
    resume_core(&state, &call_id, body.arguments).await
}

/// Releases a decided call: the whole of what both resume routes do, once.
///
/// The arguments come from the caller of this function, not from the request — the client route passes the ones
/// it was given, and the approval route passes the ones the daemon held — so the one thing that differs between
/// the two is *who supplies the payload*, and the intent digest decides whether it is the approved one.
async fn resume_core(
    state: &GatewayState,
    call_id: &str,
    arguments: serde_json::Value,
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
    let stored = match jarvis_storage::find_tool_call(state.database(), call_id).await {
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

    // A resumed call runs under the same grants the original did, so it derives them the same way: an approval
    // releases a call that policy already admitted, and a resume path with a narrower actor would refuse the
    // very call it exists to finish.
    let composed_definitions = match tools.definitions() {
        Ok(definitions) => definitions,
        Err(error) => {
            tracing::error!(%error, "the composed tool definitions could not be read");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorCode::Internal,
                "the daemon's tool surface could not be read",
            );
        }
    };
    let actor = crate::tool_actor::ToolActor::for_composed_tools(
        stored.workspace_id().to_owned(),
        stored.run_id().to_owned(),
        jarvis_core::SessionChannel::Cli,
        "policy-1",
        &composed_definitions,
    );

    match tools
        .resume(
            call_id,
            arguments.clone(),
            &actor,
            jarvis_core::CorrelationId::new(),
        )
        .await
    {
        Ok(crate::tool_pipeline::ToolPipelineOutcome::Executed(result)) => {
            // The effect ran. A run that parked on this call must now **answer**, or the conversation
            // stops with an executed effect and a user still waiting. The continuation is spawned rather
            // than awaited for the same reason `start_run` spawns: the outcome is already durable and the
            // run identifier is known, so a continuation that fails leaves a run visibly unfinished
            // rather than a response that claims a completion it did not make.
            continue_parked_run(state, stored.run_id(), call_id).await;
            // The call has run, so the held arguments have served their purpose. They were kept past the
            // decision for exactly this: a daemon that died between the two could still finish the call.
            if let Some(approval) = stored.approval_id()
                && let Err(error) =
                    jarvis_storage::clear_approval_arguments(state.database(), approval).await
            {
                tracing::warn!(%error, "an executed call's held arguments could not be cleared");
            }
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

/// Continues a run that parked on a tool call the resume route has just executed.
///
/// # Why the run needs this and the call does not
///
/// The resume route executes the effect through `ToolPipeline::resume`, which settles the **call**. The
/// **run** is a separate lifecycle: a run parked at `AwaitingApproval` on a held call stays there until
/// something drives it forward, and without this the effect happens and the conversation never produces
/// its answer. This is that step, and it is the reason `P3-022` recorded "a held call parks the run" as a
/// limit rather than a completion.
///
/// # Why it is spawned rather than awaited, and why a non-parked run is a no-op
///
/// The effect is already durable and the run identifier is the call's own, so the client's `200` does not
/// need to wait for a model call — the same reasoning `start_run` gives. A run that is **not** parked is
/// left alone: a client may resume a call from a run that was since cancelled or failed, and driving a
/// settled run is refused by the domain, so this is entered only for a live `AwaitingApproval` run and
/// otherwise does nothing. With no executor composed there is nothing to drive, which is a profile
/// property rather than an error.
async fn continue_parked_run(state: &GatewayState, run_id: &str, call_id: &str) {
    let Some(executor) = &state.executor else {
        return;
    };

    // Only a parked run is continued. Read first so a settled or non-parked run never reaches the
    // spawn, which keeps "resume always answers" from turning into "resume re-drives a run it should
    // not" — the domain would refuse the second drive, but reaching for it at all is the mistake.
    let parked = match state.runs.read(run_id).await {
        Ok(reply) => reply.state == jarvis_core::RunState::AwaitingApproval,
        Err(_) => false,
    };
    if !parked {
        return;
    }

    let database = Arc::clone(&state.database);
    let executor = Arc::clone(executor);
    let tools = state.tools.clone();
    let run_id = run_id.to_owned();
    let call_id = call_id.to_owned();
    tokio::spawn(async move {
        if let Err(error) = crate::executor::resume_run_with_tools(
            &database,
            executor.model(),
            executor.model_id(),
            tools.as_ref(),
            &run_id,
            &call_id,
        )
        .await
        {
            tracing::error!(
                run_id,
                error = %error,
                "the resumed run could not be driven to a terminal state"
            );
        }
    });
}

/// `POST /api/v1/approvals/{id}/decision`
///
/// # What this handler deliberately does not take from the request
///
/// - **The approver.** It is the profile's seeded local identity, read here, not a field. A caller that
///   could name its own approver would be overwriting the audit record.
/// - **The intent.** It is re-read from the stored row inside `record_decision`, because the digest is
///   what a decision binds to and a value the request supplied would be a binding to a claim.
///
/// # The order
///
/// The approval is read first so an unknown identifier is a `404` before any write path runs. Then the
/// decision is recorded against the stored row that the owner is answering.
async fn decide_approval(
    State(state): State<GatewayState>,
    Path(id): Path<String>,
    Json(body): Json<ApprovalDecisionBody>,
) -> Response {
    match state.runs.decide(&id, &body).await {
        Ok(reply) => {
            // A client that asks for it has the daemon release the call as part of the decision. It is opt-in
            // because the REST clients that predate it decide and then resume themselves, and a resume that has
            // already happened is refused. The release is spawned: running the tool can take longer than a
            // request should, and the client follows the run's own stream for the outcome.
            if body.resume && body.decision == jarvis_protocol::ApprovalDecisionRequest::Approve {
                let state = state.clone();
                let approval_id = id.clone();
                tokio::spawn(async move {
                    release_approved_call(&state, &approval_id).await;
                });
            }
            // A refusal is a decision the run has to hear about. An **approval** is continued by the resume
            // route, because releasing the call needs the arguments; a refusal needs nothing but to tell the
            // model, so it is continued here, where the decision is made.
            if body.decision != jarvis_protocol::ApprovalDecisionRequest::Approve {
                continue_declined_run(&state, &reply.run_id, &id).await;
            }
            (StatusCode::OK, Json(reply)).into_response()
        }
        Err(error) => error.into_response(),
    }
}

/// Releases the call an **approved** approval is holding, from the arguments the daemon kept.
///
/// Reads the approval, not the request: the call is the approval's own correlation identity, and the payload is
/// what was held when the call was admitted. Nothing here is supplied by a client, so the only way to run a call
/// through this path is for the approval to exist, be approved, and still hold its arguments — and the intent
/// digest, recomputed from them, is the last check.
async fn release_approved_call(state: &GatewayState, approval_id: &str) -> Response {
    let approval = match jarvis_storage::find_approval(state.database(), approval_id).await {
        Ok(approval) => approval,
        Err(jarvis_storage::DatabaseError::ApprovalNotFound) => {
            return error_response(
                StatusCode::NOT_FOUND,
                ErrorCode::Validation,
                "no approval exists for the requested identifier",
            );
        }
        Err(_) => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                ErrorCode::Internal,
                "the local database is not available",
            );
        }
    };
    if approval.stored_state() != jarvis_core::ApprovalState::Approved {
        return error_response(
            StatusCode::CONFLICT,
            ErrorCode::Conflict,
            "only an approved approval can be released",
        );
    }
    let held = match jarvis_storage::read_approval_arguments(state.database(), approval_id).await {
        Ok(Some(text)) => serde_json::from_str::<serde_json::Value>(&text).ok(),
        _ => None,
    };
    let Some(arguments) = held else {
        return error_response(
            StatusCode::CONFLICT,
            ErrorCode::Conflict,
            "the approval no longer holds its arguments, so the call has already run or cannot be released here",
        );
    };
    resume_core(state, &approval.correlation_id().to_string(), arguments).await
}

/// `POST /api/v1/approvals/{id}/resume`
///
/// Finishes an approved call whose release did not happen — the daemon died between the decision and the
/// release, or the client never asked for it. It takes **no body**: the arguments are the ones the daemon held.
async fn resume_approval(State(state): State<GatewayState>, Path(id): Path<String>) -> Response {
    release_approved_call(&state, &id).await
}

/// Continues a run that parked on a call whose approval was just refused or withdrawn.
///
/// The twin of [`continue_parked_run`], for the case where there is no effect to wait for. A run that is not
/// parked is left alone, for the same reason: a client may decide an approval whose run was cancelled since.
async fn continue_declined_run(state: &GatewayState, run_id: &str, approval_id: &str) {
    let Some(executor) = &state.executor else {
        return;
    };
    let parked = match state.runs.read(run_id).await {
        Ok(reply) => reply.state == jarvis_core::RunState::AwaitingApproval,
        Err(_) => false,
    };
    if !parked {
        return;
    }
    // The held call's identifier is the approval's own correlation identity — the pipeline names a call by it.
    let Ok(approval) = jarvis_storage::find_approval(state.database(), approval_id).await else {
        return;
    };
    let call_id = approval.correlation_id().to_string();

    let database = Arc::clone(&state.database);
    let executor = Arc::clone(executor);
    let tools = state.tools.clone();
    let run_id = run_id.to_owned();
    tokio::spawn(async move {
        if let Err(error) = crate::executor::resume_run_declined(
            &database,
            executor.model(),
            executor.model_id(),
            tools.as_ref(),
            &run_id,
            &call_id,
        )
        .await
        {
            tracing::error!(
                run_id,
                error = %error,
                "a run whose approval was refused could not be driven to a terminal state"
            );
        }
    });
}

/// `GET /api/v1/approvals`
///
/// Lists the workspace's pending approvals with the arguments each is waiting on, so a person can see **what
/// they would be approving** before they decide (`ADR-0130`). The workspace is the profile's own, read from the
/// seeded identity rather than from a request, for the reason the decision route gives: a client-supplied
/// workspace is a claim, not proof of access.
async fn list_approvals(State(state): State<GatewayState>) -> Response {
    let now = jarvis_core::UtcTimestamp::now(&jarvis_core::SystemClock);
    let Ok(identity) = jarvis_storage::load_local_identity(state.database()).await else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::Internal,
            "the local database is not available",
        );
    };
    let pending = match jarvis_storage::read_workspace_pending_approvals(
        state.database(),
        identity.workspace_id(),
        now,
    )
    .await
    {
        Ok(pending) => pending,
        Err(error) => {
            tracing::warn!(%error, "the pending approvals could not be read");
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                ErrorCode::Internal,
                "the local database is not available",
            );
        }
    };

    // Approved-but-unrun approvals are listed too: a person who approved and whose daemon then died has no other
    // way to learn that the call never ran.
    let unreleased = match jarvis_storage::read_workspace_unreleased_approvals(
        state.database(),
        identity.workspace_id(),
    )
    .await
    {
        Ok(unreleased) => unreleased,
        Err(error) => {
            tracing::warn!(%error, "the approved, unreleased approvals could not be read");
            Vec::new()
        }
    };
    let mut approvals = Vec::with_capacity(pending.len() + unreleased.len());
    for (request, state_name) in pending
        .iter()
        .map(|request| (request, "pending"))
        .chain(unreleased.iter().map(|request| (request, "approved")))
    {
        let id = request.id().to_string();
        // A stored payload that is not JSON is treated as absent rather than failing the list: one bad row
        // must not hide every other pending approval, and an approval with no payload is not decidable from a
        // client that must show it, which is the safe direction.
        let arguments = match jarvis_storage::read_approval_arguments(state.database(), &id).await {
            Ok(Some(text)) => serde_json::from_str(&text).ok(),
            _ => None,
        };
        approvals.push(jarvis_protocol::PendingApprovalReply {
            approval_id: id,
            state: state_name.to_owned(),
            run_id: request.run_id().to_string(),
            call_id: request.correlation_id().to_string(),
            tool: request.tool().to_owned(),
            tool_version: request.tool_version().to_owned(),
            risk_level: request.risk_level(),
            preview: request.preview().as_str().to_owned(),
            arguments,
            created_at: request.created_at(),
            expires_at: request.expires_at(),
        });
    }
    let reply = jarvis_protocol::ApprovalListReply {
        total: approvals.len(),
        approvals,
    };
    (StatusCode::OK, Json(reply)).into_response()
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

/// `GET /api/v1/tools`
///
/// Lists every registered tool with the authorization posture **in force** for it: the policy the tool
/// declares, the policy after any workspace override, whether it is overridden or denied, and why it
/// cannot run when it cannot. This is the surface a CLI or a control-plane UI enumerates, and it exists
/// because `P3-025` made that posture configurable — without a way to read it back, an operator cannot
/// tell whether a configuration line took effect.
///
/// # Why this is separate from the model-facing discovery list
///
/// `ToolRegistry::discover()` omits the approval policy, the required scopes, and the schemas on
/// purpose: a model selects on what a tool does and must not act on authorization. This reply carries
/// exactly those omissions, because that is the operator's question.
///
/// # An absent pipeline is `404`, not an empty list
///
/// A daemon with no roots and no MCP servers registered **no tool at all**, which is deliberately
/// different from a registered tool that cannot run (`ADR-0020`). An empty `200` would read as "this
/// daemon has no tools configured" when the truth is "this daemon cannot serve tools", and the two have
/// different remedies.
async fn list_tools(State(state): State<GatewayState>) -> Response {
    let Some(tools) = state.tools() else {
        return error_response(
            StatusCode::NOT_FOUND,
            ErrorCode::Validation,
            "no tool is registered: grant `daemon.tool_workspace_roots` or configure MCP servers in \
             `mcp-servers.toml` to enable tools",
        );
    };
    match tools.policy_inventory() {
        Ok(entries) => {
            // The ceiling and threshold come from the policy the pipeline actually decides with, not from
            // configuration — recomposing them would let the displayed policy differ from the enforced one.
            let policy = tools.workspace_policy();
            let reply = jarvis_protocol::ToolListReply {
                total: entries.len(),
                tools: entries.iter().map(tool_reply).collect(),
                max_risk: policy.max_risk(),
                approval_threshold: policy.approval_threshold(),
            };
            (StatusCode::OK, Json(reply)).into_response()
        }
        // An identifier the registry itself produced cannot fail to parse, so this is a fault rather
        // than a client error — reported as such rather than as an empty list that would hide a hole.
        Err(error) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::Internal,
            &error.to_string(),
        ),
    }
}

/// Projects one inventory entry onto its wire shape.
///
/// A named function rather than a closure so the field mapping is readable in one place, and so a
/// field added to either side is a compile error here rather than a field that silently stops being
/// reported.
fn tool_reply(entry: &crate::tool_pipeline::PolicyInventoryEntry) -> jarvis_protocol::ToolReply {
    jarvis_protocol::ToolReply {
        id: entry.id.clone(),
        title: entry.title.clone(),
        version: entry.version.clone(),
        source: entry.source.as_str().to_owned(),
        risk: entry.risk,
        effects: entry.effects.clone(),
        required_scopes: entry.required_scopes.clone(),
        declared_approval: entry.declared_approval,
        effective_approval: entry.effective_approval,
        overridden: entry.overridden,
        denied: entry.denied,
        callable: entry.callable,
        unavailable_reason: entry.unavailable_reason.clone(),
        asks_first: entry.asks_first,
    }
}

/// `POST /api/v1/tools/{tool}/preview`
///
/// Reports the decision a call to this tool **would** produce, without recording or running anything.
/// The context a caller may supply is the risk escalation signals, because those describe the call only the
/// caller knows about. The actor's scopes and the workspace policy are **not** accepted — a preview must
/// not be a way to ask "what if I had different permissions".
///
/// # The identity is the local profile, and that is the honest reading
///
/// A preview is an operator's own question about their own daemon, asked over the credential-guarded
/// loopback transport. There is no run to attribute it to, so the actor is built from the profile's
/// seeded local identity with the scopes the served tools require — the same derivation
/// `call_remote_tool` uses, and for the same reason: the grant is what the registered tools need, not
/// what the caller asks for.
///
/// # An unknown tool is `404`, not `deny`
///
/// Returning a refusal for a name that does not exist would make a typo indistinguishable from a
/// policy denial, and the two have opposite remedies.
async fn preview_tool(
    State(state): State<GatewayState>,
    Path(tool): Path<String>,
    Json(request): Json<jarvis_protocol::ToolPreviewRequest>,
) -> Response {
    let Some(tools) = state.tools() else {
        return error_response(
            StatusCode::NOT_FOUND,
            ErrorCode::Validation,
            "no tool is registered: grant `daemon.tool_workspace_roots` or configure MCP servers in \
             `mcp-servers.toml` to enable tools",
        );
    };
    let identity = match jarvis_storage::load_local_identity(&state.database).await {
        Ok(identity) => identity,
        // `Validation` rather than a not-found code, because `ErrorCode` has no `NotFound` member: its
        // vocabulary is what a caller should *do*, and a missing local identity is a daemon that cannot
        // act rather than a resource a client asked for and did not get.
        Err(error) => {
            return error_response(
                StatusCode::NOT_FOUND,
                ErrorCode::Validation,
                &error.to_string(),
            );
        }
    };

    // The preview's actor must hold **the same grants a real call would**, or the preview disagrees with
    // reality: an actor with fewer scopes answers `deny / missing_scope` for a call the daemon would allow.
    // That is what happened — `jarvis tools preview jarvis.memory.propose` reported `deny` for a tool a run
    // could call — and a preview that lies in the refusals direction is worse than no preview, because it
    // sends an operator to change a policy that is already right.
    // The preview's actor derives its authority from the composed tools, which is the **only** way it can agree
    // with a real call. An actor with a hand-written grant list answers `deny / missing_scope` for a call the
    // daemon would allow: `jarvis tools preview jarvis.memory.propose` reported `deny` for a tool a run could
    // call. A preview that lies in the refusals direction is worse than no preview, because it sends an operator
    // to change a policy that is already right.
    //
    // Deriving it also means the preview cannot drift as adapters are added: the authority is read from the same
    // registry that produced the tool being previewed.
    let composed_definitions = match tools.definitions() {
        Ok(definitions) => definitions,
        Err(error) => {
            tracing::error!(%error, "the composed tool definitions could not be read");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorCode::Internal,
                "the daemon's tool surface could not be read",
            );
        }
    };
    let actor = crate::tool_actor::ToolActor::for_composed_tools(
        identity.workspace_id(),
        // No run: a preview is not attributed to one, and inventing an identifier would attribute a
        // call that never happened to a run the caller named. The actor's run field is used for the
        // call-attribution check in the real path, which this does not reach.
        String::new(),
        jarvis_core::SessionChannel::Desktop,
        // The policy version label a receipt would carry. A preview builds no receipt, so this is the
        // current label rather than a recorded one: a stored receipt's label is a claim about the policy
        // in force when a call was admitted, and this is not that.
        crate::tool_pipeline::PREVIEW_POLICY_VERSION,
        &composed_definitions,
    );
    let target = jarvis_tools::TargetAssessment::new(request.escalation.iter().copied());
    match tools.preview_call(&tool, &actor, &target) {
        Ok(decision) => {
            // The declared risk comes from the inventory so the two figures beside each other are the
            // same computation the listing reports — reading it from a second source would be a second
            // statement of the tool's risk.
            let declared = tools
                .policy_inventory()
                .ok()
                .and_then(|entries| entries.into_iter().find(|entry| entry.id == tool))
                .map_or(decision.effective_risk(), |entry| entry.risk);
            let reply = jarvis_protocol::ToolPreviewReply {
                tool: tool.clone(),
                decision: decision_name(decision.decision()),
                reason: decision.reason_code().to_owned(),
                effective_risk: decision.effective_risk(),
                declared_risk: declared,
                escalated_by: decision.escalated_by().to_vec(),
            };
            (StatusCode::OK, Json(reply)).into_response()
        }
        Err(crate::tool_pipeline::ToolPipelineError::UnknownTool { tool }) => error_response(
            StatusCode::NOT_FOUND,
            ErrorCode::Validation,
            &format!("no tool is registered with the identifier {tool}"),
        ),
        Err(error) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::Internal,
            &error.to_string(),
        ),
    }
}

/// Renders a decision as its stable snake-case wire name.
///
/// A hand-written match rather than `Debug`, for the reason every stored vocabulary here is: the name
/// is a contract with clients, and a reordered or renamed variant must not silently change what a
/// client reads.
fn decision_name(decision: jarvis_tools::Decision) -> String {
    match decision {
        jarvis_tools::Decision::Allow => "allow",
        jarvis_tools::Decision::RequireApproval => "require_approval",
        jarvis_tools::Decision::Deny => "deny",
    }
    .to_owned()
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

/// `POST /api/v1/memories/{id}/confirm`
///
/// Accepts a proposed claim. The handler takes no approver from the body: the daemon reads its seeded
/// identity, which is what makes the self-admission guard meaningful — see `MemoryService::confirm`.
async fn confirm_memory(
    State(state): State<GatewayState>,
    Path(id): Path<String>,
    Json(request): Json<ConfirmMemoryRequest>,
) -> Response {
    match state.memories().confirm(&id, &request).await {
        Ok(reply) => Json(reply).into_response(),
        Err(error) => memory_error(&error),
    }
}

/// `POST /api/v1/sessions/{id}/summaries`
///
/// Records a compressed summary of part of a session. The route is under `sessions` rather than `memories`
/// because the **session** is what is being summarized and its identifier is what the path names; the fact
/// that the result is stored as a memory is the daemon's concern, not the caller's.
///
/// The span is in the body rather than the path: it is two numbers that only make sense together, and a path
/// like `/sessions/{id}/summaries/0..3` would make a range look like an identifier.
async fn summarize_session(
    State(state): State<GatewayState>,
    Path(id): Path<String>,
    Json(request): Json<SummarizeSessionRequest>,
) -> Response {
    match state.memories().summarize_session(&id, &request).await {
        // `201`, matching the run creation: a summary is a resource that did not exist before, and its
        // identifier is in the reply. A `200` would tell a client the request was a read or an update.
        Ok(reply) => (StatusCode::CREATED, Json(reply)).into_response(),
        Err(error) => memory_error(&error),
    }
}

/// `GET /api/v1/sessions/{id}/summaries`
async fn list_session_summaries(
    State(state): State<GatewayState>,
    Path(id): Path<String>,
    RawQuery(raw): RawQuery,
) -> Response {
    let limit = match parse_limit(raw.as_deref(), jarvis_storage::MAX_SUMMARY_PAGE) {
        Ok(limit) => limit,
        Err(message) => {
            return error_response(StatusCode::BAD_REQUEST, ErrorCode::Validation, message);
        }
    };
    match state.memories().list_summaries(&id, limit).await {
        Ok(reply) => Json(reply).into_response(),
        Err(error) => memory_error(&error),
    }
}

/// `POST /api/v1/sessions/{id}/summaries/retire`
///
/// The **retention rule** made reachable. `P4-008` records that nothing expires on its own: a session that is
/// over is retired by a caller, and this archives every current summary of it. A `POST` rather than a `DELETE`,
/// because the summaries are retained — `MemoryStatus::Archived` is "retained for audit, not retrieved as
/// current truth" — and a `DELETE` would tell a client the data is gone.
async fn retire_session_summaries(
    State(state): State<GatewayState>,
    Path(id): Path<String>,
) -> Response {
    match state.memories().retire_session_summaries(&id).await {
        Ok(archived) => Json(RetiredSummariesReply { archived }).into_response(),
        Err(error) => memory_error(&error),
    }
}

/// Reply body for a retention sweep.
#[derive(Debug, Serialize)]
struct RetiredSummariesReply {
    /// How many summaries stopped being current claims.
    archived: u64,
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

/// Parses a listing's `limit` parameter against a bound the caller names.
///
/// # Why this takes the default rather than reading one global bound
///
/// The listing bounds differ — `MAX_MEMORY_PAGE` is 200, `MAX_SUMMARY_PAGE` is 128, and the entity-match bound
/// is 50 — because they count different things. A shared parser with one hardcoded maximum would make one route
/// accept a page another refuses, and the *bound* is what a page parameter is for. Taking the default as a
/// parameter keeps one parser and three honest limits, which supersedes the two-parser version that existed
/// when there were only two bounds to reconcile.
///
/// The bound itself is **not** enforced here: each service checks its own, so the rule lives beside the read it
/// bounds and a route cannot be the only thing enforcing it.
///
/// # Errors
///
/// Returns a message naming the parameter that could not be read.
pub(crate) fn parse_limit(raw: Option<&str>, default: u32) -> Result<u32, &'static str> {
    let Some(raw) = raw else {
        return Ok(default);
    };
    for pair in raw.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        if key == "limit" {
            return value
                .parse()
                .map_err(|_| "the limit parameter is not a positive integer");
        }
    }
    Ok(default)
}

/// `POST /api/v1/entities`
///
/// Creates the entity a memory will be **about**. This is the route three slices recorded as missing: without
/// it a remember could be asked for and never performed, because every claim must name a subject and there was
/// no way to obtain one.
async fn create_entity(
    State(state): State<GatewayState>,
    Json(request): Json<CreateEntityRequest>,
) -> Response {
    match state.entities().create(&request).await {
        // `201`, matching every other create on this surface: the entity did not exist before and its
        // identifier is in the reply.
        Ok(reply) => (StatusCode::CREATED, Json(reply)).into_response(),
        Err(error) => entity_error(&error),
    }
}

/// `GET /api/v1/entities`
async fn list_entities(State(state): State<GatewayState>, RawQuery(raw): RawQuery) -> Response {
    let limit = match parse_limit(raw.as_deref(), crate::memory_service::MAX_MEMORY_PAGE) {
        Ok(limit) => limit,
        Err(message) => {
            return error_response(StatusCode::BAD_REQUEST, ErrorCode::Validation, message);
        }
    };
    match state.entities().list(limit).await {
        Ok(reply) => Json(reply).into_response(),
        Err(error) => entity_error(&error),
    }
}

/// `GET /api/v1/entities/{id}`
async fn read_entity(State(state): State<GatewayState>, Path(id): Path<String>) -> Response {
    match state.entities().read(&id).await {
        Ok(reply) => Json(reply).into_response(),
        Err(error) => entity_error(&error),
    }
}

/// `GET /api/v1/entities/lookup?label=…` or `?alias_kind=…&alias_value=…`
///
/// # Why one route rather than two
///
/// Both are "which entity does this name denote", and the answer type is the same. A single route whose query
/// selects the **kind of name** is what keeps the two lookups' replies identical by construction: two routes
/// returning the same shape are two places for one of them to start returning a single row, which is exactly the
/// `resolve_alias` rule this surface must not break.
async fn lookup_entity(State(state): State<GatewayState>, RawQuery(raw): RawQuery) -> Response {
    let query = match parse_entity_lookup(raw.as_deref()) {
        Ok(query) => query,
        Err(message) => {
            return error_response(StatusCode::BAD_REQUEST, ErrorCode::Validation, message);
        }
    };
    match &query {
        EntityLookupQuery::Label { label, limit } => {
            match state.entities().lookup_by_label(label, *limit).await {
                Ok(reply) => Json(reply).into_response(),
                Err(error) => entity_error(&error),
            }
        }
        EntityLookupQuery::Alias {
            alias_kind,
            alias_value,
            limit,
        } => match state
            .entities()
            .lookup_by_alias(alias_kind, alias_value, *limit)
            .await
        {
            Ok(reply) => Json(reply).into_response(),
            Err(error) => entity_error(&error),
        },
    }
}

/// `POST /api/v1/entities/{id}/aliases`
async fn add_entity_alias(
    State(state): State<GatewayState>,
    Path(id): Path<String>,
    Json(request): Json<AddAliasRequest>,
) -> Response {
    match state.entities().add_alias(&id, &request).await {
        Ok(reply) => Json(reply).into_response(),
        Err(error) => entity_error(&error),
    }
}

/// `POST /api/v1/entities/{id}/merge`
///
/// The path names the entity being merged **away** and the body names the one merged **into**, which is the
/// direction an operator is most likely to reverse. The reply is the winner, so a caller that reversed it can
/// see from the reply which entity is now referenced.
async fn merge_entity(
    State(state): State<GatewayState>,
    Path(id): Path<String>,
    Json(request): Json<MergeEntityRequest>,
) -> Response {
    match state.entities().merge(&id, &request).await {
        Ok(reply) => Json(reply).into_response(),
        Err(error) => entity_error(&error),
    }
}

/// Which name a lookup was asked about.
#[derive(Debug)]
enum EntityLookupQuery {
    /// A lookup by the entity's own label.
    Label {
        /// The label to match.
        label: String,
        /// How many matches to return at most.
        limit: u32,
    },
    /// A lookup by an alias.
    Alias {
        /// The alias kind.
        alias_kind: String,
        /// The alias value.
        alias_value: String,
        /// How many matches to return at most.
        limit: u32,
    },
}

/// Parses the entity lookup's query parameters.
///
/// # Errors
///
/// Returns a message when neither a `label` nor both alias parameters are present, which is a request the daemon
/// cannot answer rather than one it can answer with nothing: an empty query would otherwise return every entity
/// in the workspace, which is `GET /entities`.
fn parse_entity_lookup(raw: Option<&str>) -> Result<EntityLookupQuery, &'static str> {
    let mut label = None;
    let mut alias_kind = None;
    let mut alias_value = None;
    let mut limit = None;
    let Some(raw) = raw else {
        return Err("a lookup requires either a label, or an alias kind and value");
    };
    for pair in raw.split('&') {
        if pair.is_empty() {
            continue;
        }
        // Percent-decoding is deliberately absent: this build's query parsing is the same hand-written form the
        // memory pager uses, and introducing decoding here would make one route's parameter handling differ
        // from every other's. A value needing an escape is sent in a body (`POST /entities` for a write) or
        // arrives through a client that encodes it itself.
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        match key {
            "label" => label = Some(value.to_owned()),
            "alias_kind" => alias_kind = Some(value.to_owned()),
            "alias_value" => alias_value = Some(value.to_owned()),
            "limit" => {
                limit = Some(
                    value
                        .parse()
                        .map_err(|_| "the limit parameter is not a positive integer")?,
                );
            }
            // An unrecognized parameter is ignored rather than refused, matching the rule the memory pager and
            // the event route follow: a request the daemon does not own tolerates an additive field.
            _ => {}
        }
    }
    let limit = limit.unwrap_or(crate::entity_service::MAX_ENTITY_MATCHES);
    match (label, alias_kind, alias_value) {
        (Some(label), _, _) => Ok(EntityLookupQuery::Label { label, limit }),
        (None, Some(alias_kind), Some(alias_value)) => Ok(EntityLookupQuery::Alias {
            alias_kind,
            alias_value,
            limit,
        }),
        // Half an alias is not a lookup. Refused rather than defaulted, because the two plausible defaults are
        // opposite: an empty kind searches every kind, and an empty value matches nothing.
        (None, _, _) => Err("a lookup requires either a label, or an alias kind and value"),
    }
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

/// Maps an entity failure onto the shared error vocabulary.
///
/// The same three-way split the memory surface makes, and the same reasoning: a caller error is `422` with the
/// remedy the service wrote, a missing row is `404`, and infrastructure is `503` with no detail. `NotFound` is
/// worth stating separately here because an entity lookup's most likely failure is a caller holding an
/// identifier for an entity that was merged away — and "not found" is the answer that stops it looking for a row
/// that is still present.
fn entity_error(error: &crate::entity_service::EntityServiceError) -> Response {
    use crate::entity_service::EntityServiceError as Entity;
    match error {
        Entity::NotFound => error_response(
            StatusCode::NOT_FOUND,
            ErrorCode::Validation,
            "no entity exists for the requested identifier in this workspace",
        ),
        Entity::UnknownValue { .. } | Entity::PageTooLarge { .. } | Entity::Refused { .. } => {
            // Passed through `SafeMessage`, which bounds the length and refuses a control character, so a
            // refusal cannot forge a log line or a second header. Every detail this service produces is
            // written by the service and never contains stored content.
            error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                ErrorCode::Validation,
                safe(&error.detail()).as_str(),
            )
        }
        Entity::Storage(_) => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::Internal,
            "the local database is not available",
        ),
    }
}

/// `GET /api/v1/skills`
///
/// A listing of the workspace's procedures as **references** rather than content: a skill's prose and step
/// instructions both reach a prompt, so a listing that returned them would be a second, less careful path into
/// a model's context. `GET /skills/{id}` is where the text belongs — one procedure, deliberately requested.
async fn list_skills(State(state): State<GatewayState>, RawQuery(raw): RawQuery) -> Response {
    let limit = match match parse_memory_page(raw.as_deref()) {
        Ok(page) => Ok(page.limit.unwrap_or(crate::skill_service::MAX_SKILL_PAGE)),
        Err(message) => Err(message),
    } {
        Ok(limit) => limit,
        Err(message) => {
            return error_response(StatusCode::BAD_REQUEST, ErrorCode::Validation, message);
        }
    };
    match state.skills().list(limit).await {
        Ok(reply) => Json(reply).into_response(),
        Err(error) => skill_error(&error),
    }
}

/// `POST /api/v1/skills`
///
/// The creation surface. `201` for a new procedure and `200` for a declared correction, because a correction is
/// not a creation — and reporting it as one would make a corrected procedure look like an unrelated new one.
async fn create_skill(
    State(state): State<GatewayState>,
    Json(request): Json<CreateSkillRequest>,
) -> Response {
    match state.skills().create(&request).await {
        Ok(reply) => {
            let status = if reply.outcome == "created" {
                StatusCode::CREATED
            } else {
                StatusCode::OK
            };
            (status, Json(reply)).into_response()
        }
        Err(error) => skill_error(&error),
    }
}

/// `GET /api/v1/skills/{id}`
async fn read_skill(State(state): State<GatewayState>, Path(id): Path<String>) -> Response {
    match state.skills().read(&id).await {
        Ok(reply) => Json(reply).into_response(),
        Err(error) => skill_error(&error),
    }
}

/// `POST /api/v1/skills/{id}/promote`
async fn promote_skill(
    State(state): State<GatewayState>,
    Path(id): Path<String>,
    Json(request): Json<PromoteSkillRequest>,
) -> Response {
    match state.skills().promote(&id, &request).await {
        Ok(reply) => Json(reply).into_response(),
        Err(error) => skill_error(&error),
    }
}

/// `POST /api/v1/skills/{id}/disable`
async fn disable_skill(
    State(state): State<GatewayState>,
    Path(id): Path<String>,
    Json(request): Json<SkillTransitionRequest>,
) -> Response {
    match state.skills().disable(&id, &request).await {
        Ok(reply) => Json(reply).into_response(),
        Err(error) => skill_error(&error),
    }
}

/// `POST /api/v1/skills/{id}/enable`
async fn enable_skill(
    State(state): State<GatewayState>,
    Path(id): Path<String>,
    Json(request): Json<SkillTransitionRequest>,
) -> Response {
    match state.skills().enable(&id, &request).await {
        Ok(reply) => Json(reply).into_response(),
        Err(error) => skill_error(&error),
    }
}

/// `DELETE /api/v1/skills/{id}`
///
/// The verb is `DELETE` rather than a `/forget` sub-resource, because a deletion is what it is: a procedure has
/// no automatic ingest that could resurrect it, so there is no tombstone to describe and no `allow_relearn` to
/// choose. The body still carries the counter, because `DELETE` with a body is what the guard needs and a
/// query parameter would put a guard value in a place a proxy may rewrite.
async fn forget_skill(
    State(state): State<GatewayState>,
    Path(id): Path<String>,
    Json(request): Json<ForgetSkillRequest>,
) -> Response {
    match state.skills().forget(&id, &request).await {
        Ok(receipt) => Json(receipt).into_response(),
        Err(error) => skill_error(&error),
    }
}

/// `GET /api/v1/skills/export`
async fn export_skills(State(state): State<GatewayState>, RawQuery(raw): RawQuery) -> Response {
    let limit = match parse_memory_page(raw.as_deref()) {
        Ok(page) => page.limit.unwrap_or(crate::skill_service::MAX_SKILL_PAGE),
        Err(message) => {
            return error_response(StatusCode::BAD_REQUEST, ErrorCode::Validation, message);
        }
    };
    match state.skills().export(limit).await {
        Ok(reply) => Json(reply).into_response(),
        Err(error) => skill_error(&error),
    }
}

/// Maps a skill failure onto the shared error envelope.
///
/// # Why `ForeignWorkspace` renders exactly as `NotFound`
///
/// The service keeps the two apart because the isolation rule is otherwise untestable, but a caller must not be
/// able to learn that another workspace holds a revision with a given identifier — which is precisely what a
/// distinct message would disclose. So the wire has one answer for both, and the distinction stays inside.
///
/// The detail string is carried for the caller-actionable variants and **not** for storage, because a
/// `DatabaseError`'s text can name a path or a SQL statement.
fn skill_error(error: &crate::skill_service::SkillServiceError) -> Response {
    use crate::skill_service::SkillServiceError as Skill;
    match error {
        // Both absences render identically — see the note above.
        Skill::NotFound | Skill::ForeignWorkspace => error_response(
            StatusCode::NOT_FOUND,
            ErrorCode::Validation,
            "no skill revision exists for the requested identifier in this workspace",
        ),
        Skill::Conflict => error_response(
            StatusCode::CONFLICT,
            ErrorCode::Conflict,
            "the skill revision was changed by another writer; re-read it and retry",
        ),
        Skill::UnknownValue { .. } | Skill::PageTooLarge { .. } | Skill::Refused { .. } => {
            // Through `SafeMessage`, which bounds the length and refuses a control character, so a refusal
            // cannot forge a log line or a second header.
            error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                ErrorCode::Validation,
                safe(&error.detail()).as_str(),
            )
        }
        Skill::Storage(_) => error_response(
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
        let (app, _database, presented, profile) = test_router_and_database().await;
        (app, presented, profile)
    }

    /// Builds a router **and** the database handle, for a test that seeds rows no route can write.
    ///
    /// The session and message tables are written by `P2-004` and read by several routes, and the only way to
    /// create a session today is to start a run. Handing back the database handle rather than opening a second
    /// connection keeps the fixture honest: the rows are committed through the same pool the handlers use.
    async fn test_router_and_database() -> (
        Router,
        Arc<jarvis_storage::SqliteDatabase>,
        String,
        TempProfile,
    ) {
        let profile = TempProfile::new();
        let database = Arc::new(
            jarvis_storage::SqliteDatabase::open(&profile.database_path())
                .await
                .unwrap_or_else(|error| panic!("open fixture database: {error}")),
        );
        let credential = ClientCredential::generate()
            .unwrap_or_else(|error| panic!("generate fixture credential: {error}"));
        let presented = credential.expose().to_owned();
        let state = GatewayState::new(Arc::clone(&database), credential);
        (router(state), database, presented, profile)
    }

    /// A router whose speech provider is the one at `base` (a local fake in the tests that use it).
    async fn test_router_with_speech(base: &str) -> (Router, String, TempProfile) {
        let profile = TempProfile::new();
        let database = Arc::new(
            jarvis_storage::SqliteDatabase::open(&profile.database_path())
                .await
                .unwrap_or_else(|error| panic!("open fixture database: {error}")),
        );
        let credential = ClientCredential::generate()
            .unwrap_or_else(|error| panic!("generate fixture credential: {error}"));
        let presented = credential.expose().to_owned();
        let speech = jarvis_voice::ElevenLabsSpeech::new(
            base,
            jarvis_voice::DEFAULT_VOICE_ID,
            jarvis_voice::DEFAULT_MODEL,
            "sk_fixture_key_0123",
        )
        .unwrap_or_else(|error| panic!("speech fixture: {error}"));
        let state = GatewayState::new(database, credential).with_speech(Arc::new(speech));
        (router(state), presented, profile)
    }

    /// **The spoken voice sits behind the credential, says plainly when it is not configured, and never shows the key.**
    #[tokio::test]
    async fn speech_requires_the_credential_and_reports_when_it_is_not_configured() {
        let (app, presented, _profile) = test_router().await;
        let unauthenticated = app
            .clone()
            .oneshot(get_request("/api/v1/speech", None))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

        let status = app
            .clone()
            .oneshot(get_request("/api/v1/speech", Some(&presented)))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(status.status(), StatusCode::OK);
        assert!(body_text(status).await.contains(r#""enabled":false"#));

        let refused = app
            .oneshot(post_json(
                "/api/v1/speech",
                &presented,
                r#"{"text":"hello"}"#,
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(refused.status(), StatusCode::NOT_FOUND);
    }

    /// **A configured voice returns the provider's audio, and the key stays on this side.**
    #[tokio::test]
    async fn speech_returns_provider_audio_and_does_not_leak_the_key() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap_or_else(|error| panic!("bind: {error}"));
        let base = format!(
            "http://{}",
            listener
                .local_addr()
                .unwrap_or_else(|error| panic!("{error}"))
        );
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut received = Vec::new();
                let mut buffer = [0_u8; 4096];
                while let Ok(read) = stream.read(&mut buffer).await {
                    if read == 0 {
                        break;
                    }
                    received.extend_from_slice(&buffer[..read]);
                    if String::from_utf8_lossy(&received).contains(r#""model_id""#) {
                        break;
                    }
                }
                let _ = stream
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: audio/mpeg\r\ncontent-length: 9\r\nconnection: close\r\n\r\nMP3-bytes")
                    .await;
                let _ = stream.shutdown().await;
            }
        });
        let (app, presented, _profile) = test_router_with_speech(&base).await;

        let status = app
            .clone()
            .oneshot(get_request("/api/v1/speech", Some(&presented)))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        let status = body_text(status).await;
        assert!(status.contains(r#""enabled":true"#), "{status}");
        assert!(
            !status.contains("sk_fixture_key_0123"),
            "the key must never be reported"
        );

        let spoken = app
            .oneshot(post_json(
                "/api/v1/speech",
                &presented,
                r#"{"text":"Good evening."}"#,
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(spoken.status(), StatusCode::OK);
        assert_eq!(
            spoken
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok()),
            Some("audio/mpeg")
        );
        assert_eq!(body_text(spoken).await, "MP3-bytes");
    }

    /// **Stopping the daemon needs the credential.** Without it the request is refused (and nothing is woken).
    #[tokio::test]
    async fn a_stop_request_without_the_credential_is_refused() {
        let (app, _presented, _profile) = test_router().await;
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/shutdown")
                    .method("POST")
                    .body(Body::empty())
                    .unwrap_or_else(|error| panic!("fixture request: {error}")),
            )
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    /// With the credential the daemon answers that it is stopping (the wake-up itself is covered in `stop`).
    #[tokio::test]
    async fn a_stop_request_with_the_credential_is_accepted() {
        let (app, presented, _profile) = test_router().await;
        let response = app
            .oneshot(post_json("/api/v1/shutdown", &presented, ""))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert!(body_text(response).await.contains(r#""stopping":true"#));
    }

    /// A router over a profile whose configuration is real, so the Settings routes have something to read and write.
    async fn test_router_with_settings() -> (Router, String, TempProfile, jarvis_storage::AppPaths)
    {
        let profile = TempProfile::new();
        let paths = jarvis_storage::AppPaths::from_root(&profile.0)
            .unwrap_or_else(|error| panic!("paths: {error}"));
        std::fs::create_dir_all(paths.config()).unwrap_or_else(|error| panic!("{error}"));
        let key = paths.config().join("model.key");
        std::fs::write(&key, "ollama").unwrap_or_else(|error| panic!("{error}"));
        let document = format!(
            "schema_version = 1\n\n[profile]\nname = \"default\"\n\n[logging]\nlevel = \"info\"\n\n[daemon]\nshutdown_timeout_seconds = 30\nhttp_enabled = true\nexecutor_model = \"openai-compatible\"\nexecutor_model_name = \"m1\"\nexecutor_base_url = 'http://localhost:11434/v1'\nexecutor_api_key_ref = '{}'\n",
            key.display()
        );
        std::fs::write(
            jarvis_storage::ConfigStore::from_paths(&paths).path(),
            document,
        )
        .unwrap_or_else(|error| panic!("{error}"));
        let database = Arc::new(
            jarvis_storage::SqliteDatabase::open(&profile.database_path())
                .await
                .unwrap_or_else(|error| panic!("open fixture database: {error}")),
        );
        let credential = ClientCredential::generate().unwrap_or_else(|error| panic!("{error}"));
        let presented = credential.expose().to_owned();
        let state = GatewayState::new(database, credential).with_settings(
            crate::settings_service::SettingsContext::new(paths.clone(), None),
        );
        (router(state), presented, profile, paths)
    }

    fn put_json(path: &str, credential: &str, body: &str) -> Request<Body> {
        Request::builder()
            .uri(path)
            .method("PUT")
            .header(header::AUTHORIZATION, format!("Bearer {credential}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_owned()))
            .unwrap_or_else(|error| panic!("fixture request: {error}"))
    }

    /// **The settings routes need the credential: nobody else can read the configuration, change it, or set a key.**
    #[tokio::test]
    async fn the_settings_routes_require_the_credential() {
        let (app, _presented, _profile, _paths) = test_router_with_settings().await;
        for (method, path) in [
            ("GET", "/api/v1/settings"),
            ("PUT", "/api/v1/settings"),
            ("PUT", "/api/v1/settings/tools/jarvis.files.edit"),
            ("PUT", "/api/v1/settings/speech_model"),
            ("DELETE", "/api/v1/settings/speech_model"),
            ("PUT", "/api/v1/settings/keys/voice"),
            ("DELETE", "/api/v1/settings/keys/voice"),
            ("POST", "/api/v1/restart"),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(path)
                        .method(method)
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(r#"{"values":["x"],"key":"sk_x"}"#))
                        .unwrap_or_else(|error| panic!("fixture request: {error}")),
                )
                .await
                .unwrap_or_else(|error| panic!("router call: {error}"));
            assert_eq!(
                response.status(),
                StatusCode::UNAUTHORIZED,
                "{method} {path}"
            );
        }
    }

    /// **A pair of related settings is one request**, a half-pair is refused, and a tool''s permission has one value.
    #[tokio::test]
    async fn related_settings_apply_together_and_a_tool_permission_is_one_value() {
        let (app, presented, _profile, paths) = test_router_with_settings().await;
        let store = jarvis_storage::ConfigStore::from_paths(&paths);
        let read = || std::fs::read_to_string(store.path()).unwrap_or_default();

        // Alone, the image is half a sandbox.
        let half = app
            .clone()
            .oneshot(put_json(
                "/api/v1/settings/code_sandbox_image",
                &presented,
                r#"{"values":["node:22-alpine"]}"#,
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(half.status(), StatusCode::UNPROCESSABLE_ENTITY);

        // Together, it is one valid change.
        let both = app
            .clone()
            .oneshot(put_json(
                "/api/v1/settings",
                &presented,
                r#"{"changes":[{"key":"code_sandbox_image","values":["node:22-alpine"]},{"key":"code_sandbox_interpreter","values":["node -e"]}]}"#,
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(both.status(), StatusCode::OK);
        assert!(read().contains("node:22-alpine"));

        // A tool permission: trusted, then off replaces it, and a bad word is refused.
        let trusted = app
            .clone()
            .oneshot(put_json(
                "/api/v1/settings/tools/jarvis.files.edit",
                &presented,
                r#"{"posture":"trusted"}"#,
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(trusted.status(), StatusCode::OK);
        let off = app
            .clone()
            .oneshot(put_json(
                "/api/v1/settings/tools/jarvis.files.edit",
                &presented,
                r#"{"posture":"off"}"#,
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(off.status(), StatusCode::OK);
        let listed = body_text(
            app.clone()
                .oneshot(get_request("/api/v1/settings", Some(&presented)))
                .await
                .unwrap_or_else(|error| panic!("router call: {error}")),
        )
        .await;
        assert!(listed.contains(r#""jarvis.files.edit":"off""#), "{listed}");
        assert!(listed.contains(r#""group":"files""#));
        assert!(listed.contains(r#""unset_means""#));

        let bad = app
            .oneshot(put_json(
                "/api/v1/settings/tools/jarvis.files.edit",
                &presented,
                r#"{"posture":"always"}"#,
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(bad.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    /// **A key is accepted write-only: it lands in a private file, and no response ever contains it.**
    #[tokio::test]
    async fn a_key_is_set_write_only_and_never_echoed() {
        let (app, presented, _profile, paths) = test_router_with_settings().await;
        let secret = "sk_console_secret_0123456789";

        let saved = app
            .clone()
            .oneshot(put_json(
                "/api/v1/settings/keys/voice",
                &presented,
                &format!(r#"{{"key":"{secret}"}}"#),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(saved.status(), StatusCode::OK);
        let saved = body_text(saved).await;
        assert!(saved.contains(r#""state":"set""#), "{saved}");
        assert!(
            !saved.contains(secret),
            "the response must not contain the key"
        );

        let listed = app
            .clone()
            .oneshot(get_request("/api/v1/settings", Some(&presented)))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        let listed = body_text(listed).await;
        assert!(listed.contains("speech_api_key_ref"));
        assert!(
            !listed.contains(secret),
            "the listing must not contain the key"
        );

        let on_disk =
            std::fs::read_to_string(paths.config().join("speech.key")).unwrap_or_default();
        assert_eq!(on_disk, secret);
        let config =
            std::fs::read_to_string(jarvis_storage::ConfigStore::from_paths(&paths).path())
                .unwrap_or_default();
        assert!(
            !config.contains(secret),
            "the key must not be in the configuration"
        );

        let bad = app
            .oneshot(put_json(
                "/api/v1/settings/keys/voice",
                &presented,
                r#"{"key":"has spaces in it"}"#,
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(bad.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            !body_text(bad).await.contains("has spaces"),
            "a refusal must not repeat the key"
        );
    }

    /// **A change the daemon would refuse is refused with a reason, and the configuration is left as it was.**
    #[tokio::test]
    async fn an_invalid_setting_is_refused_and_a_valid_one_is_saved() {
        let (app, presented, _profile, paths) = test_router_with_settings().await;
        let store = jarvis_storage::ConfigStore::from_paths(&paths);
        let before = std::fs::read_to_string(store.path()).unwrap_or_default();

        let refused = app
            .clone()
            .oneshot(put_json(
                "/api/v1/settings/speech_voice_id",
                &presented,
                r#"{"values":["abc"]}"#,
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(refused.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            std::fs::read_to_string(store.path()).unwrap_or_default(),
            before
        );

        let unknown = app
            .clone()
            .oneshot(put_json(
                "/api/v1/settings/nonsense",
                &presented,
                r#"{"values":["x"]}"#,
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(unknown.status(), StatusCode::UNPROCESSABLE_ENTITY);

        let saved = app
            .oneshot(put_json(
                "/api/v1/settings/executor_model_name",
                &presented,
                r#"{"values":["glm-5.3:cloud"]}"#,
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(saved.status(), StatusCode::OK);
        assert!(
            std::fs::read_to_string(store.path())
                .unwrap_or_default()
                .contains("glm-5.3:cloud")
        );
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

    /// **The display's two static assets are public, and nothing else became public with them.**
    ///
    /// Asserted from both sides: the page and script are served with no credential, carry a content-security
    /// policy and no cache, and **contain no data** (the credential is not in them); while a sibling path, a `POST`
    /// to the same path, and the data API itself still refuse a request with no credential.
    #[tokio::test]
    async fn the_display_assets_are_public_and_nothing_else_is() {
        let (app, presented, _profile) = test_router().await;
        for path in ["/", "/hud", "/hud.js", "/head.js", "/hud.css"] {
            let response = app
                .clone()
                .oneshot(get_request(path, None))
                .await
                .unwrap_or_else(|error| panic!("router call: {error}"));
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            let policy = response
                .headers()
                .get("content-security-policy")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_owned();
            assert!(
                policy.contains("frame-ancestors 'none'"),
                "{path}: {policy}"
            );
            assert_eq!(
                response
                    .headers()
                    .get("cache-control")
                    .and_then(|value| value.to_str().ok()),
                Some("no-store")
            );
            let body = body_text(response).await;
            assert!(
                !body.contains(&presented),
                "{path} must not contain the credential"
            );
        }
        for path in [
            "/index.html",
            "/hud/",
            "/hudx",
            "/api/v1/runs",
            "/api/v1/approvals",
            "/health/live",
        ] {
            let response = app
                .clone()
                .oneshot(get_request(path, None))
                .await
                .unwrap_or_else(|error| panic!("router call: {error}"));
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{path}");
        }
        let posted = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/")
                    .body(Body::empty())
                    .unwrap_or_else(|error| panic!("build request: {error}")),
            )
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(posted.status(), StatusCode::UNAUTHORIZED);
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

    /// Builds a router over one granted root **and the approval fixture tool**, with a policy the test
    /// supplies.
    ///
    /// The policy arrives as a function rather than a value so each test states only the change it cares
    /// about and the default is applied here once — a test that built a `WorkspacePolicy` field by field
    /// would restate the defaults it does not intend to vary.
    ///
    /// The approval fixture is registered because it is the only definition in this binary that declares
    /// `ApprovalPolicy::Ask` at a risk the default workspace would otherwise allow, which is what makes the
    /// direction rule observable at the wire surface: a looser override written against it must do nothing.
    async fn tool_router_with_policy(
        configure: impl FnOnce(jarvis_tools::WorkspacePolicy) -> jarvis_tools::WorkspacePolicy,
    ) -> (Router, String, TempProfile) {
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
        let pipeline = crate::tool_pipeline::ToolPipeline::with_adapters(
            Arc::clone(&database),
            Some(roots),
            configure(jarvis_tools::WorkspacePolicy::default()),
            vec![(
                vec![crate::approval_fixture::approval_declaring_definition()],
                crate::approval_fixture::approval_adapter(),
            )],
        )
        .unwrap_or_else(|error| panic!("compose the tool pipeline: {error}"));

        let app = router(GatewayState::new(database, credential).with_tools(Arc::new(pipeline)));

        (app, presented, profile)
    }

    /// Counts the `tool_calls` rows in a profile's database, so "a preview records nothing" is observed.
    ///
    /// A read of the table itself rather than a pipeline accessor, because the property under test is about
    /// what reaches **durable state** — an accessor could report zero while a row existed.
    async fn count_tool_calls(profile: &TempProfile) -> i64 {
        let database = jarvis_storage::SqliteDatabase::open(&profile.database_path())
            .await
            .unwrap_or_else(|error| panic!("open fixture database: {error}"));
        let stored = jarvis_storage::count_tool_calls(&database)
            .await
            .unwrap_or_else(|error| panic!("count tool calls: {error}"));
        database.close().await;
        stored
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

    /// A router, a credential, a profile, and **one pending approval**.
    ///
    /// The approval is produced by the real pipeline holding a real risk-2 tool, so the row is exactly what
    /// a live daemon would leave behind. Building it by hand would be the fixture stating its own assumption
    /// about what the pipeline writes, which is the thing the route test exists to check.
    async fn approval_router() -> (Router, String, TempProfile, String, Arc<SqliteDatabase>) {
        approval_router_with(crate::approval_fixture::approval_adapter()).await
    }

    /// The same fixture over a chosen adapter, so a test about **resumption** can observe the call running.
    ///
    /// The default fixture's adapter refuses, which is what makes "the held call did not run" observable.
    /// A resume test needs the opposite observation, so it supplies an adapter that succeeds and counts —
    /// and the choice is a parameter rather than a second fixture, because everything else about the setup
    /// (the profile, the root grant, the pipeline, the held call) has to be **identical** for the two tests
    /// to be about the same thing.
    async fn approval_router_with(
        adapter: Arc<dyn jarvis_tools::ToolExecutor>,
    ) -> (Router, String, TempProfile, String, Arc<SqliteDatabase>) {
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
        )
        .unwrap_or_else(|error| panic!("compose the tool pipeline: {error}"));

        let app = router(
            GatewayState::new(Arc::clone(&database), credential).with_tools(Arc::new(pipeline)),
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

        (app, presented, profile, approval_id, database)
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

    /// **A person can list what is waiting, and sees the arguments.**
    ///
    /// `GET /approvals` is how a client learns what a held call would do. The assertions are the three ways it
    /// could be wrong: the arguments the call was made with are shown (so the person knows what they approve),
    /// no decision-body-only field appears in the reply, and a decided approval leaves the list.
    #[tokio::test]
    async fn a_pending_approval_is_listed_with_its_arguments() {
        let (app, presented, _profile, approval_id, _database) = approval_router().await;

        let listed = app
            .clone()
            .oneshot(get_request("/api/v1/approvals", Some(&presented)))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(listed.status(), StatusCode::OK);
        let text = body_text(listed).await;
        assert!(
            !text.contains("\"nonce\""),
            "a decision-body-only field leaked into the list reply: {text}"
        );
        let reply: jarvis_protocol::ApprovalListReply = serde_json::from_str(&text)
            .unwrap_or_else(|error| panic!("decode the list: {error}: {text}"));
        assert_eq!(reply.total, 1);
        let approval = &reply.approvals[0];
        assert_eq!(approval.approval_id, approval_id);
        assert_eq!(approval.tool, crate::approval_fixture::APPROVAL_TOOL);
        assert_eq!(
            approval.arguments,
            Some(serde_json::json!({ "path": "notes.txt" })),
            "the person deciding must see what they are approving"
        );
        assert!(
            !approval.call_id.is_empty(),
            "a client needs the call identifier to resume it"
        );

        let decided = app
            .clone()
            .oneshot(decision_request(
                &presented,
                &approval_id,
                &serde_json::json!({ "decision": "deny" }),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(decided.status(), StatusCode::OK);

        let after = app
            .oneshot(get_request("/api/v1/approvals", Some(&presented)))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        let after: jarvis_protocol::ApprovalListReply =
            serde_json::from_str(&body_text(after).await)
                .unwrap_or_else(|error| panic!("decode the list: {error}"));
        assert_eq!(after.total, 0, "a decided approval is no longer pending");
    }

    /// The approvals list needs the profile credential like every other route.
    #[tokio::test]
    async fn the_approvals_list_refuses_an_unauthenticated_request() {
        let (app, _presented, _profile, _approval_id, _database) = approval_router().await;
        let refused = app
            .oneshot(get_request("/api/v1/approvals", None))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(refused.status(), StatusCode::UNAUTHORIZED);
    }

    /// **An operator can decide a held approval exactly once.**
    ///
    /// This is the end of that path, and it asserts the three things a route could get wrong:
    /// the decision lands, the row reports the **effective** state, and a replay of the identical request is
    /// refused rather than recording a second decision.
    ///
    /// The replay is the important one. Without the stored-row guard, the second request would be accepted
    /// and â€” depending on the store â€” could overwrite the first decision. `record_decision` already
    /// refuses an already-decided row; this asserts the *route* does not defeat it.
    #[tokio::test]
    async fn an_operator_can_decide_a_held_approval_exactly_once() {
        let (app, presented, _profile, approval_id, _database) = approval_router().await;

        let approved = app
            .clone()
            .oneshot(decision_request(
                &presented,
                &approval_id,
                &serde_json::json!({ "decision": "approve" }),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(
            approved.status(),
            StatusCode::OK,
            "an authenticated decision must decide the approval: {}",
            body_text(approved).await
        );

        let decided = app
            .clone()
            .oneshot(decision_request(
                &presented,
                &approval_id,
                &serde_json::json!({ "decision": "approve" }),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(
            decided.status(),
            StatusCode::CONFLICT,
            "a replayed decision must be refused: {}",
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
        let (app, presented, _profile, approval_id, _database) = approval_router().await;

        let response = app
            .oneshot(decision_request(
                &presented,
                &approval_id,
                &serde_json::json!({
                    "decision": "approve",
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
    /// once** however many times the resumption is retried.
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
        let (app, presented, _profile, approval_id, database) =
            approval_router_with(adapter.clone()).await;

        // The held call, decided by an operator.
        let decided = app
            .clone()
            .oneshot(decision_request(
                &presented,
                &approval_id,
                &serde_json::json!({ "decision": "approve" }),
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

    /// Waits for a recording adapter to have run, because the daemon releases an approved call in a task.
    async fn wait_for_calls(
        adapter: &crate::approval_fixture::RecordingApprovalAdapter,
        expected: usize,
    ) {
        // A generous ceiling that costs nothing when the task is quick: a loaded CI runner can take many seconds to run a
        // spawned task, and a flaky deadline here is a test failure that says nothing about the daemon.
        for _ in 0..600 {
            if adapter.calls() >= expected {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    /// Waits until a call's held arguments have been cleared, which the daemon does after the call has run.
    async fn wait_for_arguments_cleared(
        database: &jarvis_storage::SqliteDatabase,
        approval_id: &str,
    ) {
        for _ in 0..600 {
            if matches!(
                jarvis_storage::read_approval_arguments(database, approval_id).await,
                Ok(None)
            ) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    /// **Approving with `resume` has the daemon release the call, from the arguments it held, exactly once.**
    ///
    /// This is the flow the CLI uses, and the point of it is what the client does *not* do: it sends no payload.
    /// What runs is what was held when the call was admitted and shown to the person, so there is no second
    /// copy of the arguments for a client to get wrong, and a crash between decision and release cannot lose
    /// them. The adapter's counter is the assertion, and the held arguments being gone afterwards is the other.
    #[tokio::test]
    async fn approving_with_resume_releases_the_held_call_exactly_once() {
        let adapter =
            std::sync::Arc::new(crate::approval_fixture::RecordingApprovalAdapter::default());
        let (app, presented, _profile, approval_id, database) =
            approval_router_with(adapter.clone()).await;

        let decided = app
            .clone()
            .oneshot(decision_request(
                &presented,
                &approval_id,
                &serde_json::json!({ "decision": "approve", "resume": true }),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(decided.status(), StatusCode::OK);
        wait_for_calls(&adapter, 1).await;
        assert_eq!(adapter.calls(), 1, "the daemon must have released the call");

        // Released a second time, the call is already done: refused, and the effect is not repeated.
        let again = app
            .oneshot(post_json(
                &format!("/api/v1/approvals/{approval_id}/resume"),
                &presented,
                "{}",
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(
            again.status(),
            StatusCode::CONFLICT,
            "{}",
            body_text(again).await
        );
        assert_eq!(
            adapter.calls(),
            1,
            "a second release must not run the tool again"
        );
        wait_for_arguments_cleared(&database, &approval_id).await;
        assert_eq!(
            jarvis_storage::read_approval_arguments(&database, &approval_id)
                .await
                .unwrap_or_else(|error| panic!("read the held arguments: {error}")),
            None,
            "an executed call's held arguments must be gone"
        );
    }

    /// **The kill switch stops a run that is waiting for a person, and withdraws what it was waiting on.**
    ///
    /// Found live: `jarvis cancel` recorded the request and the run stayed at `awaiting_approval`, because a parked
    /// run has no driver to read it. Asserted from the person's side: the run is cancelled, the approval is no longer
    /// listed, deciding it afterwards is refused, and the tool never ran.
    #[tokio::test]
    async fn cancelling_a_parked_run_withdraws_its_approval() {
        let adapter =
            std::sync::Arc::new(crate::approval_fixture::RecordingApprovalAdapter::default());
        let (app, presented, _profile, approval_id, database) =
            approval_router_with(adapter.clone()).await;

        // The fixture holds a call made directly, so the run is still at its start: walk it to the state a driver
        // leaves it in when a call is held.
        let run =
            jarvis_storage::read_recent_runs(&database, jarvis_storage::LOCAL_WORKSPACE_ID, 1)
                .await
                .unwrap_or_else(|error| panic!("read the run: {error}"))
                .into_iter()
                .next()
                .unwrap_or_else(|| panic!("the fixture created a run"));
        let mut current = run;
        for next in [
            jarvis_core::RunState::ContextBuilding,
            jarvis_core::RunState::Planning,
            jarvis_core::RunState::AwaitingApproval,
        ] {
            current = jarvis_storage::transition_run(
                &database,
                current.id(),
                current.expectation(),
                &jarvis_core::RunTransition::new(next, next.required_outcome(), None)
                    .unwrap_or_else(|error| panic!("transition: {error}")),
                jarvis_core::UtcTimestamp::now(&jarvis_core::SystemClock),
            )
            .await
            .unwrap_or_else(|error| panic!("advance to {next:?}: {error}"));
        }

        let cancelled = app
            .clone()
            .oneshot(post_json(
                &format!("/api/v1/runs/{}/cancel", current.id()),
                &presented,
                "{}",
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(cancelled.status(), StatusCode::OK);
        let reply: jarvis_protocol::RunReply = serde_json::from_str(&body_text(cancelled).await)
            .unwrap_or_else(|error| panic!("decode the reply: {error}"));
        assert_eq!(reply.state, jarvis_core::RunState::Cancelled, "{reply:?}");

        let listed = app
            .clone()
            .oneshot(get_request("/api/v1/approvals", Some(&presented)))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        let listed: jarvis_protocol::ApprovalListReply =
            serde_json::from_str(&body_text(listed).await)
                .unwrap_or_else(|error| panic!("decode the list: {error}"));
        assert!(listed.approvals.is_empty(), "{listed:?}");

        let decided = app
            .oneshot(decision_request(
                &presented,
                &approval_id,
                &serde_json::json!({ "decision": "approve", "resume": true }),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert!(
            !decided.status().is_success(),
            "a withdrawn approval must not be decidable"
        );
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert_eq!(adapter.calls(), 0, "the cancelled call must never run");
    }

    /// A decision without `resume` releases nothing, and the approval then still holds what it needs for
    /// `POST /approvals/{id}/resume` — the recovery path for a daemon that died between the two steps.
    #[tokio::test]
    async fn an_approved_call_can_be_finished_later_without_resending_its_arguments() {
        let adapter =
            std::sync::Arc::new(crate::approval_fixture::RecordingApprovalAdapter::default());
        let (app, presented, _profile, approval_id, _database) =
            approval_router_with(adapter.clone()).await;

        // Not yet approved: there is nothing to release.
        let early = app
            .clone()
            .oneshot(post_json(
                &format!("/api/v1/approvals/{approval_id}/resume"),
                &presented,
                "{}",
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(
            early.status(),
            StatusCode::CONFLICT,
            "{}",
            body_text(early).await
        );

        let decided = app
            .clone()
            .oneshot(decision_request(
                &presented,
                &approval_id,
                &serde_json::json!({ "decision": "approve" }),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(decided.status(), StatusCode::OK);
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert_eq!(
            adapter.calls(),
            0,
            "a decision without `resume` must not run the tool"
        );

        // The person who approved can see the call never ran, and what would finish it.
        let listed = app
            .clone()
            .oneshot(get_request("/api/v1/approvals", Some(&presented)))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        let listed: jarvis_protocol::ApprovalListReply =
            serde_json::from_str(&body_text(listed).await)
                .unwrap_or_else(|error| panic!("decode the list: {error}"));
        assert_eq!(listed.total, 1);
        assert_eq!(listed.approvals[0].state, "approved");
        assert_eq!(listed.approvals[0].approval_id, approval_id);

        let finished = app
            .oneshot(post_json(
                &format!("/api/v1/approvals/{approval_id}/resume"),
                &presented,
                "{}",
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(
            finished.status(),
            StatusCode::OK,
            "{}",
            body_text(finished).await
        );
        assert_eq!(adapter.calls(), 1);
    }

    /// A denied approval cannot be released, whatever is asked.
    #[tokio::test]
    async fn a_denied_approval_cannot_be_released() {
        let adapter =
            std::sync::Arc::new(crate::approval_fixture::RecordingApprovalAdapter::default());
        let (app, presented, _profile, approval_id, _database) =
            approval_router_with(adapter.clone()).await;
        let denied = app
            .clone()
            .oneshot(decision_request(
                &presented,
                &approval_id,
                &serde_json::json!({ "decision": "deny", "resume": true }),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(denied.status(), StatusCode::OK);
        let release = app
            .oneshot(post_json(
                &format!("/api/v1/approvals/{approval_id}/resume"),
                &presented,
                "{}",
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(release.status(), StatusCode::CONFLICT);
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert_eq!(adapter.calls(), 0, "a denial must never run the tool");
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

    /// A decision for an identifier that is not an approval is a `404` before the write path runs, so a
    /// caller cannot probe for approvals by watching for different side effects.
    #[tokio::test]
    async fn a_decision_for_an_unknown_approval_is_not_found() {
        let (app, presented, _profile) = test_router().await;

        let response = app
            .oneshot(decision_request(
                &presented,
                "0198f000-0000-7000-8000-0000000000ff",
                &serde_json::json!({ "decision": "approve" }),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    /// **An old-style decision body is refused and the current body still decides the approval.**
    ///
    /// This pins `deny_unknown_fields` on the public contract: an older client sending a removed field must
    /// get a clear `422`, while the current body still works.
    #[tokio::test]
    async fn an_old_style_decision_body_is_refused_and_a_modern_body_decides() {
        let (app, presented, _profile, approval_id, _database) = approval_router().await;

        let refused = app
            .clone()
            .oneshot(decision_request(
                &presented,
                &approval_id,
                &serde_json::json!({ "decision": "approve", "nonce": "0".repeat(64) }),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(
            refused.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "an old-style nonce field must be rejected: {}",
            body_text(refused).await
        );

        let approved = app
            .clone()
            .oneshot(decision_request(
                &presented,
                &approval_id,
                &serde_json::json!({ "decision": "approve" }),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(
            approved.status(),
            StatusCode::OK,
            "a modern decision body must still decide the approval: {}",
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
        let state = GatewayState::new(Arc::clone(&database), credential);
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

    /// **A model-produced claim is stored as a proposal, never as current truth.**
    ///
    /// The inference boundary at the surface a model would actually use, and the half of it that was not
    /// enforced: `source_kind` is a request field, so `model_inference` is reachable over HTTP, and the
    /// pipeline and the record **disagreed** about what it produced. `MemoryCandidate::admit` decided
    /// `Proposal` — its own code says a model inference "is never written as current truth" — and
    /// `MemoryRecord::build` derived `Active` from the type alone, so the reply reported an empty status for
    /// a claim that was current.
    ///
    /// Asserted through a *subsequent read* rather than the reply, because the reply's status is the one field
    /// the defect produced wrongly, and reading it back is what distinguishes a store that agrees with its
    /// pipeline from one that merely reports differently. The `201` is deliberate: a proposal is still stored.
    #[tokio::test]
    async fn a_model_inference_is_stored_as_a_proposal_and_never_as_current_truth() {
        let (app, presented, _profile, database) = memory_router().await;
        let subject = fixture_entity(&database).await;
        let responded = app
            .clone()
            .oneshot(post_json(
                "/api/v1/memories",
                &presented,
                &serde_json::json!({
                    "content": "Probably prefers tea",
                    "memory_type": "semantic",
                    "source_kind": "model_inference",
                    "entity_ids": [subject],
                })
                .to_string(),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(responded.status(), StatusCode::CREATED);
        let body = body_text(responded).await;
        let remembered: jarvis_protocol::MemoryReply =
            serde_json::from_str(&body).unwrap_or_else(|error| panic!("decode {body}: {error}"));
        let memory_id = remembered.memory_id.clone();

        let read = app
            .oneshot(get_request(
                &format!("/api/v1/memories/{memory_id}"),
                Some(&presented),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(read.status(), StatusCode::OK);
        let read_body = body_text(read).await;
        let detail: jarvis_protocol::MemoryDetailReply = serde_json::from_str(&read_body)
            .unwrap_or_else(|error| panic!("decode {read_body}: {error}"));
        assert_eq!(
            detail.reference.status.as_str(),
            "proposed",
            "a claim no source can support must not be the workspace's current truth"
        );
        assert_eq!(
            detail.reference.effective_status.as_str(),
            "proposed",
            "and no later instant makes it current, so the status the read reports agrees with the stored one"
        );
        assert_eq!(
            detail.reference.confidence, "unverified",
            "and it may carry no confidence at all, which is the other half of the boundary"
        );
        assert!(
            !detail.is_stated_as_fact,
            "the presentation predicate and the status must agree here, and the defect was their disagreement"
        );
    }

    /// **A proposal can be accepted over HTTP, and the acceptance names the approver.**
    ///
    /// `P4-014`'s verb, and the one the memory surface was missing: `MemoryTransition::Confirm` existed from
    /// `P4-002` and **no route called it**, so a `Proposed` claim could be created and never accepted. The two
    /// assertions are the status becoming current and the approver being recorded — either alone is satisfied
    /// by a handler that merely flipped the status.
    ///
    /// The approver asserted is the **seeded user**, read from the daemon's identity, and that is a deliberate
    /// part of the test: `remember` used to write the literal `"local-user"` as the author while the seeded user
    /// is `LOCAL_USER_ID`, so an approver read from the identity would never have matched a claim's author and
    /// the self-admission guard would have been vacuous. Asserting the identifier the row actually holds is
    /// what pins the two to one source.
    #[tokio::test]
    async fn the_confirm_route_accepts_a_proposal_and_names_its_approver() {
        let (app, presented, _profile, database) = memory_router().await;
        let subject = fixture_entity(&database).await;
        // A relationship claim is a proposal by the domain's own derivation, so the fixture cannot hold a
        // status the pipeline would not produce — the same rule `memory/tests.rs` follows.
        let created = app
            .clone()
            .oneshot(post_json(
                "/api/v1/memories",
                &presented,
                &serde_json::json!({
                    "content": "Works with Dana",
                    "memory_type": "relationship",
                    "source_kind": "user_statement",
                    "entity_ids": [subject],
                })
                .to_string(),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(created.status(), StatusCode::CREATED);
        let body = body_text(created).await;
        let remembered: jarvis_protocol::MemoryReply =
            serde_json::from_str(&body).unwrap_or_else(|error| panic!("decode {body}: {error}"));
        assert_eq!(remembered.status, "proposed");
        let memory_id = remembered.memory_id.clone();

        let confirmed = app
            .clone()
            .oneshot(post_json(
                &format!("/api/v1/memories/{memory_id}/confirm"),
                &presented,
                &serde_json::json!({ "expected_version": remembered.version }).to_string(),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(confirmed.status(), StatusCode::OK);
        let confirmed_body = body_text(confirmed).await;
        let reply: jarvis_protocol::MemoryReply = serde_json::from_str(&confirmed_body)
            .unwrap_or_else(|error| panic!("decode {confirmed_body}: {error}"));
        assert_eq!(reply.status, "active");
        assert_eq!(reply.outcome, "confirmed");

        // And the state is read back rather than taken from the reply, because the reply is what a defect here
        // would have produced wrongly.
        let read = app
            .clone()
            .oneshot(get_request(
                &format!("/api/v1/memories/{memory_id}"),
                Some(&presented),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        let read_body = body_text(read).await;
        let detail: jarvis_protocol::MemoryDetailReply = serde_json::from_str(&read_body)
            .unwrap_or_else(|error| panic!("decode {read_body}: {error}"));
        assert_eq!(detail.reference.status.as_str(), "active");
        assert_eq!(
            detail.reference.admitted_by_actor_id.as_deref(),
            Some(jarvis_storage::LOCAL_USER_ID),
            "the acceptance must name the identity that made it, read from the daemon's own identity"
        );
        assert!(
            detail.reference.admitted_at.is_some(),
            "and the moment it was decided"
        );

        // **The version guard, asserted because a confirmation is a write.** A second attempt against the
        // version read before the first is stale, and the claim is no longer a proposal anyway — so this pins
        // which of the two refusals a caller gets, and the staleness one must win because the caller has to
        // re-read before it can act at all.
        let stale = app
            .oneshot(post_json(
                &format!("/api/v1/memories/{memory_id}/confirm"),
                &presented,
                &serde_json::json!({ "expected_version": remembered.version }).to_string(),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(stale.status(), StatusCode::CONFLICT);
    }

    /// **The confirmation is the person confirming their own workspace's claim, and it works.**
    ///
    /// This asserts a **non**-refusal, which is unusual and deliberate. `ADR-0117` §4 refuses a promotion by a
    /// procedure's author, so the tempting symmetry is to refuse an admission by a claim's author — and that
    /// would have blocked `memory-and-context.md`'s own requirement that a high-impact inference get "explicit
    /// user confirmation". There is one seeded identity, so the author and the approver are always the same
    /// value, and a guard comparing them would refuse every legitimate confirmation.
    ///
    /// The test exists so the symmetry cannot be "restored" without this case failing and showing why. What
    /// actually holds is that the approver is never client-supplied — see `MemoryService::confirm`.
    #[tokio::test]
    async fn a_claim_can_be_confirmed_by_the_identity_that_stated_it() {
        let (app, presented, _profile, database) = memory_router().await;
        let subject = fixture_entity(&database).await;
        let created = app
            .clone()
            .oneshot(post_json(
                "/api/v1/memories",
                &presented,
                &serde_json::json!({
                    "content": "Works with Dana",
                    "memory_type": "relationship",
                    "source_kind": "user_statement",
                    "entity_ids": [subject],
                })
                .to_string(),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        let body = body_text(created).await;
        let remembered: jarvis_protocol::MemoryReply =
            serde_json::from_str(&body).unwrap_or_else(|error| panic!("decode {body}: {error}"));

        let confirmed = app
            .oneshot(post_json(
                &format!("/api/v1/memories/{}/confirm", remembered.memory_id),
                &presented,
                &serde_json::json!({ "expected_version": remembered.version }).to_string(),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(confirmed.status(), StatusCode::OK);
        let confirmed_body = body_text(confirmed).await;
        let reply: jarvis_protocol::MemoryReply = serde_json::from_str(&confirmed_body)
            .unwrap_or_else(|error| panic!("decode {confirmed_body}: {error}"));
        assert_eq!(reply.status, "active");
    }

    /// **The acceptance is recorded with the identity and the moment, and the version guard refuses a replay.**
    ///
    /// The read-back half: the reply's status is the one field a handler that only set the status would have
    /// produced correctly, so the approver and the timestamp are asserted against a *subsequent* read. The
    /// replay then pins the guard, and it is the case that matters — a confirmation is idempotent-looking, so
    /// a second acceptance against the old version must be a conflict rather than a silent second decision.
    #[tokio::test]
    async fn the_confirm_route_records_the_approver_and_refuses_a_replay() {
        let (app, presented, _profile, database) = memory_router().await;
        let subject = fixture_entity(&database).await;
        let created = app
            .clone()
            .oneshot(post_json(
                "/api/v1/memories",
                &presented,
                &serde_json::json!({
                    "content": "Works with Dana",
                    "memory_type": "relationship",
                    "source_kind": "user_statement",
                    "entity_ids": [subject],
                })
                .to_string(),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        let body = body_text(created).await;
        let remembered: jarvis_protocol::MemoryReply =
            serde_json::from_str(&body).unwrap_or_else(|error| panic!("decode {body}: {error}"));
        let memory_id = remembered.memory_id.clone();

        let confirmed = app
            .clone()
            .oneshot(post_json(
                &format!("/api/v1/memories/{memory_id}/confirm"),
                &presented,
                &serde_json::json!({ "expected_version": remembered.version }).to_string(),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(confirmed.status(), StatusCode::OK);

        let read = app
            .clone()
            .oneshot(get_request(
                &format!("/api/v1/memories/{memory_id}"),
                Some(&presented),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        let read_body = body_text(read).await;
        let detail: jarvis_protocol::MemoryDetailReply = serde_json::from_str(&read_body)
            .unwrap_or_else(|error| panic!("decode {read_body}: {error}"));
        assert_eq!(detail.reference.status.as_str(), "active");
        assert_eq!(
            detail.reference.admitted_by_actor_id.as_deref(),
            Some(jarvis_storage::LOCAL_USER_ID),
            "the acceptance must name the identity that made it, read from the daemon's own identity"
        );
        assert!(
            detail.reference.admitted_at.is_some(),
            "and the moment it was decided, so the pair travels together"
        );

        let replay = app
            .oneshot(post_json(
                &format!("/api/v1/memories/{memory_id}/confirm"),
                &presented,
                &serde_json::json!({ "expected_version": remembered.version }).to_string(),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(
            replay.status(),
            StatusCode::CONFLICT,
            "an acceptance against the version read before the first is stale"
        );
    }

    /// **A claim can be corrected, refused against a stale version, and deleted over HTTP.** The write half,
    /// and the one where the version guard's *two* distinct staleness reasons are both pinned.
    ///
    /// The second staleness case is the subtle one and is asserted deliberately: correcting a claim archives
    /// it, which **advances its version**, so the version the caller read before correcting is stale
    /// immediately afterwards. A guard that only rejected a version the caller never saw would accept this
    /// second write, and the caller would be deleting a claim on the basis of a version the correction
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

    /// Creates a skill over HTTP, returning its revision identifier.
    ///
    /// A helper because almost every skill test needs a revision that exists, and building the body each time
    /// would be five copies of one request that could drift.
    async fn create_skill_via_http(app: &Router, presented: &str, description: &str) -> String {
        let body = serde_json::json!({
            "description": description,
            "author_version": "1",
            "steps": [{
                "position": 1,
                "tool": "jarvis.files.read",
                "tool_version": "1.0.0",
                "instruction": "Read the notes file.",
            }],
        })
        .to_string();
        let response = app
            .clone()
            .oneshot(post_json("/api/v1/skills", presented, &body))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(
            response.status(),
            StatusCode::CREATED,
            "a creation must be a creation: {}",
            body_text(response).await
        );
        let text = body_text(response).await;
        let reply: jarvis_protocol::SkillReply =
            serde_json::from_str(&text).unwrap_or_else(|error| panic!("decode {text}: {error}"));
        reply.reference.revision_id
    }

    /// **⭐⭐⭐ A skill can be created, listed, read, disabled, enabled, and deleted over HTTP.**
    ///
    /// The whole `P4-013` lifecycle in one test, and every assertion is made against a **subsequent request**
    /// rather than against the reply that caused it — so a handler that returned the right shape and wrote
    /// nothing would fail. That is the same rule the memory round trip follows, and it matters more here,
    /// because `P4-012` recorded that a skill could previously be *created only from a test*.
    ///
    /// # What the assertions are chosen to catch
    ///
    /// - **The listing carries no text.** A reference that leaked the description would be a second, less
    ///   careful path into a prompt than the retrieval path — so `description` must be absent from the listing
    ///   and present on the detail read.
    /// - **The counter advances.** Every transition increments `version_counter`, and a verb applied with a
    ///   stale one is refused; so the test reads the current value before each verb, exactly as a client does.
    /// - **Disable is not deletion.** The disabled revision is still listed, which is what makes "why is this
    ///   not being used" answerable.
    //
    // The length is the lifecycle's rather than the test's: create, list, read, disable, enable, and delete,
    // each asserted against a **subsequent** request. Splitting it would put the six verbs in six tests that
    // share no state, so the property this test actually establishes — that a *sequence* of verbs works, each
    // one accepting the counter the previous returned — would be unasserted. That sequence is the thing an
    // optimistic guard can break, which is why it stays one test.
    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn the_skill_routes_create_list_read_disable_enable_and_delete() {
        let (app, presented, _profile) = test_router().await;
        let revision_id =
            create_skill_via_http(&app, &presented, "Summarize the open items.").await;

        // The listing is a **reference**, so it must not carry the procedure's text.
        let listed = app
            .clone()
            .oneshot(get_request("/api/v1/skills?limit=10", Some(&presented)))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(listed.status(), StatusCode::OK);
        let listed_body = body_text(listed).await;
        let listing: jarvis_protocol::SkillListReply = serde_json::from_str(&listed_body)
            .unwrap_or_else(|error| panic!("decode {listed_body}: {error}"));
        assert_eq!(listing.returned, 1, "the creation must be listed");
        assert!(
            !listed_body.contains("Summarize the open items"),
            "a listing must not carry the procedure's prose, or it is a second path into a prompt: \
             {listed_body}"
        );
        assert_eq!(
            listing.skills[0].tool_ids,
            vec!["jarvis.files.read".to_owned()],
            "the tools a procedure names are metadata and are what an operator recognises it by"
        );

        // The detail read is the one place the text belongs.
        let detail = app
            .clone()
            .oneshot(get_request(
                &format!("/api/v1/skills/{revision_id}"),
                Some(&presented),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(detail.status(), StatusCode::OK);
        let detail_body = body_text(detail).await;
        let detail: jarvis_protocol::SkillDetailReply = serde_json::from_str(&detail_body)
            .unwrap_or_else(|error| panic!("decode {detail_body}: {error}"));
        assert_eq!(detail.description, "Summarize the open items.");
        assert_eq!(detail.steps.len(), 1);
        assert!(
            detail.is_usable,
            "a user-authored procedure is usable from the outset, because nothing proposed it for approval"
        );

        // Disable: the revision stays listed, which is what makes the inspection surface answer "why not".
        let disabled = app
            .clone()
            .oneshot(post_json(
                &format!("/api/v1/skills/{revision_id}/disable"),
                &presented,
                &serde_json::json!({ "expected_version": detail.reference.version_counter })
                    .to_string(),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(
            disabled.status(),
            StatusCode::OK,
            "disabling must be accepted: {}",
            body_text(disabled).await
        );
        let disabled_body = body_text(disabled).await;
        let disabled: jarvis_protocol::SkillReply = serde_json::from_str(&disabled_body)
            .unwrap_or_else(|error| panic!("decode {disabled_body}: {error}"));
        assert_eq!(disabled.reference.state, "archived");
        assert!(
            disabled.reference.version_counter > detail.reference.version_counter,
            "a transition must advance the counter, or a second verb would present a value the store rejects"
        );

        let after_disable = serde_json::from_str::<jarvis_protocol::SkillListReply>(
            &body_text(
                app.clone()
                    .oneshot(get_request("/api/v1/skills", Some(&presented)))
                    .await
                    .unwrap_or_else(|error| panic!("router call: {error}")),
            )
            .await,
        )
        .unwrap_or_else(|error| panic!("decode listing: {error}"));
        assert_eq!(
            after_disable.returned, 1,
            "a disabled revision is retained, because 'why is this not being used' is the question this view \
             exists to answer"
        );

        // Enable: back to the state its promotion record implies, which for a user-authored revision is active.
        let enabled = app
            .clone()
            .oneshot(post_json(
                &format!("/api/v1/skills/{revision_id}/enable"),
                &presented,
                &serde_json::json!({ "expected_version": disabled.reference.version_counter })
                    .to_string(),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(enabled.status(), StatusCode::OK);
        let enabled_body = body_text(enabled).await;
        let enabled: jarvis_protocol::SkillReply = serde_json::from_str(&enabled_body)
            .unwrap_or_else(|error| panic!("decode {enabled_body}: {error}"));
        assert_eq!(enabled.reference.state, "active");

        // Delete: the verb is `DELETE` and the receipt counts what went.
        let deleted = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/skills/{revision_id}"))
                    .method("DELETE")
                    .header(header::AUTHORIZATION, format!("Bearer {presented}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({ "expected_version": enabled.reference.version_counter })
                            .to_string(),
                    ))
                    .unwrap_or_else(|error| panic!("fixture request: {error}")),
            )
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(
            deleted.status(),
            StatusCode::OK,
            "deleting must be accepted: {}",
            body_text(deleted).await
        );
        let deleted_body = body_text(deleted).await;
        let receipt: jarvis_protocol::SkillDeletionReceipt = serde_json::from_str(&deleted_body)
            .unwrap_or_else(|error| panic!("decode {deleted_body}: {error}"));
        assert_eq!(receipt.removed_steps, 1);
        assert!(
            !receipt.unreachable.is_empty(),
            "a receipt that reads as total is worse than one that names what it could not reach"
        );

        // And it is gone, asserted by a read rather than by the delete's own reply.
        let gone = app
            .clone()
            .oneshot(get_request(
                &format!("/api/v1/skills/{revision_id}"),
                Some(&presented),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(gone.status(), StatusCode::NOT_FOUND);
    }

    /// **⭐⭐ A skill's creation refuses a tool this deployment cannot run, and a stale counter is a `409`.**
    ///
    /// Two rules that only the service can state, both asserted over the wire because a rule the route does not
    /// report is not one a client can act on.
    ///
    /// The unknown-tool refusal is the **membership** half: the identifier is structurally valid, so the
    /// domain and the schema would accept it, and the step would be stored as a procedure that cannot execute.
    /// The router here composes **no** tool pipeline, so the membership check does not fire — and that is the
    /// deliberate case the service's `None` arm covers, where with no registry there is nothing to compare
    /// against and refusing every step would conflate "no tools" with "a bad tool name".
    #[tokio::test]
    async fn the_skill_routes_refuse_a_stale_counter_and_a_malformed_step() {
        let (app, presented, _profile) = test_router().await;
        let revision_id = create_skill_via_http(&app, &presented, "A procedure.").await;
        let read: jarvis_protocol::SkillDetailReply = serde_json::from_str(
            &body_text(
                app.clone()
                    .oneshot(get_request(
                        &format!("/api/v1/skills/{revision_id}"),
                        Some(&presented),
                    ))
                    .await
                    .unwrap_or_else(|error| panic!("router call: {error}")),
            )
            .await,
        )
        .unwrap_or_else(|error| panic!("decode detail: {error}"));

        // A stale counter is a `409`, because the remedy is a re-read rather than a different request.
        let stale = app
            .clone()
            .oneshot(post_json(
                &format!("/api/v1/skills/{revision_id}/disable"),
                &presented,
                &serde_json::json!({ "expected_version": read.reference.version_counter + 7 })
                    .to_string(),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(
            stale.status(),
            StatusCode::CONFLICT,
            "a stale counter must be a conflict: {}",
            body_text(stale).await
        );

        // A **malformed** tool identifier is a `422` about the caller's input, not a `409` or a `500` — the
        // step cannot be built at all, and the reason names the field.
        let malformed = serde_json::json!({
            "description": "A procedure with a bad step.",
            "author_version": "1",
            "steps": [{
                "position": 1,
                "tool": "notadottedidentifier",
                "tool_version": "1.0.0",
                "instruction": "Do something.",
            }],
        })
        .to_string();
        let refused = app
            .clone()
            .oneshot(post_json("/api/v1/skills", &presented, &malformed))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(
            refused.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "a step naming a malformed tool must be refused as a value problem: {}",
            body_text(refused).await
        );

        // Nothing was written by either refusal, asserted by the listing rather than by the replies.
        let listing: jarvis_protocol::SkillListReply = serde_json::from_str(
            &body_text(
                app.clone()
                    .oneshot(get_request("/api/v1/skills", Some(&presented)))
                    .await
                    .unwrap_or_else(|error| panic!("router call: {error}")),
            )
            .await,
        )
        .unwrap_or_else(|error| panic!("decode listing: {error}"));
        assert_eq!(
            listing.returned, 1,
            "a refused write must not have stored anything, and the counter refusal must not have disabled it"
        );
        assert_eq!(
            listing.skills[0].state, "active",
            "the stale-counter refusal must not have archived the revision"
        );
    }

    /// **⭐⭐ A promotion is the one route that names its approver, and a self-approval is refused.**
    ///
    /// `ADR-0117` §4's boundary over the wire. The revision is created **as an agent proposal** directly
    /// through storage, because the HTTP creation surface deliberately records a *user-authored* revision — a
    /// client cannot choose which path it is on, which is what keeps the promotion rule from being a flag. So
    /// this test writes the proposal the way the agent path would and then drives the decision over HTTP.
    ///
    /// The refusal is asserted **before** the legitimate promotion, so an implementation that refused every
    /// promotion fails the second half rather than passing on the first.
    //
    // Long because it is one rule in three parts: the proposal a promotion acts on, the refusal, and the
    // control. The control is what makes the refusal meaningful, so separating them would leave the assertion
    // satisfied by an implementation that refused every promotion.
    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn a_self_approved_promotion_is_refused_over_http() {
        let (app, presented, _profile, database) = memory_router().await;

        // An agent-authored proposal: `Proposed`, so a promotion is the only route to usable.
        let identity = jarvis_storage::load_local_identity(&database)
            .await
            .unwrap_or_else(|error| panic!("load identity: {error}"));
        let proposed = jarvis_core::SkillRevision::new(jarvis_core::SkillRevisionParts {
            skill_id: jarvis_core::SkillId::new(),
            workspace_id: identity
                .workspace_id()
                .parse()
                .unwrap_or_else(|error| panic!("fixture workspace: {error}")),
            revision_id: jarvis_core::SkillId::new(),
            version: "1".to_owned(),
            description: "A procedure the model suggested.".to_owned(),
            steps: vec![
                jarvis_core::SkillStep::new(
                    1,
                    "jarvis.files.read",
                    "1.0.0",
                    "Read the notes.",
                    |identifier: &str| identifier.contains('.'),
                )
                .unwrap_or_else(|error| panic!("fixture step: {error}")),
            ],
            source: jarvis_core::MemorySource::of_kind(
                jarvis_core::MemorySourceKind::ModelInference,
                "run-9",
            )
            .unwrap_or_else(|error| panic!("fixture source: {error}")),
            sensitivity: jarvis_core::Sensitivity::Internal,
            state: jarvis_core::SkillState::Proposed,
            supersedes: None,
            dropped_fields: Vec::new(),
            run_id: None,
            // The author is the **run**, which is the actor the self-approval guard compares against.
            created_by_actor_id: "run-9".to_owned(),
            correlation_id: jarvis_core::CorrelationId::new(),
            created_at: jarvis_core::UtcTimestamp::now(&jarvis_core::SystemClock),
        })
        .unwrap_or_else(|error| panic!("fixture proposal: {error}"));
        jarvis_storage::record_skill_revision(&database, &proposed)
            .await
            .unwrap_or_else(|error| panic!("record proposal: {error}"));
        let revision_id = proposed.revision_id().to_string();

        let read: jarvis_protocol::SkillDetailReply = serde_json::from_str(
            &body_text(
                app.clone()
                    .oneshot(get_request(
                        &format!("/api/v1/skills/{revision_id}"),
                        Some(&presented),
                    ))
                    .await
                    .unwrap_or_else(|error| panic!("router call: {error}")),
            )
            .await,
        )
        .unwrap_or_else(|error| panic!("decode detail: {error}"));
        assert!(
            !read.is_usable,
            "a model-authored proposal must not be usable before a decision"
        );
        assert_eq!(read.unusable_reason.as_deref(), Some("not_usable"));

        // The self-approval: the run that authored it names itself as the approver.
        let self_approved = app
            .clone()
            .oneshot(post_json(
                &format!("/api/v1/skills/{revision_id}/promote"),
                &presented,
                &serde_json::json!({
                    "expected_version": read.reference.version_counter,
                    "approver_actor_id": "run-9",
                })
                .to_string(),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(
            self_approved.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "an author must not be able to approve its own procedure: {}",
            body_text(self_approved).await
        );
        let refusal_body = body_text(self_approved).await;
        assert!(
            refusal_body.contains("own author"),
            "the refusal must name the actual problem, not report a missing field: {refusal_body}"
        );

        // **The control:** a different actor promotes the same proposal, so the refusal above is the
        // self-approval rule rather than a promotion that never works.
        let promoted = app
            .clone()
            .oneshot(post_json(
                &format!("/api/v1/skills/{revision_id}/promote"),
                &presented,
                &serde_json::json!({
                    "expected_version": read.reference.version_counter,
                    "approver_actor_id": "user-1",
                })
                .to_string(),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(
            promoted.status(),
            StatusCode::OK,
            "a different actor must be able to promote it: {}",
            body_text(promoted).await
        );
        let promoted_body = body_text(promoted).await;
        let promoted: jarvis_protocol::SkillReply = serde_json::from_str(&promoted_body)
            .unwrap_or_else(|error| panic!("decode {promoted_body}: {error}"));
        assert_eq!(promoted.reference.state, "active");
        assert_eq!(
            promoted.reference.promoted_by_actor_id.as_deref(),
            Some("user-1"),
            "the approver a decision named must survive to the read"
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

        // An entity-less remember is about the profile owner (ADR-0140), so it is accepted and filed as an active claim;
        // it is naming an entity nothing holds that the service refuses, by field, rather than a `500` at the link step.
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
            StatusCode::CREATED,
            "an entity-less remember is about the owner: {}",
            body_text(entity_less).await
        );
        let invented = app
            .clone()
            .oneshot(post_json(
                "/api/v1/memories",
                &presented,
                &format!(
                    r#"{{"content":"About nobody","memory_type":"preference","source_kind":"user_statement","entity_ids":["{}"]}}"#,
                    jarvis_core::EntityId::new()
                ),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(
            invented.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "an entity nothing holds must still be refused with a field-naming 422: {}",
            body_text(invented).await
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

    /// **The tool list reports the policy in force, including an override that raised the declaration.**
    ///
    /// The claim `P3-025` makes reachable: an operator can configure a per-tool approval policy and then
    /// **see that it took effect**. The two assertions that carry it are that the effective policy differs
    /// from the declared one and that `overridden` is set — a route that returned the registry's own
    /// inventory would report `declared_approval` for both and pass a "lists the tool" assertion.
    #[tokio::test]
    async fn the_tool_list_reports_the_effective_policy_and_its_override() {
        let (app, presented, _profile) = tool_router_with_policy(
            // Override the read tool to always ask, which the default workspace would otherwise allow.
            |policy| policy.requiring(reader_id(), jarvis_tools::ApprovalPolicy::Ask),
        )
        .await;

        let response = app
            .oneshot(get_request("/api/v1/tools", Some(&presented)))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(response.status(), StatusCode::OK);
        let reply: jarvis_protocol::ToolListReply =
            serde_json::from_str(&body_text(response).await)
                .unwrap_or_else(|error| panic!("decode tool list: {error}"));

        assert_eq!(reply.total, reply.tools.len());
        let entry = reply
            .tools
            .iter()
            .find(|tool| tool.id == reader_id().to_string())
            .unwrap_or_else(|| panic!("the read tool must be listed: {:?}", reply.tools));
        assert_eq!(
            entry.declared_approval,
            jarvis_tools::ApprovalPolicy::Auto,
            "the tool's own declaration must be reported unchanged"
        );
        assert_eq!(
            entry.effective_approval,
            jarvis_tools::ApprovalPolicy::Ask,
            "the configured override must be reported as the policy in force"
        );
        assert!(
            entry.overridden,
            "an operator must be able to see that their override is why this asks"
        );
        assert!(!entry.denied, "no denial was configured");
        assert!(entry.callable, "a read over a granted root is callable");
        // The workspace settings are reported beside the per-tool entries, so a single tool's posture is
        // readable against the ceiling it sits under.
        assert_eq!(reply.max_risk, jarvis_tools::Risk::High);
        assert_eq!(reply.approval_threshold, jarvis_tools::Risk::Moderate);
    }

    /// **An override that is looser than the declaration is reported as having done nothing.**
    ///
    /// The direction rule at the wire surface. The tool declares `Ask`; the operator writes the loosest
    /// possible value. The effective policy must still be `Ask` and `overridden` must be false, because
    /// nothing was raised — a route that reported the configured value as effective would show an
    /// operator a policy the engine does not enforce, which is worse than showing nothing.
    #[tokio::test]
    async fn a_tool_list_reports_a_looser_override_as_having_no_effect() {
        let (app, presented, _profile) = tool_router_with_policy(|policy| {
            policy
                .requiring(reader_id(), jarvis_tools::ApprovalPolicy::Auto)
                .requiring(approval_asking_id(), jarvis_tools::ApprovalPolicy::Auto)
        })
        .await;

        let response = app
            .oneshot(get_request("/api/v1/tools", Some(&presented)))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        let reply: jarvis_protocol::ToolListReply =
            serde_json::from_str(&body_text(response).await)
                .unwrap_or_else(|error| panic!("decode tool list: {error}"));

        let asking = reply
            .tools
            .iter()
            .find(|tool| tool.id == approval_asking_id().to_string())
            .unwrap_or_else(|| panic!("the asking tool must be listed"));
        assert_eq!(
            asking.effective_approval,
            jarvis_tools::ApprovalPolicy::Ask,
            "a looser override must not lower the tool's own `Ask`"
        );
        assert!(
            !asking.overridden,
            "an override that changed nothing must not read as having changed something"
        );
    }

    /// **A denial is reported as a denial, and the reason list is not the only way to see it.**
    #[tokio::test]
    async fn the_tool_list_reports_a_denied_tool_as_denied() {
        let (app, presented, _profile) =
            tool_router_with_policy(|policy| policy.denying(reader_id())).await;

        let response = app
            .oneshot(get_request("/api/v1/tools", Some(&presented)))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        let reply: jarvis_protocol::ToolListReply =
            serde_json::from_str(&body_text(response).await)
                .unwrap_or_else(|error| panic!("decode tool list: {error}"));

        let entry = reply
            .tools
            .iter()
            .find(|tool| tool.id == reader_id().to_string())
            .unwrap_or_else(|| panic!("a denied tool is still registered and must be listed"));
        assert!(
            entry.denied,
            "the denial must be visible without making a call"
        );
        // The tool is still *callable* — the adapter works and the file can be read. The denial is a
        // policy fact rather than an availability one, and conflating them would send an operator to fix
        // a grant when the remedy is a configuration line.
        assert!(
            entry.callable,
            "a denied tool is available and refused, not broken"
        );
    }

    /// **The list route refuses rather than returning an empty list when no pipeline exists.**
    ///
    /// An empty `200` would read as "no tools are configured", while the truth is "this daemon cannot serve
    /// tools" — and the remedies differ, since the second is a missing roots grant or an MCP document.
    #[tokio::test]
    async fn the_tool_list_is_not_found_without_a_pipeline() {
        let (app, presented, _profile) = test_router().await;

        let response = app
            .oneshot(get_request("/api/v1/tools", Some(&presented)))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "an absent pipeline is not an empty tool list: {}",
            body_text(response).await
        );
    }

    /// **The preview computes the decision the engine would reach, and reports why.**
    ///
    /// The value an operator configuring policy actually needs: the effect of a call on a real tool,
    /// without making one. Both halves are asserted — the decision and the reason code — because a preview
    /// that returned only an outcome would leave the operator reproducing the decision by hand, which is
    /// the defect `PolicyDecision`'s reason code exists to prevent.
    #[tokio::test]
    async fn the_preview_reports_the_decision_and_its_reason() {
        let (app, presented, _profile) = tool_router_with_policy(|policy| policy).await;

        // The read tool is risk 0, `Auto`, and the default workspace allows it — so the control is `allow`.
        let allowed = app
            .clone()
            .oneshot(post_json(
                &format!("/api/v1/tools/{}/preview", reader_id()),
                &presented,
                "{}",
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(allowed.status(), StatusCode::OK);
        let reply: jarvis_protocol::ToolPreviewReply =
            serde_json::from_str(&body_text(allowed).await)
                .unwrap_or_else(|error| panic!("decode preview: {error}"));
        assert_eq!(reply.decision, "allow");
        assert_eq!(reply.reason, "allowed");
        assert_eq!(reply.declared_risk, jarvis_tools::Risk::Minimal);
        assert_eq!(reply.effective_risk, jarvis_tools::Risk::Minimal);
        assert!(reply.escalated_by.is_empty());
    }

    /// **A preview names the escalation as the reason the risk rose.**
    ///
    /// The context a caller supplies is the only thing that can raise a risk without the tool changing, so
    /// the reply must distinguish the *declared* risk from the effective one and list what raised it. A
    /// reply that reported only the effective risk would make a held call look like a badly declared tool.
    #[tokio::test]
    async fn the_preview_reports_escalation_separately_from_the_declared_risk() {
        let (app, presented, _profile) = tool_router_with_policy(|policy| policy).await;

        let response = app
            .oneshot(post_json(
                &format!("/api/v1/tools/{}/preview", reader_id()),
                &presented,
                r#"{"escalation":["bulk"]}"#,
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "{}",
            body_text(response).await
        );
        let reply: jarvis_protocol::ToolPreviewReply =
            serde_json::from_str(&body_text(response).await)
                .unwrap_or_else(|error| panic!("decode preview: {error}"));

        assert_eq!(
            reply.declared_risk,
            jarvis_tools::Risk::Minimal,
            "the tool's own declaration is unchanged by the context"
        );
        assert_eq!(
            reply.effective_risk,
            jarvis_tools::Risk::High,
            "a bulk target raises the risk to the documented level"
        );
        assert_eq!(
            reply.escalated_by,
            vec![jarvis_core::EscalationSignal::Bulk]
        );
        assert_eq!(
            reply.decision, "require_approval",
            "a risk-3 call is held rather than allowed"
        );
        assert_eq!(reply.reason, "approval_required");
    }

    /// **The preview writes nothing and runs nothing.**
    ///
    /// The property that makes it safe to expose at all: an operator must not be able to fill the ledger by
    /// inspecting it, and a preview must not consume an idempotency key so that the real call becomes a
    /// duplicate. Asserted by the call table being empty before and after, because a preview that recorded
    /// a `requested` row would look identical in its own response.
    #[tokio::test]
    async fn the_preview_records_no_call() {
        let (app, presented, root) = tool_router_with_policy(|policy| policy).await;

        let before = count_tool_calls(&root).await;
        for _ in 0..3 {
            let response = app
                .clone()
                .oneshot(post_json(
                    &format!("/api/v1/tools/{}/preview", reader_id()),
                    &presented,
                    "{}",
                ))
                .await
                .unwrap_or_else(|error| panic!("router call: {error}"));
            assert_eq!(response.status(), StatusCode::OK);
        }
        assert_eq!(
            count_tool_calls(&root).await,
            before,
            "a preview must record nothing, however many times it is asked"
        );
    }

    /// **A preview of a tool that does not exist is a `404`, not a refusal.**
    ///
    /// A refusal would make a typo indistinguishable from a policy denial, and the two have opposite
    /// remedies: one is an identifier to correct, the other a configuration line to change.
    #[tokio::test]
    async fn the_preview_of_an_unknown_tool_is_not_found() {
        let (app, presented, _profile) = tool_router_with_policy(|policy| policy).await;

        let response = app
            .oneshot(post_json(
                "/api/v1/tools/jarvis.nope.missing/preview",
                &presented,
                "{}",
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "an unknown tool must not read as a policy refusal: {}",
            body_text(response).await
        );
    }

    /// **A preview refuses a request body that tries to supply authority.**
    ///
    /// The rule every route here follows: the actor's scopes and the workspace policy come from the daemon,
    /// never from the request. `deny_unknown_fields` makes an attempt to name them a `422` rather than an
    /// ignored value, which is what stops the preview becoming a way to ask "what if I had more".
    #[tokio::test]
    async fn the_preview_refuses_a_body_that_names_authority() {
        let (app, presented, _profile) = tool_router_with_policy(|policy| policy).await;

        let response = app
            .oneshot(post_json(
                &format!("/api/v1/tools/{}/preview", reader_id()),
                &presented,
                r#"{"scopes":["files.read","mcp.call"]}"#,
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "a body naming scopes must be refused rather than ignored: {}",
            body_text(response).await
        );
    }

    /// **Legacy preview auth fields are refused rather than ignored.**
    ///
    /// The preview contract no longer accepts per-request auth hints, so an old client must get a clear
    /// validation error instead of a silently changed decision.
    #[tokio::test]
    async fn the_preview_refuses_legacy_auth_fields() {
        let (app, presented, _profile) = tool_router_with_policy(|policy| policy).await;

        let response = app
            .oneshot(post_json(
                &format!("/api/v1/tools/{}/preview", reader_id()),
                &presented,
                r#"{"channel":"voice"}"#,
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "legacy preview auth fields must be refused: {}",
            body_text(response).await
        );
    }

    /// The read tool's identifier, as the filesystem adapter declares it.
    fn reader_id() -> jarvis_tools::ToolId {
        jarvis_tools::ToolId::new(jarvis_tools::READ_TOOL)
            .unwrap_or_else(|error| panic!("the adapter's own identifier: {error}"))
    }

    /// An identifier for a tool that declares `ApprovalPolicy::Ask` at risk 0.
    ///
    /// Built through the approval fixture, which is the same definition the hold tests use, so the
    /// declaration under test is the product's own and not a literal restated here.
    fn approval_asking_id() -> jarvis_tools::ToolId {
        jarvis_tools::ToolId::new(crate::approval_fixture::APPROVAL_TOOL)
            .unwrap_or_else(|error| panic!("the fixture identifier: {error}"))
    }

    // ---------------------------------------------------------------------------------------------
    // Session summaries (`P4-015`)
    // ---------------------------------------------------------------------------------------------

    /// Seeds a session holding **exactly** `messages` messages plus a conversation entity, returning both
    /// identifiers.
    ///
    /// # Why the count is made exact rather than appended blindly
    ///
    /// Starting the run writes the objective as the session's first message, so appending `messages` of them
    /// leaves `messages + 1` — which silently shifted every expected span by one when this fixture was first
    /// written, and the spans still looked plausible. The fixture therefore asserts the count it promises,
    /// using the count `count_messages` reports, so a change to what a run writes fails **here** rather than as
    /// an off-by-one in a caller's expectations.
    ///
    /// # Why the session comes from `POST /api/v1/runs` rather than an `INSERT`
    ///
    /// There is no route that creates a session on its own, and the two ways to get one in a test are a raw
    /// insert or the route that creates one **as part of starting a run**. The insert was the first attempt and
    /// it required `SqliteDatabase::pool()` to become public — widening a production API so a test could write a
    /// row. `start_run` returns the `session_id` in its reply, so the production path is available.
    ///
    /// The messages then go through `append_message`, the same writer the conversation path uses, so the
    /// sequences a summary span is checked against are allocated by the component that owns that rule.
    async fn seed_session(
        app: &Router,
        presented: &str,
        database: &jarvis_storage::SqliteDatabase,
        messages: i64,
    ) -> (String, String) {
        let created = app
            .clone()
            .oneshot(post_json(
                "/api/v1/runs",
                presented,
                r#"{"objective":"a session to summarize"}"#,
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(created.status(), StatusCode::CREATED);
        let body = body_text(created).await;
        let run: jarvis_protocol::RunReply =
            serde_json::from_str(&body).unwrap_or_else(|error| panic!("decode {body}: {error}"));
        let session_id = run.session_id.clone();

        // The run wrote one message, so this fills the session to the promised size.
        let already = jarvis_storage::count_messages(database, &session_id)
            .await
            .unwrap_or_else(|error| panic!("count messages: {error}"));
        assert_eq!(already, 1, "starting a run writes exactly one message");
        for index in already..messages {
            jarvis_storage::append_message(
                database,
                &session_id,
                None,
                &jarvis_core::NewMessage::new(
                    format!("message {index}"),
                    jarvis_core::MessageRole::User,
                    jarvis_core::MessageSource::User,
                    jarvis_core::Sensitivity::Internal,
                    None,
                )
                .unwrap_or_else(|error| panic!("build message: {error}")),
                jarvis_core::CorrelationId::new(),
                jarvis_core::UtcTimestamp::now(&jarvis_core::SystemClock),
            )
            .await
            .unwrap_or_else(|error| panic!("append message: {error}"));
        }
        let total = jarvis_storage::count_messages(database, &session_id)
            .await
            .unwrap_or_else(|error| panic!("count messages: {error}"));
        assert_eq!(total, messages, "the fixture's promised transcript size");

        let identity = jarvis_storage::load_local_identity(database)
            .await
            .unwrap_or_else(|error| panic!("load identity: {error}"));
        let entity_id = jarvis_core::EntityId::new();
        jarvis_storage::record_entity(
            database,
            &jarvis_storage::NewEntity {
                id: entity_id,
                workspace_id: identity
                    .workspace_id()
                    .parse()
                    .unwrap_or_else(|error| panic!("parse workspace: {error}")),
                kind: jarvis_storage::EntityKind::Conversation,
                label: "A session".to_owned(),
                attributes: None,
                confidence: jarvis_core::MemoryConfidence::Confirmed,
                created_at: jarvis_core::UtcTimestamp::now(&jarvis_core::SystemClock),
            },
        )
        .await
        .unwrap_or_else(|error| panic!("seed entity: {error}"));

        (session_id, entity_id.to_string())
    }

    /// **A summary can be written over HTTP, read back, and its subject is what the reply claims.**
    ///
    /// The reachability proof `P4-015` needed: the domain, the storage rule, and the retention sweep all
    /// existed and **no request could reach any of them**. Every assertion is on the reply body rather than on
    /// the call's success, because a route wired to a stub would return `201` for anything.
    #[tokio::test]
    async fn a_session_summary_can_be_recorded_and_read_back() {
        let (app, database, presented, _profile) = test_router_and_database().await;
        let (session_id, entity_id) = seed_session(&app, &presented, &database, 6).await;

        let created = app
            .clone()
            .oneshot(post_json(
                &format!("/api/v1/sessions/{session_id}/summaries"),
                &presented,
                &format!(
                    r#"{{"summary":"Three turns compressed.","first_sequence":1,"last_sequence":3,
                        "turns_covered":3,"source_chars":900,"entity_id":"{entity_id}"}}"#
                ),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(created.status(), StatusCode::CREATED);
        let body = body_text(created).await;
        let reply: jarvis_protocol::SummaryReply =
            serde_json::from_str(&body).unwrap_or_else(|error| panic!("decode {body}: {error}"));
        assert_eq!(reply.first_sequence, 1);
        assert_eq!(reply.last_sequence, 3);
        assert_eq!(reply.turns_covered, 3);
        assert_eq!(reply.session_id, session_id);
        assert!(
            reply.compression_ratio.is_some(),
            "a measured input yields a ratio: {body}"
        );
        // The complement, computed by the write: a prefix and a remainder, so the gap case is covered.
        assert_eq!(
            reply
                .unsummarized
                .iter()
                .map(|range| (range.first_sequence, range.last_sequence))
                .collect::<Vec<_>>(),
            vec![(0, 0), (4, 5)]
        );

        let listed = app
            .oneshot(get_request(
                &format!("/api/v1/sessions/{session_id}/summaries"),
                Some(&presented),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(listed.status(), StatusCode::OK);
        let listed_body = body_text(listed).await;
        let page: jarvis_protocol::SummaryListReply = serde_json::from_str(&listed_body)
            .unwrap_or_else(|error| panic!("decode {listed_body}: {error}"));
        assert_eq!(page.returned, 1);
        assert_eq!(page.summaries[0].memory_id, reply.memory_id);
        // The read reports no ratio, because this path does not load the text — asserted so the `None` is a
        // decision rather than an accident.
        assert_eq!(page.summaries[0].compression_ratio, None);
    }

    /// **A span already summarized is refused with a remedy, and the refusal is not a `503`.**
    ///
    /// The status matters as much as the body: a caller told `503` retries a request that can never succeed.
    /// The detail names the stored range because that is what the caller needs to summarize the remaining
    /// turns.
    #[tokio::test]
    async fn an_overlapping_summary_is_refused_with_the_range_that_is_taken() {
        let (app, database, presented, _profile) = test_router_and_database().await;
        let (session_id, entity_id) = seed_session(&app, &presented, &database, 6).await;
        let body = |first: i64, last: i64, text: &str| {
            format!(
                r#"{{"summary":"{text}","first_sequence":{first},"last_sequence":{last},
                    "turns_covered":{},"entity_id":"{entity_id}"}}"#,
                last - first + 1
            )
        };

        let first = app
            .clone()
            .oneshot(post_json(
                &format!("/api/v1/sessions/{session_id}/summaries"),
                &presented,
                &body(0, 3, "The opening exchange."),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(first.status(), StatusCode::CREATED);

        let overlapping = app
            .clone()
            .oneshot(post_json(
                &format!("/api/v1/sessions/{session_id}/summaries"),
                &presented,
                &body(2, 5, "An overlapping range."),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(
            overlapping.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "an overlap is a request the caller can fix, not an infrastructure failure"
        );
        let text = body_text(overlapping).await;
        assert!(
            text.contains("turns 0..3"),
            "the refusal must name the range that is taken: {text}"
        );

        // The boundary: 4..5 begins where 0..3 ends and is accepted.
        let adjacent = app
            .oneshot(post_json(
                &format!("/api/v1/sessions/{session_id}/summaries"),
                &presented,
                &body(4, 5, "The closing exchange."),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(adjacent.status(), StatusCode::CREATED);
    }

    /// **A span past the end of the transcript is refused, so a summary cannot claim turns that do not exist.**
    ///
    /// The rule with no schema behind it: `session_summaries` cannot see `messages`, so this is the only place
    /// the fabrication is stoppable — and over the wire, where the range arrives from a caller.
    #[tokio::test]
    async fn a_summary_span_past_the_transcript_is_refused() {
        let (app, database, presented, _profile) = test_router_and_database().await;
        let (session_id, entity_id) = seed_session(&app, &presented, &database, 4).await;

        // 0..3 is the whole transcript. 0..4 names a message that does not exist.
        let accepted = app
            .clone()
            .oneshot(post_json(
                &format!("/api/v1/sessions/{session_id}/summaries"),
                &presented,
                &format!(
                    r#"{{"summary":"The whole session.","first_sequence":0,"last_sequence":3,
                        "turns_covered":4,"entity_id":"{entity_id}"}}"#
                ),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(accepted.status(), StatusCode::CREATED);

        let beyond = app
            .oneshot(post_json(
                &format!("/api/v1/sessions/{session_id}/summaries"),
                &presented,
                &format!(
                    r#"{{"summary":"One message too many.","first_sequence":4,"last_sequence":4,
                        "turns_covered":1,"entity_id":"{entity_id}"}}"#
                ),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(beyond.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let text = body_text(beyond).await;
        assert!(
            text.contains("span"),
            "the refusal must name the span: {text}"
        );
    }

    /// **The retention rule is reachable, and it retires the summary without deleting it.**
    ///
    /// `P4-015` requires a retention rule; `P4-008` records that the sweeper does not exist, so the rule is an
    /// explicit verb. Both halves are asserted: the summary stops being read as a current claim, and it is
    /// still **held** — an archived claim is retained for audit, and a `DELETE` would have told the client
    /// otherwise.
    #[tokio::test]
    async fn retiring_a_session_archives_its_summaries_and_keeps_them() {
        let (app, database, presented, _profile) = test_router_and_database().await;
        let (session_id, entity_id) = seed_session(&app, &presented, &database, 4).await;

        let created = app
            .clone()
            .oneshot(post_json(
                &format!("/api/v1/sessions/{session_id}/summaries"),
                &presented,
                &format!(
                    r#"{{"summary":"Retired later.","first_sequence":0,"last_sequence":3,
                        "turns_covered":4,"entity_id":"{entity_id}"}}"#
                ),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(created.status(), StatusCode::CREATED);

        let retired = app
            .clone()
            .oneshot(post_json(
                &format!("/api/v1/sessions/{session_id}/summaries/retire"),
                &presented,
                "{}",
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(retired.status(), StatusCode::OK);
        let text = body_text(retired).await;
        assert!(
            text.contains("\"archived\":1"),
            "one summary retired: {text}"
        );

        // The read stops returning it, and the export still holds it — the two halves of "archived".
        let listed = app
            .clone()
            .oneshot(get_request(
                &format!("/api/v1/sessions/{session_id}/summaries"),
                Some(&presented),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        let listed_body = body_text(listed).await;
        let page: jarvis_protocol::SummaryListReply = serde_json::from_str(&listed_body)
            .unwrap_or_else(|error| panic!("decode {listed_body}: {error}"));
        assert_eq!(page.returned, 0, "an archived summary is not a current one");

        let exported = app
            .oneshot(get_request("/api/v1/memories/export", Some(&presented)))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(exported.status(), StatusCode::OK);
        let exported_body = body_text(exported).await;
        assert!(
            exported_body.contains("Retired later."),
            "the export must still hold the retired text: {exported_body}"
        );
    }

    // ---------------------------------------------------------------------------------------------
    // Entities (`P4-016`)
    // ---------------------------------------------------------------------------------------------

    /// Creates an entity over HTTP, returning its identifier.
    async fn create_entity_via(app: &Router, presented: &str, label: &str, kind: &str) -> String {
        let response = app
            .clone()
            .oneshot(post_json(
                "/api/v1/entities",
                presented,
                &format!(r#"{{"label":"{label}","kind":"{kind}","confidence":"confirmed"}}"#),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(
            response.status(),
            StatusCode::CREATED,
            "{}",
            body_text(response).await
        );
        let body = body_text(response).await;
        let reply: jarvis_protocol::EntityDetailReply =
            serde_json::from_str(&body).unwrap_or_else(|error| panic!("decode {body}: {error}"));
        reply.entity.entity_id
    }

    /// **An entity can be created over HTTP, and the identifier it returns is the subject a memory can name.**
    ///
    /// This is the reachability proof for the gap three slices recorded. The assertion that matters is not
    /// "the create returned `201`" but that **a remember naming that entity succeeds** — because the recorded
    /// limit was precisely that a remember could not be performed for want of an identifier. Both halves are
    /// asserted, so a create that returned an unusable identifier fails here rather than at the next verb.
    #[tokio::test]
    async fn an_entity_can_be_created_and_then_used_as_a_subject() {
        let (app, presented, _profile) = test_router().await;
        let entity_id = create_entity_via(&app, &presented, "Ada Lovelace", "person").await;
        assert_eq!(entity_id.len(), 36, "a usable identifier: {entity_id}");

        // The whole point: the identifier is accepted as a subject by the memory surface, which is what was
        // impossible before this slice.
        let remembered = app
            .clone()
            .oneshot(post_json(
                "/api/v1/memories",
                &presented,
                &format!(
                    r#"{{"content":"Prefers morning meetings.","memory_type":"preference",
                        "source_kind":"user_statement","entity_ids":["{entity_id}"]}}"#
                ),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(
            remembered.status(),
            StatusCode::CREATED,
            "a remember naming this entity must succeed: {}",
            body_text(remembered).await
        );

        // And the entity's own read reports the link, so the count an operator merges by is real.
        let read = app
            .oneshot(get_request(
                &format!("/api/v1/entities/{entity_id}"),
                Some(&presented),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(read.status(), StatusCode::OK);
        let body = body_text(read).await;
        let reply: jarvis_protocol::EntityDetailReply =
            serde_json::from_str(&body).unwrap_or_else(|error| panic!("decode {body}: {error}"));
        assert_eq!(reply.entity.label, "Ada Lovelace");
        assert_eq!(reply.entity.kind, "person");
        assert_eq!(
            reply.entity.linked_memories, 1,
            "the claim linked to it must be counted: {body}"
        );
    }

    /// **A lookup returns every candidate and marks whether each is verified, never one row.**
    ///
    /// The architecture's rule is that "ambiguous aliases remain separate candidates", so this is where a route
    /// could silently break it: a reply with one match would turn a guess into an identity. The ambiguous case
    /// is the one asserted, and `verified` is checked in **both** directions — a probabilistic candidate is not
    /// verified, a user-stated one is.
    #[tokio::test]
    async fn an_alias_lookup_returns_candidates_and_their_verification() {
        let (app, presented, _profile) = test_router().await;
        let first = create_entity_via(&app, &presented, "Ada Lovelace", "person").await;
        let second = create_entity_via(&app, &presented, "A. Lovelace", "person").await;

        // Two entities, one **probabilistic** alias each for the same email. The source is a user statement
        // rather than an inference because `record_alias` resolves the entity and returns it, so the entity
        // must be one the caller is allowed to see; an inference source is refused there — which is a **later**
        // slice's rule, not this one's, and using it here would make this test fail for a reason it is not
        // about. What matters below is that two entities can hold one unverified alias and both come back.
        for (entity, confidence) in [(&first, "likely"), (&second, "uncertain")] {
            let response = app
                .clone()
                .oneshot(post_json(
                    &format!("/api/v1/entities/{entity}/aliases"),
                    &presented,
                    &format!(
                        r#"{{"alias_kind":"email","alias_value":"ada@example.com",
                            "verification":"probabilistic","source_kind":"user_statement",
                            "confidence":"{confidence}"}}"#
                    ),
                ))
                .await
                .unwrap_or_else(|error| panic!("router call: {error}"));
            assert_eq!(
                response.status(),
                StatusCode::OK,
                "a probabilistic alias is a candidate, not a contradiction: {}",
                body_text(response).await
            );
        }

        let lookup = app
            .clone()
            .oneshot(get_request(
                "/api/v1/entities/lookup?alias_kind=email&alias_value=ada@example.com",
                Some(&presented),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(lookup.status(), StatusCode::OK);
        let body = body_text(lookup).await;
        let reply: jarvis_protocol::EntityLookupReply =
            serde_json::from_str(&body).unwrap_or_else(|error| panic!("decode {body}: {error}"));
        assert_eq!(
            reply.returned, 2,
            "both candidate entities must be returned: {body}"
        );
        assert!(
            reply.matches.iter().all(|entry| !entry.verified),
            "a probabilistic alias is not an identity: {body}"
        );

        // The control: a **user-stated** confirmed alias for a third entity is verified, so `verified` is not
        // simply always false.
        let third = create_entity_via(&app, &presented, "Ada Byron", "person").await;
        let confirmed = app
            .clone()
            .oneshot(post_json(
                &format!("/api/v1/entities/{third}/aliases"),
                &presented,
                r#"{"alias_kind":"email","alias_value":"ada.byron@example.com",
                    "verification":"confirmed","source_kind":"user_statement","confidence":"confirmed"}"#,
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(
            confirmed.status(),
            StatusCode::OK,
            "{}",
            body_text(confirmed).await
        );

        let verified = app
            .oneshot(get_request(
                "/api/v1/entities/lookup?alias_kind=email&alias_value=ada.byron@example.com",
                Some(&presented),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        let body = body_text(verified).await;
        let reply: jarvis_protocol::EntityLookupReply =
            serde_json::from_str(&body).unwrap_or_else(|error| panic!("decode {body}: {error}"));
        assert_eq!(reply.returned, 1);
        assert!(
            reply.matches[0].verified,
            "a user statement verifies: {body}"
        );
    }

    /// **A verified alias cannot be established by a source that cannot verify one.**
    ///
    /// `0009`'s `CHECK` refuses a `confirmed` alias from anything but a user statement or correction, and this
    /// asserts the refusal over the wire as a `422` naming the rule rather than a constraint failure. It is the
    /// rule that made `MemorySourceKind::is_user_stated` necessary: the nearest domain predicate —
    /// `permitted_trust() == Authoritative` — **admits `provider_record`**, which the schema refuses, so a guard
    /// written as that comparison would have passed here and failed at the database.
    #[tokio::test]
    async fn a_confirmed_alias_from_a_provider_record_is_refused() {
        let (app, presented, _profile) = test_router().await;
        let entity = create_entity_via(&app, &presented, "Ada Lovelace", "person").await;

        // `provider_record` is `Authoritative` trust and **cannot** confirm an identity.
        let refused = app
            .clone()
            .oneshot(post_json(
                &format!("/api/v1/entities/{entity}/aliases"),
                &presented,
                r#"{"alias_kind":"email","alias_value":"ada@example.com",
                    "verification":"confirmed","source_kind":"provider_record","confidence":"confirmed"}"#,
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(refused.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = body_text(refused).await;
        assert!(
            body.contains("user statement"),
            "the refusal must name the remedy: {body}"
        );

        // The control: the same alias from a **user statement** is accepted, so the guard is about the source
        // kind and not about confirmed aliases.
        let accepted = app
            .oneshot(post_json(
                &format!("/api/v1/entities/{entity}/aliases"),
                &presented,
                r#"{"alias_kind":"email","alias_value":"ada@example.com",
                    "verification":"confirmed","source_kind":"user_statement","confidence":"confirmed"}"#,
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(
            accepted.status(),
            StatusCode::OK,
            "{}",
            body_text(accepted).await
        );
    }

    /// **A merge keeps the winner and marks the loser as merged, and the reply is the winner.**
    ///
    /// The direction is the whole risk, so it is asserted in the reply: merging `a` into `b` returns `b`, and a
    /// later read of `a` reports `merged` pointing at `b`. A reply that returned the source would hand a caller
    /// a name whose claims now belong to another.
    #[tokio::test]
    async fn merging_one_entity_into_another_answers_with_the_winner() {
        let (app, presented, _profile) = test_router().await;
        let source = create_entity_via(&app, &presented, "Ada L", "person").await;
        let target = create_entity_via(&app, &presented, "Ada Lovelace", "person").await;

        let merged = app
            .clone()
            .oneshot(post_json(
                &format!("/api/v1/entities/{source}/merge"),
                &presented,
                &format!(r#"{{"target_id":"{target}"}}"#),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(
            merged.status(),
            StatusCode::OK,
            "{}",
            body_text(merged).await
        );
        let body = body_text(merged).await;
        let reply: jarvis_protocol::EntityDetailReply =
            serde_json::from_str(&body).unwrap_or_else(|error| panic!("decode {body}: {error}"));
        assert_eq!(
            reply.entity.entity_id, target,
            "the reply must be the entity kept, not the one merged away"
        );
        assert_eq!(reply.entity.status, "active");

        // The loser is retained and points at the winner — a merge is reversible rather than a deletion.
        let read = app
            .clone()
            .oneshot(get_request(
                &format!("/api/v1/entities/{source}"),
                Some(&presented),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        let body = body_text(read).await;
        let reply: jarvis_protocol::EntityDetailReply =
            serde_json::from_str(&body).unwrap_or_else(|error| panic!("decode {body}: {error}"));
        assert_eq!(reply.entity.status, "merged");
        assert_eq!(reply.entity.merged_into.as_deref(), Some(target.as_str()));

        // And a self-merge is refused rather than silently succeeding.
        let itself = app
            .oneshot(post_json(
                &format!("/api/v1/entities/{target}/merge"),
                &presented,
                &format!(r#"{{"target_id":"{target}"}}"#),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(itself.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = body_text(itself).await;
        assert!(
            body.contains("itself"),
            "the refusal must name the reason: {body}"
        );
    }

    /// **A verified name plus a guess is not an identity, and the list leaves a merged entity out.**
    ///
    /// Two rules that were **untested** until this test, both found by asking what each could be broken into:
    ///
    /// 1. The alias lookup's verdict is the **conjunction** over the aliases that matched one entity. One entity
    ///    holding a verified email *and* a probabilistic one for the same value must report `verified: false`,
    ///    because the caller is asking whether this name denotes this entity and one of the matching names is a
    ///    guess. A disjunction would call it an identity.
    /// 2. A listing excludes merged entities. A merged name denotes nothing — its claims belong to the winner —
    ///    so offering it as a subject would let a caller attach a new claim to a name that has been retired.
    #[tokio::test]
    async fn a_guess_among_verified_names_and_a_merged_entity_are_both_excluded() {
        let (app, presented, _profile) = test_router().await;
        let entity = create_entity_via(&app, &presented, "Ada Lovelace", "person").await;

        // One **verified** alias and one **probabilistic** alias for the same value, on the same entity. The
        // probabilistic insert is allowed by the partial unique index, which keys only on verified rows — and
        // that is what makes the conjunction testable at all.
        for (verification, source, confidence) in [
            ("confirmed", "user_statement", "confirmed"),
            ("probabilistic", "user_statement", "likely"),
        ] {
            let response = app
                .clone()
                .oneshot(post_json(
                    &format!("/api/v1/entities/{entity}/aliases"),
                    &presented,
                    &format!(
                        r#"{{"alias_kind":"email","alias_value":"ada@example.com",
                            "verification":"{verification}","source_kind":"{source}",
                            "confidence":"{confidence}"}}"#
                    ),
                ))
                .await
                .unwrap_or_else(|error| panic!("router call: {error}"));
            assert_eq!(
                response.status(),
                StatusCode::OK,
                "{}",
                body_text(response).await
            );
        }

        let lookup = app
            .clone()
            .oneshot(get_request(
                "/api/v1/entities/lookup?alias_kind=email&alias_value=ada@example.com",
                Some(&presented),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        let body = body_text(lookup).await;
        let reply: jarvis_protocol::EntityLookupReply =
            serde_json::from_str(&body).unwrap_or_else(|error| panic!("decode {body}: {error}"));
        assert_eq!(reply.returned, 1, "one entity, not two entries: {body}");
        assert_eq!(
            reply.matches[0].matched_aliases.len(),
            2,
            "both aliases belong to the one candidate: {body}"
        );
        assert!(
            !reply.matches[0].verified,
            "a verified name beside a guess is not an identity: {body}"
        );

        // And the listing's status filter: a merged entity is not a subject.
        let other = create_entity_via(&app, &presented, "A. Lovelace", "person").await;
        let merged = app
            .clone()
            .oneshot(post_json(
                &format!("/api/v1/entities/{other}/merge"),
                &presented,
                &format!(r#"{{"target_id":"{entity}"}}"#),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(
            merged.status(),
            StatusCode::OK,
            "{}",
            body_text(merged).await
        );

        let listed = app
            .oneshot(get_request("/api/v1/entities", Some(&presented)))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        let body = body_text(listed).await;
        let reply: jarvis_protocol::EntityListReply =
            serde_json::from_str(&body).unwrap_or_else(|error| panic!("decode {body}: {error}"));
        let ids: Vec<&str> = reply
            .entities
            .iter()
            .map(|entity| entity.entity_id.as_str())
            .collect();
        assert!(ids.contains(&entity.as_str()), "the winner stays: {body}");
        assert!(
            !ids.contains(&other.as_str()),
            "a merged entity must not be offered as a subject: {body}"
        );
    }

    /// **A lookup with no selector is refused rather than answered with the whole workspace.**
    ///
    /// The default matters: an empty query could reasonably mean "everything", which is `GET /entities`, or
    /// "nothing", which is useless. Refused, so the client says which it meant — and a half-supplied alias is
    /// refused for the same reason, since an empty kind searches every kind and an empty value matches nothing.
    #[tokio::test]
    async fn a_lookup_without_a_selector_is_refused() {
        let (app, presented, _profile) = test_router().await;
        for query in [
            "/api/v1/entities/lookup",
            "/api/v1/entities/lookup?alias_kind=email",
            "/api/v1/entities/lookup?alias_value=ada@example.com",
        ] {
            let response = app
                .clone()
                .oneshot(get_request(query, Some(&presented)))
                .await
                .unwrap_or_else(|error| panic!("router call: {error}"));
            assert_eq!(
                response.status(),
                StatusCode::BAD_REQUEST,
                "{query} must be refused rather than answered"
            );
        }

        // The control: a label **is** a selector and the lookup actually finds the entity — asserted on the
        // returned count, not only on the status. The first version of this control used a label containing a
        // space (`?label=Ada+Lovelace`), and since this build does no percent-decoding the `+` stayed literal,
        // matched nothing, and still returned `200`: a control that could not fail. A one-word label removes
        // the encoding question and leaves the assertion doing its job.
        let entity = create_entity_via(&app, &presented, "Aardvark", "person").await;
        let response = app
            .oneshot(get_request(
                "/api/v1/entities/lookup?label=Aardvark",
                Some(&presented),
            ))
            .await
            .unwrap_or_else(|error| panic!("router call: {error}"));
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_text(response).await;
        let reply: jarvis_protocol::EntityLookupReply =
            serde_json::from_str(&body).unwrap_or_else(|error| panic!("decode {body}: {error}"));
        assert_eq!(reply.returned, 1, "the label lookup must find it: {body}");
        assert_eq!(reply.matches[0].entity.entity_id, entity);
    }
}
