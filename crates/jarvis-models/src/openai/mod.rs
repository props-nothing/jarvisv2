//! The OpenAI-compatible Chat Completions adapter.
//!
//! The adapter is split so that retry, timeout, error mapping, and streaming
//! behaviour can be tested without a network:
//!
//! - [`wire`] holds the private provider JSON shapes.
//! - [`transport`] is the byte-level port; [`HttpTransport`] is the `reqwest`
//!   implementation.
//! - [`adapter`] is the state machine that turns JARVIS requests into provider
//!   requests and back.
//! - [`sse`] decodes the streaming wire format.
//! - [`retry`] owns the retry and backoff policy.

mod adapter;
mod config;
mod http;
mod retry;
mod sse;
mod transport;
mod wire;

pub use adapter::{CHAT_COMPLETIONS_PATH, MODELS_PATH, OpenAiCompatibleProvider};
pub use config::{ApiKey, ApiKeyError, BaseUrl, BaseUrlError};
pub use http::{
    DEFAULT_CONNECT_TIMEOUT, DEFAULT_READ_TIMEOUT, DEFAULT_REQUEST_TIMEOUT, HttpTransport,
};
pub use retry::{RetryPolicy, is_retryable_status, parse_retry_after};
pub use transport::{
    ResponseBody, Transport, TransportError, TransportHeaders, TransportRequest, TransportResponse,
};
