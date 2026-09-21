//! Offline contract tests for the OpenAI-compatible adapter.
//!
//! Every test here runs against a scripted transport, so the default suite performs
//! no network I/O and no provider call. The tests assert the behaviours that are
//! easy to get wrong and expensive to discover live: retry bounds, `Retry-After`
//! precedence, quota never being retried, truncated streams, and the credential
//! never appearing in any rendered output.
//!
//! A panic inside a test is the intended failure signal, so the workspace
//! `unwrap_used`/`expect_used` denials are relaxed for this file only.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};

use jarvis_core::CorrelationId;
use jarvis_models::openai::{
    ApiKey, BaseUrl, OpenAiCompatibleProvider, ResponseBody, RetryPolicy, Transport,
    TransportError, TransportRequest, TransportResponse,
};
use jarvis_models::{
    ChatMessage, ChatRequest, ChatResponse, FinishReason, ModelErrorKind, ModelGateway, ModelId,
    ProviderId, StreamEvent, StreamValidator,
};

/// One scripted transport reply.
enum Reply {
    /// A non-2xx buffered response.
    Status {
        /// HTTP status code.
        status: u16,
        /// Raw response body.
        body: &'static str,
        /// Optional `Retry-After` value in seconds.
        retry_after: Option<u64>,
    },
    /// A 2xx buffered response.
    Body(&'static str),
    /// A 2xx stream delivering the given chunks.
    Chunks(Vec<&'static [u8]>),
    /// A transport-level failure.
    Failure(TransportError),
}

/// A transport that replays scripted replies and records what it was asked to send.
struct ScriptedTransport {
    replies: Mutex<Vec<Reply>>,
    sent: Mutex<Vec<TransportRequest>>,
}

impl ScriptedTransport {
    fn new(replies: Vec<Reply>) -> Arc<Self> {
        // Reversed so `pop` returns them in the order written.
        let mut replies = replies;
        replies.reverse();
        Arc::new(Self {
            replies: Mutex::new(replies),
            sent: Mutex::new(Vec::new()),
        })
    }

    fn request_count(&self) -> usize {
        self.sent
            .lock()
            .map(|value| value.len())
            .unwrap_or_default()
    }

    fn last_request(&self) -> Option<String> {
        self.sent
            .lock()
            .ok()
            .and_then(|value| value.last().map(|request| request.body().to_owned()))
    }

    fn next_reply(&self) -> Reply {
        self.replies
            .lock()
            .ok()
            .and_then(|mut value| value.pop())
            .unwrap_or(Reply::Failure(TransportError::Connect))
    }
}

/// A body over a fixed list of chunks.
struct ChunkBody {
    chunks: std::collections::VecDeque<Vec<u8>>,
}

impl ResponseBody for ChunkBody {
    fn next_chunk(
        &mut self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Option<Vec<u8>>, TransportError>> + Send + '_>,
    > {
        let next = self.chunks.pop_front();
        Box::pin(async move { Ok(next) })
    }
}

#[async_trait::async_trait]
impl Transport for ScriptedTransport {
    async fn send(
        &self,
        request: &TransportRequest,
        _streaming: bool,
    ) -> Result<TransportResponse, TransportError> {
        if let Ok(mut sent) = self.sent.lock() {
            sent.push(request.clone());
        }

        match self.next_reply() {
            Reply::Status {
                status,
                body,
                retry_after,
            } => Ok(TransportResponse::Buffered {
                status,
                retry_after_seconds: retry_after,
                provider_request_id: Some("req_fixture_1".to_owned()),
                body: body.to_owned(),
            }),
            Reply::Body(body) => Ok(TransportResponse::Buffered {
                status: 200,
                retry_after_seconds: None,
                provider_request_id: Some("req_fixture_1".to_owned()),
                body: body.to_owned(),
            }),
            Reply::Chunks(chunks) => Ok(TransportResponse::Streaming {
                status: 200,
                provider_request_id: Some("req_fixture_1".to_owned()),
                body: Box::new(ChunkBody {
                    chunks: chunks
                        .into_iter()
                        .map(<[u8]>::to_vec)
                        .collect::<std::collections::VecDeque<_>>(),
                }),
            }),
            Reply::Failure(error) => Err(error),
        }
    }
}

/// The canary that must never appear in rendered output.
const CANARY: &str = "sk-canary-9f3b2a7e1d";

fn provider_with(
    transport: Arc<ScriptedTransport>,
    retry: RetryPolicy,
) -> OpenAiCompatibleProvider {
    OpenAiCompatibleProvider::new(
        provider_id("openai-compatible"),
        base_url("https://api.example.invalid/v1"),
        api_key(CANARY),
        transport,
        retry,
    )
}

/// Builds a validated provider identifier, failing loudly on a bad fixture.
fn provider_id(value: &str) -> ProviderId {
    ProviderId::new(value).unwrap_or_else(|error| panic!("valid fixture provider: {error}"))
}

/// Builds a validated base URL, failing loudly on a bad fixture.
fn base_url(value: &str) -> BaseUrl {
    BaseUrl::new(value).unwrap_or_else(|error| panic!("valid fixture base url: {error}"))
}

/// Builds a validated credential, failing loudly on a bad fixture.
fn api_key(value: &str) -> ApiKey {
    ApiKey::new(value).unwrap_or_else(|error| panic!("valid fixture key: {error}"))
}

/// Builds a validated model identifier, failing loudly on a bad fixture.
fn model_id(value: &str) -> ModelId {
    ModelId::new(value).unwrap_or_else(|error| panic!("valid fixture model: {error}"))
}

fn request() -> ChatRequest {
    ChatRequest::new(
        model_id("gpt-oss:20b"),
        vec![ChatMessage::system("be brief"), ChatMessage::user("hello")],
        CorrelationId::new(),
    )
}

#[tokio::test]
async fn a_quota_failure_is_never_retried() {
    // A dead account cannot be fixed by waiting, so a retry loop here would spin
    // against a wall and burn the caller's deadline.
    let body = r#"{"error":{"type":"insufficient_quota","code":"credit_balance_exhausted"}}"#;
    let transport = ScriptedTransport::new(vec![
        Reply::Status {
            status: 429,
            body,
            retry_after: Some(1),
        },
        Reply::Body("{}"),
    ]);
    let provider = provider_with(Arc::clone(&transport), RetryPolicy::new(5));

    let error = provider
        .complete(request())
        .await
        .expect_err("a quota failure must surface as an error");

    assert_eq!(error.kind(), ModelErrorKind::QuotaExhausted);
    assert_eq!(
        transport.request_count(),
        1,
        "quota must be attempted exactly once, not retried"
    );
}

#[tokio::test]
async fn a_rate_limit_is_retried_up_to_the_bound() {
    let body = r#"{"error":{"type":"rate_limit_error","code":"slow_down"}}"#;
    let transport = ScriptedTransport::new(vec![
        Reply::Status {
            status: 429,
            body,
            retry_after: Some(0),
        },
        Reply::Status {
            status: 429,
            body,
            retry_after: Some(0),
        },
        Reply::Body(
            r#"{"model":"gpt-oss:20b","choices":[{"message":{"content":"ok"},"finish_reason":"stop"}]}"#,
        ),
    ]);
    let provider = provider_with(Arc::clone(&transport), RetryPolicy::new(3));

    let response = provider
        .complete(request())
        .await
        .expect("the third attempt succeeds");
    assert_eq!(response.text(), "ok");
    assert_eq!(transport.request_count(), 3);

    // The bound must actually bind: a fourth failure is not attempted.
    let transport = ScriptedTransport::new(vec![
        Reply::Status {
            status: 503,
            body: "",
            retry_after: Some(0),
        },
        Reply::Status {
            status: 503,
            body: "",
            retry_after: Some(0),
        },
        Reply::Status {
            status: 503,
            body: "",
            retry_after: Some(0),
        },
        Reply::Status {
            status: 503,
            body: "",
            retry_after: Some(0),
        },
    ]);
    let provider = provider_with(Arc::clone(&transport), RetryPolicy::new(2));
    assert!(provider.complete(request()).await.is_err());
    assert_eq!(
        transport.request_count(),
        2,
        "attempts must stop at the configured ceiling"
    );
}

#[tokio::test]
async fn the_api_key_reaches_the_header_and_no_rendered_output() {
    let transport = ScriptedTransport::new(vec![Reply::Status {
        status: 401,
        body: r#"{"error":{"type":"invalid_request_error","code":"invalid_api_key"}}"#,
        retry_after: None,
    }]);
    let provider = provider_with(Arc::clone(&transport), RetryPolicy::none());

    let error = provider
        .complete(request())
        .await
        .expect_err("a 401 is an authentication failure");
    assert_eq!(error.kind(), ModelErrorKind::Authentication);

    // The credential must be present where it belongs.
    let headers = transport
        .sent
        .lock()
        .ok()
        .and_then(|value| {
            value
                .first()
                .map(|request| request.headers().authorization().map(str::to_owned))
        })
        .flatten()
        .unwrap_or_default();
    assert!(
        headers.contains(CANARY),
        "the credential must reach the authorization header"
    );

    // And absent from every diagnostic rendering.
    let rendered = format!(
        "{provider:?} {error:?} {error} {}",
        transport.last_request().unwrap_or_default()
    );
    assert!(
        !rendered.contains(CANARY),
        "the credential leaked into rendered output: {rendered}"
    );
}

#[tokio::test]
async fn a_truncated_stream_is_reported_as_incomplete_not_as_a_finished_answer() {
    // The socket closes after text but before any finish reason. Presenting that as a
    // complete answer is the exact failure this contract exists to prevent.
    let transport = ScriptedTransport::new(vec![Reply::Chunks(vec![
        b"data: {\"choices\":[{\"delta\":{\"content\":\"half an answ\"}}]}\n\n",
    ])]);
    let provider = provider_with(Arc::clone(&transport), RetryPolicy::none());

    let mut stream = provider
        .stream(request())
        .await
        .expect("the stream starts successfully");

    let mut validator = StreamValidator::new();
    let mut saw_error = None;
    while let Some(item) = futures_util::StreamExt::next(&mut stream).await {
        match item {
            Ok(envelope) => {
                validator.accept(envelope).expect("events must be ordered");
            }
            Err(error) => {
                saw_error = Some(error);
                break;
            }
        }
    }

    let error = saw_error.expect("a truncated stream must surface an error");
    assert_eq!(error.kind(), ModelErrorKind::Incomplete);
    assert!(
        validator.finish().is_err(),
        "an unterminated stream must not be finalized as complete"
    );
}

#[tokio::test]
async fn a_complete_stream_yields_ordered_events_and_a_terminal_reason() {
    let transport = ScriptedTransport::new(vec![Reply::Chunks(vec![
        b"data: {\"choices\":[{\"delta\":{\"content\":\"hello \"}}]}\n\n",
        b"data: {\"choices\":[{\"delta\":{\"content\":\"world\"}}]}\n\n",
        b"data: {\"choices\":[],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":2}}\n\n",
        b"data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
        b"data: [DONE]\n\n",
    ])]);
    let provider = provider_with(Arc::clone(&transport), RetryPolicy::none());

    let mut stream = provider.stream(request()).await.expect("stream starts");
    let mut validator = StreamValidator::new();
    while let Some(item) = futures_util::StreamExt::next(&mut stream).await {
        let envelope = item.expect("no error in a healthy stream");
        validator.accept(envelope).expect("events are ordered");
    }

    let summary = validator.finish().expect("the stream completed");
    assert_eq!(summary.text(), "hello world");
    assert_eq!(summary.finish_reason(), Some(FinishReason::Stop));
    assert_eq!(
        summary.usage().map(jarvis_models::TokenUsage::input_tokens),
        Some(5)
    );
}

#[tokio::test]
async fn a_non_streaming_response_maps_model_usage_and_finish_reason() {
    let transport = ScriptedTransport::new(vec![Reply::Body(
        r#"{"model":"served-alias-2","choices":[{"message":{"content":"hi"},"finish_reason":"length"}],
            "usage":{"prompt_tokens":10,"completion_tokens":3}}"#,
    )]);
    let provider = provider_with(Arc::clone(&transport), RetryPolicy::none());

    let response: ChatResponse = provider
        .complete(request())
        .await
        .expect("response decodes");
    assert_eq!(response.text(), "hi");
    assert_eq!(response.finish_reason(), FinishReason::Length);
    assert_eq!(
        response.model().as_str(),
        "served-alias-2",
        "the served model must be recorded, not the requested alias"
    );
    assert_eq!(response.usage().map(|usage| usage.input_tokens()), Some(10));
}

#[tokio::test]
async fn the_instruction_message_is_sent_with_the_interoperable_role() {
    let transport = ScriptedTransport::new(vec![Reply::Body(r#"{"choices":[]}"#)]);
    let provider = provider_with(Arc::clone(&transport), RetryPolicy::none());
    let _ = provider.complete(request()).await;

    let body = transport.last_request().unwrap_or_default();
    assert!(
        body.contains("\"role\":\"system\""),
        "the interoperable system role must be used: {body}"
    );
    assert!(!body.contains("developer"));
}

#[tokio::test]
async fn a_connection_failure_is_retried_as_transient() {
    let transport = ScriptedTransport::new(vec![
        Reply::Failure(TransportError::Connect),
        Reply::Body(r#"{"choices":[{"message":{"content":"recovered"},"finish_reason":"stop"}]}"#),
    ]);
    let provider = provider_with(Arc::clone(&transport), RetryPolicy::new(3));

    let response = provider.complete(request()).await.expect("retry succeeds");
    assert_eq!(response.text(), "recovered");
    assert_eq!(transport.request_count(), 2);
}

#[tokio::test]
async fn an_empty_choice_list_does_not_claim_a_stopped_finish_reason() {
    // A response with no choices has no answer. Defaulting to `stop` would present an
    // empty reply as a completed one.
    let transport = ScriptedTransport::new(vec![Reply::Body(r#"{"choices":[]}"#)]);
    let provider = provider_with(Arc::clone(&transport), RetryPolicy::none());

    let response = provider.complete(request()).await.expect("decodes");
    assert_eq!(response.finish_reason(), FinishReason::Incomplete);
    assert!(response.text().is_empty());
}

#[tokio::test]
async fn a_stream_event_carries_its_sequence_number() {
    let transport = ScriptedTransport::new(vec![Reply::Chunks(vec![
        b"data: {\"choices\":[{\"delta\":{\"content\":\"a\"}}]}\n\n",
        b"data: {\"choices\":[{\"delta\":{\"content\":\"b\"}}]}\n\n",
        b"data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
    ])]);
    let provider = provider_with(Arc::clone(&transport), RetryPolicy::none());
    let mut stream = provider.stream(request()).await.expect("stream starts");

    let mut sequences = Vec::new();
    while let Some(item) = futures_util::StreamExt::next(&mut stream).await {
        sequences.push(item.expect("healthy stream").sequence());
    }

    assert_eq!(
        sequences,
        vec![0, 1, 2],
        "sequence numbers must be contiguous from zero so a consumer can detect a gap"
    );
}

#[tokio::test]
async fn a_health_probe_does_not_perform_a_completion_call() {
    // A health check must not bill the account.
    let transport = ScriptedTransport::new(vec![Reply::Body(r#"{"data":[]}"#)]);
    let provider = provider_with(Arc::clone(&transport), RetryPolicy::none());

    let health = provider.health().await;
    assert_eq!(health.status(), jarvis_models::ProviderStatus::Ready);

    let method = transport
        .sent
        .lock()
        .ok()
        .and_then(|value| value.first().map(TransportRequest::method))
        .unwrap_or("POST");
    assert_eq!(
        method, "GET",
        "a probe must not be a billable completion request"
    );
}

#[tokio::test]
async fn placement_is_local_only_for_a_loopback_authority() {
    let transport = ScriptedTransport::new(vec![Reply::Body("{}")]);
    let local = OpenAiCompatibleProvider::new(
        provider_id("ollama"),
        base_url("http://127.0.0.1:11434/v1"),
        api_key("ollama"),
        transport,
        RetryPolicy::none(),
    );
    let capabilities = local
        .capabilities(&model_id("llama3.2"))
        .await
        .expect("capabilities resolve");
    assert_eq!(capabilities.placement(), jarvis_models::Placement::Local);
}

#[tokio::test]
async fn a_tool_call_fragment_is_not_reported_as_a_complete_invocation() {
    // Streaming tool calls arrive in fragments; reporting one early would ask JARVIS
    // to authorize a call whose arguments are still incomplete.
    let transport = ScriptedTransport::new(vec![Reply::Chunks(vec![
        b"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"search\",\"arguments\":\"{\\\"q\\\":\"}}]}}]}\n\n",
        b"data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
    ])]);
    let provider = provider_with(Arc::clone(&transport), RetryPolicy::none());
    let mut stream = provider.stream(request()).await.expect("stream starts");

    let mut validator = StreamValidator::new();
    while let Some(item) = futures_util::StreamExt::next(&mut stream).await {
        validator
            .accept(item.expect("healthy stream"))
            .expect("ordered");
    }
    let summary = validator.finish().expect("stream finished");
    assert!(
        summary.tool_calls().is_empty(),
        "an incomplete tool call must not be surfaced"
    );
    assert_eq!(summary.finish_reason(), Some(FinishReason::ToolCalls));
}

#[test]
fn the_adapter_debug_output_omits_the_credential_and_the_transport() {
    let transport = ScriptedTransport::new(vec![]);
    let provider = provider_with(transport, RetryPolicy::none());
    let rendered = format!("{provider:?}");
    assert!(!rendered.contains(CANARY));
    assert!(
        rendered.contains(".."),
        "the renderer must mark itself non-exhaustive: {rendered}"
    );
}

#[test]
fn stream_events_name_themselves_stably() {
    assert_eq!(StreamEvent::Started.name(), "started");
    assert_eq!(
        StreamEvent::TextDelta {
            text: String::new()
        }
        .name(),
        "text_delta"
    );
    assert_eq!(
        StreamEvent::Finished {
            reason: FinishReason::Stop
        }
        .name(),
        "finished"
    );
}

#[test]
fn a_quota_error_display_never_contains_provider_text() {
    // The provider's own message can echo the prompt, so only JARVIS text is carried.
    let error = jarvis_models::ModelError::from_static(
        ModelErrorKind::QuotaExhausted,
        "the provider account has no remaining quota",
    );
    let rendered = format!("{error}");
    assert!(!rendered.contains(CANARY));
    assert!(rendered.contains("quota"));
}
