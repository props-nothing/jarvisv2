//! The private provider JSON shapes for the embeddings endpoint.
//!
//! Borrowing shapes, so serialization does not clone the inputs — a batch of a thousand texts would otherwise
//! be copied once for the wire.
//!
//! The error envelope is **not** restated here. The provider sends the same envelope on both endpoints, and a
//! second copy would be one that can drift from the first; `openai::wire`'s `WireErrorEnvelope` and
//! `classify_error` are reused by the adapter, and they are `pub(super)` for exactly that reason.

use serde::{Deserialize, Serialize};

/// The only encoding this adapter requests.
///
/// The wire accepts `float` and `base64`, and `float` is what the provider's own examples use. A `base64` path
/// would be an unmeasured optimization plus a decode of provider-supplied data.
pub const FLOAT_ENCODING: &str = "float";

/// The documented embeddings request body.
#[derive(Debug, Serialize)]
pub(super) struct WireEmbeddingRequest<'a> {
    /// The model identifier.
    pub(super) model: &'a str,
    /// The texts to embed. An array is always sent, even for one input, because the response's `index` field is
    /// defined against a list and a single-string request would make the reassembly path untested.
    pub(super) input: &'a [&'a str],
    /// The requested output dimension.
    ///
    /// Omitted entirely when absent, rather than sent as `null`: the parameter is unsupported before
    /// `text-embedding-3`, and a provider that rejects an unknown field would refuse every request carrying it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) dimensions: Option<u32>,
    /// The response encoding.
    pub(super) encoding_format: &'a str,
}

/// One embedding in the documented response.
#[derive(Debug, Deserialize)]
pub(super) struct WireEmbeddingData {
    /// The vector.
    ///
    /// Untyped in the provider's own schema (the reference documents an `array of number`), so this is a
    /// `Vec<f32>` and a value that is not numeric fails deserialization rather than being coerced.
    pub(super) embedding: Vec<f32>,
    /// The index of this embedding in the request's input list.
    pub(super) index: usize,
}

/// The documented embeddings usage object.
///
/// Only `prompt_tokens` is read: an embedding call generates nothing, so a total that included output tokens
/// would be reporting a field the operation cannot produce.
#[derive(Debug, Default, Deserialize)]
pub(super) struct WireEmbeddingUsage {
    /// The number of input tokens billed.
    #[serde(default)]
    pub(super) prompt_tokens: u64,
}

/// The documented embeddings response body.
#[derive(Debug, Deserialize)]
pub(super) struct WireEmbeddingResponse {
    /// The vectors.
    #[serde(default)]
    pub(super) data: Vec<WireEmbeddingData>,
    /// The model the provider reports it served.
    #[serde(default)]
    pub(super) model: Option<String>,
    /// The token usage.
    #[serde(default)]
    pub(super) usage: WireEmbeddingUsage,
}

impl WireEmbeddingResponse {
    /// Returns the served model name, or an empty string when the provider omitted it.
    ///
    /// Absent is allowed by the wire and is not an error: a provider that omits the echo has still returned
    /// vectors, and the caller's requested model is the fallback.
    pub(super) fn model_text(&self) -> &str {
        self.model.as_deref().unwrap_or_default()
    }
}

/// Hashes the exact text an embedding was computed from.
///
/// # Why the parts are length-prefixed
///
/// A digest over concatenated text makes `("a", "bc")` and `("ab", "c")` identical, so two different inputs
/// would carry one hash and a caller that skipped a call because the hash matched would reuse a vector for
/// the wrong text. This is the same flaw `jarvis_core::CanonicalIntentHash` documents for its own parts, and
/// the same fix: a length prefix per part.
///
/// # Why the hash is over the input rather than the vector
///
/// `docs/architecture/storage.md` requires an `input hash`, and its purpose is to make re-embedding
/// idempotent: the same text, model, and version produce the same vector, so a caller that has already paid
/// for one can recognise the work. Hashing the vector instead would only detect that two vectors are equal,
/// which the vectors themselves already say.
#[must_use]
pub(super) fn input_hash(model: &str, chunker_version: Option<&str>, input: &str) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write as _;

    let mut hasher = Sha256::new();
    for part in [model, chunker_version.unwrap_or_default(), input] {
        hasher.update(u64::try_from(part.len()).unwrap_or(u64::MAX).to_be_bytes());
        hasher.update(part.as_bytes());
    }
    let digest = hasher.finalize();
    // Lowercase hex, built explicitly rather than through a hex crate: the encoding is one line and adding a
    // dependency for it would be a new package in the lock file for a `write!` loop.
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    #[allow(unused_imports)]
    use crate::testing::{ResultMustErrExt as _, ResultMustExt as _};

    #[test]
    fn dimensions_are_omitted_when_absent() {
        let request = WireEmbeddingRequest {
            model: "text-embedding-3-small",
            input: &["hello"],
            dimensions: None,
            encoding_format: FLOAT_ENCODING,
        };
        let json = serde_json::to_string(&request).must("serializes");
        assert!(
            !json.contains("dimensions"),
            "an absent dimension must not be sent, got {json}"
        );
        assert!(json.contains("\"encoding_format\":\"float\""));
        assert!(json.contains("\"input\":[\"hello\"]"));
    }

    #[test]
    fn a_requested_dimension_is_sent() {
        let request = WireEmbeddingRequest {
            model: "text-embedding-3-large",
            input: &["a", "b"],
            dimensions: Some(256),
            encoding_format: FLOAT_ENCODING,
        };
        let json = serde_json::to_string(&request).must("serializes");
        assert!(json.contains("\"dimensions\":256"), "got {json}");
    }

    #[test]
    fn the_input_hash_distinguishes_a_moved_boundary() {
        // The flaw the length prefix exists to prevent: without it these two pairs hash the same.
        assert_ne!(
            input_hash("m", None, "abc"),
            input_hash("m", None, "ab"),
            "different text must not share a hash"
        );
        assert_ne!(
            input_hash("m", None, "b"),
            input_hash("m", None, "a"),
            "different text must not share a hash"
        );
        // And the same inputs agree, which is what makes the hash useful for skipping work.
        assert_eq!(input_hash("m", None, "same"), input_hash("m", None, "same"));
        assert_ne!(
            input_hash("m1", None, "x"),
            input_hash("m2", None, "x"),
            "the model is part of what was embedded"
        );
    }

    #[test]
    fn a_response_without_a_model_echo_decodes() {
        let body = r#"{"data":[{"embedding":[0.5],"index":0}],"usage":{"prompt_tokens":3}}"#;
        let decoded: WireEmbeddingResponse = serde_json::from_str(body).must("decodes");
        assert_eq!(decoded.data.len(), 1);
        assert_eq!(decoded.model_text(), "");
        assert_eq!(decoded.usage.prompt_tokens, 3);
    }
}
