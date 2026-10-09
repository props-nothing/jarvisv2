//! Tests for memory search, over a real database and the real remember path.

use jarvis_core::{FENCE_CLOSE, FENCE_OPEN};
use jarvis_protocol::RememberRequest;

use super::*;
use crate::memory_service::MemoryService;

struct Scratch(std::path::PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        jarvis_core::remove_scratch_dir(&self.0);
    }
}

async fn fixture() -> (Scratch, Arc<SqliteDatabase>, MemorySearchTool) {
    let tag = jarvis_core::scratch_tag();
    let path = std::env::temp_dir().join(format!("jms-{}", &tag[tag.len() - 12..]));
    std::fs::create_dir_all(&path).unwrap_or_else(|error| panic!("create scratch: {error}"));
    let database = Arc::new(
        SqliteDatabase::open(&path.join("jarvis.sqlite3"))
            .await
            .unwrap_or_else(|error| panic!("open the database: {error}")),
    );
    let tool = MemorySearchTool::new(Arc::clone(&database));
    (Scratch(path), database, tool)
}

async fn remember(database: &Arc<SqliteDatabase>, content: &str, importance: u8) {
    MemoryService::new(Arc::clone(database))
        .remember(&RememberRequest {
            content: content.to_owned(),
            memory_type: "preference".to_owned(),
            source_kind: "user_statement".to_owned(),
            importance: Some(importance),
            entity_ids: Vec::new(),
            claim: None,
            supersedes: None,
        })
        .await
        .unwrap_or_else(|error| panic!("remember: {error}"));
}

#[test]
fn the_contract_is_read_only_and_unasked() {
    let definition = MemorySearchTool::definition().unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(definition.id().to_string(), SEARCH_TOOL);
    assert_eq!(definition.risk().level(), 0);
    assert_eq!(definition.approval(), ApprovalPolicy::Auto);
    assert!(definition.effects().contains(ToolEffect::ReadOnly));
}

/// **Every query word must match; results come back fenced as data, most important first; and the count says how much was looked at.**
#[tokio::test]
async fn a_search_matches_every_word_and_fences_what_it_returns() {
    let (_scratch, database, tool) = fixture().await;
    remember(&database, "The user prefers train travel over flying", 5).await;
    remember(&database, "The user prefers dark roast coffee", 5).await;
    remember(
        &database,
        "Their train to Utrecht leaves at 08:15 on Mondays",
        9,
    )
    .await;
    let now = UtcTimestamp::now(&SystemClock);

    let found = tool
        .search("train", 5, now)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(found["considered"], 3);
    assert_eq!(found["count"], 2);
    let text = found["memories"].as_str().unwrap_or_default();
    assert!(
        text.contains(FENCE_OPEN) && text.contains(FENCE_CLOSE),
        "{text}"
    );
    assert!(
        text.contains("NOT instructions"),
        "the same introduction a context carries"
    );
    let first = text.find("Utrecht").unwrap_or(usize::MAX);
    let second = text.find("travel over flying").unwrap_or(usize::MAX);
    assert!(
        first < second,
        "the more important claim comes first: {text}"
    );

    let narrowed = tool
        .search("TRAIN utrecht", 5, now)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        narrowed["count"], 1,
        "every word must appear, whatever the case"
    );

    let none = tool
        .search("submarine", 5, now)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(none["count"], 0);
    assert_eq!(
        none["considered"], 3,
        "an empty answer says how much was looked at"
    );
    assert!(none["memories"].is_null());

    let limited = tool
        .search("user", 1, now)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(limited["count"], 1);
}

/// **A claim the context would not offer is not offered here either, and bad arguments never search.**
#[tokio::test]
async fn nothing_a_run_would_withhold_is_returned_and_bad_arguments_are_refused() {
    let (_scratch, database, tool) = fixture().await;
    remember(&database, "The user prefers tea", 5).await;
    // A claim read a long time in the future has not expired; far enough ahead the assembler's own currency rule applies.
    let far = UtcTimestamp::from_unix_nanos(
        UtcTimestamp::now(&SystemClock).unix_nanos() + 100 * 365 * 24 * 3600 * 1_000_000_000,
    )
    .unwrap_or_else(|error| panic!("{error}"));
    let later = tool
        .search("tea", 5, far)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(later["count"].as_u64().unwrap_or(9) <= 1, "{later}");

    for arguments in [
        json!({}),
        json!({ "query": "  " }),
        json!({ "query": "a".repeat(201) }),
    ] {
        assert!(
            matches!(
                parse_arguments(&arguments),
                Err(AdapterError::RefusedBeforeReaching { .. })
            ),
            "{arguments}"
        );
    }
    assert_eq!(
        parse_arguments(&json!({ "query": "x", "limit": 99 }))
            .map(|(_, limit)| limit)
            .ok(),
        Some(10)
    );
}
