//! What the console is told about a tool call: bounded, free of secrets, and links only where a link is safe.

use std::fmt::Write as _;

use super::*;

#[test]
fn only_plain_web_addresses_become_links() {
    assert_eq!(
        safe_link("https://example.org/a?q=1#frag").as_deref(),
        Some("https://example.org/a?q=1")
    );
    assert!(safe_link("http://example.org").is_some());
    for refused in [
        "javascript:alert(1)",
        "JaVaScRiPt:alert(1)",
        "data:text/html,<script>1</script>",
        "file:///etc/passwd",
        "ftp://example.org/x",
        "//example.org/x",
        "/relative",
        "https://user:pass@example.org/",
        "https://user@example.org/",
        "https://exa\nmple.org/",
        "",
    ] {
        assert!(safe_link(refused).is_none(), "{refused:?} must be refused");
    }
    assert!(safe_link(&format!("https://example.org/{}", "a".repeat(400))).is_none());
}

#[test]
fn a_fetch_target_shows_where_and_never_the_query_or_credentials() {
    let target = target_for(
        "jarvis.web.fetch",
        &json!({ "url": "https://user:pw@example.org/blog/post?token=SECRET#x" }),
    )
    .unwrap_or_default();
    assert_eq!(target, "example.org/blog/post");
    assert!(!target.contains("SECRET") && !target.contains("pw"));
    assert_eq!(
        target_for(
            "jarvis.web.fetch",
            &json!({ "url": "https://example.org/" })
        )
        .as_deref(),
        Some("example.org")
    );
}

#[test]
fn searches_and_delegations_show_their_words_bounded() {
    assert_eq!(
        target_for(
            "jarvis.web.search",
            &json!({ "query": "  dutch   logistics  " })
        )
        .as_deref(),
        Some("dutch logistics")
    );
    let long = target_for("jarvis.agent.delegate", &json!({ "task": "x".repeat(500) }))
        .unwrap_or_default();
    assert_eq!(long.chars().count(), MAX_TARGET_CHARS);
    assert!(target_for("jarvis.files.read", &json!({ "path": "a" })).is_none());
    assert!(target_for("jarvis.web.fetch", &json!({})).is_none());
}

fn search_output() -> String {
    let results = "1. Ollama\nhttps://ollama.com/\nCloud models.\n\n2. Evil\njavascript:alert(1)\nx\n\n3. Credentials\nhttps://a:b@example.org/\ny\n\n4. Fine\nhttps://example.org/x\nz\n\n";
    json!({ "outcome": "searched", "count": 4, "results": results }).to_string()
}

#[test]
fn a_search_result_lists_its_safe_links_and_drops_the_rest() {
    let payload: Value =
        serde_json::from_str(&result_payload("jarvis.web.search", &search_output()))
            .unwrap_or_default();
    assert_eq!(payload["phase"], "tool_result");
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["count"], 4);
    let urls: Vec<&str> = payload["links"]
        .as_array()
        .map(|links| links.iter().filter_map(|l| l["url"].as_str()).collect())
        .unwrap_or_default();
    assert_eq!(urls, ["https://ollama.com/", "https://example.org/x"]);
}

#[test]
fn a_search_never_carries_more_than_eight_links() {
    let mut results = String::new();
    for index in 0..20 {
        let _ = writeln!(results, "{0}. T{0}\nhttps://e.org/{0}\nz\n", index + 1);
    }
    let body = json!({ "outcome": "searched", "count": 20, "results": results }).to_string();
    let payload: Value =
        serde_json::from_str(&result_payload("jarvis.web.search", &body)).unwrap_or_default();
    assert_eq!(payload["links"].as_array().map(Vec::len), Some(MAX_LINKS));
}

#[test]
fn a_fetch_result_links_the_page_it_ended_on() {
    let body =
        json!({ "outcome": "fetched", "url": "https://example.org/final#top", "status": 200 })
            .to_string();
    let payload: Value =
        serde_json::from_str(&result_payload("jarvis.web.fetch", &body)).unwrap_or_default();
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["links"][0]["url"], "https://example.org/final");
    assert_eq!(payload["links"][0]["title"], "example.org/final");
}

#[test]
fn a_failure_is_reported_as_one_with_no_links() {
    for text in [
        json!({ "outcome": "refused", "detail": "that address is private" }).to_string(),
        json!({ "outcome": "failed", "detail": "the page did not answer" }).to_string(),
        "the tool was refused by policy: denied".to_owned(),
        "the tool call could not be completed: boom".to_owned(),
        "error: the arguments were not a JSON object".to_owned(),
    ] {
        let payload: Value =
            serde_json::from_str(&result_payload("jarvis.web.fetch", &text)).unwrap_or_default();
        assert_eq!(payload["ok"], false, "{text}");
        assert!(payload.get("links").is_none(), "{text}");
        assert!(payload["detail"].as_str().is_some_and(|d| !d.is_empty()));
    }
}

#[test]
fn an_ordinary_tool_result_is_ok_without_links() {
    let payload: Value =
        serde_json::from_str(&result_payload("jarvis.files.read", "some file text"))
            .unwrap_or_default();
    assert_eq!(payload["ok"], true);
    assert!(payload.get("links").is_none() && payload.get("detail").is_none());
}
