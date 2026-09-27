//! The transport port: what performs one HTTP request, and what a caller must answer.
//!
//! # Why this is a trait here rather than a `reqwest` dependency
//!
//! `ADR-0058` put Google's decisions in this crate precisely because it has no HTTP stack, so a request is a
//! value and every rule is a pure function of its arguments. Adding a client library here would undo that: the
//! provider's request and response *types* would become the shape the rules are written against, and every
//! mapping would need a server or a mock to test.
//!
//! So this module states the **port** and nothing else. It mirrors `jarvis-models`' `Transport`, with two
//! differences that are forced rather than stylistic:
//!
//! - **An access token is a parameter, not a field of the request.** The request is a value that may be logged;
//!   a credential is not. Passing the [`AccessToken`] separately means a transport cannot accidentally put the
//!   request-with-credential into a structure, and it is the only way the material crosses this boundary.
//! - **The method is an `enum`, not a `&'static str`.** All three declared operations are `GET`s, so a string
//!   would be a field with one value and a typo would be a runtime fault. When a write operation arrives
//!   (`P5-009`) the enum grows by one variant and every `match` on it is a compile error — which is what makes
//!   a new method a deliberate edit.
//!
//! # What a transport must not do
//!
//! - **Follow a redirect.** A redirect can move a bearer credential to another origin. `jarvis-models` builds
//!   its client with `redirect(Policy::none())` for exactly this reason, and a transport that followed one
//!   would defeat the credential boundary this crate spent a slice building.
//! - **Retry.** Retry is a declaration on the tool contract and a decision of the pipeline. A transport that
//!   retried internally would make a non-idempotent effect repeat without the decision being visible or
//!   recorded — `ToolExecutor`'s own doc says the same for an adapter.
//! - **Read a proxy from the environment.** `HTTP_PROXY` would route a credential and a user's mail through an
//!   unchosen intermediary.
//! - **Return an error for a non-2xx status.** A provider's refusal is a *response* the caller classifies, not
//!   a transport failure. Collapsing the two would lose the status and the reason, which is everything
//!   `client::classify` needs.

use std::fmt;

use crate::google::credential::AccessToken;
use crate::google::request::HttpRequest;

/// The HTTP methods a Google connector will ever use.
///
/// A closed set with one member today. Every declared operation is a read, so `POST`, `PATCH`, `PUT` and
/// `DELETE` are deliberately **absent** rather than present-and-unused: a variant nothing constructs is a
/// method a reader would assume is reachable, and adding one is `P5-009`'s decision rather than a convenience.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HttpMethod {
    /// A read. The only method this connector currently uses.
    Get,
}

impl HttpMethod {
    /// Returns the method as the wire spells it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
        }
    }
}

impl fmt::Display for HttpMethod {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Why a request could not be performed at all.
///
/// **None of these is a provider's answer.** Each means the exchange did not produce one, which is why every
/// variant belongs to the *ambiguous* side of the outcome boundary: a request that was sent and not answered
/// may still have had an effect, and only the caller can decide what that means. A provider that answered — with
/// any status, including a refusal — is a [`TransportResponse`] rather than an error.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum TransportFailure {
    /// The request could not be sent: a DNS failure, a refused connection, a TLS failure.
    ///
    /// The one case where nothing reached the provider, and a transport can only report it when it is certain.
    /// A connect failure is certain; a failure *during* a write is not, which is why [`Self::Send`] exists
    /// separately.
    #[error("the request could not be sent")]
    Connect,
    /// The request was written and the connection failed before a complete answer arrived.
    ///
    /// **The ambiguous case, and the reason this is a separate variant.** A provider may have received and acted
    /// on the request, so a caller must treat it as `AmbiguousAfterReaching` rather than as "nothing happened".
    #[error("the request was sent but no complete response arrived")]
    Send,
    /// A response arrived and its body could not be read, or the body was not the expected encoding.
    #[error("the response body could not be read")]
    Body,
    /// The deadline passed.
    ///
    /// **Ambiguous for the same reason as [`Self::Send`]**: a request that timed out may have arrived. A
    /// transport cannot distinguish "the provider never saw it" from "the provider answered too slowly", and
    /// guessing either way is worse than reporting the ambiguity.
    #[error("the request did not complete before its deadline")]
    Timeout,
    /// The transport refused to send, before writing anything.
    ///
    /// The only variant a **policy** produces, and the one an explicit refusal — a forbidden redirect, a
    /// hostname off the allowlist — must use, because the request never left. Distinct from [`Self::Connect`]
    /// because a refusal is this client's decision and a connect failure is the network's.
    #[error("the transport refused the request: {reason}")]
    Refused {
        /// A bounded explanation, safe to show an operator.
        reason: &'static str,
    },
}

impl TransportFailure {
    /// Returns whether the request may have been received and acted on.
    ///
    /// The predicate that maps a failure onto the outcome boundary, and it is deliberately **not**
    /// `!is_certain_nothing_happened`: an unanswered request is ambiguous, so `true` here means a caller must
    /// refuse an automatic retry for a non-idempotent effect.
    #[must_use]
    pub const fn may_have_reached_the_provider(self) -> bool {
        match self {
            // The two certain cases: nothing was written, so nothing happened.
            Self::Connect | Self::Refused { .. } => false,
            // A sent request with no answer, a timed-out request, and a response whose body could not be read
            // may all have been acted on. The last is the subtle one: the provider answered, so it certainly
            // received the request, and an unreadable body says nothing about what it did.
            Self::Send | Self::Timeout | Self::Body => true,
        }
    }
}

/// One HTTP response, as a transport observed it.
///
/// Holds the status, the provider's `Retry-After` when present, and the body. It does **not** hold the
/// request or the credential: a response value may be logged, a request-with-credential may not.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransportResponse {
    /// The HTTP status.
    pub status: u16,
    /// The provider's stated delay in seconds, when the response carried one.
    ///
    /// Optional and not interpreted: whether a stated delay is *honoured* is `client::classify`'s decision, and
    /// a transport that clamped or defaulted it would be making a retry decision it does not own.
    pub retry_after_seconds: Option<u32>,
    /// The response body as text.
    ///
    /// Text rather than bytes because both APIs are JSON, and a byte body would make every caller decode. A
    /// provider that sent a non-UTF-8 body is a [`TransportFailure::Body`], which is a classification the
    /// transport makes rather than forwarding bytes a caller must guess about.
    pub body: String,
}

impl TransportResponse {
    /// Returns whether the status is a success.
    #[must_use]
    pub const fn is_success(&self) -> bool {
        self.status == 200
    }
}

/// Performs one HTTP request.
///
/// An implementation is the only place a socket exists, and the only place [`AccessToken`]'s material is read.
/// Nothing else in this crate can reach the bytes, which is what `ADR-0061` bought.
///
/// # The two arguments are separate on purpose
///
/// [`HttpRequest`] is a value that may be rendered — its own `Display` prints the method, the path, and
/// parameter *names* — and [`AccessToken`] is a value that may not. Merging them into one authenticated-request
/// type would make a single `{:?}` leak the credential, which is the mistake the split exists to prevent.
pub trait GoogleTransport: Send + Sync {
    /// Performs a request and returns the response.
    ///
    /// # Errors
    ///
    /// Returns [`TransportFailure`] when the exchange produced **no** provider answer. A non-2xx status is a
    /// response, not an error.
    ///
    /// # Cancellation
    ///
    /// The deadline is the request's own and is carried by `jarvis_core`'s request type in a real
    /// implementation; this signature does not take one because a cancellation token belongs to a run and
    /// nothing here has one — the same absence `ToolExecutor`'s module records for the same reason. A
    /// transport that cannot bound its own wait would hang a worker, so an implementation **must** apply a
    /// timeout and report [`TransportFailure::Timeout`] rather than waiting indefinitely.
    fn send(
        &self,
        method: HttpMethod,
        request: &HttpRequest,
        token: &AccessToken,
    ) -> Result<TransportResponse, TransportFailure>;
}

impl fmt::Debug for dyn GoogleTransport {
    /// Names the transport without printing its internals.
    ///
    /// An implementation holds a client, and a client may hold a connection pool, headers, and a proxy
    /// configuration — none of which belongs in a formatted value.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("GoogleTransport { .. }")
    }
}

/// Returns the method a request declares, checked against the port's closed set.
///
/// # Errors
///
/// Returns [`TransportFailure::Refused`] for a method [`HttpMethod`] cannot express, which is a defect in this
/// crate rather than a caller's mistake — `HttpRequest` builds only `GET`s today. It is reported rather than
/// unreachable, because when `P5-009` adds a write the two must be extended together and a silent default would
/// send a `GET` for a write.
pub fn method_of(request: &HttpRequest) -> Result<HttpMethod, TransportFailure> {
    match request.method() {
        "GET" => Ok(HttpMethod::Get),
        _ => Err(TransportFailure::Refused {
            reason: "the request names an HTTP method this transport port cannot express",
        }),
    }
}

#[cfg(test)]
#[path = "transport_tests.rs"]
mod tests;
