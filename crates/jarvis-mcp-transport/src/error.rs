//! Failures that happen before a tool list exists.
//!
//! Both types are deliberately free of SDK types. `AGENTS.md` forbids provider SDK types crossing a
//! JARVIS boundary, and an error is a boundary: a caller that matched on an `rmcp` error variant
//! would have a reason to keep the SDK on its dependency list, which is exactly the coupling the
//! split into two crates exists to prevent. So the SDK's error is **classified** here — transport
//! fault, protocol fault, or a refusal the peer stated — and only the classification travels.

use rmcp::service::{ClientInitializeError, ServiceError};

/// A connection could not be established, or the negotiation did not complete.
#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    /// The child process could not be spawned, or the HTTP endpoint could not be reached.
    ///
    /// The message is the transport's own description, which is the actionable part. It is kept as
    /// text rather than as a variant per transport: which transport failed is already known from
    /// what the caller asked for, and encoding it twice would let the two disagree.
    #[error("the MCP server could not be reached: {0}")]
    Unreachable(String),

    /// The peer answered, and the answer could not be understood or accepted.
    ///
    /// Distinct from [`Self::Unreachable`] because the remedy differs: a protocol disagreement needs
    /// the revision read, a transport fault needs the network or the command checked. Collapsing
    /// them sends an operator to the wrong one.
    #[error("the MCP server refused the connection at the protocol level: {0}")]
    Refused(String),

    /// The peer did not answer the discovery request within the deadline.
    ///
    /// A distinct variant because the remedy differs from [`Self::Unreachable`]: nothing was
    /// refused and nothing errored, the peer simply never replied. That is what a **legacy** server
    /// looks like from here — it is listening for an `initialize` request that a modern client never
    /// sends — so an operator seeing this should suspect the server's protocol era before its
    /// network.
    ///
    /// This exists because the SDK's own discovery deadline is applied **only** in
    /// `ClientLifecycleMode::Auto`, not in `Discover` (verified in the pinned SDK's
    /// `service/client.rs`: `DEFAULT_AUTO_DISCOVER_TIMEOUT` is passed only in the `Auto` arm). So
    /// without this deadline a connection to a silent peer would wait forever, and a daemon starting
    /// against a dead or legacy server would hang rather than report.
    #[error(
        "the MCP server did not answer server/discover within {seconds}s; a server that speaks only \
         the legacy initialize handshake looks exactly like this from here"
    )]
    DiscoveryTimedOut {
        /// The deadline that elapsed.
        seconds: u64,
    },

    /// The peer negotiated a **legacy** revision even though a modern one was requested.
    ///
    /// This must never be silently accepted. The two eras are not interoperable: the legacy one has
    /// an `initialize` handshake and a session, the modern one is stateless with per-request
    /// metadata. A connection that falls back would appear healthy while speaking a protocol whose
    /// behaviour the rest of this crate does not implement — so it is a hard failure naming both
    /// revisions.
    #[error(
        "the server negotiated MCP {negotiated}, but JARVIS asked for {wanted}; the legacy era has an \
         initialize handshake and a session and is not interchangeable with the stateless revision"
    )]
    LegacyNegotiated {
        /// The revision the server agreed to.
        negotiated: String,
        /// The revision JARVIS asked for.
        wanted: String,
    },
}

/// A connection was established but its tool list could not be read.
#[derive(Debug, thiserror::Error)]
pub enum ListError {
    /// The request did not complete — the peer went away, or the page timed out.
    #[error("the tool list could not be read: {0}")]
    Unavailable(String),

    /// The peer answered with a protocol error rather than a tool list.
    ///
    /// Kept separate from [`Self::Unavailable`] because a protocol error is a fact about the server
    /// that a retry will not change, whereas an unavailable peer is worth another attempt.
    #[error("the server answered the tool list request with an error: {0}")]
    PeerError(String),
}

// NOTE: there was briefly a `NoIdentity(String)` variant here, described as "the server's
// initialization result carried no usable identity". It was **wrong twice**, and it is recorded
// rather than quietly deleted because both halves are the class of defect this project keeps
// finding.
//
// First, **no code ever constructed it.** Under a modern `server/discover` response the identity is
// optional, and `McpConnection::reported_identity` returns an *empty* `ReportedIdentity` for its
// absence — deliberately, because a server that *stops* naming itself is exactly the drift
// `P3-008d` must be able to observe, and a connection error would discard that observation. So the
// variant was unreachable, and a declared-but-unconstructed error variant reads as a live condition
// to everyone downstream.
//
// Second, its doc comment said a missing identity is "a signal worth surfacing", which **contradicts
// the code**: the code surfaces it as an empty identity in the *value*, not as an error. That is the
// same shape as the `expected_version` claim corrected at `P3-006c` — a statement written from the
// intent of a change rather than from what the change does.
//
// A third `ListError` variant is therefore not added until something can *produce* it. Absence of an
// identity is a fact on `ReportedIdentity`, not a failure of the listing.

/// A tool call was sent and did not produce a result this crate can hand back.
///
/// Four outcomes, kept apart because their remedies differ and collapsing them would send an operator
/// to the wrong one. In particular **a tool that reports a failure is not an error here**: a
/// `CallToolResult` with `isError` set is a *successful call whose tool refused*, and it belongs in
/// the outcome vocabulary (`ToolOutcome`), not in a transport error. Only the cases below mean the
/// call did not complete in a way this crate can describe.
#[derive(Debug, thiserror::Error)]
pub enum CallError {
    /// The request did not reach a result — the peer went away, or the transport failed.
    ///
    /// The caller cannot know whether the server began the effect, which is exactly what
    /// `AdapterError::AmbiguousAfterReaching` encodes one layer up.
    #[error("the tool call did not complete: {0}")]
    Unavailable(String),

    /// The peer answered with a JSON-RPC protocol error rather than a result.
    ///
    /// A fact about the server or the request that a retry will not change: an unknown tool, a bad
    /// argument shape, a refused method.
    #[error("the server answered the tool call with an error: {0}")]
    PeerError(String),

    /// The server's answer could not be decoded into a result this client recognises.
    ///
    /// **Its own case, and the reason is a measured SDK behaviour.** The protocol's result union is
    /// deserialized as an **untagged** enum, so a mismatch between the declared `resultType` and the
    /// object's actual fields does not fail as "a malformed `input_required`" — it fails as the SDK's
    /// generic `UnexpectedResponse`, which carries no information about which field was wrong. This
    /// variant exists so an operator sees "the server sent a shape this client could not read" rather
    /// than "the call did not complete", which would send them to inspect the network. It is reported
    /// as a *protocol* disagreement, because the peer answered and the answer was unusable.
    #[error(
        "the server's tool call result could not be decoded: {0}; the protocol's result union is \
         untagged, so this usually means the declared resultType and the object's fields disagree"
    )]
    Undecodable(String),

    /// The server asked for more input, which JARVIS cannot supply.
    ///
    /// Revision `2026-07-28` added **MRTR** (multi round-trip requests), where a server answers a
    /// call with `input_required` and the client retries the original request with the answers. The
    /// SDK can drive those rounds by invoking a client `ClientHandler`. **JARVIS declares no
    /// such handler**, because the only human in this system answers through JARVIS's own approval
    /// path and not through a third-party server's form — so a round could not be fulfilled and this
    /// is refused by name rather than left to fail deeper in.
    #[error(
        "the server requires additional input to complete the call (MRTR), and JARVIS supplies no \
         input handler; a tool that needs a human answer must ask through JARVIS's approval path"
    )]
    InputRequired,

    /// The server answered with a long-running **task** instead of a result.
    ///
    /// `2026-07-28` moved Tasks to an extension. This crate declares no tasks capability and polls no
    /// task, so a server returning one is speaking a mode JARVIS did not ask for. Refused by name
    /// rather than treated as an empty success.
    #[error(
        "the server answered the tool call with a long-running task, which JARVIS does not poll"
    )]
    Task,
}

impl CallError {
    /// Classifies an SDK call failure without retaining its type.
    ///
    /// Matched on variants rather than message text, so a reworded SDK error cannot silently
    /// reclassify a protocol refusal as an unreachable peer.
    ///
    /// `UnexpectedResponse` is mapped to [`Self::Undecodable`] rather than to
    /// [`Self::Unavailable`], and that distinction is the whole reason the variant exists. The SDK
    /// produces it when a response did not match any variant of the **untagged** result union — so
    /// the peer *did* answer, and the answer was unusable. Reporting that as "the call did not
    /// complete" would describe a network fault and send an operator to check a connection that is
    /// working.
    pub(crate) fn from_sdk(error: &ServiceError) -> Self {
        match error {
            ServiceError::McpError(_) => Self::PeerError(error.to_string()),
            ServiceError::UnexpectedResponse => Self::Undecodable(error.to_string()),
            _ => Self::Unavailable(error.to_string()),
        }
    }
}

impl ConnectError {
    /// Classifies the SDK's initialization failure without retaining its type.
    ///
    /// The variants are matched rather than read from the message text, because a reworded error in
    /// a patch release would otherwise silently reclassify a transport fault as a refusal — the same
    /// defect class as deciding a safety property from a `Display` string (`P3-008c`).
    pub(crate) fn from_sdk(error: &ClientInitializeError) -> Self {
        match error {
            // The transport itself failed: the child could not be spawned, the socket was refused,
            // the response could not be read. A caller should check the command or the endpoint.
            ClientInitializeError::TransportError { .. }
            | ClientInitializeError::ConnectionClosed(_)
            | ClientInitializeError::Cancelled => Self::Unreachable(error.to_string()),
            // The peer answered and the answer was not usable: an unexpected message, an
            // uncorrelated id, a JSON-RPC error, or no shared revision. Reconnecting will not fix
            // it, so it is reported as a refusal.
            _ => Self::Refused(error.to_string()),
        }
    }
}
