//! The OpenAI-compatible embeddings adapter.
//!
//! The only place where this wire shape becomes a JARVIS type. It reuses the chat adapter's
//! [`Transport`](super::transport::Transport) seam rather than a second HTTP client, so one tested byte pipe
//! serves both operations and the retry/timeout policy stays where the chat adapter already put it.

use std::sync::Arc;

use crate::embedding::{EmbeddingGateway, EmbeddingRequest, EmbeddingResponse};
use crate::error::{ModelError, ModelErrorKind};
use crate::identity::ProviderId;
use crate::usage::TokenUsage;
use crate::vector::{
    Embedding, EmbeddingDimensions, EmbeddingMetadata, EmbeddingVector, Normalization,
};

use super::adapter::{map_transport_error, provider_request_id, safe_message_for};
use super::config::{ApiKey, BaseUrl};
use super::transport::{Transport, TransportHeaders, TransportRequest, TransportResponse};
use super::wire::{WireErrorEnvelope, classify_error as classify_wire_error};
use super::wire_embedding::{self, WireEmbeddingRequest, WireEmbeddingResponse};

/// The relative path of the embeddings operation.
pub const EMBEDDINGS_PATH: &str = "/embeddings";

/// The normalization the researched provider documents for its vectors.
///
/// The guide states they "are normalized to length 1", which is what makes a dot product a valid cosine
/// comparison for this provider. Recorded as a constant here rather than assumed in the port, because it is a
/// **provider fact** and the port is provider-neutral.
pub const PROVIDER_NORMALIZATION: Normalization = Normalization::Normalized;

/// A provider speaking the OpenAI-compatible embeddings dialect.
pub struct OpenAiCompatibleEmbeddingProvider {
    provider: ProviderId,
    base_url: BaseUrl,
    api_key: ApiKey,
    transport: Arc<dyn Transport>,
}

impl std::fmt::Debug for OpenAiCompatibleEmbeddingProvider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The credential is a secret and the transport may hold pooled connections, so neither is rendered.
        formatter
            .debug_struct("OpenAiCompatibleEmbeddingProvider")
            .field("provider", &self.provider)
            .field("base_url", &self.base_url)
            .finish_non_exhaustive()
    }
}

impl OpenAiCompatibleEmbeddingProvider {
    /// Creates an embeddings adapter.
    #[must_use]
    pub fn new(
        provider: ProviderId,
        base_url: BaseUrl,
        api_key: ApiKey,
        transport: Arc<dyn Transport>,
    ) -> Self {
        Self {
            provider,
            base_url,
            api_key,
            transport,
        }
    }

    /// Returns the configured base URL.
    #[must_use]
    pub const fn base_url(&self) -> &BaseUrl {
        &self.base_url
    }

    /// Builds the headers for one attempt.
    fn headers(&self) -> TransportHeaders {
        // The chat adapter also sends the client's correlation id, and this operation has no correlation id in
        // its request shape: a memory's embedding is not part of a run, so there is nothing to correlate it
        // with. Sending an invented one would put a value in a header that names nothing.
        TransportHeaders::new().with_authorization(self.api_key.header_value())
    }

    /// Serializes a request for the provider.
    ///
    /// # Errors
    ///
    /// Returns [`ModelError`] when the request cannot be serialized, which is an [`ModelErrorKind::Internal`]
    /// failure because the shape is built here rather than supplied by the caller.
    fn serialize(request: &EmbeddingRequest) -> Result<String, ModelError> {
        let inputs: Vec<&str> = request.inputs().iter().map(String::as_str).collect();
        let wire = WireEmbeddingRequest {
            model: request.model().as_str(),
            input: &inputs,
            // Sent only when requested: the parameter is unsupported before `text-embedding-3`, and a provider
            // that rejects an unknown field would refuse every request that carried a default.
            dimensions: request.dimensions().map(EmbeddingDimensions::get),
            // Always `float`. The only other value the wire accepts is `base64`, and a decode path for
            // provider-supplied base64 is an unmeasured optimization.
            encoding_format: wire_embedding::FLOAT_ENCODING,
        };
        serde_json::to_string(&wire).map_err(|_| {
            ModelError::from_static(
                ModelErrorKind::Internal,
                "the embedding request could not be serialized",
            )
        })
    }
}

#[async_trait::async_trait]
impl EmbeddingGateway for OpenAiCompatibleEmbeddingProvider {
    fn provider_id(&self) -> &ProviderId {
        &self.provider
    }

    async fn embed(&self, request: EmbeddingRequest) -> Result<EmbeddingResponse, ModelError> {
        let body = Self::serialize(&request)?;
        let transport_request =
            TransportRequest::new(self.base_url.join(EMBEDDINGS_PATH), body, self.headers());

        let response = self
            .transport
            .send(&transport_request, false)
            .await
            .map_err(map_transport_error)?;

        match response.status() {
            status if (200..300).contains(&status) => decode(&response, &request, &self.provider),
            _ => Err(classify_response(&response)),
        }
    }
}

/// Classifies a non-2xx response into a normalized error.
///
/// The wire classification is reused from the chat adapter, because the provider's error envelope is the same
/// shape on both endpoints and the kind mapping is a provider fact rather than an operation one.
fn classify_response(response: &TransportResponse) -> ModelError {
    let (status, body_text, retry_after) = match response {
        TransportResponse::Buffered {
            status,
            retry_after_seconds,
            body,
            ..
        } => (*status, body.as_str(), *retry_after_seconds),
        TransportResponse::Streaming { status, .. } => (*status, "", None),
    };

    let parsed: Option<WireErrorEnvelope> = serde_json::from_str(body_text).ok();
    let provider_error = parsed.as_ref().and_then(|envelope| envelope.error.as_ref());
    let kind = classify_wire_error(status, provider_error);

    ModelError::new(kind, safe_message_for(kind))
        .with_retry_after_seconds(retry_after)
        .with_provider_request_id(provider_request_id(response))
        .with_provider_status(Some(status))
}

/// Decodes a buffered body into an embedding response.
fn decode(
    response: &TransportResponse,
    request: &EmbeddingRequest,
    provider: &ProviderId,
) -> Result<EmbeddingResponse, ModelError> {
    let body = match response {
        TransportResponse::Buffered { body, .. } => body.as_str(),
        // The call is never sent with `streaming: true`, so a streaming response is an adapter invariant
        // failure rather than a provider fault.
        TransportResponse::Streaming { .. } => {
            return Err(ModelError::from_static(
                ModelErrorKind::Internal,
                "the embedding call received a streaming response",
            ));
        }
    };

    let decoded: WireEmbeddingResponse = serde_json::from_str(body).map_err(|_| {
        ModelError::from_static(
            ModelErrorKind::MalformedResponse,
            "the provider response body was not the expected shape",
        )
    })?;

    // The requested dimension is what the metadata declares, falling back to the first vector's length when
    // the request asked for none. That fallback is safe **only** because it is the response's own length: a
    // declared dimension no vector matches is refused below, so the metadata can never describe a shape the
    // provider did not return.
    let declared = match request.dimensions() {
        Some(dimensions) => dimensions,
        None => EmbeddingDimensions::new(
            u32::try_from(decoded.data.first().map_or(0, |item| item.embedding.len())).unwrap_or(0),
        )
        .map_err(|_| {
            ModelError::from_static(
                ModelErrorKind::MalformedResponse,
                "the provider returned a vector with no usable length",
            )
        })?,
    };

    let input_hash_base = request.model().as_str();
    let chunker_version: Option<&str> = None;
    let mut slots: Vec<Option<Embedding>> = vec![None; request.inputs().len()];
    for item in &decoded.data {
        // Reassembled by the provider's own `index`, because the wire defines it as the index of the embedding
        // in the list of embeddings. Positional pairing would hold today and silently mis-assign if the provider
        // ever reordered.
        let slot = slots.get_mut(item.index).ok_or_else(|| {
            ModelError::from_static(
                ModelErrorKind::MalformedResponse,
                "the provider returned an embedding index outside the request",
            )
        })?;
        // Two vectors claiming the same input is a provider fault, and taking the last one silently would be
        // the same class of defect as pairing positionally: a plausible response that is not the one asked
        // for. Which of the two is "the" answer for that input is not knowable here.
        if slot.is_some() {
            return Err(ModelError::from_static(
                ModelErrorKind::MalformedResponse,
                "the provider returned two embeddings for one input",
            ));
        }
        let vector = EmbeddingVector::new(item.embedding.clone()).map_err(|_| {
            ModelError::from_static(
                ModelErrorKind::MalformedResponse,
                "the provider returned a vector that is not a usable one",
            )
        })?;
        let input = request
            .inputs()
            .get(item.index)
            .map(String::as_str)
            .unwrap_or_default();
        let metadata = EmbeddingMetadata {
            provider: provider.clone(),
            model: request.model().clone(),
            version: None,
            dimensions: declared,
            normalization: PROVIDER_NORMALIZATION,
            input_hash: wire_embedding::input_hash(input_hash_base, chunker_version, input),
            chunker_version: chunker_version.map(str::to_owned),
        };
        *slot = Some(Embedding::new(vector, metadata).map_err(|_| {
            ModelError::from_static(
                ModelErrorKind::MalformedResponse,
                "the provider returned a vector whose length is not the declared dimension",
            )
        })?);
    }

    // A `None` here means the provider omitted an index, so fewer vectors came back than inputs were sent.
    // Collected into a count rather than reported per slot, because the fact is about the response.
    if slots.iter().any(Option::is_none) {
        return Err(ModelError::from_static(
            ModelErrorKind::MalformedResponse,
            "the provider omitted an embedding for one of the inputs",
        ));
    }
    let embeddings: Vec<Embedding> = slots.into_iter().flatten().collect();

    // The response's own model name wins when present, because a provider may serve an alias and only the
    // response reveals what ran.
    let served_model = crate::identity::ModelId::new(decoded.model_text())
        .unwrap_or_else(|_| request.model().clone());

    let usage = TokenUsage::new(decoded.usage.prompt_tokens, 0);

    EmbeddingResponse::new(embeddings, served_model, usage, request.inputs().len())
}
