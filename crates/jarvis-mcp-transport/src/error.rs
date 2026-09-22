//! Failures that happen before a tool list exists.
//!
//! Both types are deliberately free of SDK types. `AGENTS.md` forbids provider SDK types crossing a
//! JARVIS boundary, and an error is a boundary: a caller that matched on an `rmcp` error variant
//! would have a reason to keep the SDK on its dependency list, which is exactly the coupling the
//! split into two crates exists to prevent. So the SDK's error is **classified** here — transport
//! fault, protocol fault, or a refusal the peer stated — and only the classification travels.

use rmcp::service::ClientInitializeError;

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

    /// The reported server identity could not be read from the initialization result.
    ///
    /// A missing name is not a connection failure — the connection worked — but it does mean the
    /// operator's record of what answered cannot be checked, which `P3-008d` treats as a signal
    /// worth surfacing rather than inventing a placeholder for.
    #[error("the server's initialization result carried no usable identity: {0}")]
    NoIdentity(String),
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
