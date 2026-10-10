//! Asking a second model when the first is rate limited or overloaded (`ADR-0159`).
//!
//! A provider limit usually belongs to one model, not to the account: a different model at the same provider is often available while the
//! first is not. This wraps the live gateway so that when a call is refused with a rate limit or an overload **before any stream opens**
//! (after the adapter's own quick retries), the same request is made once more for the configured fallback model. Nothing was said, so
//! nothing can be said twice; the answer comes from the fallback for that call only, and the next call tries the main model again.
//!
//! Only those two refusals fall back. A refused credential, a model that does not exist, a context that is too large or a content refusal are
//! not a reason to ask someone else, and are reported as they are.

use async_trait::async_trait;
use jarvis_models::{
    ChatRequest, ChatResponse, ModelCapabilities, ModelError, ModelErrorKind, ModelGateway,
    ModelId, ModelStream, ProviderHealth, ProviderId,
};

/// A gateway that falls back to a second model identifier when the first is limited.
pub(crate) struct FallbackGateway {
    inner: Box<dyn ModelGateway>,
    fallback: ModelId,
}

impl FallbackGateway {
    /// Wraps `inner`, asking `fallback` when a call for any other model is limited.
    pub(crate) fn new(inner: Box<dyn ModelGateway>, fallback: ModelId) -> Self {
        Self { inner, fallback }
    }

    /// Whether this refusal is one a different model might not share.
    fn worth_falling_back(&self, error: &ModelError, request: &ChatRequest) -> bool {
        matches!(
            error.kind(),
            ModelErrorKind::RateLimited | ModelErrorKind::Overloaded
        ) && request.model() != &self.fallback
    }
}

#[async_trait]
impl ModelGateway for FallbackGateway {
    fn provider_id(&self) -> &ProviderId {
        self.inner.provider_id()
    }

    async fn capabilities(&self, model: &ModelId) -> Result<ModelCapabilities, ModelError> {
        self.inner.capabilities(model).await
    }

    async fn complete(&self, request: ChatRequest) -> Result<ChatResponse, ModelError> {
        match self.inner.complete(request.clone()).await {
            Err(error) if self.worth_falling_back(&error, &request) => {
                tracing::warn!(
                    kind = error.kind().as_str(),
                    fallback = self.fallback.as_str(),
                    "the model is limited, so the fallback model is asked instead"
                );
                self.inner
                    .complete(request.with_model(self.fallback.clone()))
                    .await
            }
            other => other,
        }
    }

    async fn stream(&self, request: ChatRequest) -> Result<ModelStream, ModelError> {
        match self.inner.stream(request.clone()).await {
            Err(error) if self.worth_falling_back(&error, &request) => {
                tracing::warn!(
                    kind = error.kind().as_str(),
                    fallback = self.fallback.as_str(),
                    "the model is limited, so the fallback model is asked instead"
                );
                self.inner
                    .stream(request.with_model(self.fallback.clone()))
                    .await
            }
            other => other,
        }
    }

    async fn health(&self) -> ProviderHealth {
        self.inner.health().await
    }
}

#[cfg(test)]
#[path = "model_fallback_tests.rs"]
mod tests;
