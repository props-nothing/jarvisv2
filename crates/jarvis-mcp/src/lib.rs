//! Model Context Protocol translation for JARVIS.
//!
//! # What this crate is, and what it deliberately is not
//!
//! This crate holds the **pure, offline** part of the MCP integration: how a server is identified,
//! how its tool names become canonical JARVIS identifiers, what its tools are permitted to do, and
//! which of them this client is willing to offer. Every function here is a function of its arguments
//! — no sockets, no child processes, no clock, no database.
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
//! # The four rules this crate exists to enforce
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
//! 3. **A server does not decide what its tools may do.** The protocol's `annotations` carry
//!    `readOnlyHint`/`destructiveHint`/`idempotentHint` and warns that clients must treat them as
//!    untrusted, and those are precisely the facts policy needs. Effects, risk, and approval come
//!    from an operator's [`ToolEffectPolicy`]; a server nobody has classified gets a posture under
//!    which every call is held for a human.
//!
//! 4. **A server-supplied schema is untrusted input.** Its `$ref`s are never resolved, its
//!    `x-mcp-header` annotations are validated against the protocol's own constraints before the
//!    tool is offered, and a tool that fails is excluded **by itself** rather than failing the list —
//!    which is the specification's explicit requirement.
//!
//! Rule 2 is enforced across servers by [`McpCatalog`], and the enforcement has a consequence worth
//! knowing before reading it: when two servers' tools collide, the catalog **reports it rather than
//! dropping one**, because dropping one would make the reachable tool set depend on configuration
//! order.
//!
//! A fifth rule follows from rule 1 rather than standing beside it. **A server's change of
//! self-description is recorded, not refused.** Because rule 1 means the server's own name is never
//! an identity, nothing else in the protocol can notice that the process behind an operator's chosen
//! name has changed — so [`McpCatalog`] compares each server's [`ReportedIdentity`] against a previous
//! observation and reports an [`IdentityDrift`] when the name it claims differs. It is deliberately
//! not an error: a vendor renaming its product is ordinary, and making that an outage would be
//! worse than the thing it guards against. What matters is that the operator who classified *that
//! name* can find out.
//!
//! # A note on what is not yet proven
//!
//! Nothing here has been exercised against a real MCP server. The rules are derived from the
//! specification revision and the SDK source recorded in `docs/research/integrations/mcp.md`, and
//! they are tested as pure functions, but "the translation is right" and "a real server's tool list
//! translates" are different claims. `P3-008` is where the second one is tested.
//!
//! There is also **no configuration surface yet**: a [`ConfiguredServer`] and its policy are built in
//! code, so nothing reads them from `config.toml`. Every type here is therefore a capability a
//! caller can use, not one an operator can currently reach.
//!
//! # The server side, and why it is a policy value here rather than SDK configuration
//!
//! [`ServerExposure`] is the first of this crate's types that faces **inward**: it decides which browser
//! origins JARVIS will serve when JARVIS is the MCP server rather than the client. It lives here, and not
//! in the transport crate next to the SDK, because the SDK's server defaults are **permissive on the
//! specification's MUSTs** — an empty `allowed_origins` disables `Origin` validation entirely, and its own
//! comparison makes a portless entry a wildcard over every port while treating an explicit default port as
//! literal. A default is the one value that changes without a line in this repository changing, so the
//! policy is stated as a value here and mapped onto the SDK's fields explicitly. See ADR-0031.

mod catalog;
mod conformance;
mod definition;
mod exposure;
mod served;
mod server;

pub use catalog::{
    CatalogEntry, CatalogError, CatalogExclusion, ConfiguredServer, IdentityDrift, MAX_MCP_SERVERS,
    McpCatalog, ObservedServer, ServerListing,
};
pub use conformance::{
    ConformanceError, ConformanceReport, HEADER_ANNOTATION, MAX_CONFORMANCE_PROBLEMS,
    check_tool_schema, describe_schema_source, to_tool_schema,
};
pub use definition::{
    DEFAULT_MCP_SCOPE, DEFAULT_MCP_TIMEOUT_SECONDS, ExcludedTool, ListingOutcome, McpToolListing,
    PolicyError, ToolEffectPolicy, TranslatedTool, TranslationError, sanitize, tool_version,
    translate_listing, translate_tool,
};
pub use exposure::{
    AllowedOrigin, ExposureError, MAX_ALLOWED_ORIGINS, MAX_ORIGIN_ENTRY_BYTES, OriginError,
    OriginVerdict, ServerExposure, is_loopback_host,
};
pub use served::{
    ExposureError as ServedExposureError, ExposureExclusion, MAX_EXPOSED_TOOLS, ServableName,
    ServedTool, served_tools,
};
pub use server::{
    CanonicalToolName, MAX_REPORTED_TEXT_CHARS, MAX_SERVER_NAME_CHARS, NameAssignments,
    NameCollision, NamingError, NamingStrategy, ReportedIdentity, ServerName, ServerNameError,
    canonical_tool_name,
};
