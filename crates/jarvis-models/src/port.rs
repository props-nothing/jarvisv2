use std::pin::Pin;

use futures_core::Stream;

use crate::capability::ModelCapabilities;
use crate::error::ModelError;
use crate::identity::{ModelId, ProviderId};
use crate::request::ChatRequest;
use crate::response::ChatResponse;
use crate::stream::StreamEnvelope;

/// A stream of sequence-numbered normalized model events.
///
/// The item is a [`StreamEnvelope`] rather than a bare [`StreamEvent`] because the
/// sequence number is what lets a consumer detect a gap. Dropping it here would
/// force every consumer to reconstruct ordering it cannot observe from the adapter.
///
/// Boxed because the adapter's concrete stream type stays private to the adapter, in
/// the same way provider SDK types never cross a JARVIS boundary.
pub type ModelStream = Pin<Box<dyn Stream<Item = Result<StreamEnvelope, ModelError>> + Send>>;

/// The reachability of a provider.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderStatus {
    /// The provider answered a health probe.
    Ready,
    /// The provider is reachable but not usable, for example when unauthenticated.
    Degraded,
    /// The provider could not be reached.
    Unreachable,
}

impl ProviderStatus {
    /// Returns the stable snake-case wire code.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Degraded => "degraded",
            Self::Unreachable => "unreachable",
        }
    }
}

/// The result of a provider health probe.
///
/// `detail` is a fixed, non-secret explanation. A probe must never require a
/// credential to report reachability and must never include one in its output.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProviderHealth {
    status: ProviderStatus,
    /// A bounded explanation that must not contain a secret.
    pub detail: &'static str,
}

impl ProviderHealth {
    /// Creates a health report.
    #[must_use]
    pub const fn new(status: ProviderStatus, detail: &'static str) -> Self {
        Self { status, detail }
    }

    /// Returns the reachability status.
    #[must_use]
    pub const fn status(self) -> ProviderStatus {
        self.status
    }
}

/// The port every model provider adapter implements.
///
/// The trait is expressed entirely in JARVIS types: no HTTP client, SDK, or
/// provider response type appears in the signature. Implementations own
/// transport, credentials, retries, and timeouts, and convert every failure into
/// [`ModelError`].
///
/// # Contract
///
/// - `capabilities` returns what is *known*; unknown is not supported.
/// - `complete` performs exactly one provider attempt. Retry, backoff, and deadline
///   policy belong to the caller, so an attempt is independently observable and
///   separately recorded.
/// - `stream` yields events whose sequence numbers start at zero and increase by
///   one. Dropping the stream must abort the provider request.
/// - `health` must not require a credential to report reachability.
#[async_trait::async_trait]
pub trait ModelGateway: Send + Sync {
    /// Returns the provider's stable identifier.
    fn provider_id(&self) -> &ProviderId;

    /// Returns what the model is known to support.
    ///
    /// # Errors
    ///
    /// Returns [`ModelError`] when the capability cannot be determined.
    async fn capabilities(&self, model: &ModelId) -> Result<ModelCapabilities, ModelError>;

    /// Performs one provider attempt and returns the completed response.
    ///
    /// # Errors
    ///
    /// Returns [`ModelError`] for every failure, normalized into a stable category.
    async fn complete(&self, request: ChatRequest) -> Result<ChatResponse, ModelError>;

    /// Performs one provider attempt and returns a normalized event stream.
    ///
    /// # Errors
    ///
    /// Returns [`ModelError`] when the request cannot be established. Failures that
    /// occur after the stream begins arrive as `Err` items on the stream itself, so
    /// a mid-stream provider failure is not lost.
    async fn stream(&self, request: ChatRequest) -> Result<ModelStream, ModelError>;

    /// Reports provider reachability.
    async fn health(&self) -> ProviderHealth;
}
