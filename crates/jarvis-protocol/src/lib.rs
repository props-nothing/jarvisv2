//! Versioned wire contracts and conversions for JARVIS clients and runtimes.

mod frame;
mod rest;
mod session;
mod version;
mod wire;

pub use frame::{FrameError, MAX_FRAME_BYTES, decode_frame, encode_frame, read_frame, write_frame};
pub use rest::{
    CancelRunRequest, JSON_CONTENT_TYPE, MAX_STREAM_PAGE, RESYNC_HINT_SECONDS, RunEventPageReply,
    RunEventReply, RunReply, SSE_CONTENT_TYPE, StartRunRequest, rest_error, safe,
};
pub use session::{
    AdmittedClient, ClientContext, ClientSession, HANDSHAKE_TIMEOUT, MAX_REQUESTS_PER_CONNECTION,
    Responder, ServerContext, SessionError, serve,
};
pub use version::{
    MAX_SUPPORTED_PROTOCOL, MIN_SUPPORTED_PROTOCOL, NegotiationError, PROTOCOL_VERSION, negotiate,
};
pub use wire::{
    ClientHandshake, ClientKind, Command, DaemonHandshake, HandshakeError, HandshakeOutcome,
    HealthReply, MAX_CLIENT_CAPABILITIES, MAX_TOKEN_BYTES, Outcome, Reply, Request, Response,
    ServerHandshake, StatusReply, WireError, is_safe_token,
};
