//! The fallback gateway asks a second model only for the refusals a second model might not share, and only once.

use std::sync::{Arc, Mutex};

use jarvis_core::CorrelationId;
use jarvis_models::{ChatMessage, Turn, scripted};

use super::*;

/// A scripted model that remembers which model each request asked for.
struct Recording {
    inner: jarvis_models::ScriptedModel,
    asked: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl ModelGateway for Recording {
    fn provider_id(&self) -> &ProviderId {
        self.inner.provider_id()
    }
    async fn capabilities(&self, model: &ModelId) -> Result<ModelCapabilities, ModelError> {
        self.inner.capabilities(model).await
    }
    async fn complete(&self, request: ChatRequest) -> Result<ChatResponse, ModelError> {
        self.asked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(request.model().as_str().to_owned());
        self.inner.complete(request).await
    }
    async fn stream(&self, request: ChatRequest) -> Result<ModelStream, ModelError> {
        self.asked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(request.model().as_str().to_owned());
        self.inner.stream(request).await
    }
    async fn health(&self) -> ProviderHealth {
        self.inner.health().await
    }
}

fn id(name: &str) -> ModelId {
    ModelId::new(name).unwrap_or_else(|error| panic!("{error}"))
}

fn request(model: &str) -> ChatRequest {
    ChatRequest::new(
        id(model),
        vec![ChatMessage::user("hello")],
        CorrelationId::new(),
    )
}

fn gateway(turns: Vec<Turn>) -> (FallbackGateway, Arc<Mutex<Vec<String>>>) {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let inner = Recording {
        inner: scripted("scripted", "primary", turns).unwrap_or_else(|error| panic!("{error}")),
        asked: Arc::clone(&asked),
    };
    (FallbackGateway::new(Box::new(inner), id("backup")), asked)
}

fn asked(log: &Arc<Mutex<Vec<String>>>) -> Vec<String> {
    log.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

#[tokio::test]
async fn a_rate_limited_call_is_asked_again_of_the_fallback_model_once() {
    let (gateway, log) = gateway(vec![
        Turn::Fail(ModelErrorKind::RateLimited),
        Turn::answer("from the backup"),
    ]);
    assert!(gateway.stream(request("primary")).await.is_ok());
    assert_eq!(asked(&log), ["primary", "backup"]);
}

#[tokio::test]
async fn an_overloaded_model_falls_back_too_and_complete_follows_the_same_rule() {
    let (gateway, log) = gateway(vec![
        Turn::Fail(ModelErrorKind::Overloaded),
        Turn::answer("ok"),
    ]);
    assert!(gateway.complete(request("primary")).await.is_ok());
    assert_eq!(asked(&log), ["primary", "backup"]);
}

#[tokio::test]
async fn when_the_fallback_is_limited_too_the_refusal_is_reported_and_nothing_loops() {
    let (gateway, log) = gateway(vec![
        Turn::Fail(ModelErrorKind::RateLimited),
        Turn::Fail(ModelErrorKind::RateLimited),
        Turn::answer("never reached"),
    ]);
    let refused = gateway.stream(request("primary")).await.err();
    assert_eq!(
        refused.map(|error| error.kind()),
        Some(ModelErrorKind::RateLimited)
    );
    assert_eq!(asked(&log).len(), 2, "one try of each, no more");
}

#[tokio::test]
async fn other_refusals_and_a_request_already_for_the_fallback_do_not_fall_back() {
    for kind in [
        ModelErrorKind::Authentication,
        ModelErrorKind::ModelNotFound,
        ModelErrorKind::ContextOverflow,
        ModelErrorKind::ContentRefusal,
        ModelErrorKind::QuotaExhausted,
    ] {
        let (gateway, log) = gateway(vec![Turn::Fail(kind), Turn::answer("no")]);
        let refused = gateway.stream(request("primary")).await.err();
        assert_eq!(refused.map(|error| error.kind()), Some(kind));
        assert_eq!(
            asked(&log),
            ["primary"],
            "{kind:?} is reported, not worked around"
        );
    }
    let (gateway, log) = gateway(vec![
        Turn::Fail(ModelErrorKind::RateLimited),
        Turn::answer("no"),
    ]);
    assert!(gateway.stream(request("backup")).await.is_err());
    assert_eq!(
        asked(&log),
        ["backup"],
        "the fallback is not asked to fall back"
    );
}

#[tokio::test]
async fn a_call_that_works_is_asked_once() {
    let (gateway, log) = gateway(vec![Turn::answer("fine")]);
    assert!(gateway.stream(request("primary")).await.is_ok());
    assert_eq!(asked(&log), ["primary"]);
}
