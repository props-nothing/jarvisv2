//! The daemon's `/api/v1` HTTP API, as a client.
//!
//! # Why the CLI talks HTTP for runs
//!
//! ADR-0011 makes HTTP a first-class peer transport and records local IPC as the **preferred**
//! transport for the CLI and desktop. Runs are not on the local protocol yet: protocol v1 serves
//! `status` and `health` only, and the run surface lives on `/api/v1`. Talking HTTP here therefore
//! follows the decision rather than working around it — the CLI uses "the daemon API" that
//! `ADR-0011` names, and it stays a transport client with no orchestration of its own.
//!
//! The transport is `reqwest` 0.13.5, already resolved in this workspace through the model
//! adapter, and it is configured to match the adapter's hardening exactly. Each of the four
//! decisions below corrects a default that fails open:
//!
//! 1. **Proxies are disabled.** `reqwest` reads `HTTP_PROXY`, `HTTPS_PROXY`, and `ALL_PROXY` from
//!    the environment by default, so a request carrying the profile credential could be routed
//!    through an intermediary the user never chose.
//! 2. **Redirects are not followed.** A redirect can move an `Authorization` header to a different
//!    origin, and the credential is a bearer token.
//! 3. **Certificate verification stays on.** No insecure path is offered.
//! 4. **A read timeout is set.** It is the only thing that bounds a stalled stream; a request
//!    timeout alone does not bound each read.
//!
//! # The credential is header-only and taken verbatim
//!
//! It is sent in `Authorization: Bearer` and never in a query parameter, where it would become a
//! substring of every access log the request passed through. It is not trimmed, matching the
//! daemon's own extractor: a padded value is a different byte sequence, not the credential.
//!
//! # The target is loopback by construction
//!
//! [`LoopbackHost`] cannot express another address, so a client cannot be pointed at a host the
//! daemon does not serve. See that type for why the rule is a shape rather than a check.

use std::fmt;
use std::time::Duration;

use jarvis_core::LoopbackHost;
use jarvis_protocol::{
    JSON_BODY_CONTENT_TYPE, RunPathError, RunReply, RunStreamDecoder, RunStreamFrame, SSE_ACCEPT,
    StartRunRequest, WireError, run_path, run_stream_path, runs_path,
};

/// Time allowed to establish a connection to a loopback daemon.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Time allowed for a complete non-streaming request.
///
/// Applied **per request** rather than on the client. A client-level `reqwest` timeout bounds the
/// whole response body, so on a client that also serves an open-ended SSE stream it would terminate
/// a healthy stream at the deadline — which is exactly what the first live run of this client did.
/// `STREAM_READ_TIMEOUT` bounds a dead stream instead, and it resets on every read.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Time allowed between streamed bytes.
///
/// Generous, because the daemon holds the connection open for as long as the run is active and poll
/// interval is sub-second. It exists so a wedged daemon fails the client rather than parking it
/// forever, not to bound a healthy idle stream. This is the **only** thing that bounds a stalled
/// stream: a total timeout cannot, because a stream has no defined total length.
pub const STREAM_READ_TIMEOUT: Duration = Duration::from_secs(120);

/// Why a request to the daemon failed.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    /// The daemon could not be reached, or the transport failed.
    ///
    /// Carries a classification rather than a bare message: "could not be reached" alone does not
    /// say whether the daemon is absent, the connection was refused, or a read stalled, and those
    /// have different fixes. The raw `reqwest` error is deliberately not kept, because it can
    /// contain the URL and platform error text.
    #[error("the daemon API could not be reached: {0}")]
    Transport(TransportFailure),
    /// The daemon answered with its stable error envelope.
    #[error("the daemon refused the request: {0}")]
    Refused(Box<WireError>),
    /// The daemon answered with a status but no readable envelope.
    ///
    /// Distinct from [`Self::Refused`] on purpose: a `500` whose body is not an envelope is a
    /// different diagnosis from a `500` that explains itself, and collapsing them would report
    /// "the daemon refused" for a response the daemon never intended.
    #[error("the daemon answered with status {status} and no readable error envelope")]
    UnexpectedStatus {
        /// The HTTP status the daemon returned.
        status: u16,
    },
    /// The response body could not be decoded as the expected shape.
    #[error("the daemon reply was not the shape this client understands")]
    Decode,
    /// A run identifier in a reply could not be used to build a request path.
    #[error(transparent)]
    Identifier(#[from] RunPathError),
}

/// A classified transport failure.
///
/// The categories mirror the model adapter's transport classification, so one vocabulary describes
/// a failed daemon call and a failed provider call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransportFailure {
    /// The connection could not be established.
    Connect,
    /// The request or a read exceeded its deadline.
    Timeout,
    /// The response body or its encoding was unreadable.
    Body,
    /// The connection failed after it was established.
    Io,
}

impl fmt::Display for TransportFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Connect => "the connection was refused or could not be established",
            Self::Timeout => "the request or stream read timed out",
            Self::Body => "the response body could not be read",
            Self::Io => "the connection failed",
        })
    }
}

/// Classifies a `reqwest` error without keeping its text.
fn classify(error: &reqwest::Error) -> TransportFailure {
    if error.is_timeout() {
        TransportFailure::Timeout
    } else if error.is_connect() {
        TransportFailure::Connect
    } else if error.is_body() || error.is_decode() {
        TransportFailure::Body
    } else {
        TransportFailure::Io
    }
}

/// The authenticated client for the daemon's HTTP API.
#[derive(Clone, Debug)]
pub struct ApiClient {
    host: LoopbackHost,
    credential: String,
    client: reqwest::Client,
}

impl ApiClient {
    /// Builds a client for a loopback endpoint and credential.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError::Transport`] when the HTTP client cannot be constructed, for example when
    /// the TLS backend cannot initialize. This is reported rather than ignored because every later
    /// request would fail with a less specific error.
    pub fn new(host: LoopbackHost, credential: String) -> Result<Self, ApiError> {
        let client = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            // Deliberately NO `.timeout(..)` here: it bounds the whole response body, which on this
            // client would terminate every SSE stream at the deadline. `read_timeout` bounds a
            // stalled stream instead, and non-streaming calls set their own total timeout.
            .read_timeout(STREAM_READ_TIMEOUT)
            // Inherited from the environment by default, which would expose the credential and the
            // objective to an intermediary the user did not select.
            .no_proxy()
            // A redirect could move the bearer credential to another origin.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| ApiError::Transport(classify(&error)))?;
        Ok(Self {
            host,
            credential,
            client,
        })
    }

    /// Returns the endpoint this client targets.
    #[must_use]
    pub const fn host(&self) -> LoopbackHost {
        self.host
    }

    /// Starts a run, continuing an existing conversation when one is named.
    ///
    /// A second turn is a **new run in the same session**, not a mutation of the first: the daemon
    /// replays the session's transcript into the model call, so a run is the unit of work and the
    /// session is the unit of continuity. That is why this takes a session identifier rather than
    /// a run identifier.
    ///
    /// `session_id` of `None` begins a new conversation, which is the ordinary single-turn case. There
    /// is deliberately no separate "start a fresh run" method: a second wrapper would be one more
    /// place for the session argument to be dropped by accident.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError`] when the objective is not storable, the named session is absent or
    /// closed to new work, the daemon refuses the request, or the transport fails.
    pub async fn start_run_in_session(
        &self,
        objective: &str,
        session_id: Option<&str>,
    ) -> Result<RunReply, ApiError> {
        let body = StartRunRequest {
            objective: objective.to_owned(),
            session_id: session_id.map(ToOwned::to_owned),
            // Omitted rather than sent as null. The daemon refuses a present idempotency key
            // because no deduplication ledger exists, and sending one would be claiming a
            // guarantee this build does not provide.
            idempotency_key: None,
        };

        let response = self
            .bounded_request(reqwest::Method::POST, &runs_path())
            .json(&body)
            .send()
            .await
            .map_err(|error| ApiError::Transport(classify(&error)))?;
        let status = response.status();
        if status.is_success() {
            return response
                .json::<RunReply>()
                .await
                .map_err(|_| ApiError::Decode);
        }
        Err(self.refusal(response).await)
    }

    /// Reads one run.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError`] when the identifier is unusable, the run is unknown, or the transport
    /// fails.
    pub async fn read_run(&self, run_id: &str) -> Result<RunReply, ApiError> {
        let path = run_path(run_id)?;
        let response = self
            .bounded_request(reqwest::Method::GET, &path)
            .send()
            .await
            .map_err(|error| ApiError::Transport(classify(&error)))?;
        let status = response.status();
        if status.is_success() {
            return response
                .json::<RunReply>()
                .await
                .map_err(|_| ApiError::Decode);
        }
        Err(self.refusal(response).await)
    }

    /// Opens a run's event stream.
    ///
    /// Returns a stream handle rather than a collected page, because a run is consumed as it
    /// happens: buffering would defeat the point of a stream and would report nothing until the run
    /// settled.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError`] when the identifier is unusable, the daemon refuses to open the stream
    /// (an unknown run is `404`, a cursor past the end is `409`), or the transport fails. The
    /// refusal is resolved *before* the stream is returned, so an unknown run is an error here
    /// rather than a stream that opens and immediately closes.
    pub async fn open_stream(&self, run_id: &str) -> Result<RunEventStream, ApiError> {
        let path = run_stream_path(run_id)?;
        let response = self
            .request(reqwest::Method::GET, &path)
            .header(reqwest::header::ACCEPT, SSE_ACCEPT)
            .send()
            .await
            .map_err(|error| ApiError::Transport(classify(&error)))?;
        let status = response.status();
        if !status.is_success() {
            return Err(self.refusal(response).await);
        }
        Ok(RunEventStream {
            response,
            decoder: RunStreamDecoder::new(),
        })
    }

    /// Builds an authenticated request.
    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.client
            .request(method, self.host.url(path))
            .header(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {}", self.credential),
            )
            .header(reqwest::header::CONTENT_TYPE, JSON_BODY_CONTENT_TYPE)
    }

    /// Builds an authenticated request that must complete within [`REQUEST_TIMEOUT`].
    ///
    /// Used for every non-streaming call, so a wedged daemon fails the command rather than hanging
    /// it. A streaming request deliberately does not use this.
    fn bounded_request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.request(method, path).timeout(REQUEST_TIMEOUT)
    }

    /// Reads a refusal into the shared error envelope.
    ///
    /// A body that is not the envelope is reported as [`ApiError::UnexpectedStatus`] rather than
    /// fabricated into a `WireError`: the daemon's contract is that every failure carries the
    /// envelope, so a failure to parse one is a fact worth reporting on its own.
    async fn refusal(&self, response: reqwest::Response) -> ApiError {
        let status = response.status().as_u16();
        match response.json::<WireError>().await {
            Ok(error) => ApiError::Refused(Box::new(error)),
            Err(_) => ApiError::UnexpectedStatus { status },
        }
    }
}

/// An open run event stream.
///
/// Yields decoded frames one at a time. A caller stops at a terminal reading, which the daemon
/// guarantees is the last thing it sends, because its append path refuses to write after the run
/// settled.
pub struct RunEventStream {
    response: reqwest::Response,
    decoder: RunStreamDecoder,
}

impl RunEventStream {
    /// Returns the next frame, or `None` when the daemon closed the stream cleanly.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError::Transport`] when the connection fails, and [`ApiError::Decode`] when a
    /// frame cannot be interpreted. Both are errors rather than `None`: `None` means the daemon
    /// finished the stream, and reporting a transport failure that way would make a dropped
    /// connection look like a completed run.
    pub async fn next_frame(&mut self) -> Result<Option<RunStreamFrame>, ApiError> {
        loop {
            if let Some(frame) = self.decoder.next_frame() {
                return frame.map(Some).map_err(|_| ApiError::Decode);
            }
            match self.response.chunk().await {
                Ok(Some(chunk)) => self.decoder.feed(&chunk),
                Ok(None) => {
                    // A frame left in the buffer at close is malformed rather than absent: the
                    // daemon ends every event with a separator, so a partial frame means the
                    // connection was severed mid-event and reporting it as a clean end would
                    // present a truncated run as a finished one.
                    return match self.decoder.next_frame() {
                        None => Ok(None),
                        Some(_) => Err(ApiError::Decode),
                    };
                }
                Err(error) => return Err(ApiError::Transport(classify(&error))),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jarvis_core::LOOPBACK_ADDRESS;

    fn client() -> ApiClient {
        let host = LoopbackHost::new(8765)
            .unwrap_or_else(|error| panic!("8765 must be a usable port: {error}"));
        ApiClient::new(host, "not-a-real-secret".to_owned())
            .unwrap_or_else(|error| panic!("a client over loopback must build: {error}"))
    }

    /// Every request this client can build targets loopback. This is the property the credential's
    /// safety depends on, so it is asserted rather than assumed.
    #[test]
    fn every_request_targets_loopback_and_names_the_bearer_header() {
        let client = client();
        let id = "018f0000-0000-7000-8000-000000000000";
        let run = run_path(id).unwrap_or_else(|error| panic!("a uuid is a safe segment: {error}"));
        for path in [runs_path(), run] {
            let request = client.request(reqwest::Method::GET, &path);
            let built = request
                .build()
                .unwrap_or_else(|error| panic!("a loopback request must build: {error}"));
            assert_eq!(built.url().host_str(), Some(LOOPBACK_ADDRESS));
            assert_eq!(built.url().path(), path);
            assert_eq!(
                built
                    .headers()
                    .get(reqwest::header::AUTHORIZATION)
                    .map(|value| value.to_str().ok()),
                Some(Some("Bearer not-a-real-secret")),
                "the credential is presented only as a bearer header"
            );
            assert!(
                built.url().query().is_none(),
                "no request may carry a credential in the query string"
            );
        }
    }

    /// A run identifier that could change the request's target is refused before any request is
    /// built, so a hostile or broken reply cannot redirect a request.
    #[tokio::test]
    async fn an_unsafe_identifier_is_refused_before_a_request_exists() {
        let client = client();
        for unsafe_id in ["", "a/b", "a?from=0", "../../admin"] {
            let Err(error) = client.read_run(unsafe_id).await else {
                panic!("{unsafe_id:?} must be refused rather than requested");
            };
            assert!(
                matches!(error, ApiError::Identifier(_)),
                "expected a path error for {unsafe_id:?}, got {error:?}"
            );
        }
    }
}
