//! The transport implementation: the one place in this crate a socket exists.
//!
//! # Why this module is separate from [`crate::google::client`]
//!
//! `repository-layout.md` names `client.rs` as the "provider HTTP client and normalized errors", and this crate
//! put the provider's **decisions** there — status-and-reason classification, page tokens, cursor advance —
//! precisely because those are pure functions of a response (`ADR-0058`). The socket is a different job with a
//! different failure mode, so it lives here and the decision layer keeps its property: nothing in
//! [`crate::google::client`], [`crate::google::request`] or [`crate::google::operations`] names a `reqwest`
//! type, and every rule there is still testable with no server at all.
//!
//! `jarvis-models` splits the same way — contracts and decisions in the crate, a `Transport` implementation
//! against a real client — and `jarvis-mcp-transport` builds its own client so no-proxy and no-redirect are
//! statements this project makes rather than defaults inherited from a dependency's manifest (`ADR-0027`).
//!
//! # The port's requirements are enforced HERE, and they are now testable
//!
//! [`GoogleTransport`]'s module lists what an implementation must not do. That list was unenforceable while the
//! only implementations were test doubles; three of the four are now real controls on the client this module
//! builds, and each has a test against a hand-written server that observes the connection:
//!
//! - **No redirect is followed.** `Policy::none()` means a `3xx` arrives as an ordinary [`TransportResponse`]
//!   carrying its status, which [`crate::google::client::classify`] then classifies — so a redirect cannot move
//!   the bearer credential to another origin, and it is reported rather than silently resolved. There is
//!   deliberately **no** `error.is_redirect()` branch: with `Policy::none()` no redirect produces an error, so
//!   such a branch could never fire — the unreachable-refusal defect `P5-001` and `P5-003` each record.
//! - **No retry.** A transport that retried internally would make a non-idempotent effect repeat without the
//!   decision being visible or recorded, so this module sends exactly one request per call.
//! - **No proxy from the environment.** `no_proxy()` is called explicitly: `reqwest`'s `system-proxy` default is
//!   on, so `HTTP_PROXY` would otherwise route a credential and a user's mail through an unchosen intermediary.
//! - **A non-2xx status is not an error** — it is a [`TransportResponse`], because the status and the
//!   machine-readable reason are everything the caller's decision needs.
//!
//! # The failure mapping, and the ordering that decides it
//!
//! The dangerous direction is reporting **certainty** when the request may have been written, because a caller
//! that reads a failure as "nothing happened" will retry, and for a non-idempotent effect a retry is a second
//! effect. So `is_timeout()` is checked **before** `is_connect()`, which is a deliberate ordering and not the
//! obvious one: a timeout is ambiguous by definition — the request may have been written and the answer merely
//! late — while `is_connect()` fires for connection-establishment failures where nothing was written. Reporting
//! the ambiguous variant where the certain one might also apply can only over-refuse a retry, which is the safe
//! direction; the reverse mistake could repeat an effect. The mapping is a wildcard-free `match` on the error's
//! own predicates with `Send` as the final arm, and that arm is itself the conservative answer.
//!
//! # What is deliberately not here
//!
//! - **No body-size bound**, and that is a recorded limit rather than an oversight: [`TransportFailure`] has no
//!   variant meaning "the answer was too large to read", and [`TransportFailure::Body`] means the body could not
//!   be read rather than that this client declined to. A streaming cap needs `bytes_stream` and a policy that
//!   belongs with the output handling `P5-009` owns. The downstream `tool_call_repository` bounds the *stored*
//!   output at 32 KiB, so what is unbounded is the transient buffer, not the durable row.
//! - **No token refresh.** The token is presented as given; a stale one becomes a provider refusal, which is the
//!   honest reading and the reason [`crate::google::operations::GoogleReadTool`] documents taking a token rather
//!   than minting one.
//! - **No `Retry-After` interpretation.** The header is carried into [`TransportResponse`] and never acted on,
//!   because honouring a stated delay is a retry decision and this module does not own retry.

use std::time::Duration;

use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, RETRY_AFTER};

use crate::google::credential::AccessToken;
use crate::google::request::HttpRequest;
use crate::google::transport::{GoogleTransport, HttpMethod, TransportFailure, TransportResponse};

/// How a client identifies itself to the provider.
///
/// A versioned JARVIS string rather than a browser's: a provider that sees an unexpected client can attribute
/// its traffic, and a default user agent would impersonate whatever the library happened to choose.
pub const USER_AGENT: &str = concat!("jarvis-connectors/", env!("CARGO_PKG_VERSION"));

/// The default request deadline, in seconds.
///
/// The port requires an implementation to bound its own wait and report [`TransportFailure::Timeout`] rather
/// than hanging a worker. Thirty seconds matches the tool contract's own `TOOL_TIMEOUT_SECONDS`, because a
/// transport that waited longer than the call's declared deadline could not report the timeout the caller
/// already decided on.
pub const DEFAULT_TIMEOUT_SECONDS: u64 = 30;

/// How long to allow for establishing a connection, in seconds.
///
/// Shorter than the request deadline, because a connection that cannot be established quickly is a different
/// condition from an answer that is slow, and the two must be distinguishable. A connect timeout surfaces
/// through `is_timeout()`, so it is reported as the ambiguous [`TransportFailure::Timeout`] — see the module doc
/// for why that direction is the deliberate one.
pub const DEFAULT_CONNECT_TIMEOUT_SECONDS: u64 = 10;

/// The transport that speaks HTTP through `reqwest`.
///
/// One client is built once and reused, so the connection pool, the TLS configuration and the redirect policy
/// are decided in one place and cannot drift between calls.
pub struct ReqwestTransport {
    client: reqwest::Client,
}

impl ReqwestTransport {
    /// Builds a client with the default deadline.
    ///
    /// # Errors
    ///
    /// Returns [`TransportFailure::Refused`] if the client cannot be built, which is a configuration fault in
    /// this process rather than a network condition — and it is reported rather than unwrapped so a broken TLS
    /// or resolver setup surfaces as a diagnostic instead of a panic on the first call.
    pub fn new() -> Result<Self, TransportFailure> {
        Self::with_timeouts(
            Duration::from_secs(DEFAULT_TIMEOUT_SECONDS),
            Duration::from_secs(DEFAULT_CONNECT_TIMEOUT_SECONDS),
        )
    }

    /// Builds a client with the given request and connect deadlines.
    ///
    /// The two are separate because they answer different questions — "how long may the whole exchange take"
    /// and "how long may establishing the connection take" — and a test needs a short deadline to exercise the
    /// timeout path without waiting a default thirty seconds.
    ///
    /// # Errors
    ///
    /// As [`Self::new`].
    pub fn with_timeouts(
        timeout: Duration,
        connect_timeout: Duration,
    ) -> Result<Self, TransportFailure> {
        let client = reqwest::Client::builder()
            // `reqwest`'s `system-proxy` default is ON, so without this a local model's or a mailbox's traffic
            // would be routed through whatever `HTTP_PROXY` names. `jarvis-models` and `jarvis-mcp-transport`
            // both make the same call for the same reason.
            .no_proxy()
            // A redirect can move a bearer credential to another origin. With `none()` a `3xx` is returned as a
            // response carrying its status, so it is classified rather than followed.
            .redirect(reqwest::redirect::Policy::none())
            .timeout(timeout)
            .connect_timeout(connect_timeout)
            .user_agent(USER_AGENT)
            .build()
            .map_err(|_| TransportFailure::Refused {
                // The error's own rendering is not forwarded: it can name a path or a proxy URL, and this
                // module's whole job is to keep the boundary between a value that may be logged and one that
                // may not.
                reason: "the HTTP client could not be built",
            })?;
        Ok(Self { client })
    }
}

impl std::fmt::Debug for ReqwestTransport {
    /// Names the transport without printing its internals.
    ///
    /// `reqwest::Client`'s own `Debug` renders its configuration, which may include resolver and TLS settings;
    /// a transport's rendering belongs in a diagnostic and not in a formatted value.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ReqwestTransport { .. }")
    }
}

#[async_trait::async_trait]
impl GoogleTransport for ReqwestTransport {
    /// Sends exactly one request and reads its answer.
    ///
    /// # Errors
    ///
    /// Returns [`TransportFailure`] when the exchange produced **no** provider answer, classifying by *when*
    /// the failure happened rather than by what it was called. A non-2xx status — including a `3xx`, which is
    /// not followed — is a [`TransportResponse`] and not an error.
    async fn send(
        &self,
        method: HttpMethod,
        request: &HttpRequest,
        token: &AccessToken,
    ) -> Result<TransportResponse, TransportFailure> {
        // `match` rather than a conversion, so adding a method to the port is a compile error here rather than
        // a silent default. `HttpMethod` has one variant today and `P5-009`'s write will grow it.
        let verb = match method {
            HttpMethod::Get => reqwest::Method::GET,
        };
        // The query is already percent-encoded by the request layer, and `url_with_query` joins rather than
        // re-encoding: encoding twice would turn `%20` into `%2520` and the provider would receive the literal
        // text. The credential is NOT in the URL — `HttpRequest` has no field for one — so the rendered target
        // is safe to hand to a client that may put it in an error.
        let response = self
            .client
            .request(verb, request.url_with_query())
            // The header value is built by the token, not here: a transport assembling `"Bearer " + value`
            // itself is a second implementation of the scheme, which is where a missing space or a doubled
            // `Bearer ` comes from.
            .header(AUTHORIZATION, token.authorization_header_value())
            .header(ACCEPT, request.accept())
            .send()
            .await
            .map_err(|error| classify_error(&error))?;

        let status = response.status().as_u16();
        // Carried and never interpreted: honouring a stated delay is a retry decision, and `client::classify`
        // owns retry. The form is preserved rather than flattened — a value stated as an `HTTP-date` is
        // `Some(RetryAfter::NotSeconds)`, never `None`, because `None` means the header was **absent** and the
        // two must not be confused or a stated delay is silently retried too soon (`ADR-0076`).
        let retry_after = crate::google::transport::parse_retry_after(
            response
                .headers()
                .get(RETRY_AFTER)
                .and_then(|value| value.to_str().ok()),
        );
        // A body that cannot be read is `Body`, which `may_have_reached_the_provider` reports as ambiguous —
        // correct, because the status here proves the provider answered and an unreadable body says nothing
        // about what it did.
        let body = response.text().await.map_err(|_| TransportFailure::Body)?;

        Ok(TransportResponse {
            status,
            retry_after,
            body,
        })
    }

    /// Sends a form `POST` and reads its answer.
    ///
    /// # Why `.body(..)` and not `.form(..)`
    ///
    /// `reqwest`'s `.form()` is behind a feature this workspace does not enable, and enabling it would add a
    /// **second** form encoder to the tree — the dependency's — beside the one `ADR-0071`/`ADR-0072` exist to
    /// keep single. The two would disagree about a space exactly as they did before, and the disagreement would
    /// be invisible because both produce a plausible body. So the body arrives already rendered and this sends
    /// it verbatim, setting the `Content-Type` from the request rather than letting a helper decide it.
    ///
    /// The answer is read exactly as [`Self::send`] reads one: `Retry-After` carried and never interpreted, and
    /// a body that cannot be read reported as [`TransportFailure::Body`] — which is ambiguous, because the
    /// status proves the provider answered.
    async fn send_form(
        &self,
        request: &crate::google::request::FormRequest,
    ) -> Result<TransportResponse, TransportFailure> {
        let response = self
            .client
            .request(reqwest::Method::POST, request.url())
            .header(CONTENT_TYPE, request.content_type())
            .header(ACCEPT, crate::google::request::JSON_ACCEPT)
            // `rendered_body` returns the text; it is never logged here and the request type has no `Display`,
            // which is what keeps it out of an artifact.
            .body(request.rendered_body().to_owned())
            .send()
            .await
            .map_err(|error| classify_error(&error))?;

        let status = response.status().as_u16();
        let retry_after = crate::google::transport::parse_retry_after(
            response
                .headers()
                .get(RETRY_AFTER)
                .and_then(|value| value.to_str().ok()),
        );
        let body = response.text().await.map_err(|_| TransportFailure::Body)?;
        Ok(TransportResponse {
            status,
            retry_after,
            body,
        })
    }
}

/// Maps a `reqwest` failure onto the port's vocabulary by **when** it happened.
///
/// # Why the ordering is timeout-before-connect
///
/// `is_timeout()` is checked first although `is_connect()` is the more specific answer where it also applies.
/// The reason is the failure direction: `Timeout` is ambiguous — the request may have been written — so
/// choosing it where the certain variant might also fit can only make a caller *less* willing to retry, whereas
/// the reverse mistake would let a non-idempotent effect repeat. A pure connect failure (DNS, refused
/// connection) is not a timeout and still reports [`TransportFailure::Connect`], so the specific case is not
/// lost.
///
/// # Why there is no `is_redirect` or `is_builder` arm
///
/// With `Policy::none()` no redirect produces an error, so an `is_redirect()` arm could never fire — the
/// unreachable-refusal defect this repository records. A builder failure for a request this module constructs is
/// likewise unreachable in practice, and if one occurred it would fall to the `Send` arm below, which is the
/// conservative answer rather than a claim of certainty.
fn classify_error(error: &reqwest::Error) -> TransportFailure {
    if error.is_timeout() {
        return TransportFailure::Timeout;
    }
    if error.is_connect() {
        return TransportFailure::Connect;
    }
    if error.is_body() || error.is_decode() {
        return TransportFailure::Body;
    }
    if error.is_request() {
        // The request was being written when the exchange broke, so it may have arrived — the ambiguous case
        // `TransportFailure::Send` exists for, and the one that must never be reported as certain.
        return TransportFailure::Send;
    }
    // The conservative default: an unrecognised failure is treated as one that may have reached the provider.
    TransportFailure::Send
}

#[cfg(test)]
#[path = "http_tests.rs"]
mod tests;
