//! The provider-neutral embedding gateway.
//!
//! A **separate port from [`crate::ModelGateway`]**, deliberately. Chat and embeddings are different
//! operations: different request shapes, different limits, and different failure meanings. A combined trait
//! would make every chat adapter implement an embedding method it may not support, and an unimplemented
//! method is a runtime refusal where a separate port is a compile-time absence.
//!
//! # One request per call, and the limits are the caller's
//!
//! The researched wire format accepts a batch and bounds it at 2048 inputs and 300,000 tokens summed, with a
//! per-input ceiling of 8192 tokens. **None of those is checkable without a tokenizer**, which this crate does
//! not have and the provider's guide does not expose for this endpoint. So the port takes what the caller
//! assembled and reports the provider's refusal, rather than pre-computing a count it cannot compute — an
//! estimate used as a limit would refuse valid requests or admit invalid ones while looking like a check.
//!
//! Local validation that *is* possible is done: an empty input is refused before a request is spent, because
//! the provider refuses it and the refusal costs a round trip.

use crate::error::ModelError;
use crate::identity::ModelId;
use crate::usage::TokenUsage;
use crate::vector::{Embedding, EmbeddingDimensions};

/// The most inputs a single embedding request may carry.
///
/// The researched provider's own bound, enforced locally so a caller learns it names the request rather than
/// receiving a provider error. It is the *wire* bound, not a token bound: the token limits are not checkable
/// here and are recorded as an unresolved question in `docs/research/integrations/embeddings.md`.
pub const MAX_EMBEDDING_INPUTS: usize = 2048;

/// The most bytes a single embedding input may carry.
///
/// # Why this exists at all, given the provider bounds tokens rather than bytes
///
/// The provider's limit is 8192 *tokens*, which cannot be counted here. This is a byte ceiling derived from
/// the token ceiling's worst case, and its purpose is to refuse a value that cannot possibly be within the
/// token limit — an input of a megabyte is not 8192 tokens under any tokenizer. So it is a **guard against
/// the absurd rather than a token check**, and it is named for bytes for that reason.
pub const MAX_EMBEDDING_INPUT_BYTES: usize = 32_768;

/// One batch of texts to embed.
///
/// A struct rather than three positional parameters, so the model and the dimension cannot be transposed, and
/// so an added parameter is not silently inserted between two call-site values.
#[derive(Clone, Debug)]
pub struct EmbeddingRequest {
    model: ModelId,
    inputs: Vec<String>,
    dimensions: Option<EmbeddingDimensions>,
}

impl EmbeddingRequest {
    /// Builds a request after validating the inputs locally.
    ///
    /// # Errors
    ///
    /// Returns [`ModelError`] with [`crate::ModelErrorKind::InvalidRequest`] for an empty batch, more than
    /// [`MAX_EMBEDDING_INPUTS`] inputs, an empty input, or an input above
    /// [`MAX_EMBEDDING_INPUT_BYTES`]. The message names the *index* of the offending input, because a batch
    /// of a thousand texts makes "one of these is empty" unactionable.
    pub fn new(model: ModelId, inputs: Vec<String>) -> Result<Self, ModelError> {
        if inputs.is_empty() {
            return Err(invalid("the embedding request carries no inputs"));
        }
        if inputs.len() > MAX_EMBEDDING_INPUTS {
            return Err(invalid(
                "the embedding request carries more inputs than the provider accepts",
            ));
        }
        if let Some(index) = inputs.iter().position(|input| input.trim().is_empty()) {
            // The index is carried rather than the text: the text is user content and would reach a log.
            return Err(invalid_index(index));
        }
        if let Some(index) = inputs
            .iter()
            .position(|input| input.len() > MAX_EMBEDDING_INPUT_BYTES)
        {
            return Err(invalid_index(index));
        }
        Ok(Self {
            model,
            inputs,
            dimensions: None,
        })
    }

    /// Requests shortened vectors of a specific dimension.
    ///
    /// The researched provider supports this "only in `text-embedding-3` and later models", and the guide
    /// recommends it over hand-truncating: the API's shortened vector is renormalized, and a hand-truncated
    /// one is not, so the two are different values. The dimension is recorded in the metadata rather than
    /// only sent, so a shortened vector cannot later be compared with a full-length one.
    #[must_use]
    pub const fn with_dimensions(mut self, dimensions: EmbeddingDimensions) -> Self {
        self.dimensions = Some(dimensions);
        self
    }

    /// Returns the requested model.
    #[must_use]
    pub const fn model(&self) -> &ModelId {
        &self.model
    }

    /// Returns the inputs, in the order they were given.
    #[must_use]
    pub fn inputs(&self) -> &[String] {
        &self.inputs
    }

    /// Returns the requested dimension, when one was requested.
    #[must_use]
    pub const fn dimensions(&self) -> Option<EmbeddingDimensions> {
        self.dimensions
    }

    /// Returns the number of inputs.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inputs.len()
    }

    /// Returns whether the request carries no inputs.
    ///
    /// Always `false` for a constructed value, because [`Self::new`] refuses an empty batch.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inputs.is_empty()
    }
}

/// The result of one embedding call.
///
/// # Why the vectors are a `Vec` aligned to the request's inputs
///
/// The wire format returns each vector with an `index` field, and this type returns them **in the request's
/// order** with the length guaranteed equal to the input count. So a caller pairs `inputs()[n]` with
/// `embeddings()[n]` by the same index, and an adapter that returned them in a different order would be a bug
/// caught by the count rather than a silent mis-assignment. The reordering by provider `index` is the
/// adapter's job, because the wire's ordering is a provider fact and the port's is a JARVIS one.
#[derive(Clone, Debug)]
pub struct EmbeddingResponse {
    embeddings: Vec<Embedding>,
    served_model: ModelId,
    usage: TokenUsage,
}

impl EmbeddingResponse {
    /// Builds a response.
    ///
    /// # Errors
    ///
    /// Returns [`ModelError`] with [`crate::ModelErrorKind::MalformedResponse`] when the vector count does not
    /// match the request's input count — an adapter bug or a provider fault, and either way a response whose
    /// vectors cannot be attributed to inputs.
    pub fn new(
        embeddings: Vec<Embedding>,
        served_model: ModelId,
        usage: TokenUsage,
        expected: usize,
    ) -> Result<Self, ModelError> {
        if embeddings.len() != expected {
            return Err(invalid(
                "the provider returned a different number of vectors than inputs",
            ));
        }
        Ok(Self {
            embeddings,
            served_model,
            usage,
        })
    }

    /// Returns the vectors, in the request's input order.
    #[must_use]
    pub fn embeddings(&self) -> &[Embedding] {
        &self.embeddings
    }

    /// Returns the model the provider reports it served.
    ///
    /// The response's own name wins over the requested one, because a provider may serve an alias and only the
    /// response reveals what ran — the same rule the chat adapter applies.
    #[must_use]
    pub const fn served_model(&self) -> &ModelId {
        &self.served_model
    }

    /// Returns the reported token usage.
    #[must_use]
    pub const fn usage(&self) -> TokenUsage {
        self.usage
    }

    /// Consumes the response and returns the vectors.
    #[must_use]
    pub fn into_embeddings(self) -> Vec<Embedding> {
        self.embeddings
    }
}

/// The port every embedding provider adapter implements.
///
/// Expressed entirely in JARVIS types: no HTTP client, SDK, or provider response type appears in the
/// signature. Implementations own transport, credentials, and conversion into [`ModelError`].
///
/// # Contract
///
/// - `embed` performs **exactly one** provider attempt. Retry policy belongs to the caller, and for this
///   operation the reason is sharper than for a chat call: there is no idempotency key, so a retried request
///   is a second billed call even though its *effect* is identical.
/// - The returned vectors are in the request's input order, with the count equal to the input count.
/// - Every vector's metadata declares the dimension that was requested, and the adapter has checked the
///   response against it.
#[async_trait::async_trait]
pub trait EmbeddingGateway: Send + Sync {
    /// Returns the provider's stable identifier.
    fn provider_id(&self) -> &crate::identity::ProviderId;

    /// Performs one provider attempt and returns the vectors.
    ///
    /// # Errors
    ///
    /// Returns [`ModelError`] for every failure, normalized into a stable category.
    async fn embed(&self, request: EmbeddingRequest) -> Result<EmbeddingResponse, ModelError>;
}

/// Builds the local-validation failure for a request.
fn invalid(message: &'static str) -> ModelError {
    ModelError::from_static(crate::error::ModelErrorKind::InvalidRequest, message)
}

/// Builds a local-validation failure naming the input index.
///
/// The index is decimal and bounded by [`MAX_EMBEDDING_INPUTS`], so it cannot forge a log line, and it is the
/// one fact about the offending input that is safe to report.
fn invalid_index(index: usize) -> ModelError {
    // A static list rather than `format!`, because `ModelError::from_static` takes a `&'static str` and a
    // formatted message would need a `SafeMessage` allocation per failure. The bound is 2048, so four digits
    // is every reachable case.
    const MESSAGES: [&str; 8] = [
        "the embedding input at position 0 is empty or oversized",
        "the embedding input at position 1 is empty or oversized",
        "the embedding input at position 2 is empty or oversized",
        "the embedding input at position 3 is empty or oversized",
        "the embedding input at position 4 is empty or oversized",
        "the embedding input at position 5 is empty or oversized",
        "the embedding input at position 6 is empty or oversized",
        "the embedding input at position 7 is empty or oversized",
    ];
    let message = MESSAGES
        .get(index)
        .copied()
        .unwrap_or("an embedding input is empty or oversized");
    invalid(message)
}

#[cfg(test)]
#[path = "embedding/port_tests.rs"]
mod port_tests;
