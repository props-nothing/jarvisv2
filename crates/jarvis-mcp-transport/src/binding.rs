//! Binding the MCP endpoint: the tower service a daemon mounts, and the two decisions every request passes.
//!
//! # What this closes
//!
//! `P3-009i` built [`RequestGate`], which applies the `Origin` policy and the caller policy to one request in a
//! stated order. Every slice up to it recorded the same limit — **nothing binds**, so no request reached the
//! gate. This module is the layer a network listener mounts: it turns a real [`http::Request`] into the four
//! values the gate takes, calls it, and answers the status the refusal obliges.
//!
//! # Why this is a tower service rather than a route handler
//!
//! `repository-layout.md` says the daemon owns the network listener and that handlers translate protocols into
//! application commands. A route handler would work, but a `tower` service composes the SDK's own service with
//! no intermediate representation: the SDK's service **is** a `Service<Request<ReqBody>>`, so wrapping it is one
//! `impl Service` rather than a second routing layer and a second body type. It also means this layer is proven
//! by driving a real request through it, which is the only evidence that the values were derived from a request
//! rather than handed to it.
//!
//! # The SDK type appears in the private field, never in a signature
//!
//! This crate's invariant is that **no provider SDK type appears in its public surface**
//! (`repository-layout.md`, from `AGENTS.md`), and the SDK's service is
//! `StreamableHttpService<JarvisMcpServer, NeverSessionManager>`. So the field type is a private alias, every
//! public signature names `http`'s `Request`/`Response`, and [`ServedEndpoint::into_service`] returns
//! `impl Service<...>` — an opaque type a caller can mount on a router without this crate naming it.
//!
//! That is not a loophole: `impl Trait` in return position keeps a type out of the crate's *contract* while
//! letting a caller use it, which is exactly the distinction `P3-009f`'s boundary test draws. The wrapped
//! service is **private**, so it is not merely unnamed but unnameable. A `pub fn` returning
//! `StreamableHttpService` would fail that test; this does not, and the test is extended to the new signatures.
//!
//! # The order is the gate's, and this layer does not restate it
//!
//! The origin is derived, handed to the gate, and the gate decides. This layer contains **no** `if origin ...`
//! beyond extracting the header, because a second place the order lives is a second place it can be wrong — the
//! defect class `P3-009f` spent a slice on.

use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;

use bytes::Bytes;
use http::{Request, Response, StatusCode};
use http_body::Body;
use http_body_util::combinators::BoxBody;
use rmcp::transport::streamable_http_server::session::never::NeverSessionManager;
use rmcp::transport::streamable_http_server::tower::StreamableHttpService;
use tower::Service;

use crate::admission::{CallerAdmission, CallerOrigin, Fingerprint};
use crate::enforcement::{RequestAdmission, RequestGate, RequestRefusal};
use crate::serve::JarvisMcpServer;
use crate::serving::{MCP_ENDPOINT_PATH, ServiceError, ServingConfig};

/// The SDK's Streamable HTTP service, named **privately**.
///
/// A private alias is the only place the SDK's service type is written down. A public one would put the SDK in
/// this crate's contract, and `boundary_tests.rs` checks that no public signature names it.
type SdkService = StreamableHttpService<JarvisMcpServer, NeverSessionManager>;

/// The body this layer produces.
///
/// `BoxBody<Bytes, Infallible>` is the SDK's own response-body type — its private `BoxResponse` alias is
/// `Response<BoxBody<Bytes, Infallible>>`. Naming it is a fact about `http-body-util` rather than about the MCP
/// SDK: `BoxBody` is a general-purpose type-erased body, so no protocol's vocabulary enters this crate's
/// contract, and the type must be exact because the SDK's response is passed through unchanged when admitted.
pub type ResponseBody = BoxBody<Bytes, Infallible>;

/// The future this layer returns from one request.
///
/// **Named and public because `Send` has to be in the contract, not just in the type.** An `impl Service`
/// return type exposes only its declared bounds, and the future is an *associated* type — so an opaque
/// `impl Service<…>` says nothing about whether `Service::Future` is `Send`. `axum::Router::fallback_service`
/// requires `T::Future: Send + 'static`, so mounting this without naming the future fails at the **mount
/// site**, where the error names the caller's generic parameter rather than anything in this crate.
///
/// Pinning the associated type to this alias makes the whole contract checkable here: the boxing, the `Send`,
/// and the `'static` lifetime are statements this crate makes, and a future change that dropped one fails to
/// compile in this file rather than in a daemon.
pub type ResponseFuture =
    Pin<Box<dyn Future<Output = Result<Response<ResponseBody>, Infallible>> + Send + 'static>>;

/// The header an MCP client presents its credential in.
///
/// `Authorization`, and the value is expected to be a **digest** rather than the credential itself — see
/// [`Fingerprint`], which admits only the digest alphabet so a pasted token fails locally. The header is read,
/// never authenticated: audience binding (RFC 8707) is the OAuth slice, so what this layer establishes is *which
/// configured fingerprint is speaking*, not that the speaker is entitled to it.
pub const CREDENTIAL_HEADER: &str = "authorization";

/// The scheme an `Authorization` value must carry to be considered a credential.
///
/// Required rather than optional because a bare digest in `Authorization` is not valid HTTP, and accepting it
/// would admit a value no real client sends — the same reasoning `Fingerprint::parse` uses to refuse a pasted
/// URL. The comparison is case-insensitive because HTTP auth schemes are, so a client sending `bearer` is not
/// refused for spelling.
pub const BEARER_SCHEME: &str = "Bearer";

/// The most **characters** copied out of an `Origin` header before the rest is discarded.
///
/// The policy's own bound (`jarvis_mcp::MAX_ORIGIN_ENTRY_BYTES` = 512) means a longer header cannot match any
/// configured entry, so truncating cannot admit anything the policy would refuse. It can only turn an
/// unparseable value into a shorter unparseable value, which the policy refuses either way. The bound is on
/// characters rather than bytes because truncating a UTF-8 string by bytes needs a char-boundary check, and
/// `char`-based truncation is total.
pub const MAX_ORIGIN_HEADER_CHARS: usize = 512;

/// The JSON-RPC code a policy refusal carries.
///
/// `-32000` is the range JSON-RPC reserves for implementation-defined server errors, which a policy refusal is.
/// The revision's error table reserves specific codes for protocol conditions (an unsupported version, a header
/// mismatch), and reusing one would tell a caller to look for a protocol problem that is not there.
pub const REFUSAL_CODE: i32 = -32000;

/// A served MCP endpoint: the SDK's service behind the two inbound policies.
///
/// Built from a [`ServingConfig`] and a [`CallerAdmission`]. Holding the [`RequestGate`] rather than the two
/// policies separately is deliberate — the gate is the thing that applies both, and a struct holding the parts
/// would be a place a caller could reach past it.
pub struct ServedEndpoint {
    inner: SdkService,
    gate: RequestGate,
}

impl std::fmt::Debug for ServedEndpoint {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Hand-written rather than derived, because the SDK's service holds a session manager and a tool-schema
        // cache: a derived `Debug` would print internals of a dependency this crate does not own, and the gate is
        // the part an operator needs to see.
        formatter
            .debug_struct("ServedEndpoint")
            .field("gate", &self.gate)
            .field("path", &MCP_ENDPOINT_PATH)
            .finish_non_exhaustive()
    }
}

impl ServedEndpoint {
    /// Builds an endpoint from a serving configuration, a caller policy, and a handler.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::NoToolsAdvertised`] when the handler serves no tools, through
    /// [`ServingConfig::check_servable`]. The check is **not** repeated here: one place a daemon consults is what
    /// makes the refusal reliable, and a second copy is a second thing to keep in step.
    pub fn new(
        serving: ServingConfig,
        admission: CallerAdmission,
        server: JarvisMcpServer,
    ) -> Result<Self, ServiceError> {
        ServingConfig::check_servable(&server)?;
        let inner = serving.service(server);
        Ok(Self {
            inner,
            gate: RequestGate::new(serving, admission),
        })
    }

    /// Returns the gate in force.
    ///
    /// Exposed so a daemon can report the policy it serves under without this crate publishing the SDK service. A
    /// diagnostic that could not state the policy would leave an operator comparing configuration files against a
    /// running process by hand.
    #[must_use]
    pub const fn gate(&self) -> &RequestGate {
        &self.gate
    }

    /// Returns the service to mount on a router.
    ///
    /// `impl Service` rather than the concrete type, so the SDK's service stays out of this crate's public
    /// surface while a caller can still mount it. `axum::Router::fallback_service` accepts any `Service`, and
    /// **naming** the type is the only thing an SDK-free contract forbids.
    ///
    /// # Why the returned type is `Clone + Send + Sync`
    ///
    /// `axum::Router::fallback_service` requires all three, and `P3-009c-b` mounts this on a real router — so
    /// they are stated as bounds rather than left to be discovered at the mount site, where the error names the
    /// **caller's** generic parameter and not this function. `CheckingService` is `Clone` because the SDK's
    /// service is, `Send`/`Sync` because every field is, and the future is `Send` because the SDK's `Service`
    /// impl returns `BoxFuture<'static, …> + Send`.
    #[must_use]
    pub fn into_service<B>(
        self,
    ) -> impl Service<
        Request<B>,
        Response = Response<ResponseBody>,
        Error = Infallible,
        // The associated type is pinned rather than left opaque, so the `Send` bound is part of this
        // crate's contract and a change that dropped it fails here. See [`ResponseFuture`].
        Future = ResponseFuture,
    > + Clone
    + Send
    + Sync
    + 'static
    where
        B: Body + Send + 'static,
        B::Data: Send + 'static,
        B::Error: std::fmt::Display,
    {
        CheckingService {
            inner: self.inner,
            gate: self.gate,
            marker: std::marker::PhantomData,
        }
    }
}

/// The gate, wrapped around the SDK's service.
///
/// Private, so it is not merely unnamed in a signature but unnameable: the only way to obtain one is
/// [`ServedEndpoint::into_service`].
///
/// The wrapping is a `Service` rather than a function so it cannot be mounted *beside* the SDK's service by
/// mistake. A router that mounted the SDK's service directly would have no policy at all and nothing would
/// notice — which is why the SDK's service is private to this crate and this is the only door to it.
struct CheckingService<B> {
    inner: SdkService,
    gate: RequestGate,
    /// `fn(B)` rather than `B`, so the marker is `Send`/`Sync` regardless of the body type and adds no drop glue.
    marker: std::marker::PhantomData<fn(B)>,
}

impl<B> CheckingService<B> {
    /// Decides one request's `Origin` and caller, or reports which policy refused it.
    ///
    /// Reads no body, so a decision is a function of headers alone. Returning [`RequestAdmission`] rather than a
    /// `bool` is the guarantee `P3-009i` built: a caller that holds one cannot have skipped a check.
    fn decide<B2>(&self, request: &Request<B2>) -> Result<RequestAdmission, RequestRefusal> {
        // An `Origin` is copied out bounded rather than unbounded, so a hostile request cannot make this layer
        // allocate an arbitrarily long string. Truncation cannot admit anything: a value longer than the policy's
        // own 512-byte entry bound could not match a configured origin, and a truncated unparseable value is
        // still unparseable — both refused by the policy's parse.
        let origin = request
            .headers()
            .get(http::header::ORIGIN)
            .and_then(|value| value.to_str().ok())
            .map(|value| {
                value
                    .chars()
                    .take(MAX_ORIGIN_HEADER_CHARS)
                    .collect::<String>()
            });

        let presented = request
            .headers()
            .get(CREDENTIAL_HEADER)
            .and_then(|value| value.to_str().ok())
            .and_then(strip_bearer)
            .and_then(|digest| Fingerprint::parse(digest).ok());

        // **Always remote, because a request that arrived over a network listener is remote.**
        //
        // `CallerOrigin::Local` is the operator on their own machine, and what establishes it is the daemon's
        // **transport** — a named pipe, or a socket it opened itself and never exposed. A loopback *bind* is not
        // that evidence: any local process, and a browser on the same machine, can reach `127.0.0.1`. Deriving
        // `Local` from a peer address would be `CallerOrigin`'s own defect one layer down — a self-reported
        // property deciding admission — so this layer never claims it, and a daemon that wants a local caller
        // admitted does so through the **allowlist**.
        let caller = CallerOrigin::Remote;

        self.gate
            .decide(origin.as_deref(), caller, presented.as_ref(), false)
    }
}

impl<B> Clone for CheckingService<B> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            gate: self.gate.clone(),
            marker: std::marker::PhantomData,
        }
    }
}

/// Strips the bearer scheme from an `Authorization` value, returning the credential.
///
/// Case-insensitive in the scheme, because HTTP's auth schemes are, and `split_once` rather than a prefix strip
/// so `Bearer` with no value yields `None` instead of an empty credential — an empty credential must be an
/// absent one, not a fingerprint that could match a configured entry.
fn strip_bearer(value: &str) -> Option<&str> {
    let (scheme, credential) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case(BEARER_SCHEME) {
        return None;
    }
    let credential = credential.trim();
    (!credential.is_empty()).then_some(credential)
}

impl<B> Service<Request<B>> for CheckingService<B>
where
    B: Body + Send + 'static,
    B::Data: Send + 'static,
    B::Error: std::fmt::Display,
{
    type Response = Response<ResponseBody>;
    type Error = Infallible;
    type Future = ResponseFuture;

    fn poll_ready(
        &mut self,
        _context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: Request<B>) -> Self::Future {
        match self.decide(&request) {
            Ok(admitted) => {
                // The admission is **recorded on the span** rather than dropped, so the decision is observable
                // in a log without a field the compiler would elide. What it does *not* do is attribute the call:
                // an `run_events` row needs a correlation id an MCP request does not carry (`P3-012`), and
                // inventing one here would put a fabricated identity in the audit trail.
                tracing::debug!(
                    caller = ?admitted.caller(),
                    origin = ?admitted.origin(),
                    "admitted an MCP request"
                );
                let mut inner = self.inner.clone();
                Box::pin(async move { inner.call(request).await })
            }
            Err(refusal) => {
                let status =
                    StatusCode::from_u16(refusal.refusal_status()).unwrap_or(StatusCode::FORBIDDEN);
                tracing::debug!(
                    status = status.as_u16(),
                    reason = %refusal.reason(),
                    "refused an MCP request before it reached the handler"
                );
                Box::pin(async move { Ok(refusal_response(status, &refusal)) })
            }
        }
    }
}

/// Builds the refusal response.
///
/// A short JSON body rather than an empty one, because a bare `403` tells a caller nothing it did not already
/// know, and because the revision describes refusals in JSON-RPC terms. The body states the **policy class
/// only**, never the verdict: telling a hostile caller whether the origin was unlisted or malformed would help
/// them enumerate the allowlist, which is why [`RequestRefusal::reason`] is for the log and this is the wire.
///
/// Built without the fallible builder — status, one static header, and a body — so no refusal path can panic or
/// return a malformed response while answering a request.
fn refusal_response(status: StatusCode, refusal: &RequestRefusal) -> Response<ResponseBody> {
    let message = match refusal {
        RequestRefusal::Origin { .. } => "the request did not satisfy the server's origin policy",
        RequestRefusal::Admission { .. } => {
            "the request did not satisfy the server's caller policy"
        }
    };
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "error": { "code": REFUSAL_CODE, "message": message },
        "id": serde_json::Value::Null,
    })
    .to_string();

    let mut response = Response::new(BoxBody::new(http_body_util::Full::new(Bytes::from(body))));
    *response.status_mut() = status;
    response.headers_mut().insert(
        http::header::CONTENT_TYPE,
        http::HeaderValue::from_static("application/json"),
    );
    response
}

#[cfg(test)]
#[path = "binding_tests.rs"]
mod tests;
