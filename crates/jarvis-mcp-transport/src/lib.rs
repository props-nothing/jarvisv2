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
//! # What is not here yet
//!
//! Nothing in this crate is reachable from `jarvisd`: there is no configuration surface for MCP
//! servers, and `P3-009` owns the daemon that would load one. So this is a capability a caller can
//! use, not one an operator can reach — the same honest limit `P3-008a`..`P3-008d` each recorded.

mod client;
mod error;
mod revision;

pub use client::{
    McpConnection, OwnedListing, ServerCapabilities, StdioCommand, ToolBuffer, connect_http,
    connect_over, connect_stdio,
};
pub use error::{ConnectError, ListError};
pub use revision::{
    MODERN_REVISION, describe_negotiated, modern_revision, sdk_default_is_modern,
    sdk_default_revision,
};
