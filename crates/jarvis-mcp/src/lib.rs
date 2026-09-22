//! Model Context Protocol translation for JARVIS.
//!
//! # What this crate is, and what it deliberately is not
//!
//! This crate holds the **pure, offline** part of the MCP integration: how a server is identified,
//! how its tool names become canonical JARVIS identifiers, and which of its tool definitions this
//! client is willing to offer. Every function here is a function of its arguments — no sockets, no
//! child processes, no clock, no database.
//!
//! The protocol work that is *not* pure — the JSON-RPC transport, stdio child-process supervision,
//! HTTP header mirroring, OAuth, and the version negotiation handshake — is `P3-008`. Splitting it
//! this way means the rules a security reviewer most needs to read are testable without standing up
//! a peer, and the transport can be reimplemented (or the SDK replaced) without revisiting them.
//!
//! It deliberately does **not** depend on the MCP SDK. The translation rules below encode decisions
//! about *JARVIS* identifiers, and expressing them against the SDK's types would couple the security
//! boundary to a third-party crate's data model — the same reason `jarvis-tools` keeps provider SDK
//! types out of the domain.
//!
//! # The three rules this crate exists to enforce
//!
//! 1. **A server does not name itself.** An operator chooses the local name; the server's own
//!    claim about itself is recorded as evidence and never used as an identifier. The specification
//!    says the server name "is not guaranteed to be unique across servers and **SHOULD NOT** be
//!    relied upon for disambiguation", so treating it as an authority would be relying on the one
//!    field the protocol warns about.
//!
//! 2. **Two MCP tool names must never become one JARVIS identifier.** An MCP tool name may legally
//!    contain a dot while a JARVIS name half may not, so flattening or truncating would map distinct
//!    tools onto one identifier — and a call intended for one tool would run the other, with the
//!    other's declared risk. Names that cannot be represented are refused or digested, never
//!    rewritten, and every translation is checked for a collision against what is already assigned.
//!
//! 3. **A server-supplied schema is untrusted input.** Its `$ref`s are never resolved, its
//!    `x-mcp-header` annotations are validated against the protocol's own constraints before the
//!    tool is offered, and a tool that fails is excluded **by itself** rather than failing the list —
//!    which is the specification's explicit requirement.
//!
//! # A note on what is not yet proven
//!
//! Nothing here has been exercised against a real MCP server. The rules are derived from the
//! specification revision and the SDK source recorded in `docs/research/integrations/mcp.md`, and
//! they are tested as pure functions, but "the translation is right" and "a real server's tool list
//! translates" are different claims. `P3-008` is where the second one is tested.

mod conformance;
mod server;

pub use conformance::{
    ConformanceError, ConformanceReport, HEADER_ANNOTATION, MAX_CONFORMANCE_PROBLEMS,
    check_tool_schema, describe_schema_source, to_tool_schema,
};
pub use server::{
    CanonicalToolName, MAX_REPORTED_TEXT_CHARS, MAX_SERVER_NAME_CHARS, NameAssignments,
    NameCollision, NamingError, NamingStrategy, ReportedIdentity, ServerName, ServerNameError,
    canonical_tool_name,
};
