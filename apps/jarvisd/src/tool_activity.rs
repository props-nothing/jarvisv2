//! What the console is told about a tool call, so it can show what JARVIS is doing and where it looked (`ADR-0152`).
//!
//! Two small pieces of information ride the run's own event stream, and nothing else is added to it:
//!
//! * the **target** of a request (the page being fetched, the query being searched), so the live view says "fetching ollama.com/blog"
//!   rather than "fetching a page";
//! * a **result** event once the call has finished: whether it worked and, for web tools, the links it touched.
//!
//! Neither is trusted. A target and a link come from a model's arguments or from a page, so this module bounds them, strips what could
//! carry a secret (a URL's query, fragment and credentials are dropped from a *target*), and offers a link only when it is a plain
//! `http` or `https` address with no embedded credentials. The console shows them as text and opens them in a new tab with
//! `rel="noopener noreferrer"`; nothing here is ever an instruction to the model, which is told nothing new.

use reqwest::Url;
use serde_json::{Value, json};

/// The longest target shown.
const MAX_TARGET_CHARS: usize = 160;

/// The most links one result carries.
const MAX_LINKS: usize = 8;

/// The longest link accepted.
const MAX_LINK_CHARS: usize = 300;

/// The longest title or detail shown.
const MAX_TEXT_CHARS: usize = 120;

fn clip(text: &str, limit: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= limit {
        return flat;
    }
    let mut cut: String = flat.chars().take(limit.saturating_sub(1)).collect();
    cut.push('\u{2026}');
    cut
}

/// An address the console may offer as a link: `http` or `https`, with a host and no embedded credentials.
///
/// Fails closed: anything else (`javascript:`, `data:`, `file:`, a relative reference, an address with a user name) is `None`.
#[must_use]
pub(crate) fn safe_link(candidate: &str) -> Option<String> {
    let candidate = candidate.trim();
    if candidate.chars().count() > MAX_LINK_CHARS || candidate.chars().any(char::is_control) {
        return None;
    }
    let mut url = Url::parse(candidate).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    url.host_str()?;
    if !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    url.set_fragment(None);
    Some(url.to_string())
}

/// A page address reduced to where it is, for display: no query, fragment or credentials, which is where a secret would be.
fn display_address(candidate: &str) -> String {
    let Ok(url) = Url::parse(candidate.trim()) else {
        return clip(candidate, MAX_TARGET_CHARS);
    };
    let Some(host) = url.host_str() else {
        return clip(candidate, MAX_TARGET_CHARS);
    };
    let path = if url.path() == "/" { "" } else { url.path() };
    clip(&format!("{host}{path}"), MAX_TARGET_CHARS)
}

/// What a request is about, in a few words, for the tools whose argument says it. `None` when there is nothing useful to show.
#[must_use]
pub(crate) fn target_for(tool: &str, arguments: &Value) -> Option<String> {
    let text = |name: &str| arguments.get(name).and_then(Value::as_str);
    match tool {
        "jarvis.web.fetch" => text("url").map(display_address),
        "jarvis.web.search" | "jarvis.memory.search" => {
            text("query").map(|query| clip(query, MAX_TARGET_CHARS))
        }
        "jarvis.agent.delegate" => text("task").map(|task| clip(task, MAX_TARGET_CHARS)),
        _ => None,
    }
}

/// An outcome a tool reports when it did not do what was asked.
fn outcome_is_bad(outcome: &str) -> bool {
    matches!(
        outcome,
        "failed" | "refused" | "error" | "cancelled" | "timed_out"
    )
}

/// The payload of the event that says a tool call finished.
///
/// `result_text` is what the model was given back. For a web tool it is JSON with an `outcome`; for a refusal or a fault it is a
/// sentence. Links are the pages a search listed or a fetch ended on, each checked by [`safe_link`].
#[must_use]
pub(crate) fn result_payload(tool: &str, result_text: &str) -> String {
    let parsed: Option<Value> = serde_json::from_str(result_text).ok();
    let outcome = parsed
        .as_ref()
        .and_then(|value| value.get("outcome"))
        .and_then(Value::as_str);
    let sentence_failure = [
        "the tool was refused",
        "the tool call could not",
        "the tool reported",
        "error:",
    ]
    .iter()
    .any(|prefix| result_text.starts_with(prefix));
    let ok = !sentence_failure && !outcome.is_some_and(outcome_is_bad);

    let mut links: Vec<Value> = Vec::new();
    let mut count: Option<u64> = None;
    if ok && let Some(value) = &parsed {
        match tool {
            "jarvis.web.search" => {
                count = value.get("count").and_then(Value::as_u64);
                let results = value
                    .get("results")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                for (title, url) in jarvis_web::links_from_results(results) {
                    if links.len() >= MAX_LINKS {
                        break;
                    }
                    if let Some(url) = safe_link(&url) {
                        links.push(json!({ "title": clip(&title, MAX_TEXT_CHARS), "url": url }));
                    }
                }
            }
            "jarvis.web.fetch" => {
                if let Some(url) = value.get("url").and_then(Value::as_str).and_then(safe_link) {
                    links.push(json!({ "title": display_address(&url), "url": url }));
                }
            }
            _ => {}
        }
    }

    let mut payload = json!({ "phase": "tool_result", "tool": tool, "ok": ok });
    if let Some(count) = count {
        payload["count"] = json!(count);
    }
    if !links.is_empty() {
        payload["links"] = Value::Array(links);
    }
    if !ok {
        let detail = parsed
            .as_ref()
            .and_then(|value| value.get("detail"))
            .and_then(Value::as_str)
            .unwrap_or(result_text);
        payload["detail"] = Value::String(clip(detail, MAX_TEXT_CHARS));
    }
    payload.to_string()
}

#[cfg(test)]
#[path = "tool_activity_tests.rs"]
mod tests;
