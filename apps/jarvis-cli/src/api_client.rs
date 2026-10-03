//! The daemon's `/api/v1` HTTP API, as a client.
//!
//! # Why the CLI talks HTTP for runs
//!
//! ADR-0011 makes HTTP a first-class peer transport and records local IPC as the **preferred**
//! transport for the CLI and desktop. Runs are not on the local protocol yet: protocol v1 serves
//! `status` and `health` only, and the run surface lives on `/api/v1`. Talking HTTP here therefore
//! follows the decision rather than working around it — the CLI uses "the daemon API" that
//! `ADR-0011` names, and it stays a transport client with no orchestration of its own.
//!
//! The transport is `reqwest` 0.13.5, already resolved in this workspace through the model
//! adapter, and it is configured to match the adapter's hardening exactly. Each of the four
//! decisions below corrects a default that fails open:
//!
//! 1. **Proxies are disabled.** `reqwest` reads `HTTP_PROXY`, `HTTPS_PROXY`, and `ALL_PROXY` from
//!    the environment by default, so a request carrying the profile credential could be routed
//!    through an intermediary the user never chose.
//! 2. **Redirects are not followed.** A redirect can move an `Authorization` header to a different
//!    origin, and the credential is a bearer token.
//! 3. **Certificate verification stays on.** No insecure path is offered.
//! 4. **A read timeout is set.** It is the only thing that bounds a stalled stream; a request
//!    timeout alone does not bound each read.
//!
//! # The credential is header-only and taken verbatim
//!
//! It is sent in `Authorization: Bearer` and never in a query parameter, where it would become a
//! substring of every access log the request passed through. It is not trimmed, matching the
//! daemon's own extractor: a padded value is a different byte sequence, not the credential.
//!
//! # The target is loopback by construction
//!
//! [`LoopbackHost`] cannot express another address, so a client cannot be pointed at a host the
//! daemon does not serve. See that type for why the rule is a shape rather than a check.

use std::fmt;
use std::fmt::Write as _;
use std::time::Duration;

use jarvis_core::LoopbackHost;
// The CLI does not depend on serde directly, so the trait is reached through serde_json, which is
// already a dependency for parsing the daemon's replies. Adding serde to the manifest for one bound would
// be a new direct dependency for a name.

use jarvis_protocol::{
    AddAliasRequest, ApprovalDecisionBody, ApprovalListReply, ApprovalReply, ConfirmMemoryRequest,
    CorrectMemoryRequest, CreateEntityRequest, CreateScheduleRequest, CreateSkillRequest,
    DeletionReceipt, EntityDetailReply, EntityListReply, EntityLookupReply, ForgetMemoryRequest,
    ForgetSkillRequest, JSON_BODY_CONTENT_TYPE, MemoryDetailReply, MemoryExportReply,
    MemoryListReply, MemoryReply, MemorySearchReply, MemorySearchRequest, MergeEntityRequest,
    PromoteSkillRequest, RememberRequest, RunListReply, RunPathError, RunReply, RunStreamDecoder,
    RunStreamFrame, SSE_ACCEPT, ScheduleListReply, ScheduleReply, SkillDeletionReceipt,
    SkillDetailReply, SkillExportReply, SkillListReply, SkillReply, SkillTransitionRequest,
    StartRunRequest, ToolListReply, ToolPreviewReply, ToolPreviewRequest, WireError,
    dotted_path_segment, path_segment, run_path, run_stream_path, runs_path,
};

/// The base path of the memory surface.
///
/// A constant rather than a function because it takes no parameter, unlike a run path: there is one memory
/// collection per profile, and its scope comes from the credential rather than from the URL.
const MEMORIES_PATH: &str = "/api/v1/memories";

/// The base path of the tool control-plane surface.
const TOOLS_PATH: &str = "/api/v1/tools";

/// Builds the preview path for one tool, validating the identifier first.
///
/// The identifier is a **tool identifier** (`jarvis.files.read`), which is dotted but contains no `/`, and
/// it is passed through the same [`jarvis_protocol::path_segment`] rule the run and memory paths use. A
/// tool identifier is validated by `ToolId` in the daemon, so this is not a second opinion about the
/// name's shape — it is the check that stops a name containing a path separator from addressing a
/// different route, which is a property of the *URL* rather than of the tool.
/// Builds the path for one tool's preview.
///
/// Uses [`jarvis_protocol::dotted_path_segment`] rather than the run identifier rule, because a tool identifier
/// is `namespace.name` **by construction** — so the UUID rule rejected every tool the daemon can register, and
/// `jarvis tools preview` failed for all of them with a message about a run identifier. The narrower rule was
/// the wrong one to reuse here, and a dot is path-safe (RFC 3986 `pchar`), so this is not a loosened check.
fn tool_preview_path(tool: &str) -> Result<String, ApiError> {
    let segment = dotted_path_segment(tool).map_err(ApiError::Identifier)?;
    Ok(format!("{TOOLS_PATH}/{segment}/preview"))
}

/// Builds the path for one memory, validating the identifier first.
///
/// The identifier is validated by [`jarvis_protocol::path_segment`], which is the **segment** rule the run
/// paths are built from. Reusing it rather than writing a second validator is deliberate: two validators over
/// the same class of untrusted value are two chances for one of them to miss a character, and a memory
/// identifier is no more trustworthy than a run identifier.
///
/// # Errors
///
/// Returns [`RunPathError`] when the identifier contains a character that would change the request target.
fn memory_path(memory_id: &str) -> Result<String, RunPathError> {
    Ok(format!("/api/v1/memories/{}", path_segment(memory_id)?))
}

/// The base path of the entity surface (`P4-016`).
const ENTITIES_PATH: &str = "/api/v1/entities";

/// Builds the path for one entity, validating the identifier first.
///
/// [`jarvis_protocol::path_segment`], the same segment rule the run and memory paths use: two validators over
/// the same class of untrusted value are two places for the traversal case to be handled differently.
fn entity_path(entity_id: &str) -> Result<String, RunPathError> {
    Ok(format!("/api/v1/entities/{}", path_segment(entity_id)?))
}

/// The skill collection's path.
const SKILLS_PATH: &str = "/api/v1/skills";

/// Builds the path for one skill revision, validating the identifier first.
///
/// Validation is [`jarvis_protocol::path_segment`], the same segment rule the run and memory paths use, for
/// the reason that function's callers record: two validators over the same class of untrusted value are two
/// chances for one of them to miss a character, and a revision identifier is no more trustworthy than a run
/// identifier.
///
/// # Errors
///
/// Returns [`RunPathError`] when the identifier contains a character that would change the request target.
fn skill_path(revision_id: &str) -> Result<String, RunPathError> {
    Ok(format!("{SKILLS_PATH}/{}", path_segment(revision_id)?))
}

/// The scheduled-task collection's path.
const SCHEDULES_PATH: &str = "/api/v1/schedules";

/// The approval collection's path.
const APPROVALS_PATH: &str = "/api/v1/approvals";

/// Builds the decision path for one approval, validating the identifier first.
///
/// The same segment rule the run and memory paths use, for the reason `memory_path` gives.
fn approval_decision_path(approval_id: &str) -> Result<String, RunPathError> {
    Ok(format!(
        "{APPROVALS_PATH}/{}/decision",
        path_segment(approval_id)?
    ))
}

/// Builds the release path for one approval, validating the identifier first.
fn approval_resume_path(approval_id: &str) -> Result<String, RunPathError> {
    Ok(format!(
        "{APPROVALS_PATH}/{}/resume",
        path_segment(approval_id)?
    ))
}

/// Time allowed to establish a connection to a loopback daemon.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Time allowed for a complete non-streaming request.
///
/// Applied **per request** rather than on the client. A client-level `reqwest` timeout bounds the
/// whole response body, so on a client that also serves an open-ended SSE stream it would terminate
/// a healthy stream at the deadline — which is exactly what the first live run of this client did.
/// `STREAM_READ_TIMEOUT` bounds a dead stream instead, and it resets on every read.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Time allowed between streamed bytes.
///
/// Generous, because the daemon holds the connection open for as long as the run is active and poll
/// interval is sub-second. It exists so a wedged daemon fails the client rather than parking it
/// forever, not to bound a healthy idle stream. This is the **only** thing that bounds a stalled
/// stream: a total timeout cannot, because a stream has no defined total length.
pub const STREAM_READ_TIMEOUT: Duration = Duration::from_secs(120);

/// Why a request to the daemon failed.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    /// The daemon could not be reached, or the transport failed.
    ///
    /// Carries a classification rather than a bare message: "could not be reached" alone does not
    /// say whether the daemon is absent, the connection was refused, or a read stalled, and those
    /// have different fixes. The raw `reqwest` error is deliberately not kept, because it can
    /// contain the URL and platform error text.
    #[error("the daemon API could not be reached: {0}")]
    Transport(TransportFailure),
    /// The daemon answered with its stable error envelope.
    #[error("the daemon refused the request: {0}")]
    Refused(Box<WireError>),
    /// The daemon answered with a status but no readable envelope.
    ///
    /// Distinct from [`Self::Refused`] on purpose: a `500` whose body is not an envelope is a
    /// different diagnosis from a `500` that explains itself, and collapsing them would report
    /// "the daemon refused" for a response the daemon never intended.
    #[error("the daemon answered with status {status} and no readable error envelope")]
    UnexpectedStatus {
        /// The HTTP status the daemon returned.
        status: u16,
    },
    /// The response body could not be decoded as the expected shape.
    #[error("the daemon reply was not the shape this client understands")]
    Decode,
    /// A run identifier in a reply could not be used to build a request path.
    #[error(transparent)]
    Identifier(#[from] RunPathError),
}

/// A classified transport failure.
///
/// The categories mirror the model adapter's transport classification, so one vocabulary describes
/// a failed daemon call and a failed provider call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransportFailure {
    /// The connection could not be established.
    Connect,
    /// The request or a read exceeded its deadline.
    Timeout,
    /// The response body or its encoding was unreadable.
    Body,
    /// The connection failed after it was established.
    Io,
}

impl fmt::Display for TransportFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Connect => "the connection was refused or could not be established",
            Self::Timeout => "the request or stream read timed out",
            Self::Body => "the response body could not be read",
            Self::Io => "the connection failed",
        })
    }
}

/// Classifies a `reqwest` error without keeping its text.
fn classify(error: &reqwest::Error) -> TransportFailure {
    if error.is_timeout() {
        TransportFailure::Timeout
    } else if error.is_connect() {
        TransportFailure::Connect
    } else if error.is_body() || error.is_decode() {
        TransportFailure::Body
    } else {
        TransportFailure::Io
    }
}

/// The authenticated client for the daemon's HTTP API.
#[derive(Clone, Debug)]
pub struct ApiClient {
    host: LoopbackHost,
    credential: String,
    client: reqwest::Client,
}

impl ApiClient {
    /// Builds a client for a loopback endpoint and credential.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError::Transport`] when the HTTP client cannot be constructed, for example when
    /// the TLS backend cannot initialize. This is reported rather than ignored because every later
    /// request would fail with a less specific error.
    pub fn new(host: LoopbackHost, credential: String) -> Result<Self, ApiError> {
        let client = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            // Deliberately NO `.timeout(..)` here: it bounds the whole response body, which on this
            // client would terminate every SSE stream at the deadline. `read_timeout` bounds a
            // stalled stream instead, and non-streaming calls set their own total timeout.
            .read_timeout(STREAM_READ_TIMEOUT)
            // Inherited from the environment by default, which would expose the credential and the
            // objective to an intermediary the user did not select.
            .no_proxy()
            // A redirect could move the bearer credential to another origin.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| ApiError::Transport(classify(&error)))?;
        Ok(Self {
            host,
            credential,
            client,
        })
    }

    /// Returns the endpoint this client targets.
    #[must_use]
    pub const fn host(&self) -> LoopbackHost {
        self.host
    }

    /// Starts a run, continuing an existing conversation when one is named.
    ///
    /// A second turn is a **new run in the same session**, not a mutation of the first: the daemon
    /// replays the session's transcript into the model call, so a run is the unit of work and the
    /// session is the unit of continuity. That is why this takes a session identifier rather than
    /// a run identifier.
    ///
    /// `session_id` of `None` begins a new conversation, which is the ordinary single-turn case. There
    /// is deliberately no separate "start a fresh run" method: a second wrapper would be one more
    /// place for the session argument to be dropped by accident.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError`] when the objective is not storable, the named session is absent or
    /// closed to new work, the daemon refuses the request, or the transport fails.
    pub async fn start_run_in_session(
        &self,
        objective: &str,
        session_id: Option<&str>,
    ) -> Result<RunReply, ApiError> {
        let body = StartRunRequest {
            objective: objective.to_owned(),
            session_id: session_id.map(ToOwned::to_owned),
            // Omitted rather than sent as null. The daemon refuses a present idempotency key
            // because no deduplication ledger exists, and sending one would be claiming a
            // guarantee this build does not provide.
            idempotency_key: None,
        };

        let response = self
            .bounded_request(reqwest::Method::POST, &runs_path())
            .json(&body)
            .send()
            .await
            .map_err(|error| ApiError::Transport(classify(&error)))?;
        let status = response.status();
        if status.is_success() {
            return response
                .json::<RunReply>()
                .await
                .map_err(|_| ApiError::Decode);
        }
        Err(self.refusal(response).await)
    }

    /// Reads one run.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError`] when the identifier is unusable, the run is unknown, or the transport
    /// fails.
    pub async fn read_run(&self, run_id: &str) -> Result<RunReply, ApiError> {
        let path = run_path(run_id)?;
        let response = self
            .bounded_request(reqwest::Method::GET, &path)
            .send()
            .await
            .map_err(|error| ApiError::Transport(classify(&error)))?;
        let status = response.status();
        if status.is_success() {
            return response
                .json::<RunReply>()
                .await
                .map_err(|_| ApiError::Decode);
        }
        Err(self.refusal(response).await)
    }

    /// Lists the workspace's remembered claims.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError`] when the daemon refuses the page size or the transport fails.
    pub async fn list_memories(&self, limit: Option<u32>) -> Result<MemoryListReply, ApiError> {
        let path = match limit {
            Some(limit) => format!("{MEMORIES_PATH}?limit={limit}"),
            None => MEMORIES_PATH.to_owned(),
        };
        self.get_json(&path).await
    }

    /// Lists the workspace's pending approvals, with the arguments each is waiting on.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError`] when the transport fails or the daemon refuses.
    pub async fn list_approvals(&self) -> Result<ApprovalListReply, ApiError> {
        self.get_json(APPROVALS_PATH).await
    }

    /// Records a decision on a pending approval.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError::Refused`] when the approval is unknown, already decided, lapsed, or the nonce does
    /// not match.
    pub async fn decide_approval(
        &self,
        approval_id: &str,
        body: &ApprovalDecisionBody,
    ) -> Result<ApprovalReply, ApiError> {
        let path = approval_decision_path(approval_id)?;
        self.send_json(reqwest::Method::POST, &path, body).await
    }

    /// Releases an approved call from the arguments the daemon held, for one whose release did not happen.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError::Refused`] when the approval is unknown, not approved, or no longer holds its arguments.
    pub async fn resume_approval(&self, approval_id: &str) -> Result<serde_json::Value, ApiError> {
        let path = approval_resume_path(approval_id)?;
        self.send_json(reqwest::Method::POST, &path, &serde_json::json!({}))
            .await
    }

    /// Creates a scheduled task.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError::Refused`] with a `422` naming the rule a cadence or objective broke.
    pub async fn create_schedule(
        &self,
        request: &CreateScheduleRequest,
    ) -> Result<ScheduleReply, ApiError> {
        self.send_json(reqwest::Method::POST, SCHEDULES_PATH, request)
            .await
    }

    /// Lists the workspace's scheduled tasks.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError`] when the transport fails or the daemon refuses.
    pub async fn list_schedules(&self) -> Result<ScheduleListReply, ApiError> {
        self.get_json(SCHEDULES_PATH).await
    }

    /// Pauses or resumes a scheduled task.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError::Refused`] when the task is unknown, or a finished one-off is resumed.
    pub async fn set_schedule_enabled(
        &self,
        schedule_id: &str,
        enabled: bool,
    ) -> Result<ScheduleReply, ApiError> {
        let verb = if enabled { "resume" } else { "pause" };
        let path = format!("{SCHEDULES_PATH}/{}/{verb}", path_segment(schedule_id)?);
        self.send_json(reqwest::Method::POST, &path, &serde_json::json!({}))
            .await
    }

    /// Removes a scheduled task.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError::Refused`] with a `404` when there is no such task.
    pub async fn remove_schedule(&self, schedule_id: &str) -> Result<(), ApiError> {
        let path = format!("{SCHEDULES_PATH}/{}", path_segment(schedule_id)?);
        let response = self
            .bounded_request(reqwest::Method::DELETE, &path)
            .send()
            .await
            .map_err(|error| ApiError::Transport(classify(&error)))?;
        if response.status().is_success() {
            return Ok(());
        }
        Err(self.refusal(response).await)
    }

    /// Asks the daemon to stop a run: the kill switch.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError::Refused`] when the run is unknown or already settled.
    pub async fn cancel_run(&self, run_id: &str) -> Result<RunReply, ApiError> {
        let path = format!("/api/v1/runs/{}/cancel", path_segment(run_id)?);
        self.send_json(reqwest::Method::POST, &path, &serde_json::json!({}))
            .await
    }

    /// Lists the most recent runs with what each answered.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError::Refused`] with a `422` for a limit outside 1 to 50.
    pub async fn list_runs(&self, limit: u32) -> Result<RunListReply, ApiError> {
        self.get_json(&format!("/api/v1/runs?limit={limit}")).await
    }

    /// Lists every registered tool with the authorization posture in force for it.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError`] when the daemon has no tool surface at all, which answers `404` rather than an
    /// empty list — the two have different remedies, so they are different replies.
    pub async fn list_tools(&self) -> Result<ToolListReply, ApiError> {
        self.get_json(TOOLS_PATH).await
    }

    /// Asks what a call to one tool **would** decide, without making one.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError::Identifier`] when the tool name is unusable in a path, and [`ApiError::Refused`]
    /// when the tool is unknown — which is a `404` rather than a refusal, because a typo and a policy
    /// denial have opposite remedies.
    pub async fn preview_tool(
        &self,
        tool: &str,
        request: &ToolPreviewRequest,
    ) -> Result<ToolPreviewReply, ApiError> {
        let path = tool_preview_path(tool)?;
        let response = self
            .bounded_request(reqwest::Method::POST, &path)
            .json(request)
            .send()
            .await
            .map_err(|error| ApiError::Transport(classify(&error)))?;
        let status = response.status();
        if status.is_success() {
            return response
                .json::<ToolPreviewReply>()
                .await
                .map_err(|_| ApiError::Decode);
        }
        Err(self.refusal(response).await)
    }

    /// Reads one claim, with its content.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError::Identifier`] when the identifier is not usable in a path, and
    /// [`ApiError::Refused`] when the claim is unknown.
    pub async fn read_memory(&self, memory_id: &str) -> Result<MemoryDetailReply, ApiError> {
        self.get_json(&memory_path(memory_id)?).await
    }

    /// Searches the workspace's claims, returning the ranked matches and their explanations.
    ///
    /// A `POST` with a body rather than a query string, so the search term never reaches a URL: a URL lands
    /// in access logs, browser history, and `Referer` headers, and a memory search term is exactly the kind
    /// of phrase that should not. It is also the value that would otherwise need percent-encoding and
    /// re-validation, which is a second parsing path for something the body carries safely.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError`] when the daemon refuses a value or the transport fails.
    pub async fn search_memories(
        &self,
        request: &MemorySearchRequest,
    ) -> Result<MemorySearchReply, ApiError> {
        let response = self
            .bounded_request(reqwest::Method::POST, &format!("{MEMORIES_PATH}/search"))
            .json(request)
            .send()
            .await
            .map_err(|error| ApiError::Transport(classify(&error)))?;
        let status = response.status();
        if status.is_success() {
            return response
                .json::<MemorySearchReply>()
                .await
                .map_err(|_| ApiError::Decode);
        }
        Err(self.refusal(response).await)
    }

    /// Stores a claim.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError`] when the daemon refuses the claim — which is an ordinary outcome, since the
    /// candidate pipeline applies the confidence caps and sensitivity floors — or the transport fails.
    pub async fn remember(&self, request: &RememberRequest) -> Result<MemoryReply, ApiError> {
        let response = self
            .bounded_request(reqwest::Method::POST, MEMORIES_PATH)
            .json(request)
            .send()
            .await
            .map_err(|error| ApiError::Transport(classify(&error)))?;
        let status = response.status();
        if status.is_success() {
            return response
                .json::<MemoryReply>()
                .await
                .map_err(|_| ApiError::Decode);
        }
        Err(self.refusal(response).await)
    }

    /// Creates an entity, which is what a memory will be *about*.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError::Refused`] when the daemon refuses the kind, the confidence, or the attributes.
    pub async fn create_entity(
        &self,
        request: &CreateEntityRequest,
    ) -> Result<EntityDetailReply, ApiError> {
        let response = self
            .bounded_request(reqwest::Method::POST, ENTITIES_PATH)
            .json(request)
            .send()
            .await
            .map_err(|error| ApiError::Transport(classify(&error)))?;
        let status = response.status();
        if status.is_success() {
            return response
                .json::<EntityDetailReply>()
                .await
                .map_err(|_| ApiError::Decode);
        }
        Err(self.refusal(response).await)
    }

    /// Lists the workspace's entities.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError`] when the page is refused or the transport fails.
    pub async fn list_entities(&self, limit: Option<u32>) -> Result<EntityListReply, ApiError> {
        let path = match limit {
            Some(limit) => format!("{ENTITIES_PATH}?limit={limit}"),
            None => ENTITIES_PATH.to_owned(),
        };
        self.get_json(&path).await
    }

    /// Reads one entity with its aliases.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError`] when no entity has that identifier.
    pub async fn read_entity(&self, entity_id: &str) -> Result<EntityDetailReply, ApiError> {
        self.get_json(&entity_path(entity_id)?).await
    }

    /// Looks an entity up by its label, returning every match.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError`] when the label is blank or the transport fails.
    pub async fn lookup_entity_by_label(&self, label: &str) -> Result<EntityLookupReply, ApiError> {
        // The label goes in a query parameter rather than the path. A **path** segment would need the label to
        // satisfy a route grammar, and a label is user text — a person called "A/B" would be unroutable, and
        // percent-encoding it here would be a second parsing path for a value the daemon re-reads.
        self.get_json(&format!("{ENTITIES_PATH}/lookup?label={label}"))
            .await
    }

    /// Resolves an alias, returning every candidate and whether each is verified.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError`] when a parameter is blank or the transport fails.
    pub async fn lookup_entity_by_alias(
        &self,
        alias_kind: &str,
        alias_value: &str,
    ) -> Result<EntityLookupReply, ApiError> {
        self.get_json(&format!(
            "{ENTITIES_PATH}/lookup?alias_kind={alias_kind}&alias_value={alias_value}"
        ))
        .await
    }

    /// Attaches an alias to an entity.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError::Refused`] when the schema's rules refuse the pair of verification and source kind, or
    /// when another entity already holds the alias as verified.
    pub async fn add_entity_alias(
        &self,
        entity_id: &str,
        request: &AddAliasRequest,
    ) -> Result<EntityDetailReply, ApiError> {
        let path = format!("{}/aliases", entity_path(entity_id)?);
        let response = self
            .bounded_request(reqwest::Method::POST, &path)
            .json(request)
            .send()
            .await
            .map_err(|error| ApiError::Transport(classify(&error)))?;
        let status = response.status();
        if status.is_success() {
            return response
                .json::<EntityDetailReply>()
                .await
                .map_err(|_| ApiError::Decode);
        }
        Err(self.refusal(response).await)
    }

    /// Merges one entity into another, returning the winner.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError::Refused`] for a self-merge, a cross-workspace merge, or a target that is not active.
    pub async fn merge_entity(
        &self,
        source_id: &str,
        request: &MergeEntityRequest,
    ) -> Result<EntityDetailReply, ApiError> {
        let path = format!("{}/merge", entity_path(source_id)?);
        let response = self
            .bounded_request(reqwest::Method::POST, &path)
            .json(request)
            .send()
            .await
            .map_err(|error| ApiError::Transport(classify(&error)))?;
        let status = response.status();
        if status.is_success() {
            return response
                .json::<EntityDetailReply>()
                .await
                .map_err(|_| ApiError::Decode);
        }
        Err(self.refusal(response).await)
    }

    /// Corrects a claim, replacing its text.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError::Refused`] with a `409` when the caller's version is stale.
    pub async fn correct_memory(
        &self,
        memory_id: &str,
        request: &CorrectMemoryRequest,
    ) -> Result<MemoryReply, ApiError> {
        let path = format!("{}/correct", memory_path(memory_id)?);
        let response = self
            .bounded_request(reqwest::Method::POST, &path)
            .json(request)
            .send()
            .await
            .map_err(|error| ApiError::Transport(classify(&error)))?;
        let status = response.status();
        if status.is_success() {
            return response
                .json::<MemoryReply>()
                .await
                .map_err(|_| ApiError::Decode);
        }
        Err(self.refusal(response).await)
    }

    /// Forgets a claim, returning the receipt of what was removed.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError::Refused`] with a `409` when the caller's version is stale.
    pub async fn forget_memory(
        &self,
        memory_id: &str,
        request: &ForgetMemoryRequest,
    ) -> Result<DeletionReceipt, ApiError> {
        let path = format!("{}/forget", memory_path(memory_id)?);
        let response = self
            .bounded_request(reqwest::Method::POST, &path)
            .json(request)
            .send()
            .await
            .map_err(|error| ApiError::Transport(classify(&error)))?;
        let status = response.status();
        if status.is_success() {
            return response
                .json::<DeletionReceipt>()
                .await
                .map_err(|_| ApiError::Decode);
        }
        Err(self.refusal(response).await)
    }

    /// Accepts a proposed claim, recording the daemon's own identity as the approver.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError::Refused`] with a `409` when the caller's version is stale, and with a `422` when the
    /// claim is not a proposal — the two refusals are distinct because their remedies are: one is a re-read, the
    /// other is a different verb.
    pub async fn confirm_memory(
        &self,
        memory_id: &str,
        request: &ConfirmMemoryRequest,
    ) -> Result<MemoryReply, ApiError> {
        let path = format!("{}/confirm", memory_path(memory_id)?);
        let response = self
            .bounded_request(reqwest::Method::POST, &path)
            .json(request)
            .send()
            .await
            .map_err(|error| ApiError::Transport(classify(&error)))?;
        let status = response.status();
        if status.is_success() {
            return response
                .json::<MemoryReply>()
                .await
                .map_err(|_| ApiError::Decode);
        }
        Err(self.refusal(response).await)
    }

    /// Exports every claim the workspace holds, including archived and deleted ones.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError`] when the page size is refused or the transport fails.
    pub async fn export_memories(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<MemoryExportReply, ApiError> {
        let mut path = format!("{MEMORIES_PATH}/export");
        let mut separator = '?';
        for (name, value) in [("limit", limit), ("offset", offset)] {
            if let Some(value) = value {
                path.push(separator);
                separator = '&';
                // `write!` rather than `push_str(&format!(..))`, which the lint forbids: the temporary
                // `String` is allocation the formatter can write into the path directly. The `Result` is
                // discarded because writing to a `String` cannot fail.
                let _ = write!(path, "{name}={value}");
            }
        }
        self.get_json(&path).await
    }

    /// Performs an authenticated `GET` and decodes the reply.
    ///
    /// The shared body of every read on this surface, so the timeout, the credential, and the refusal
    /// handling are written once — the same reason `bounded_request` exists.
    async fn get_json<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, ApiError> {
        let response = self
            .bounded_request(reqwest::Method::GET, path)
            .send()
            .await
            .map_err(|error| ApiError::Transport(classify(&error)))?;
        let status = response.status();
        if status.is_success() {
            return response.json::<T>().await.map_err(|_| ApiError::Decode);
        }
        Err(self.refusal(response).await)
    }

    /// Performs an authenticated request carrying a JSON body and decodes the reply.
    ///
    /// The shared body of every **write** on this surface, so a verb added later cannot quietly omit the
    /// credential, the timeout, or the refusal decoding. The skill verbs are four calls that differ only in
    /// method and path, which is exactly the shape that grows a divergent copy when written out.
    async fn send_json<B: serde::Serialize, T: serde::de::DeserializeOwned>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: &B,
    ) -> Result<T, ApiError> {
        let response = self
            .bounded_request(method, path)
            .json(body)
            .send()
            .await
            .map_err(|error| ApiError::Transport(classify(&error)))?;
        let status = response.status();
        if status.is_success() {
            return response.json::<T>().await.map_err(|_| ApiError::Decode);
        }
        Err(self.refusal(response).await)
    }

    /// Lists the workspace's skill revisions, newest first.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError`] when the page size is refused or the transport fails.
    pub async fn list_skills(&self, limit: Option<u32>) -> Result<SkillListReply, ApiError> {
        let path = match limit {
            Some(limit) => format!("{SKILLS_PATH}?limit={limit}"),
            None => SKILLS_PATH.to_owned(),
        };
        self.get_json(&path).await
    }

    /// Reads one skill revision, with the procedure's text.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError::Refused`] with a `404` when the revision is not in this workspace.
    pub async fn read_skill(&self, revision_id: &str) -> Result<SkillDetailReply, ApiError> {
        self.get_json(&skill_path(revision_id)?).await
    }

    /// Records a new skill revision.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError::Refused`] with a `422` when a step names a tool this deployment cannot run, or when
    /// the revision is refused by a rule — a refusal is an ordinary outcome here, so it is decoded rather than
    /// thrown away.
    pub async fn create_skill(&self, request: &CreateSkillRequest) -> Result<SkillReply, ApiError> {
        self.send_json(reqwest::Method::POST, SKILLS_PATH, request)
            .await
    }

    /// Promotes a proposal, naming the approver.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError::Refused`] with a `409` for a stale counter and a `422` for a self-approval.
    pub async fn promote_skill(
        &self,
        revision_id: &str,
        request: &PromoteSkillRequest,
    ) -> Result<SkillReply, ApiError> {
        let path = format!("{}/promote", skill_path(revision_id)?);
        self.send_json(reqwest::Method::POST, &path, request).await
    }

    /// Disables a revision by archiving it.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError::Refused`] with a `409` for a stale counter.
    pub async fn disable_skill(
        &self,
        revision_id: &str,
        request: &SkillTransitionRequest,
    ) -> Result<SkillReply, ApiError> {
        let path = format!("{}/disable", skill_path(revision_id)?);
        self.send_json(reqwest::Method::POST, &path, request).await
    }

    /// Enables a disabled revision.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError::Refused`] with a `409` for a stale counter.
    pub async fn enable_skill(
        &self,
        revision_id: &str,
        request: &SkillTransitionRequest,
    ) -> Result<SkillReply, ApiError> {
        let path = format!("{}/enable", skill_path(revision_id)?);
        self.send_json(reqwest::Method::POST, &path, request).await
    }

    /// Deletes a revision, returning the receipt of what was removed.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError::Refused`] with a `409` for a stale counter and a `422` when another revision
    /// declares a supersession with this one.
    pub async fn forget_skill(
        &self,
        revision_id: &str,
        request: &ForgetSkillRequest,
    ) -> Result<SkillDeletionReceipt, ApiError> {
        self.send_json(reqwest::Method::DELETE, &skill_path(revision_id)?, request)
            .await
    }

    /// Exports every revision the workspace holds.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError`] when the page size is refused or the transport fails.
    pub async fn export_skills(&self, limit: Option<u32>) -> Result<SkillExportReply, ApiError> {
        let path = match limit {
            Some(limit) => format!("{SKILLS_PATH}/export?limit={limit}"),
            None => format!("{SKILLS_PATH}/export"),
        };
        self.get_json(&path).await
    }

    /// Opens a run's event stream.
    ///
    /// Returns a stream handle rather than a collected page, because a run is consumed as it
    /// happens: buffering would defeat the point of a stream and would report nothing until the run
    /// settled.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError`] when the identifier is unusable, the daemon refuses to open the stream
    /// (an unknown run is `404`, a cursor past the end is `409`), or the transport fails. The
    /// refusal is resolved *before* the stream is returned, so an unknown run is an error here
    /// rather than a stream that opens and immediately closes.
    pub async fn open_stream(&self, run_id: &str) -> Result<RunEventStream, ApiError> {
        let path = run_stream_path(run_id)?;
        let response = self
            .request(reqwest::Method::GET, &path)
            .header(reqwest::header::ACCEPT, SSE_ACCEPT)
            .send()
            .await
            .map_err(|error| ApiError::Transport(classify(&error)))?;
        let status = response.status();
        if !status.is_success() {
            return Err(self.refusal(response).await);
        }
        Ok(RunEventStream {
            response,
            decoder: RunStreamDecoder::new(),
        })
    }

    /// Builds an authenticated request.
    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.client
            .request(method, self.host.url(path))
            .header(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {}", self.credential),
            )
            .header(reqwest::header::CONTENT_TYPE, JSON_BODY_CONTENT_TYPE)
    }

    /// Builds an authenticated request that must complete within [`REQUEST_TIMEOUT`].
    ///
    /// Used for every non-streaming call, so a wedged daemon fails the command rather than hanging
    /// it. A streaming request deliberately does not use this.
    fn bounded_request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.request(method, path).timeout(REQUEST_TIMEOUT)
    }

    /// Reads a refusal into the shared error envelope.
    ///
    /// A body that is not the envelope is reported as [`ApiError::UnexpectedStatus`] rather than
    /// fabricated into a `WireError`: the daemon's contract is that every failure carries the
    /// envelope, so a failure to parse one is a fact worth reporting on its own.
    async fn refusal(&self, response: reqwest::Response) -> ApiError {
        let status = response.status().as_u16();
        match response.json::<WireError>().await {
            Ok(error) => ApiError::Refused(Box::new(error)),
            Err(_) => ApiError::UnexpectedStatus { status },
        }
    }
}

/// An open run event stream.
///
/// Yields decoded frames one at a time. A caller stops at a terminal reading, which the daemon
/// guarantees is the last thing it sends, because its append path refuses to write after the run
/// settled.
pub struct RunEventStream {
    response: reqwest::Response,
    decoder: RunStreamDecoder,
}

impl RunEventStream {
    /// Returns the next frame, or `None` when the daemon closed the stream cleanly.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError::Transport`] when the connection fails, and [`ApiError::Decode`] when a
    /// frame cannot be interpreted. Both are errors rather than `None`: `None` means the daemon
    /// finished the stream, and reporting a transport failure that way would make a dropped
    /// connection look like a completed run.
    pub async fn next_frame(&mut self) -> Result<Option<RunStreamFrame>, ApiError> {
        loop {
            if let Some(frame) = self.decoder.next_frame() {
                return frame.map(Some).map_err(|_| ApiError::Decode);
            }
            match self.response.chunk().await {
                Ok(Some(chunk)) => self.decoder.feed(&chunk),
                Ok(None) => {
                    // A frame left in the buffer at close is malformed rather than absent: the
                    // daemon ends every event with a separator, so a partial frame means the
                    // connection was severed mid-event and reporting it as a clean end would
                    // present a truncated run as a finished one.
                    return match self.decoder.next_frame() {
                        None => Ok(None),
                        Some(_) => Err(ApiError::Decode),
                    };
                }
                Err(error) => return Err(ApiError::Transport(classify(&error))),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jarvis_core::LOOPBACK_ADDRESS;

    fn client() -> ApiClient {
        let host = LoopbackHost::new(8765)
            .unwrap_or_else(|error| panic!("8765 must be a usable port: {error}"));
        ApiClient::new(host, "not-a-real-secret".to_owned())
            .unwrap_or_else(|error| panic!("a client over loopback must build: {error}"))
    }

    /// Every request this client can build targets loopback. This is the property the credential's
    /// safety depends on, so it is asserted rather than assumed.
    #[test]
    fn every_request_targets_loopback_and_names_the_bearer_header() {
        let client = client();
        let id = "018f0000-0000-7000-8000-000000000000";
        let run = run_path(id).unwrap_or_else(|error| panic!("a uuid is a safe segment: {error}"));
        for path in [runs_path(), run] {
            let request = client.request(reqwest::Method::GET, &path);
            let built = request
                .build()
                .unwrap_or_else(|error| panic!("a loopback request must build: {error}"));
            assert_eq!(built.url().host_str(), Some(LOOPBACK_ADDRESS));
            assert_eq!(built.url().path(), path);
            assert_eq!(
                built
                    .headers()
                    .get(reqwest::header::AUTHORIZATION)
                    .map(|value| value.to_str().ok()),
                Some(Some("Bearer not-a-real-secret")),
                "the credential is presented only as a bearer header"
            );
            assert!(
                built.url().query().is_none(),
                "no request may carry a credential in the query string"
            );
        }
    }

    /// A run identifier that could change the request's target is refused before any request is
    /// built, so a hostile or broken reply cannot redirect a request.
    #[tokio::test]
    async fn an_unsafe_identifier_is_refused_before_a_request_exists() {
        let client = client();
        for unsafe_id in ["", "a/b", "a?from=0", "../../admin"] {
            let Err(error) = client.read_run(unsafe_id).await else {
                panic!("{unsafe_id:?} must be refused rather than requested");
            };
            assert!(
                matches!(error, ApiError::Identifier(_)),
                "expected a path error for {unsafe_id:?}, got {error:?}"
            );
        }
    }
}
