//! MCP wire transport and protocol client for JARVIS.
//!
//! This crate is the **impure** half of the MCP integration. [`jarvis_mcp`](jarvis_mcp) holds the
//! offline translation rules — what a server is called, what its tools are called, what they are
//! permitted to do — and decides nothing about bytes. This crate adds the part that cannot be a
//! function of its arguments: a socket, a child process, a negotiated protocol revision, and a
//! clock.
//!
//! # Why this is a separate crate rather than a module of `jarvis-mcp`
//!
//! `jarvis-mcp` depends only on `jarvis-core` and `jarvis-tools` and is verified as a set of pure
//! functions, with no peer standing. Adopting the SDK here would put a third-party protocol data
//! model in the same crate as the authority rules a security reviewer needs to read, and it would
//! make the rules untestable without the SDK's types in scope. The split keeps `jarvis-mcp`'s tests
//! exactly as cheap as they are, and keeps the SDK replaceable: if the transport is reimplemented
//! against the wire, none of the naming, posture, or conformance rules move.
//!
//! # The one fact from `P3-007` that this crate exists to respect
//!
//! Protocol revision `2026-07-28` is a **stateless rewrite**. It removed the `initialize` handshake,
//! so a client that calls `serve()` — the SDK's default, which *is* the handshake — silently
//! negotiates the **legacy** era whatever version it names. Worse, the SDK's own
//! `ProtocolVersion::LATEST` is `V_2025_11_25`, i.e. the legacy era, and its README describes
//! `LATEST` as "newest stable version this SDK defaults to" while the same file states the SDK
//! implements `2026-07-28`. So the SDK's prose points one way and its source the other, and only the
//! source is a contract. **Every connection here names `V_2026_07_28` explicitly through
//! `ClientLifecycleMode::Discover`, and a test asserts the SDK's `LATEST` is not that revision**, so
//! that a future bump which changes this is caught rather than absorbed.
//!
//! # A note on the two directions
//!
//! The client half discovers and calls *someone else's* server; the server half
//! ([`JarvisMcpServer`]) is JARVIS being called. The server half's important property is that filtering
//! `tools/list` is **not** authorization: an MCP client may call any name, whether or not it was
//! advertised, so the handler refuses an unserved name **before anything runs**. Both halves override the
//! SDK's permissive or legacy defaults explicitly — see the module docs on `serve`.
//!
//! # What is not here yet
//!
//! **An MCP server is operator-reachable as of `P3-008j`.** `apps/jarvisd` reads `mcp-servers.toml`, builds the
//! host, registers each adapter, and dispatches an authorized request to the adapter its identifier names, so
//! the limit that `P3-008a`..`P3-008i` each recorded — "a capability a caller can use, not one an operator can
//! reach" — is closed. The wiring lives in the daemon rather than here, because composition is the
//! composition root's job (`repository-layout.md`).
//!
//! **Nothing is bound yet.** `JarvisMcpServer` is a handler with no listener: no loopback HTTP bind, no
//! stdio serve, and no daemon wiring, so a remote client cannot reach it (`P3-009b`/`P3-009c`). What is
//! proven is the part that is a decision rather than plumbing — that an unadvertised tool cannot run.
//!
//! Still not built, and recorded in `TODO.md` rather than implied: nothing is exercised against a real
//! third-party server (the fixture is hand-written here, which proves the wire framing but not that a stranger
//! agrees with it); no connection is pooled and nothing reconnects, so a server that dies stays unavailable
//! until the daemon restarts; a server's self-report is observed only within one process, so an identity drift
//! across a restart is invisible; the `NamingStrategy` is fixed at `Prefixed`; and no `run_events` row is
//! written for an MCP call (`P3-012`).

mod adapter;
mod admission;
#[cfg(test)]
#[path = "boundary_tests.rs"]
mod boundary_tests;
mod client;
mod endpoint;
mod enforcement;
mod error;
mod host;
mod host_config;
mod revision;
mod serve;
mod serving;

pub use adapter::McpToolAdapter;
pub use admission::{
    AdmissionError, AdmissionVerdict, AdmittedCaller, CallerAdmission, CallerLabel, CallerOrigin,
    DEFAULT_REQUESTS_PER_MINUTE, Fingerprint, MAX_ADMITTED_CALLERS, MAX_CALLER_LABEL_CHARS,
    MAX_FINGERPRINT_CHARS,
};
pub use client::{
    McpCallResult, McpConnection, OwnedListing, ServerCapabilities, StdioCommand, ToolBuffer,
    connect_http, connect_over, connect_stdio,
};
pub use endpoint::{EndpointError, MAX_ENDPOINT_BYTES, McpHttpEndpoint};
pub use enforcement::{RequestAdmission, RequestGate, RequestRefusal};
pub use error::{CallError, ConnectError, ListError};
pub use host::{HostBuild, HostedServer, UnreadableServer, build_catalog};
pub use host_config::{
    HostConfigError, HostError, MAX_HOST_CONFIG_BYTES, McpHost, McpHostConfig, ServerTransport,
};
pub use serve::{
    JarvisMcpServer, SERVER_NAME, SERVER_VERSION, ServedToolRunner, served_protocol_version,
};
pub use serving::{
    MAX_REQUEST_BODY_BYTES, MCP_ENDPOINT_PATH, ServiceError, ServingConfig, ServingConfigError,
};
// The naming strategy is **MCP** vocabulary, and it is re-exported so a composition root can choose one
// without taking `jarvis-mcp` as a dependency of its own. A daemon declaring the pure translation crate
// just to name a strategy would be a dependency in the direction `repository-layout.md` reserves for
// adapters, and the choice belongs to the crate that builds the host.
pub use jarvis_mcp::NamingStrategy;
// The scope every MCP tool requires, re-exported for the same reason as the strategy: the daemon must grant
// it to an actor, and **two copies of this literal that must match** is the defect class this project keeps
// finding. The daemon asserts its own constant against this one, so a divergence is a failing test rather
// than every MCP call being denied for a missing scope.
pub use jarvis_mcp::DEFAULT_MCP_SCOPE as DEFAULT_MCP_CALL_SCOPE;
pub use revision::{
    MODERN_REVISION, describe_negotiated, modern_revision, sdk_default_is_modern,
    sdk_default_revision,
};
