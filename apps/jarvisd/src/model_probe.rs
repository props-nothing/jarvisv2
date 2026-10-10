//! Looking at a model provider before it is saved: which models it serves, and whether one of them answers (`ADR-0164`).
//!
//! The Settings screen offers a list of models to choose from and a "test" button. Both are asked of the **daemon**, which makes the
//! request, so a stored key never reaches the browser. The one rule that keeps that safe is in [`resolve`]: a stored key is only ever sent to
//! the address it was stored for. A different address needs its own key, typed in the same request.

use std::sync::Arc;
use std::time::{Duration, Instant};

use jarvis_core::CorrelationId;
use jarvis_models::openai::{ApiKey, BaseUrl, OpenAiCompatibleProvider, RetryPolicy, Transport};
use jarvis_models::{
    ChatMessage, ChatRequest, ModelError, ModelErrorKind, ModelGateway, ModelId, ProviderId,
};

/// How long listing may take.
const LIST_SECONDS: u64 = 20;
/// How long the test call may take.
const TEST_SECONDS: u64 = 45;
/// The key sent to a server on this machine when none was given: such a server ignores it, but the protocol wants one.
const LOCAL_PLACEHOLDER_KEY: &str = "ollama";

/// Words that mark a model as something other than a chat model, so the list can show the likely ones first.
///
/// A guess from the name, never a claim: the screen says "probably", and any name can still be chosen. Besides the obvious (embeddings,
/// speech, images), it covers names the vendors serve only through other endpoints (the completions-only `davinci`, `babbage` and
/// `turbo-instruct`, video, deep research, `codex`). Words that other vendors use for ordinary chat models (`instruct`, `pro`) are left out.
const NOT_CHAT: &[&str] = &[
    "sora",
    "babbage",
    "davinci",
    "turbo-instruct",
    "deep-research",
    "codex",
    "embed",
    "whisper",
    "tts",
    "dall-e",
    "moderation",
    "image",
    "audio",
    "transcribe",
    "realtime",
    "rerank",
    "speech",
];

/// A provider the Settings screen offers to choose, so the address does not have to be typed.
///
/// Only the address and how to get a key: the model names are asked of the provider itself and are not kept here, where they would go
/// stale. Every address here is the OpenAI-compatible one the vendor documents (`docs/research/integrations/model-providers.md`).
pub(crate) struct Preset {
    /// A stable identifier for the screen.
    pub id: &'static str,
    /// What the owner sees.
    pub name: &'static str,
    /// The OpenAI-compatible base address. Empty for "other".
    pub url: &'static str,
    /// Whether the provider wants a key (a server on this machine does not).
    pub needs_key: bool,
    /// Where the key comes from, and anything the owner should know.
    pub help: &'static str,
}

/// The providers offered, in the order shown.
pub(crate) const PRESETS: [Preset; 6] = [
    Preset {
        id: "ollama-local",
        name: "Ollama (this computer)",
        url: "http://localhost:11434/v1",
        needs_key: false,
        help: "No key needed. Ollama's cloud models also work here once you have signed in to Ollama on this computer.",
    },
    Preset {
        id: "ollama-cloud",
        name: "Ollama Cloud",
        url: "https://ollama.com/v1",
        needs_key: true,
        help: "A key from ollama.com/settings/keys.",
    },
    Preset {
        id: "openai",
        name: "OpenAI",
        url: "https://api.openai.com/v1",
        needs_key: true,
        help: "An API key from your OpenAI account.",
    },
    Preset {
        id: "gemini",
        name: "Google Gemini",
        url: "https://generativelanguage.googleapis.com/v1beta/openai/",
        needs_key: true,
        help: "A key from aistudio.google.com/apikey.",
    },
    Preset {
        id: "anthropic",
        name: "Anthropic (Claude)",
        url: "https://api.anthropic.com/v1/",
        needs_key: true,
        help: "A key from platform.claude.com/settings/keys. Anthropic calls this compatibility layer a way to try Claude, not a production route: no prompt caching, and its thinking is not returned.",
    },
    Preset {
        id: "custom",
        name: "Other (OpenAI-compatible)",
        url: "",
        needs_key: true,
        help: "Any server that speaks the OpenAI chat API: give its address, ending in /v1.",
    },
];

/// A model identifier from a provider's list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Listed {
    /// The identifier a chat request names.
    pub id: String,
    /// False when the name says it is for embeddings, speech, images or the like.
    pub likely_chat: bool,
}

/// What looking at a provider found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Report {
    /// The models it serves: chat models, then the owner's fine-tunes, then the rest, each group in name order. Empty when listing failed.
    pub models: Vec<Listed>,
    /// Why listing failed, in words for the owner. The address and key may still be right: some providers do not list.
    pub list_problem: Option<String>,
    /// The result of the test call, when one was asked for.
    pub test: Option<Test>,
}

/// The outcome of one tiny call to one model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Test {
    /// Whether the model answered.
    pub ok: bool,
    /// What happened, in words for the owner.
    pub message: String,
    /// How long it took, in milliseconds.
    pub millis: u64,
}

fn same_address(left: &str, right: &str) -> bool {
    left.trim().trim_end_matches('/') == right.trim().trim_end_matches('/')
}

/// Works out the address and key to use.
///
/// The address is the one given, else the configured one. The key is the one typed in this request; failing that, the **stored** key,
/// but only when the address is the one it is stored for. A stored key is never combined with an address someone supplied, which
/// would let a request carry it to a server of their choosing. A server on this machine needs no key.
///
/// # Errors
///
/// A sentence for the owner when there is no usable address or no key to send.
pub(crate) fn resolve(
    configured_url: Option<&str>,
    given_url: Option<&str>,
    typed_key: Option<&str>,
    stored_key: impl FnOnce() -> Option<String>,
) -> Result<(BaseUrl, String), String> {
    let url = given_url
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .or(configured_url)
        .ok_or_else(|| "Give the provider's address first.".to_owned())?;
    let base = BaseUrl::new(url).map_err(|_| {
        "That is not a usable address: it must start with http:// or https:// and carry no login."
            .to_owned()
    })?;
    if let Some(typed) = typed_key.map(str::trim).filter(|key| !key.is_empty()) {
        let key = jarvis_storage::settings::validate_secret(typed)?;
        return Ok((base, key));
    }
    if configured_url.is_some_and(|configured| same_address(configured, url))
        && let Some(stored) = stored_key().filter(|key| !key.trim().is_empty())
    {
        return Ok((base, stored.trim().to_owned()));
    }
    if base.is_loopback() {
        return Ok((base, LOCAL_PLACEHOLDER_KEY.to_owned()));
    }
    Err("Paste the key for this address first.".to_owned())
}

/// Where a model goes in the list: 0 a chat model, 1 one of the owner's own fine-tunes (`ft:...`), 2 probably not for chat.
fn rank(id: &str) -> u8 {
    let lower = id.to_ascii_lowercase();
    if NOT_CHAT.iter().any(|word| lower.contains(word)) {
        2
    } else {
        u8::from(lower.starts_with("ft:"))
    }
}

fn explain(error: &ModelError) -> String {
    match error.kind() {
        ModelErrorKind::Authentication => "The provider rejected the key.".to_owned(),
        ModelErrorKind::Authorization => "The key has no access to that.".to_owned(),
        ModelErrorKind::ModelNotFound => "The provider does not serve that model.".to_owned(),
        ModelErrorKind::RateLimited | ModelErrorKind::Overloaded => {
            "The provider is limiting or overloaded right now; the key and address are probably fine.".to_owned()
        }
        ModelErrorKind::QuotaExhausted => "The account has no credit or quota left.".to_owned(),
        ModelErrorKind::Transient | ModelErrorKind::Timeout => {
            "The provider could not be reached at that address.".to_owned()
        }
        ModelErrorKind::MalformedResponse => {
            "That address answered, but not like an OpenAI-style model server (does it end in /v1?).".to_owned()
        }
        other => format!("The provider refused ({}).", other.as_str()),
    }
}

/// Lists the provider's models and, when `test_model` is given, asks that model for one short answer.
///
/// # Errors
///
/// A sentence for the owner when the key or address cannot be turned into a provider at all. A provider that refuses is a [`Report`] with
/// the reason in it, not an error: the screen shows it either way.
pub(crate) async fn run(
    transport: Arc<dyn Transport>,
    base: BaseUrl,
    key: &str,
    test_model: Option<&str>,
) -> Result<Report, String> {
    let api_key = ApiKey::new(key).map_err(|_| "That key is not usable.".to_owned())?;
    let provider_id = ProviderId::new("openai-compatible").map_err(|_| "internal".to_owned())?;
    let provider =
        OpenAiCompatibleProvider::new(provider_id, base, api_key, transport, RetryPolicy::none());
    let (models, list_problem) =
        match tokio::time::timeout(Duration::from_secs(LIST_SECONDS), provider.list_models()).await
        {
            Ok(Ok(ids)) => {
                let mut listed: Vec<Listed> = ids
                    .into_iter()
                    .map(|id| Listed {
                        likely_chat: rank(&id) < 2,
                        id,
                    })
                    .collect();
                // Chat models, then the owner's fine-tunes, then the rest; each group keeps the provider's name order.
                listed.sort_by_key(|model| rank(&model.id));
                (listed, None)
            }
            Ok(Err(error)) => (Vec::new(), Some(explain(&error))),
            Err(_) => (
                Vec::new(),
                Some("The provider took too long to list its models.".to_owned()),
            ),
        };
    let test = match test_model {
        Some(name) => Some(test_call(&provider, name).await),
        None => None,
    };
    Ok(Report {
        models,
        list_problem,
        test,
    })
}

async fn test_call(provider: &OpenAiCompatibleProvider, name: &str) -> Test {
    let started = Instant::now();
    let millis =
        |started: Instant| u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let Ok(model) = ModelId::new(name) else {
        return Test {
            ok: false,
            message: "That is not a usable model name.".to_owned(),
            millis: 0,
        };
    };
    let request = ChatRequest::new(
        model,
        vec![ChatMessage::user("Reply with the single word OK.")],
        CorrelationId::new(),
    )
    .with_max_output_tokens(Some(64));
    match tokio::time::timeout(
        Duration::from_secs(TEST_SECONDS),
        provider.complete(request),
    )
    .await
    {
        Ok(Ok(_)) => Test {
            ok: true,
            message: "The model answered. Tool use was not checked.".to_owned(),
            millis: millis(started),
        },
        Ok(Err(error)) => Test {
            ok: false,
            message: explain(&error),
            millis: millis(started),
        },
        Err(_) => Test {
            ok: false,
            message: "The model took too long to answer.".to_owned(),
            millis: millis(started),
        },
    }
}

#[cfg(test)]
#[path = "model_probe_tests.rs"]
mod tests;
