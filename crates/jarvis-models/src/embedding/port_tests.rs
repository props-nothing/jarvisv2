//! Tests for the embedding port and its OpenAI-compatible adapter.
//!
//! # Why a scripted transport rather than a mock provider
//!
//! The tests drive the real adapter through the same [`Transport`] seam the chat adapter uses, so the request
//! body that goes out and the response parsing that comes back are the production paths. A mock that returned
//! a typed response would test the port's own arithmetic and nothing about the wire — and the wire is where
//! the provider's `index` field, its optional model echo, and its error envelope live.

use std::sync::{Arc, Mutex};

use super::*;
use crate::ModelErrorKind;
use crate::identity::ProviderId;
use crate::openai::{
    ApiKey, BaseUrl, OpenAiCompatibleEmbeddingProvider, Transport, TransportError,
    TransportRequest, TransportResponse,
};
#[allow(unused_imports)]
use crate::testing::{ResultMustErrExt as _, ResultMustExt as _};
use crate::vector::{EmbeddingDimensions, Normalization};

/// A transport that records the requests it was given and replays one buffered response.
///
/// Only a buffered response is modelled, deliberately: the embedding call is never sent with
/// `streaming: true`, so a streaming response is unreachable from this adapter and a test double that could
/// produce one would suggest otherwise.
struct ScriptedTransport {
    status: u16,
    body: String,
    seen: Mutex<Vec<String>>,
    seen_streaming: Mutex<Vec<bool>>,
}

impl ScriptedTransport {
    fn answering(status: u16, body: &str) -> Self {
        Self {
            status,
            body: body.to_owned(),
            seen: Mutex::new(Vec::new()),
            seen_streaming: Mutex::new(Vec::new()),
        }
    }

    /// Returns the bodies of the requests the adapter sent.
    fn bodies(&self) -> Vec<String> {
        self.seen.lock().must("not poisoned").clone()
    }

    /// Returns whether each request was sent as a streaming one.
    fn streaming_flags(&self) -> Vec<bool> {
        self.seen_streaming.lock().must("not poisoned").clone()
    }
}

#[async_trait::async_trait]
impl Transport for ScriptedTransport {
    async fn send(
        &self,
        request: &TransportRequest,
        streaming: bool,
    ) -> Result<TransportResponse, TransportError> {
        self.seen
            .lock()
            .must("not poisoned")
            .push(request.body().to_owned());
        self.seen_streaming
            .lock()
            .must("not poisoned")
            .push(streaming);
        Ok(TransportResponse::Buffered {
            status: self.status,
            retry_after_seconds: None,
            provider_request_id: Some("req-123".to_owned()),
            body: self.body.clone(),
        })
    }
}

fn adapter(transport: Arc<dyn Transport>) -> OpenAiCompatibleEmbeddingProvider {
    OpenAiCompatibleEmbeddingProvider::new(
        ProviderId::new("openai-compatible").must("valid provider"),
        BaseUrl::new("https://api.example.invalid/v1").must("valid base url"),
        ApiKey::new("test-key").must("valid key"),
        transport,
    )
}

fn model() -> crate::identity::ModelId {
    crate::identity::ModelId::new("text-embedding-3-small").must("valid model")
}

fn request(inputs: &[&str]) -> EmbeddingRequest {
    EmbeddingRequest::new(
        model(),
        inputs.iter().map(|value| (*value).to_owned()).collect(),
    )
    .must("valid request")
}

/// A response body carrying one vector per input, at the given dimension.
fn body(vectors: usize, dimension: usize) -> String {
    let data: Vec<String> = (0..vectors)
        .map(|index| {
            let values: Vec<String> = (0..dimension).map(|_| "0.25".to_owned()).collect();
            format!(
                r#"{{"embedding":[{}],"index":{index},"object":"embedding"}}"#,
                values.join(",")
            )
        })
        .collect();
    format!(
        r#"{{"object":"list","data":[{}],"model":"text-embedding-3-small","usage":{{"prompt_tokens":7,"total_tokens":7}}}}"#,
        data.join(",")
    )
}

// ------------------------------------------------------------------------------------------------
// The request
// ------------------------------------------------------------------------------------------------

/// **Local shape rules refuse before a request is spent.**
///
/// The provider refuses an empty input and bounds a batch at 2048, and each of those refusals costs a round
/// trip. The message names the *index* of an offending input, because a batch of a thousand texts makes "one
/// of these is empty" unactionable — and it names the index rather than the text, because the text is user
/// content that would reach a log.
#[test]
fn a_malformed_request_is_refused_locally() {
    let empty_batch = EmbeddingRequest::new(model(), Vec::new());
    assert_eq!(
        empty_batch.must_err("refused").kind(),
        ModelErrorKind::InvalidRequest
    );

    let too_many = EmbeddingRequest::new(
        model(),
        (0..=MAX_EMBEDDING_INPUTS)
            .map(|index| format!("text {index}"))
            .collect(),
    );
    assert_eq!(
        too_many.must_err("refused").kind(),
        ModelErrorKind::InvalidRequest
    );

    let blank = EmbeddingRequest::new(model(), vec!["fine".to_owned(), "   ".to_owned()]);
    let error = blank.must_err("refused");
    assert_eq!(error.kind(), ModelErrorKind::InvalidRequest);
    assert!(
        error.to_string().contains("position 1"),
        "the message must name the index, got {error}"
    );
    assert!(
        !error.to_string().contains("fine"),
        "the message must not carry the input text, got {error}"
    );

    let oversized = EmbeddingRequest::new(model(), vec!["a".repeat(MAX_EMBEDDING_INPUT_BYTES + 1)]);
    assert_eq!(
        oversized.must_err("refused").kind(),
        ModelErrorKind::InvalidRequest
    );

    // The control: a well-formed batch is accepted, so the refusals above are about the values.
    assert!(EmbeddingRequest::new(model(), vec!["ok".to_owned()]).is_ok());
}

/// **The request body is the documented shape, and `dimensions` is omitted when not requested.**
///
/// The field is omitted rather than sent as `null` because the provider does not support it before
/// `text-embedding-3`, and a provider that rejected an unknown field would refuse every request carrying a
/// default. Asserted on the *sent body* rather than on the wire struct, so the serialization is what is
/// checked.
#[tokio::test]
async fn the_request_body_is_the_documented_shape() {
    let transport = Arc::new(ScriptedTransport::answering(200, &body(1, 4)));
    let provider = adapter(transport.clone());

    provider.embed(request(&["hello"])).await.must("succeeds");

    let bodies = transport.bodies();
    assert_eq!(bodies.len(), 1);
    let sent = &bodies[0];
    assert!(sent.contains(r#""input":["hello"]"#), "got {sent}");
    assert!(sent.contains(r#""encoding_format":"float""#), "got {sent}");
    assert!(
        sent.contains(r#""model":"text-embedding-3-small""#),
        "got {sent}"
    );
    assert!(
        !sent.contains("dimensions"),
        "an unrequested dimension must be omitted, got {sent}"
    );
    // The call is never streaming, so a streaming response is an adapter invariant failure rather than
    // something a provider could send.
    assert_eq!(transport.streaming_flags(), vec![false]);
}

/// **A requested dimension is sent, and it is the dimension the metadata declares.**
///
/// Sending it without storing it would let a shortened vector compare against a full-length one. The assertion
/// is on both: the body the provider received and the metadata of the vector that came back.
#[tokio::test]
async fn a_requested_dimension_is_sent_and_stored() {
    let transport = Arc::new(ScriptedTransport::answering(200, &body(2, 256)));
    let provider = adapter(transport.clone());
    let dimensions = EmbeddingDimensions::new(256).must("valid dimensions");

    let response = provider
        .embed(request(&["a", "b"]).with_dimensions(dimensions))
        .await
        .must("succeeds");

    assert!(
        transport.bodies()[0].contains(r#""dimensions":256"#),
        "got {}",
        transport.bodies()[0]
    );
    for embedding in response.embeddings() {
        assert_eq!(embedding.metadata().dimensions, dimensions);
        assert_eq!(embedding.vector().len(), 256);
    }
}

// ------------------------------------------------------------------------------------------------
// The response
// ------------------------------------------------------------------------------------------------

/// **Vectors are returned in the request's order, reassembled by the provider's `index`.**
///
/// The wire defines `index` as "the index of the embedding in the list of embeddings", so a response that
/// arrived out of order must still be paired with the right input. The fixture deliberately returns them
/// reversed, because a positional implementation produces the same answer for an in-order response and is
/// therefore untestable against one.
#[tokio::test]
async fn vectors_are_reassembled_by_the_provider_index_not_by_position() {
    let transport = Arc::new(ScriptedTransport::answering(
        200,
        r#"{"object":"list","data":[
            {"embedding":[3.0,3.0,3.0,3.0],"index":2,"object":"embedding"},
            {"embedding":[2.0,2.0,2.0,2.0],"index":1,"object":"embedding"},
            {"embedding":[1.0,1.0,1.0,1.0],"index":0,"object":"embedding"}
        ],"model":"text-embedding-3-small","usage":{"prompt_tokens":9,"total_tokens":9}}"#,
    ));
    let provider = adapter(transport);

    let response = provider
        .embed(request(&["first", "second", "third"]))
        .await
        .must("succeeds");

    assert_eq!(response.embeddings().len(), 3);
    // The input at position 0 was embedded as the vector whose `index` is 0, which the fixture sent last.
    for (position, embedding) in response.embeddings().iter().enumerate() {
        let expected = f32::from(u8::try_from(position + 1).must("small"));
        assert_eq!(
            embedding.vector().values(),
            &[expected, expected, expected, expected],
            "position {position} must receive the vector whose index is {position}"
        );
    }
    assert_eq!(response.usage().input_tokens(), 9);
    assert_eq!(response.served_model().as_str(), "text-embedding-3-small");
}

/// **A response with fewer vectors than inputs is refused rather than silently short.**
///
/// The property that keeps pairing safe: if the provider omitted an index, the returned list cannot be
/// aligned to the inputs, and a caller indexing by position would attribute a vector to the wrong text.
#[tokio::test]
async fn a_response_missing_a_vector_is_refused() {
    let transport = Arc::new(ScriptedTransport::answering(
        200,
        r#"{"object":"list","data":[{"embedding":[1.0,1.0,1.0,1.0],"index":0,"object":"embedding"}],
            "model":"text-embedding-3-small","usage":{"prompt_tokens":3,"total_tokens":3}}"#,
    ));
    let provider = adapter(transport);

    let error = provider
        .embed(request(&["first", "second"]))
        .await
        .must_err("refused");
    assert_eq!(error.kind(), ModelErrorKind::MalformedResponse);
}

/// **An `index` outside the request is refused.**
///
/// A provider returning `index: 5` for a two-input request is either a fault or a different request's
/// response; either way the vectors cannot be attributed, and writing one into a slot by `get_mut` returning
/// `None` is what surfaces it rather than a panic or a silent drop.
#[tokio::test]
async fn an_index_outside_the_request_is_refused() {
    let transport = Arc::new(ScriptedTransport::answering(
        200,
        r#"{"object":"list","data":[{"embedding":[1.0,1.0,1.0,1.0],"index":5,"object":"embedding"}],
            "model":"text-embedding-3-small","usage":{"prompt_tokens":3,"total_tokens":3}}"#,
    ));
    let provider = adapter(transport);

    let error = provider.embed(request(&["only"])).await.must_err("refused");
    assert_eq!(error.kind(), ModelErrorKind::MalformedResponse);
}

/// **Two vectors claiming the same input is refused, not resolved by taking the later one.**
///
/// The same defect class as pairing positionally: a provider that sends two vectors for one input has not
/// answered the question that was asked, and a "last one wins" rule would silently pick one. Which of the two
/// is the answer for that input is not knowable here.
#[tokio::test]
async fn a_duplicated_index_is_refused_rather_than_overwritten() {
    let transport = Arc::new(ScriptedTransport::answering(
        200,
        r#"{"object":"list","data":[
            {"embedding":[1.0,1.0,1.0,1.0],"index":0,"object":"embedding"},
            {"embedding":[2.0,2.0,2.0,2.0],"index":0,"object":"embedding"}
        ],"model":"text-embedding-3-small","usage":{"prompt_tokens":4,"total_tokens":4}}"#,
    ));
    let provider = adapter(transport);

    let error = provider.embed(request(&["only"])).await.must_err("refused");
    assert_eq!(error.kind(), ModelErrorKind::MalformedResponse);
}

/// **A vector whose length is not the declared dimension is refused at the adapter.**
///
/// The check that makes the metadata guard meaningful, exercised through the provider path: the fixture
/// declares a 4-dimension request and returns 2-component vectors, so the pair cannot be stored with metadata
/// describing a shape the provider did not return.
#[tokio::test]
async fn a_vector_of_the_wrong_length_is_refused() {
    let transport = Arc::new(ScriptedTransport::answering(200, &body(1, 2)));
    let provider = adapter(transport);
    let dimensions = EmbeddingDimensions::new(4).must("valid dimensions");

    let error = provider
        .embed(request(&["a"]).with_dimensions(dimensions))
        .await
        .must_err("refused");
    assert_eq!(error.kind(), ModelErrorKind::MalformedResponse);
}

/// **A non-2xx response is classified, and the provider's own message is not carried.**
///
/// The error envelope is the same shape the chat adapter reads, and the classification is the same provider
/// fact. The provider's text is deliberately not carried, because it can echo request content.
#[tokio::test]
async fn an_error_response_is_classified_without_its_text() {
    let transport = Arc::new(ScriptedTransport::answering(
        401,
        r#"{"error":{"message":"Incorrect API key provided: sk-abc","type":"invalid_request_error","code":"invalid_api_key"}}"#,
    ));
    let provider = adapter(transport);

    let error = provider.embed(request(&["a"])).await.must_err("refused");
    assert_eq!(error.kind(), ModelErrorKind::Authentication);
    assert_eq!(error.provider_status(), Some(401));
    assert!(
        !error.to_string().contains("sk-abc"),
        "the provider's message must not be carried, got {error}"
    );

    // A malformed body is still an authentication failure, because the status is the evidence when the body
    // cannot be read.
    let transport = Arc::new(ScriptedTransport::answering(401, "not json"));
    let provider = adapter(transport);
    assert_eq!(
        provider
            .embed(request(&["a"]))
            .await
            .must_err("refused")
            .kind(),
        ModelErrorKind::Authentication
    );
}

/// **The metadata the adapter stores names the requested model and the provider's normalization.**
///
/// The mapping `docs/architecture/storage.md` requires, asserted through the adapter so the value a caller
/// receives is the one described. `Normalization::Normalized` is the researched provider's documented
/// property, and it is set by the adapter rather than assumed by the port.
#[tokio::test]
async fn the_adapter_records_the_provider_normalization() {
    let transport = Arc::new(ScriptedTransport::answering(200, &body(1, 4)));
    let provider = adapter(transport);

    let response = provider.embed(request(&["hello"])).await.must("succeeds");
    let metadata = response.embeddings()[0].metadata();

    assert_eq!(metadata.provider.as_str(), "openai-compatible");
    assert_eq!(metadata.model.as_str(), "text-embedding-3-small");
    assert_eq!(metadata.normalization, Normalization::Normalized);
    assert_eq!(metadata.dimensions.get(), 4);
    assert_eq!(
        metadata.input_hash.len(),
        64,
        "a SHA-256 digest is 64 hex chars"
    );
    assert!(
        metadata
            .input_hash
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit()),
        "the hash must be hex, got {}",
        metadata.input_hash
    );
    assert!(
        metadata.version.is_none(),
        "the researched provider states no version"
    );
    assert!(metadata.chunker_version.is_none());
}

/// **Two different inputs get different hashes, and the same input gets the same one.**
///
/// The property that makes re-embedding idempotent: a caller can skip a call whose hash it already holds, so
/// two different texts must never share one. The fixture embeds two texts and compares their metadata.
#[tokio::test]
async fn different_inputs_do_not_share_an_input_hash() {
    let transport = Arc::new(ScriptedTransport::answering(200, &body(2, 4)));
    let provider = adapter(transport);

    let response = provider
        .embed(request(&["first text", "second text"]))
        .await
        .must("succeeds");
    let hashes: Vec<&str> = response
        .embeddings()
        .iter()
        .map(|embedding| embedding.metadata().input_hash.as_str())
        .collect();
    assert_ne!(
        hashes[0], hashes[1],
        "different inputs must not share a hash"
    );

    // And the same input in a fresh response produces the same hash, which is what makes it usable as a key.
    let transport = Arc::new(ScriptedTransport::answering(200, &body(1, 4)));
    let provider = adapter(transport);
    let again = provider
        .embed(request(&["first text"]))
        .await
        .must("succeeds");
    assert_eq!(again.embeddings()[0].metadata().input_hash, hashes[0]);
}

/// **A response whose model echo is absent still succeeds, and the requested model is recorded.**
///
/// The wire allows the echo to be absent, and a provider that omits it has still returned vectors. The
/// metadata then names the requested model, which is the only model the caller asked for.
#[tokio::test]
async fn a_response_without_a_model_echo_still_succeeds() {
    let transport = Arc::new(ScriptedTransport::answering(
        200,
        r#"{"object":"list","data":[{"embedding":[0.5,0.5,0.5,0.5],"index":0,"object":"embedding"}],
            "usage":{"prompt_tokens":2,"total_tokens":2}}"#,
    ));
    let provider = adapter(transport);

    let response = provider.embed(request(&["a"])).await.must("succeeds");
    assert_eq!(response.served_model().as_str(), "text-embedding-3-small");
    assert_eq!(
        response.embeddings()[0].metadata().model.as_str(),
        "text-embedding-3-small"
    );
}

/// **A body that is not the expected shape is a malformed response, not a panic.**
///
/// A provider returning HTML from a proxy, or a truncated body, must be a classified failure rather than a
/// deserialization panic inside a request handler.
#[tokio::test]
async fn an_uninterpretable_body_is_a_malformed_response() {
    for body in ["", "not json", r#"{"object":"list"}"#, r#"{"data":5}"#] {
        let transport = Arc::new(ScriptedTransport::answering(200, body));
        let provider = adapter(transport);
        let error = provider.embed(request(&["a"])).await.must_err("refused");
        assert_eq!(
            error.kind(),
            ModelErrorKind::MalformedResponse,
            "body {body:?} must be a malformed response"
        );
    }
}
