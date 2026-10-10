//! Looking at a provider: the key goes only where it was stored for, the list is read, the test says what went wrong, and nothing leaks.

use std::sync::Mutex;

use jarvis_models::openai::{Transport, TransportError, TransportRequest, TransportResponse};

use super::*;

/// A transport that answers each request in turn and remembers where each went and with what credential.
struct Scripted {
    replies: Mutex<Vec<Result<(u16, &'static str), TransportError>>>,
    seen: Mutex<Vec<(String, Option<String>)>>,
}

impl Scripted {
    fn new(mut replies: Vec<Result<(u16, &'static str), TransportError>>) -> Arc<Self> {
        replies.reverse();
        Arc::new(Self {
            replies: Mutex::new(replies),
            seen: Mutex::new(Vec::new()),
        })
    }

    fn seen(&self) -> Vec<(String, Option<String>)> {
        self.seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

#[async_trait::async_trait]
impl Transport for Scripted {
    async fn send(
        &self,
        request: &TransportRequest,
        _streaming: bool,
    ) -> Result<TransportResponse, TransportError> {
        self.seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((
                request.url().to_owned(),
                request.headers().authorization().map(str::to_owned),
            ));
        let next = self
            .replies
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop()
            .unwrap_or(Err(TransportError::Connect));
        next.map(|(status, body)| TransportResponse::Buffered {
            status,
            retry_after_seconds: None,
            provider_request_id: None,
            body: body.to_owned(),
        })
    }
}

fn base(url: &str) -> BaseUrl {
    BaseUrl::new(url).unwrap_or_else(|error| panic!("{error}"))
}

const LIST: &str = r#"{"data":[{"id":"text-embedding-3-small"},{"id":"gpt-4.1"},{"id":"whisper-1"},{"id":"gpt-4.1-mini"}]}"#;
const ANSWER: &str = r#"{"model":"gpt-4.1","choices":[{"index":0,"message":{"role":"assistant","content":"OK"},"finish_reason":"stop"}]}"#;

#[test]
fn a_stored_key_is_only_ever_used_for_the_address_it_was_stored_for() {
    let stored = || Some("sk-stored-secret".to_owned());
    // The configured address: the stored key is used, with or without a trailing slash.
    for given in [
        None,
        Some("https://api.example.com/v1"),
        Some("https://api.example.com/v1/"),
    ] {
        let (_, key) = resolve(Some("https://api.example.com/v1"), given, None, stored)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(key, "sk-stored-secret");
    }
    // Falsifies the guard: an address that is not the stored one never receives the stored key, even though one exists.
    for other in [
        "https://evil.example.net/v1",
        "https://api.example.com.evil.net/v1",
        "https://api.example.com/v2",
    ] {
        let refused = resolve(
            Some("https://api.example.com/v1"),
            Some(other),
            None,
            stored,
        );
        assert_eq!(
            refused.err().as_deref(),
            Some("Paste the key for this address first."),
            "{other}"
        );
    }
    // A key typed in the request is used for whatever address came with it.
    let (_, key) = resolve(
        Some("https://api.example.com/v1"),
        Some("https://other.example.org/v1"),
        Some("sk-typed"),
        stored,
    )
    .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(key, "sk-typed");
}

#[test]
fn addresses_and_keys_that_cannot_work_are_refused_in_words() {
    let none = || None;
    assert!(
        resolve(None, None, None, none).is_err(),
        "no address at all"
    );
    for bad in [
        "ftp://x.example/v1",
        "not a url",
        "https://user:pw@x.example/v1",
    ] {
        let refused = resolve(None, Some(bad), Some("sk-x"), none);
        assert!(
            refused
                .err()
                .is_some_and(|message| message.contains("usable address")),
            "{bad}"
        );
    }
    assert!(resolve(None, Some("https://x.example/v1"), Some("has space"), none).is_err());
    // A server on this machine needs no key.
    let (_, key) = resolve(None, Some("http://localhost:11434/v1"), None, none)
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(key, "ollama");
    // A remote server does.
    assert!(resolve(None, Some("https://api.example.com/v1"), None, none).is_err());
}

#[tokio::test]
async fn the_list_comes_back_with_the_likely_chat_models_first_and_the_key_goes_to_the_models_path()
{
    let transport = Scripted::new(vec![Ok((200, LIST))]);
    let report = run(
        Arc::clone(&transport) as Arc<dyn Transport>,
        base("https://api.example.com/v1"),
        "sk-secret-canary",
        None,
    )
    .await
    .unwrap_or_else(|error| panic!("{error}"));
    let ids: Vec<&str> = report
        .models
        .iter()
        .map(|model| model.id.as_str())
        .collect();
    assert_eq!(
        ids,
        [
            "gpt-4.1",
            "gpt-4.1-mini",
            "text-embedding-3-small",
            "whisper-1"
        ]
    );
    assert!(report.models[0].likely_chat && !report.models[3].likely_chat);
    assert_eq!(report.list_problem, None);
    assert!(report.test.is_none());
    let seen = transport.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].0, "https://api.example.com/v1/models");
    assert_eq!(seen[0].1.as_deref(), Some("Bearer sk-secret-canary"));
}

#[tokio::test]
async fn a_provider_that_will_not_list_still_gets_tested_and_every_failure_is_explained() {
    // The list is refused (some providers do not list), the test call works.
    let transport = Scripted::new(vec![Ok((404, "{}")), Ok((200, ANSWER))]);
    let report = run(
        transport as Arc<dyn Transport>,
        base("https://api.example.com/v1"),
        "sk-x",
        Some("gpt-4.1"),
    )
    .await
    .unwrap_or_else(|error| panic!("{error}"));
    assert!(report.models.is_empty() && report.list_problem.is_some());
    assert_eq!(report.test.as_ref().map(|test| test.ok), Some(true));

    for (reply, expected) in [
        (
            Ok((401, r#"{"error":{"message":"bad key sk-secret-canary"}}"#)),
            "rejected the key",
        ),
        (
            Ok((404, r#"{"error":{"code":"model_not_found"}}"#)),
            "does not serve that model",
        ),
        (Ok((200, "<html>")), "not like an OpenAI-style"),
        (Err(TransportError::Connect), "could not be reached"),
    ] {
        let transport = Scripted::new(vec![reply, reply]);
        let report = run(
            transport as Arc<dyn Transport>,
            base("https://api.example.com/v1"),
            "sk-secret-canary",
            Some("gpt-4.1"),
        )
        .await
        .unwrap_or_else(|error| panic!("{error}"));
        let test = report
            .test
            .unwrap_or_else(|| panic!("a test was asked for"));
        assert!(!test.ok);
        assert!(test.message.contains(expected), "{}", test.message);
        let everything = format!("{} {:?}", test.message, report.list_problem);
        assert!(!everything.contains("sk-secret-canary"), "{everything}");
    }
}

#[tokio::test]
async fn chat_models_come_first_then_the_owners_fine_tunes_then_what_is_probably_not_for_chat() {
    let transport = Scripted::new(vec![Ok((
        200,
        r#"{"data":[{"id":"sora-2"},{"id":"ft:gpt-4.1-nano:me:x:1"},{"id":"davinci-002"},{"id":"gpt-5"},{"id":"gpt-5-codex"},{"id":"deepseek-v4-pro:0813"},{"id":"llama3-instruct"},{"id":"gpt-4o-mini-tts"},{"id":"o3"}]}"#,
    ))]);
    let report = run(
        transport as Arc<dyn Transport>,
        base("https://api.example.com/v1"),
        "sk-x",
        None,
    )
    .await
    .unwrap_or_else(|error| panic!("{error}"));
    let order: Vec<(&str, bool)> = report
        .models
        .iter()
        .map(|model| (model.id.as_str(), model.likely_chat))
        .collect();
    assert_eq!(
        order,
        [
            ("deepseek-v4-pro:0813", true),
            ("gpt-5", true),
            ("llama3-instruct", true),
            ("o3", true),
            ("ft:gpt-4.1-nano:me:x:1", true),
            ("davinci-002", false),
            ("gpt-4o-mini-tts", false),
            ("gpt-5-codex", false),
            ("sora-2", false),
        ]
    );
}

#[test]
fn every_preset_is_a_usable_address_and_the_ids_are_unique() {
    let mut ids = std::collections::HashSet::new();
    for preset in &PRESETS {
        assert!(ids.insert(preset.id), "{} repeats", preset.id);
        assert!(!preset.name.is_empty() && !preset.help.is_empty());
        if preset.id == "custom" {
            assert!(preset.url.is_empty());
            continue;
        }
        let base =
            BaseUrl::new(preset.url).unwrap_or_else(|error| panic!("{}: {error}", preset.id));
        // Only a server on this machine may be plain http, and only it may go without a key.
        assert_eq!(base.is_loopback(), !preset.needs_key, "{}", preset.id);
        assert!(
            base.is_loopback() || preset.url.starts_with("https://"),
            "{}",
            preset.id
        );
    }
}
