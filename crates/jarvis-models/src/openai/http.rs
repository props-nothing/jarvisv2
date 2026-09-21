//! The `reqwest` implementation of the transport port.
//!
//! # Security-Relevant Configuration
//!
//! Four decisions here are deliberate and each one corrects a default that fails
//! open for private content:
//!
//! 1. **Proxies are disabled.** `reqwest` reads `HTTP_PROXY`, `HTTPS_PROXY`, and
//!    `ALL_PROXY` from the environment by default. A prompt bound for a local model
//!    server could therefore be routed through an unrelated proxy, and a remote
//!    prompt would traverse an intermediary the user never chose.
//! 2. **Redirects are not followed.** A redirect can move an `Authorization` header
//!    to a different origin.
//! 3. **Certificate verification stays on.** No insecure TLS path is offered.
//! 4. **A read timeout is set.** Without it, a stalled response body hangs a run
//!    indefinitely, because a request timeout alone does not bound each read.
//!
//! `reqwest` 0.13 defaults to the `rustls` backend, so no system OpenSSL is
//! required. That matches the project's existing choice to bundle SQLite rather
//! than depend on a system library.

use std::time::Duration;

use futures_util::StreamExt;

use super::transport::{
    ResponseBody, Transport, TransportError, TransportRequest, TransportResponse,
};

/// Default time allowed to establish a connection.
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Default time allowed for a complete non-streaming request.
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// Default time allowed between streamed bytes.
///
/// Bounds a stalled stream. A model that legitimately pauses before its first token
/// can exceed a short value, so this is generous relative to the interval between
/// tokens once generation begins.
pub const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(90);

/// The `reqwest`-backed transport.
#[derive(Clone, Debug)]
pub struct HttpTransport {
    client: reqwest::Client,
}

impl HttpTransport {
    /// Builds a transport with the documented timeouts and hardened defaults.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError::Connect`] when the client cannot be constructed,
    /// for example when the TLS backend cannot initialize.
    pub fn new() -> Result<Self, TransportError> {
        Self::with_timeouts(
            DEFAULT_CONNECT_TIMEOUT,
            DEFAULT_REQUEST_TIMEOUT,
            DEFAULT_READ_TIMEOUT,
        )
    }

    /// Builds a transport with explicit timeouts.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError::Connect`] when the client cannot be constructed.
    pub fn with_timeouts(
        connect: Duration,
        request: Duration,
        read: Duration,
    ) -> Result<Self, TransportError> {
        let client = reqwest::Client::builder()
            .connect_timeout(connect)
            .timeout(request)
            .read_timeout(read)
            // A redirect could carry the bearer credential to another origin.
            .redirect(reqwest::redirect::Policy::none())
            // Inherited from the environment by default, which would expose prompts
            // and local-model traffic to an unchosen intermediary.
            .no_proxy()
            .build()
            .map_err(|_| TransportError::Connect)?;
        Ok(Self { client })
    }
}

/// The open body of a streaming response.
struct ReqwestBody {
    stream: futures_util::stream::BoxStream<'static, Result<bytes::Bytes, reqwest::Error>>,
}

impl ResponseBody for ReqwestBody {
    fn next_chunk(&mut self) -> super::transport::ChunkFuture<'_> {
        Box::pin(async move {
            match self.stream.next().await {
                Some(Ok(bytes)) => Ok(Some(bytes.to_vec())),
                Some(Err(error)) => {
                    if error.is_timeout() {
                        Err(TransportError::StalledRead)
                    } else if error.is_body() || error.is_decode() {
                        Err(TransportError::Body)
                    } else {
                        Err(TransportError::Io)
                    }
                }
                None => Ok(None),
            }
        })
    }
}

#[async_trait::async_trait]
impl Transport for HttpTransport {
    async fn send(
        &self,
        request: &TransportRequest,
        streaming: bool,
    ) -> Result<TransportResponse, TransportError> {
        let method = match request.method() {
            "GET" => reqwest::Method::GET,
            _ => reqwest::Method::POST,
        };
        let is_post = method == reqwest::Method::POST;

        let mut builder = self.client.request(method, request.url());
        if let Some(authorization) = request.headers().authorization() {
            builder = builder.header(reqwest::header::AUTHORIZATION, authorization);
        }
        if let Some(client_request_id) = request.headers().client_request_id() {
            builder = builder.header("X-Client-Request-Id", client_request_id);
        }
        if is_post {
            builder = builder
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(request.body().to_owned());
        }

        let response = builder
            .send()
            .await
            .map_err(|error| map_reqwest_error(&error))?;
        let status = response.status().as_u16();
        let retry_after_seconds = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(super::retry::parse_retry_after);
        let provider_request_id = response
            .headers()
            .get("x-request-id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);

        if streaming {
            return Ok(TransportResponse::Streaming {
                status,
                provider_request_id,
                body: Box::new(ReqwestBody {
                    stream: response.bytes_stream().boxed(),
                }),
            });
        }

        let body = response
            .text()
            .await
            .map_err(|error| map_reqwest_error(&error))?;
        Ok(TransportResponse::Buffered {
            status,
            retry_after_seconds,
            provider_request_id,
            body,
        })
    }
}

/// Maps a `reqwest` failure onto the coarse transport categories.
///
/// The provider's URL and body are deliberately absent from the mapped error,
/// because a `reqwest::Error` renders its URL, and a URL from some providers
/// contains an API key in the path.
fn map_reqwest_error(error: &reqwest::Error) -> TransportError {
    if error.is_timeout() {
        // A timeout while reading a body is a stalled stream, which is a different
        // operational problem from a request that never completed.
        if error.is_body() || error.is_decode() {
            TransportError::StalledRead
        } else {
            TransportError::RequestTimeout
        }
    } else if error.is_connect() {
        TransportError::Connect
    } else if error.is_decode() || error.is_body() {
        TransportError::Body
    } else {
        TransportError::Io
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::openai::transport::TransportHeaders;

    #[test]
    fn the_transport_builds_without_a_system_tls_dependency() {
        // Proves the rustls backend initializes on this platform. If this fails, the
        // build needs a system OpenSSL, which the bundling policy avoids.
        assert!(HttpTransport::new().is_ok());
    }

    #[test]
    fn header_display_reports_presence_without_the_credential() {
        // The header set is Debug/Display-able and travels inside request diagnostics,
        // so it must never render the bearer value.
        let headers = TransportHeaders::new()
            .with_authorization("Bearer sk-live-secret-value".to_owned())
            .with_client_request_id("abc".to_owned());
        let rendered = format!("{headers}");
        assert!(!rendered.contains("sk-live-secret-value"));
        assert!(rendered.contains("present"));
    }

    #[test]
    fn a_get_probe_carries_no_body() {
        // A reachability probe must not perform a billable completion call.
        let request =
            TransportRequest::get("http://127.0.0.1:11434/v1/models", TransportHeaders::new());
        assert_eq!(request.method(), "GET");
        assert!(request.body().is_empty());
    }
}
