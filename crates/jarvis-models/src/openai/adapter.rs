//! The OpenAI-compatible provider adapter.
//!
//! This is the only place where a provider wire shape becomes a JARVIS type. Retry,
//! timeout, and classification policy live here rather than in the HTTP client, so
//! every attempt is observable and the retry decision is a JARVIS decision.

use std::{collections::VecDeque, sync::Arc};

use crate::capability::{ModelCapabilities, Placement, Support};
use crate::error::{ModelError, ModelErrorKind, ProviderRequestId};
use crate::identity::{ModelId, ProviderId};
use crate::port::{ModelGateway, ModelStream, ProviderHealth, ProviderStatus};
use crate::request::{ChatMessage, ChatRequest};
use crate::response::ChatResponse;
use crate::stream::{StreamEnvelope, StreamEvent, StreamValidator};

use super::config::{ApiKey, BaseUrl};
use super::retry::RetryPolicy;
use super::sse::{DONE_SENTINEL, SseDecoder, SseStep};
use super::transport::{
    ResponseBody, Transport, TransportError, TransportHeaders, TransportRequest, TransportResponse,
};
use super::wire::{self, WireChatRequest, WireChatResponse, WireErrorEnvelope, WireStreamChunk};

/// The relative path of the chat completions operation.
pub const CHAT_COMPLETIONS_PATH: &str = "/chat/completions";

/// The relative path used for capability and reachability probes.
///
/// A probe uses this rather than a completion call, because a health check must not
/// incur a billable request.
pub const MODELS_PATH: &str = "/models";

/// A provider speaking the OpenAI-compatible Chat Completions dialect.
pub struct OpenAiCompatibleProvider {
    provider: ProviderId,
    base_url: BaseUrl,
    api_key: ApiKey,
    transport: Arc<dyn Transport>,
    retry: RetryPolicy,
}

impl std::fmt::Debug for OpenAiCompatibleProvider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The credential is a secret and the transport may hold pooled connections,
        // so neither is rendered even in debug output.
        formatter
            .debug_struct("OpenAiCompatibleProvider")
            .field("provider", &self.provider)
            .field("base_url", &self.base_url)
            .field("retry", &self.retry)
            .finish_non_exhaustive()
    }
}

impl OpenAiCompatibleProvider {
    /// Creates a provider adapter.
    #[must_use]
    pub fn new(
        provider: ProviderId,
        base_url: BaseUrl,
        api_key: ApiKey,
        transport: Arc<dyn Transport>,
        retry: RetryPolicy,
    ) -> Self {
        Self {
            provider,
            base_url,
            api_key,
            transport,
            retry,
        }
    }

    /// Returns the configured base URL.
    #[must_use]
    pub const fn base_url(&self) -> &BaseUrl {
        &self.base_url
    }

    /// Returns the configured retry policy.
    #[must_use]
    pub const fn retry_policy(&self) -> RetryPolicy {
        self.retry
    }

    /// Builds the headers for one attempt.
    fn headers(&self, correlation_id: jarvis_core::CorrelationId) -> TransportHeaders {
        TransportHeaders::new()
            .with_authorization(self.api_key.header_value())
            .with_client_request_id(correlation_id.to_string())
    }

    /// Serializes a request for the provider.
    ///
    /// Local validation runs first so a malformed request fails without a round trip
    /// and without spending a provider call.
    fn serialize(request: &ChatRequest, streaming: bool) -> Result<String, ModelError> {
        if !request.messages().iter().all(ChatMessage::is_consistent) {
            return Err(ModelError::from_static(
                ModelErrorKind::InvalidRequest,
                "the request contains an inconsistent tool message",
            ));
        }
        if !request.is_within_bounds() {
            return Err(ModelError::from_static(
                ModelErrorKind::InvalidRequest,
                "the request exceeds the local size bounds",
            ));
        }

        // The wire struct borrows each message's text, so the owned text is collected
        // first and the borrows are built from it.
        let contents: Vec<String> = request
            .messages()
            .iter()
            .map(|message| message.content().text())
            .collect();
        let messages: Vec<wire::WireMessage<'_>> = request
            .messages()
            .iter()
            .zip(contents.iter())
            .map(|(message, content)| wire::WireMessage {
                role: wire::role_wire_name(message.role()),
                content,
                tool_call_id: message.tool_call_id(),
            })
            .collect();

        let wire = WireChatRequest {
            model: request.model().as_str(),
            messages,
            stream: streaming,
            stream_options: streaming.then_some(wire::WireStreamOptions {
                include_usage: request.include_usage(),
            }),
            max_tokens: request.max_output_tokens(),
            temperature: request
                .temperature_milli()
                .map(|value| f64::from(value) / 1000.0),
            stop: request.stop().iter().map(String::as_str).collect(),
        };

        serde_json::to_string(&wire).map_err(|_| {
            ModelError::from_static(
                ModelErrorKind::Internal,
                "the provider request could not be serialized",
            )
        })
    }

    /// Performs exactly one attempt.
    async fn attempt(
        &self,
        request: &ChatRequest,
        streaming: bool,
    ) -> Result<TransportResponse, ModelError> {
        let body = Self::serialize(request, streaming)?;
        let transport_request = TransportRequest::new(
            self.base_url.join(CHAT_COMPLETIONS_PATH),
            body,
            self.headers(request.correlation_id()),
        );

        self.transport
            .send(&transport_request, streaming)
            .await
            .map_err(map_transport_error)
    }

    /// Runs the bounded attempt loop and returns the accepted response.
    ///
    /// A non-2xx response is classified **before** the retry decision, because a
    /// `429` may carry either a transient rate limit or a permanent quota failure,
    /// and retrying the latter can never succeed.
    async fn send_with_retry(
        &self,
        request: &ChatRequest,
        streaming: bool,
    ) -> Result<TransportResponse, ModelError> {
        let mut attempts = 0_u32;

        loop {
            attempts += 1;

            // A non-2xx response becomes an error *here* rather than returning
            // directly, so that a retryable status such as 429 takes the same retry
            // path as a transport failure. Returning early on a status would silently
            // disable HTTP-level retries entirely.
            let error = match self.attempt(request, streaming).await {
                Ok(response) if (200..300).contains(&response.status()) => return Ok(response),
                Ok(response) => Self::classify_response(&response),
                Err(error) => error,
            };

            // The classification decides retryability, not the status: a 429 may be a
            // transient rate limit or a permanent quota failure.
            if !error.is_retryable() || !self.retry.allows_attempt(attempts) {
                return Err(error);
            }

            let delay = self
                .retry
                .delay_before_attempt(attempts, error.retry_after_seconds());
            if !delay.is_zero() {
                // Bounded by the policy, and dropping the caller's future during this
                // wait cancels the retry.
                tokio::time::sleep(delay).await;
            }
        }
    }

    /// Classifies a non-2xx response into a normalized error.
    fn classify_response(response: &TransportResponse) -> ModelError {
        let (status, body_text, retry_after) = match response {
            TransportResponse::Buffered {
                status,
                retry_after_seconds,
                body,
                ..
            } => (*status, body.as_str(), *retry_after_seconds),
            // A streaming error response has not had its body read, so the status is
            // the only evidence available.
            TransportResponse::Streaming { status, .. } => (*status, "", None),
        };

        let parsed: Option<WireErrorEnvelope> = serde_json::from_str(body_text).ok();
        let provider_error = parsed.as_ref().and_then(|envelope| envelope.error.as_ref());
        let kind = wire::classify_error(status, provider_error);

        ModelError::new(kind, safe_message_for(kind))
            .with_retry_after_seconds(retry_after)
            .with_provider_request_id(provider_request_id(response))
            .with_provider_status(Some(status))
    }
}

/// Extracts a validated provider request identifier from a response.
fn provider_request_id(response: &TransportResponse) -> Option<ProviderRequestId> {
    response
        .provider_request_id()
        .and_then(|value| ProviderRequestId::new(value).ok())
}

/// Returns fixed, non-secret text for a failure category.
///
/// The provider's own message is never carried, because it can echo request content
/// or interpolate a credential. JARVIS owns the explanation for its own categories.
fn safe_message_for(kind: ModelErrorKind) -> jarvis_core::SafeMessage {
    let text = match kind {
        ModelErrorKind::InvalidRequest => "the provider rejected the request as invalid",
        ModelErrorKind::Authentication => "provider authentication failed",
        ModelErrorKind::Authorization => "the credential lacks access to this model",
        ModelErrorKind::ContentRefusal => "the provider refused the output",
        ModelErrorKind::RateLimited => "the provider rate limit was reached",
        ModelErrorKind::Overloaded => "the model is temporarily overloaded",
        ModelErrorKind::Transient => "the provider reported a temporary failure",
        ModelErrorKind::QuotaExhausted => "the provider account has no remaining quota",
        ModelErrorKind::ContextOverflow => "the request exceeded the model context window",
        ModelErrorKind::ModelNotFound => "the configured model is not served by this provider",
        ModelErrorKind::Timeout => "the provider did not respond within the deadline",
        ModelErrorKind::Cancelled => "the model call was cancelled",
        ModelErrorKind::Incomplete => "the response stream ended before completion",
        ModelErrorKind::MalformedResponse => "the provider response could not be interpreted",
        ModelErrorKind::Internal => "the model adapter failed unexpectedly",
    };
    jarvis_core::SafeMessage::new(text).unwrap_or_default()
}

/// Maps a transport failure onto a normalized model error.
fn map_transport_error(error: TransportError) -> ModelError {
    let kind = match error {
        TransportError::Connect | TransportError::Io | TransportError::Tls => {
            ModelErrorKind::Transient
        }
        TransportError::RequestTimeout | TransportError::StalledRead => ModelErrorKind::Timeout,
        TransportError::Cancelled => ModelErrorKind::Cancelled,
        // A body that could not be read is an unknown outcome: the provider may have
        // accepted the request, so it must not be retried as though it never arrived.
        TransportError::Body => ModelErrorKind::MalformedResponse,
    };
    ModelError::new(kind, safe_message_for(kind))
}

/// Decodes a buffered body into a completed response.
fn decode_buffered(
    provider: &ProviderId,
    requested_model: &ModelId,
    body: &str,
    response: &TransportResponse,
) -> Result<ChatResponse, ModelError> {
    let decoded: WireChatResponse = serde_json::from_str(body).map_err(|_| {
        ModelError::from_static(
            ModelErrorKind::MalformedResponse,
            "the provider response body was not the expected shape",
        )
    })?;

    let first = decoded.choices.first();
    let output = wire::output_items(first.and_then(|choice| choice.message.as_ref()));
    let finish = wire::finish_reason(first.and_then(|choice| choice.finish_reason.as_deref()));

    // The response's own model name wins when present, because a provider may serve
    // an alias and only the response reveals what actually ran.
    let served_model = decoded
        .model
        .as_deref()
        .and_then(|name| ModelId::new(name).ok())
        .unwrap_or_else(|| requested_model.clone());

    Ok(
        ChatResponse::new(provider.clone(), served_model, output, finish)
            .with_usage(decoded.usage.as_ref().map(wire::WireUsage::to_usage))
            .with_provider_request_id(provider_request_id(response)),
    )
}

#[async_trait::async_trait]
impl ModelGateway for OpenAiCompatibleProvider {
    fn provider_id(&self) -> &ProviderId {
        &self.provider
    }

    async fn capabilities(&self, _model: &ModelId) -> Result<ModelCapabilities, ModelError> {
        let probe = TransportRequest::get(
            self.base_url.join(MODELS_PATH),
            self.headers(jarvis_core::CorrelationId::new()),
        );

        // Placement is reported as remote unless the authority is loopback, because
        // wrongly reporting remote costs a routing opportunity while wrongly
        // reporting local lets private content leave the machine.
        let placement = if self.base_url.is_loopback() {
            Placement::Local
        } else {
            Placement::Remote
        };

        let reachable = matches!(
            self.transport.send(&probe, false).await,
            Ok(response) if (200..300).contains(&response.status())
        );

        if !reachable {
            // An unreachable provider has no established capabilities. Reporting them
            // as unsupported would be a claim this code has not verified.
            return Ok(ModelCapabilities::unknown().with_placement(Some(placement)));
        }

        Ok(ModelCapabilities::unknown()
            .with_placement(Some(placement))
            .with_streaming(Support::Supported)
            .with_usage_on_stream(Support::Supported)
            // The adapter sends the interoperable `system` spelling, so the
            // higher-authority role is deliberately not claimed by default.
            .with_developer_role(Support::Unsupported))
    }

    async fn complete(&self, request: ChatRequest) -> Result<ChatResponse, ModelError> {
        let response = self.send_with_retry(&request, false).await?;
        let TransportResponse::Buffered { body, .. } = &response else {
            return Err(ModelError::from_static(
                ModelErrorKind::MalformedResponse,
                "the provider returned a stream where a complete response was expected",
            ));
        };

        decode_buffered(&self.provider, request.model(), body, &response)
    }

    async fn stream(&self, request: ChatRequest) -> Result<ModelStream, ModelError> {
        let response = self.send_with_retry(&request, true).await?;
        let TransportResponse::Streaming { body, .. } = response else {
            return Err(ModelError::from_static(
                ModelErrorKind::MalformedResponse,
                "the provider returned a complete response where a stream was expected",
            ));
        };

        Ok(Box::pin(sse_stream(body)))
    }

    async fn health(&self) -> ProviderHealth {
        let probe = TransportRequest::get(
            self.base_url.join(MODELS_PATH),
            self.headers(jarvis_core::CorrelationId::new()),
        );

        match self.transport.send(&probe, false).await {
            Ok(response) => {
                let status = response.status();
                if (200..300).contains(&status) {
                    ProviderHealth::new(ProviderStatus::Ready, "the provider answered a probe")
                } else if status == 401 || status == 403 {
                    ProviderHealth::new(
                        ProviderStatus::Degraded,
                        "the provider is reachable but rejected the credential",
                    )
                } else {
                    ProviderHealth::new(
                        ProviderStatus::Degraded,
                        "the provider is reachable but returned an unexpected status",
                    )
                }
            }
            Err(_) => {
                ProviderHealth::new(ProviderStatus::Unreachable, "the provider did not answer")
            }
        }
    }
}

/// The state of one streaming response.
struct StreamState {
    body: Box<dyn ResponseBody>,
    decoder: SseDecoder,
    validator: StreamValidator,
    /// Events decoded from one provider chunk but not yet delivered.
    pending: VecDeque<StreamEvent>,
    sequence: u64,
    /// A terminal event has been accepted.
    terminal: bool,
    /// The stream is over, whether by completion or by a reported error.
    exhausted: bool,
}

impl StreamState {
    fn new(body: Box<dyn ResponseBody>) -> Self {
        Self {
            body,
            decoder: SseDecoder::new(),
            validator: StreamValidator::new(),
            pending: VecDeque::new(),
            sequence: 0,
            terminal: false,
            exhausted: false,
        }
    }

    /// Validates an event and assigns it the next sequence number.
    fn pack(&mut self, event: StreamEvent) -> Result<StreamEnvelope, ModelError> {
        let envelope = StreamEnvelope::new(self.sequence, event);
        self.validator.accept(envelope.clone()).map_err(|_| {
            ModelError::from_static(
                ModelErrorKind::MalformedResponse,
                "the provider stream violated its ordering contract",
            )
        })?;
        self.sequence += 1;
        if envelope.event().is_terminal() {
            self.terminal = true;
        }
        Ok(envelope)
    }

    /// Decodes one SSE data payload into zero or more pending events.
    fn decode_chunk(&mut self, text: &str) -> Result<(), ModelError> {
        if text.trim() == DONE_SENTINEL {
            return self.finish_provider_stream();
        }

        let chunk: WireStreamChunk = serde_json::from_str(text).map_err(|_| {
            ModelError::from_static(
                ModelErrorKind::MalformedResponse,
                "a provider stream chunk was not the expected shape",
            )
        })?;

        let choice = chunk.choices.first();

        if let Some(content) = choice
            .and_then(|value| value.delta.as_ref())
            .and_then(|delta| delta.content.as_ref())
            .filter(|content| !content.is_empty())
        {
            self.pending.push_back(StreamEvent::TextDelta {
                text: content.clone(),
            });
        }

        // Tool call deltas arrive in fragments across chunks; assembling them is a
        // later slice, so a fragment is not reported as a complete invocation.
        if let Some(usage) = chunk.usage.as_ref() {
            self.pending.push_back(StreamEvent::Usage {
                usage: usage.to_usage(),
            });
        }

        if let Some(label) = choice.and_then(|value| value.finish_reason.as_deref()) {
            self.pending.push_back(StreamEvent::Finished {
                reason: wire::finish_reason(Some(label)),
            });
        }

        Ok(())
    }

    /// Handles the provider's done sentinel.
    ///
    /// A sentinel that arrives before any finish reason means the provider closed
    /// early, which is not a completed answer.
    fn finish_provider_stream(&mut self) -> Result<(), ModelError> {
        self.exhausted = true;
        if self.validator.is_finished() || self.terminal {
            return Ok(());
        }
        Err(ModelError::new(
            ModelErrorKind::Incomplete,
            safe_message_for(ModelErrorKind::Incomplete),
        ))
    }

    /// Returns the next event, or `None` when the stream is complete.
    ///
    /// # Errors
    ///
    /// Returns [`ModelError`] when the body fails, the SSE buffer overflows, a
    /// payload is malformed, or the stream ends without a terminal event.
    async fn next(&mut self) -> Result<Option<StreamEnvelope>, ModelError> {
        loop {
            if let Some(event) = self.pending.pop_front() {
                return Ok(Some(self.pack(event)?));
            }

            if self.terminal || self.exhausted {
                return Ok(None);
            }

            let chunk = self.body.next_chunk().await;

            let step = match chunk {
                Ok(Some(chunk)) => self.decoder.push(&chunk),
                Ok(None) => {
                    // The connection ended. Without a terminal event this is a
                    // truncated answer, not a finished one.
                    self.exhausted = true;
                    if self.validator.is_finished() || self.terminal {
                        return Ok(None);
                    }
                    return Err(ModelError::new(
                        ModelErrorKind::Incomplete,
                        safe_message_for(ModelErrorKind::Incomplete),
                    ));
                }
                Err(error) => {
                    self.exhausted = true;
                    return Err(map_transport_error(error));
                }
            };

            match step {
                SseStep::Data(text) => self.decode_chunk(&text)?,
                SseStep::Done => {
                    self.finish_provider_stream()?;
                    return Ok(None);
                }
                SseStep::Overflow => {
                    self.exhausted = true;
                    return Err(ModelError::from_static(
                        ModelErrorKind::MalformedResponse,
                        "the provider stream exceeded its buffer bound",
                    ));
                }
                SseStep::Incomplete => {}
            }
        }
    }
}

/// Turns a provider body into a normalized event stream.
fn sse_stream(
    body: Box<dyn ResponseBody>,
) -> impl futures_core::Stream<Item = Result<StreamEnvelope, ModelError>> {
    futures_util::stream::unfold(StreamState::new(body), |mut state| async move {
        match state.next().await {
            Ok(Some(envelope)) => Some((Ok(envelope), state)),
            Ok(None) => None,
            Err(error) => {
                // Report the failure once and then end, so a consumer is not fed the
                // same error repeatedly.
                state.exhausted = true;
                Some((Err(error), state))
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_transport_body_failure_is_not_reported_as_retryable() {
        // The provider may have accepted the request, so an unreadable body is an
        // unknown outcome rather than a clean transient failure.
        let error = map_transport_error(TransportError::Body);
        assert_eq!(error.kind(), ModelErrorKind::MalformedResponse);
        assert!(!error.is_retryable());
    }

    #[test]
    fn a_never_sent_request_is_retryable_but_a_stall_is_a_timeout() {
        assert!(map_transport_error(TransportError::Connect).is_retryable());
        assert_eq!(
            map_transport_error(TransportError::StalledRead).kind(),
            ModelErrorKind::Timeout
        );
    }

    #[test]
    fn a_cancelled_transport_reports_cancellation_not_failure() {
        let error = map_transport_error(TransportError::Cancelled);
        assert_eq!(error.kind(), ModelErrorKind::Cancelled);
        assert!(
            !error.is_retryable(),
            "a cancelled call must not restart itself"
        );
    }

    #[test]
    fn every_category_has_non_secret_explanatory_text() {
        for kind in [
            ModelErrorKind::InvalidRequest,
            ModelErrorKind::Authentication,
            ModelErrorKind::Authorization,
            ModelErrorKind::ContentRefusal,
            ModelErrorKind::RateLimited,
            ModelErrorKind::Overloaded,
            ModelErrorKind::Transient,
            ModelErrorKind::QuotaExhausted,
            ModelErrorKind::ContextOverflow,
            ModelErrorKind::ModelNotFound,
            ModelErrorKind::Timeout,
            ModelErrorKind::Cancelled,
            ModelErrorKind::Incomplete,
            ModelErrorKind::MalformedResponse,
            ModelErrorKind::Internal,
        ] {
            let text = safe_message_for(kind);
            assert!(
                !text.as_str().trim().is_empty(),
                "{kind} must explain itself"
            );
        }
    }
}
