//! Versioned wire contracts and conversions for JARVIS clients and runtimes.

mod approval;
mod entity_api;
mod frame;
mod memory_api;
mod project_api;
mod rest;
mod run_api;
mod schedule_api;
mod session;
mod skill_api;
mod tool_api;
mod version;
mod wire;
pub use approval::{
    ApprovalDecisionBody, ApprovalDecisionRequest, ApprovalListReply, ApprovalReply,
    PendingApprovalReply,
};
pub use entity_api::{
    AddAliasRequest, AliasVerificationName, CreateEntityRequest, EntityAliasReply,
    EntityDetailReply, EntityKindName, EntityListReply, EntityLookupReply, EntityMatchReply,
    EntityReply, EntityStatusName, MergeEntityRequest,
};
pub use frame::{FrameError, MAX_FRAME_BYTES, decode_frame, encode_frame, read_frame, write_frame};
pub use memory_api::{
    ClaimBody, ConfirmMemoryRequest, CorrectMemoryRequest, DeletionReceipt, ExportedMemory,
    ForgetMemoryRequest, MemoryDetailReply, MemoryExportReply, MemoryListReply, MemoryReference,
    MemoryReply, MemorySearchHit, MemorySearchReply, MemorySearchRequest, RememberRequest,
    SequenceRange, SignalContribution, SummarizeSessionRequest, SummaryListReply, SummaryReply,
};
pub use project_api::{
    AddProjectNoteRequest, CreateProjectRequest, ProjectDetailReply, ProjectListReply,
    ProjectNoteReply, ProjectReply, UpdateProjectRequest,
};
pub use rest::{
    JSON_CONTENT_TYPE, MAX_STREAM_PAGE, RESYNC_HINT_SECONDS, RunEventPageReply, RunEventReply,
    RunReply, SSE_CONTENT_TYPE, StartRunRequest, rest_error, safe,
};
pub use run_api::{
    API_BASE_PATH, ERROR_EVENT_NAME, JSON_BODY_CONTENT_TYPE, MAX_PATH_SEGMENT_CHARS,
    MAX_PENDING_BYTES, RunPathError, RunStreamDecoder, RunStreamError, RunStreamFrame, SSE_ACCEPT,
    StreamReading, dotted_path_segment, output_text, path_segment, run_path, run_stream_path,
    runs_path, state_name,
};
pub use schedule_api::{
    CreateScheduleRequest, RunListReply, RunSummaryReply, ScheduleListReply, ScheduleReply,
};
pub use session::{
    AdmittedClient, ClientContext, ClientSession, HANDSHAKE_TIMEOUT, MAX_REQUESTS_PER_CONNECTION,
    Responder, ServerContext, SessionError, serve,
};
pub use skill_api::{
    CreateSkillRequest, CreateSkillStep, DroppedFieldBody, ExportedSkill, ForgetSkillRequest,
    PromoteSkillRequest, SkillDeletionReceipt, SkillDetailReply, SkillExportReply, SkillListReply,
    SkillReference, SkillReply, SkillStateName, SkillStepBody, SkillTransitionRequest,
};
pub use tool_api::{ToolListReply, ToolPreviewReply, ToolPreviewRequest, ToolReply};
pub use version::{
    MAX_SUPPORTED_PROTOCOL, MIN_SUPPORTED_PROTOCOL, NegotiationError, PROTOCOL_VERSION, negotiate,
};
pub use wire::{
    ClientHandshake, ClientKind, Command, DaemonHandshake, HandshakeError, HandshakeOutcome,
    HealthReply, MAX_CLIENT_CAPABILITIES, MAX_TOKEN_BYTES, Outcome, Reply, Request, Response,
    ServerHandshake, StatusReply, WireError, is_safe_token,
};
