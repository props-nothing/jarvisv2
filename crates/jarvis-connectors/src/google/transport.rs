//! The transport port: what performs one HTTP request, and what a caller must answer.
//!
//! # Why this is a trait here rather than a `reqwest` dependency
//!
//! `ADR-0058` put Google's decisions in this crate precisely because it has no HTTP stack, so a request is a
//! value and every rule is a pure function of its arguments. Adding a client library here would undo that: the
//! provider's request and response *types* would become the shape the rules are written against, and every
//! mapping would need a server or a mock to test.
//!
//! So this module states the **port** and nothing else. It mirrors `jarvis-models`' `Transport`, with three
//! differences that are forced rather than stylistic:
//!
//! - **An access token is a parameter, not a field of the request.** The request is a value that may be logged;
//!   a credential is not. Passing the [`AccessToken`] separately means a transport cannot accidentally put the
//!   request-with-credential into a structure, and it is the only way the material crosses this boundary.
//! - **The method is an `enum`, not a `&'static str`.** Every declared operation is a `GET`, so a string
//!   would be a field with one value and a typo would be a runtime fault. When a write operation arrives
//!   (`P5-009`) the enum grows by one variant and every `match` on it is a compile error — which is what makes
//!   a new method a deliberate edit.
//! - **`send` is `async`.** Not a preference: [`jarvis_tools::ToolExecutor::execute`] is `async`, so an
//!   adapter binding this port must be too, and an `async` function calling a blocking one blocks a runtime
//!   worker for the whole round trip. The interface this port must satisfy decides the shape.
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
    /// Returns a bounded, operator-facing reason that is **certain nothing was written**.
    ///
    /// # Why the certainty is part of the contract
    ///
    /// A caller reports this as "the request was never sent", so a variant that may have reached the provider
    /// must not answer here — doing so would state a certainty the failure does not carry. The two ambiguous
    /// variants therefore return the *ambiguous* sentence rather than a reassuring one, and the assertion in the
    /// tests checks that pairing against [`Self::may_have_reached_the_provider`] rather than trusting the text.
    ///
    /// This exists because a caller needs a `&'static str` for a variant that must not own allocated text —
    /// notably [`crate::token::TokenRequestOutcome::NeverSent`]. Formatting the `Display` there would allocate
    /// and would lose the static lifetime for no benefit, since every reason here is a constant.
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            // The two certain cases: nothing was written, so nothing happened.
            Self::Connect => "the request could not be sent",
            Self::Refused { reason } => reason,
            // The ambiguous three. A caller reporting `NeverSent` on one of these would be claiming a certainty
            // it does not have, so the text says what is actually known.
            Self::Send => "the request was written and no complete answer arrived",
            Self::Timeout => "the request may have been written before its deadline",
            Self::Body => "the provider answered and the body could not be read",
        }
    }

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

/// The provider's `Retry-After`, as one of the states the field can be in.
///
/// RFC 9110 §10.2.3 defines `Retry-After = HTTP-date / delay-seconds`, and the two forms are **different facts
/// a caller must be able to tell apart**. A `delay-seconds` value is a wait this client can honour directly; the
/// `HTTP-date` form is an absolute instant, and converting it to a wait needs a clock — which a transport does
/// not own and a pure classifier does not have. [`Self::NotSeconds`] therefore covers the date form **and** any
/// other present value this client cannot express in seconds; its doc states exactly what it does and does not
/// claim.
///
/// # Why this is an enum rather than `Option<u32>`
///
/// `Option<u32>` has two values and the field has **three** situations:
///
/// 1. no `Retry-After` at all — the provider stated nothing;
/// 2. `Retry-After: 120` — a stated `delay-seconds`;
/// 3. `Retry-After: Fri, 31 Dec 1999 23:59:59 GMT` — a stated `HTTP-date`.
///
/// Folding 1 and 3 into `None` is the defect this type removes: `None` then reads as "no delay stated" when in
/// fact a delay **was** stated and this client could not use it. That is the direction that retries **too
/// soon** — before the time the provider asked for — and a `None` that means two things is a value nothing
/// holds both of. RFC 9110 §5.6.7 makes the date form one a recipient **MUST accept**, so "we could not parse
/// it" is not the same as "it was not there".
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetryAfter {
    /// `delay-seconds`: a non-negative integer of seconds (RFC 9110 §10.2.3).
    Seconds(u32),
    /// A value was **present** and is not a `delay-seconds` number this client can convert.
    ///
    /// Named for what is **readable** — "not seconds" — rather than for the conclusion. The field's grammar has
    /// one other alternative, an `HTTP-date` (RFC 9110 §5.6.7), so an `HTTP-date` is the usual member; but the
    /// name would be a lie for an all-digit value too large for a `u32`, which **is** `delay-seconds` by
    /// grammar (`1*DIGIT` has no upper bound) and simply does not fit. Both land here because both are "a delay
    /// was stated that this client cannot express in seconds", and that is the only distinction a retry
    /// decision needs.
    ///
    /// # What this variant does and does not claim
    ///
    /// It claims the field was present with a value this client did **not** read as a number of seconds. It
    /// does **not** claim the value is a well-formed date, and it does not convert it to a delay: doing so needs
    /// a clock, and a transport that read its own clock would be making a retry decision it does not own — the
    /// same rule that keeps [`Self::Seconds`] uninterpreted. A value that is neither form is grouped here too,
    /// rather than silently dropped to `None`, so it can never be mistaken for "the provider stated nothing".
    NotSeconds,
}

impl RetryAfter {
    /// Returns the delay in seconds when the provider stated an unambiguous one, and `None` otherwise.
    ///
    /// `None` here is **not** "no delay stated" — [`RetryAfter`] has no such value; it is "a delay was stated in
    /// a form this client does not convert to seconds" ([`Self::NotSeconds`]). A caller that treats this `None`
    /// as "no delay" reintroduces the conflation the type exists to prevent.
    #[must_use]
    pub const fn seconds(self) -> Option<u32> {
        match self {
            Self::Seconds(seconds) => Some(seconds),
            Self::NotSeconds => None,
        }
    }

    /// Returns whether the provider stated the delay as an unambiguous number of seconds.
    #[must_use]
    pub const fn is_delay_seconds(self) -> bool {
        matches!(self, Self::Seconds(_))
    }
}

/// Interprets a `Retry-After` header value into the form it was stated in.
///
/// `None` — **no header** — maps to `None`. A `1*DIGIT` value maps to [`RetryAfter::Seconds`]. Anything else
/// present maps to [`RetryAfter::NotSeconds`] (see that variant for what it does and does not claim).
///
/// # Why a pure function here rather than inline in the transport
///
/// The parse is a **rule about a header value**, so it is testable without a socket, and the two `reqwest`
/// sites in `google::http` both need the identical interpretation. Keeping it beside [`RetryAfter`] means the
/// grammar and the type that represents it live together.
///
/// # The three cases, and why an oversize number is not `None`
///
/// An all-digit value too large for a `u32` — RFC 9110 §10.2.3 sets no upper bound on `delay-seconds` — is
/// grouped with [`RetryAfter::NotSeconds`] rather than dropped. Dropping it would read as "the provider stated
/// nothing" and retry at once, which is the exact inversion of an instruction to wait; grouping it keeps it a
/// **stated** delay this client cannot use as a number. An empty value is the one present-but-empty case: both
/// alternatives require content, so it carries nothing to interpret and is `None`, matching an absent field.
#[must_use]
pub fn parse_retry_after(value: Option<&str>) -> Option<RetryAfter> {
    // RFC 9110 §5.5 requires a recipient to exclude surrounding whitespace before evaluating a field value.
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }
    // `delay-seconds = 1*DIGIT` is the grammar, so requiring every octet to be an ASCII digit is the rule and
    // not a heuristic — `+30`, `30.5`, and ` 30 ` are not `delay-seconds`.
    if value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Some(
            value
                .parse::<u32>()
                .map_or(RetryAfter::NotSeconds, RetryAfter::Seconds),
        );
    }
    Some(RetryAfter::NotSeconds)
}

/// One HTTP response, as a transport observed it.
///
/// Holds the status, the provider's `Retry-After` when present, and the body. It does **not** hold the
/// request or the credential: a response value may be logged, a request-with-credential may not.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransportResponse {
    /// The HTTP status.
    pub status: u16,
    /// The provider's stated delay, when the response carried one — and **which form** it carried.
    ///
    /// `None` means the header was **absent**; it never means "present but unreadable", which is
    /// [`RetryAfter::NotSeconds`]. Optional and not interpreted: whether a stated delay is *honoured* is
    /// `client::classify`'s decision, and a transport that clamped or defaulted it would be making a retry
    /// decision it does not own.
    pub retry_after: Option<RetryAfter>,
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
///
/// # Why `async` rather than a blocking method
///
/// [`jarvis_tools::ToolExecutor::execute`] is `async`, so an adapter that bound this port must itself be
/// `async` — and an `async` function calling a blocking one is not a style choice, it is a defect: it blocks a
/// runtime worker for the whole round trip, which is the failure `P2-007` records for a blocking read inside a
/// stream. So the port is `async` because the interface it must satisfy is `async`, and the honest alternative
/// — an adapter that spawns a blocking thread per call — would be a threading decision taken inside one
/// connector. `#[async_trait]` is used rather than native `async fn` in a trait because the trait must stay
/// **object safe** to be held as `&dyn GoogleTransport`.
#[async_trait::async_trait]
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
    async fn send(
        &self,
        method: HttpMethod,
        request: &HttpRequest,
        token: &AccessToken,
    ) -> Result<TransportResponse, TransportFailure>;

    /// Performs a form `POST` and returns the response.
    ///
    /// # Why this is a second method rather than a third [`HttpMethod`] variant
    ///
    /// [`HttpMethod`] is deliberately a closed set with one member, and `P5-009` owns the decision to add a
    /// write. This is not a write in that sense: it is the **token endpoint's framing**, which carries its
    /// credential in the *body* rather than in a header, so it has no bearer token to pass. Folding it into
    /// `send` would give that method a credential parameter the token request does not use and a body the read
    /// path does not have — one method with two disjoint modes, which is the shape this port's `HttpMethod`
    /// comment already argues against.
    ///
    /// # The two arguments are separate on purpose, and here it is a different separation
    ///
    /// `send` separates [`HttpRequest`] from [`AccessToken`] because the first may be rendered and the second
    /// may not. [`crate::google::request::FormRequest`] already contains credential-bearing text — the
    /// rendered body — so there is nothing to separate from it, and instead the type controls its own
    /// rendering: it has a hand-written `Debug` and no `Display` at all. A transport therefore takes one value
    /// and must not print it.
    ///
    /// # Errors
    ///
    /// As [`Self::send`]: [`TransportFailure`] when the exchange produced **no** provider answer. RFC 6749
    /// §5.2 makes a refusal a well-formed body, so a `400` carrying `error` is a [`TransportResponse`].
    async fn send_form(
        &self,
        request: &crate::google::request::FormRequest,
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
