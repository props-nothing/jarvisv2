use std::{fmt, future::Future, pin::Pin};

use thiserror::Error;

/// Explains why a transport operation failed.
///
/// The variants are deliberately coarse and contain no provider text. A transport
/// is a byte pipe: it reports *how* it failed and lets the adapter decide what that
/// means, so a TLS error and a DNS error cannot be confused with a provider
/// rejection that happens to share an HTTP status.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum TransportError {
    /// The connection could not be established within the deadline.
    Connect,
    /// The whole request exceeded its deadline.
    RequestTimeout,
    /// No bytes arrived for longer than the read deadline.
    ///
    /// Distinct from [`Self::RequestTimeout`] because a stalled stream is the
    /// failure that otherwise hangs a run forever.
    StalledRead,
    /// The connection failed or was closed mid-transfer.
    Io,
    /// TLS negotiation or certificate verification failed.
    Tls,
    /// The request was cancelled before it completed.
    Cancelled,
    /// The response body was not valid UTF-8 or could not be read to the end.
    Body,
}

impl TransportError {
    /// Returns whether the operation never reached the provider.
    ///
    /// A request that was never sent is safe to retry without considering provider
    /// effects; a failure after transmission is not.
    #[must_use]
    pub const fn is_before_send(self) -> bool {
        matches!(self, Self::Connect)
    }
}

impl fmt::Display for TransportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::Connect => "transport could not connect",
            Self::RequestTimeout => "transport request timed out",
            Self::StalledRead => "transport read stalled",
            Self::Io => "transport connection failed",
            Self::Tls => "transport TLS verification failed",
            Self::Cancelled => "transport request was cancelled",
            Self::Body => "transport response body was unreadable",
        };
        formatter.write_str(text)
    }
}

/// Headers already validated as safe to transmit.
///
/// Only the header names this adapter uses are representable, so a caller cannot
/// inject an arbitrary header name carrying a secret into a log line.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TransportHeaders {
    authorization: Option<String>,
    client_request_id: Option<String>,
}

impl TransportHeaders {
    /// Creates an empty header set.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            authorization: None,
            client_request_id: None,
        }
    }

    /// Sets the bearer credential used for provider authentication.
    #[must_use]
    pub fn with_authorization(mut self, value: String) -> Self {
        self.authorization = Some(value);
        self
    }

    /// Sets the client-supplied correlation identifier.
    #[must_use]
    pub fn with_client_request_id(mut self, value: String) -> Self {
        self.client_request_id = Some(value);
        self
    }

    /// Returns the bearer credential, for the transport's use only.
    #[must_use]
    pub fn authorization(&self) -> Option<&str> {
        self.authorization.as_deref()
    }

    /// Returns the client-supplied correlation identifier.
    #[must_use]
    pub fn client_request_id(&self) -> Option<&str> {
        self.client_request_id.as_deref()
    }
}

impl fmt::Display for TransportHeaders {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The authorization value is a secret; report only its presence.
        formatter.write_str("TransportHeaders { ")?;
        write!(
            formatter,
            "authorization: {}, ",
            if self.authorization.is_some() {
                "present"
            } else {
                "absent"
            }
        )?;
        write!(
            formatter,
            "client_request_id: {} }}",
            if self.client_request_id.is_some() {
                "present"
            } else {
                "absent"
            }
        )
    }
}

/// One HTTP request for a transport to perform.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransportRequest {
    method: &'static str,
    url: String,
    body: String,
    headers: TransportHeaders,
}

impl TransportRequest {
    /// Creates a `POST` request, the method every chat completion call uses.
    #[must_use]
    pub fn new(url: impl Into<String>, body: impl Into<String>, headers: TransportHeaders) -> Self {
        Self {
            method: "POST",
            url: url.into(),
            body: body.into(),
            headers,
        }
    }

    /// Creates a bodyless `GET` request, used only for reachability probes.
    ///
    /// A probe must not perform a completion call, because that would bill the
    /// account for a health check.
    #[must_use]
    pub fn get(url: impl Into<String>, headers: TransportHeaders) -> Self {
        Self {
            method: "GET",
            url: url.into(),
            body: String::new(),
            headers,
        }
    }

    /// Returns the HTTP method.
    #[must_use]
    pub const fn method(&self) -> &'static str {
        self.method
    }

    /// Returns the absolute request URL.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Returns the serialized JSON body.
    #[must_use]
    pub fn body(&self) -> &str {
        &self.body
    }

    /// Returns the request headers.
    #[must_use]
    pub const fn headers(&self) -> &TransportHeaders {
        &self.headers
    }
}

impl fmt::Display for TransportRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The body contains the user's prompt, which may be private, and the URL may
        // carry a key in its path for some providers. Report only the shape.
        write!(
            formatter,
            "TransportRequest {{ body_bytes: {}, {} }}",
            self.body.len(),
            self.headers
        )
    }
}

/// A response head: status, diagnostic headers, and an unread body.
///
/// The body is a separate call so the streaming path can consume it incrementally
/// while the non-streaming path can read it whole.
pub enum TransportResponse {
    /// A non-streaming response whose body is fully read.
    Buffered {
        /// The HTTP status code.
        status: u16,
        /// The parsed `Retry-After` delay in seconds, if the provider sent one.
        retry_after_seconds: Option<u64>,
        /// The provider-assigned request identifier, if present.
        provider_request_id: Option<String>,
        /// The response body as text.
        body: String,
    },
    /// A streaming response whose body is still open.
    Streaming {
        /// The HTTP status code.
        status: u16,
        /// The provider-assigned request identifier, if present.
        provider_request_id: Option<String>,
        /// The open body.
        body: Box<dyn ResponseBody>,
    },
}

impl fmt::Debug for TransportResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Buffered { status, .. } => write!(
                formatter,
                "TransportResponse::Buffered {{ status: {status} }}"
            ),
            Self::Streaming { status, .. } => write!(
                formatter,
                "TransportResponse::Streaming {{ status: {status} }}"
            ),
        }
    }
}

impl TransportResponse {
    /// Returns the HTTP status code.
    #[must_use]
    pub const fn status(&self) -> u16 {
        match self {
            Self::Buffered { status, .. } | Self::Streaming { status, .. } => *status,
        }
    }

    /// Returns the provider-assigned request identifier, when captured.
    #[must_use]
    pub fn provider_request_id(&self) -> Option<&str> {
        match self {
            Self::Buffered {
                provider_request_id,
                ..
            }
            | Self::Streaming {
                provider_request_id,
                ..
            } => provider_request_id.as_deref(),
        }
    }
}

/// An open response body read incrementally as byte chunks.
///
/// The boxed future returned by [`ResponseBody::next_chunk`].
///
/// Named so the trait signature stays readable, and boxed because the trait is used
/// as `dyn ResponseBody`, which an `impl Future` return type does not permit.
pub type ChunkFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Option<Vec<u8>>, TransportError>> + Send + 'a>>;

/// An open response body read incrementally as byte chunks.
pub trait ResponseBody: Send + Unpin {
    /// Returns the next chunk, or `None` at end of body.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError::StalledRead`] when no bytes arrive within the read
    /// deadline, or another [`TransportError`] when the transfer fails.
    fn next_chunk(&mut self) -> ChunkFuture<'_>;
}

/// Performs HTTP requests on behalf of the adapter.
///
/// Implementations must not follow redirects: a redirect can move a bearer
/// credential to a different origin.
#[async_trait::async_trait]
pub trait Transport: Send + Sync {
    /// Performs a request and returns the response head.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] when the request could not be completed. A
    /// non-2xx status is **not** an error here; it is a response the adapter maps.
    async fn send(
        &self,
        request: &TransportRequest,
        streaming: bool,
    ) -> Result<TransportResponse, TransportError>;
}
