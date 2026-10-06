use std::{
    fs,
    path::{Path, PathBuf},
};

use super::*;
use crate::{
    DEFAULT_DATABASE_FILENAME, DatabaseError, LOCAL_USER_ID, LOCAL_WORKSPACE_ID, NewRun,
    SqliteDatabase, StoredRun, create_run,
};
use jarvis_core::{
    ApprovalDecision, ApprovalDecisionOutcome, ApprovalId, ApprovalRequest, ApprovalRequestParts,
    ApprovalState, CanonicalIntentHash, CorrelationId, UtcTimestamp,
};

const SESSION: &str = "0198f000-0000-7000-8000-000000000003";
const RUN: &str = "0198f000-0000-7000-8000-0000000000c3";
const APPROVER: &str = "owner";

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("jarvis-approvals-{}", jarvis_core::scratch_tag()));
        must(fs::create_dir_all(&path));
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        jarvis_core::remove_scratch_dir(&self.0);
    }
}

fn must<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("expected success, got {error:?}"),
    }
}

fn at(minute: i128) -> UtcTimestamp {
    must(UtcTimestamp::from_unix_nanos(
        1_774_000_000_000_000_000 + minute * 60_000_000_000,
    ))
}

async fn seeded_database() -> (TestDirectory, SqliteDatabase) {
    let directory = TestDirectory::new();
    let path = directory.path().join(DEFAULT_DATABASE_FILENAME);
    let database = must(SqliteDatabase::open(&path).await);
    must(
        sqlx::query(
            "INSERT INTO sessions (id, workspace_id, user_id, channel, status, created_at, \
             updated_at, version) VALUES (?1, ?2, ?3, 'cli', 'active', ?4, ?4, 1)",
        )
        .bind(SESSION)
        .bind(LOCAL_WORKSPACE_ID)
        .bind(LOCAL_USER_ID)
        .bind(at(0).to_string())
        .execute(database.pool())
        .await
        .map(|_| ()),
    );
    (directory, database)
}

async fn live_run(database: &SqliteDatabase) -> StoredRun {
    let new = must(NewRun::new(
        RUN,
        SESSION,
        LOCAL_WORKSPACE_ID,
        LOCAL_USER_ID,
        "Send the quarterly report",
        CorrelationId::new(),
        at(0),
    ));
    must(create_run(database, &new).await)
}

fn approval_expiring_at(
    tool: &str,
    arguments: &serde_json::Value,
    expires_minute: i128,
) -> ApprovalRequest {
    let intent = must(CanonicalIntentHash::compute(tool, "1.0.0", arguments));
    must(ApprovalRequest::new(ApprovalRequestParts {
        id: ApprovalId::new(),
        workspace_id: must(LOCAL_WORKSPACE_ID.parse()),
        run_id: must(RUN.parse()),
        actor_id: LOCAL_USER_ID.to_owned(),
        tool: tool.to_owned(),
        tool_version: "1.0.0".to_owned(),
        intent,
        preview: "Delete every message in the mailbox.".to_owned(),
        risk_level: 3,
        correlation_id: CorrelationId::new(),
        created_at: at(0),
        expires_at: at(expires_minute),
    }))
}

fn approval_for(tool: &str, arguments: &serde_json::Value) -> ApprovalRequest {
    approval_expiring_at(tool, arguments, 10)
}

fn approve(minute: i128) -> ApprovalDecision {
    ApprovalDecision::new(
        ApprovalDecisionOutcome::Approve,
        jarvis_core::ApprovalChannel::Desktop,
        at(minute),
        APPROVER,
    )
    .unwrap_or_else(|error| panic!("decision: {error}"))
}

fn deny(minute: i128) -> ApprovalDecision {
    ApprovalDecision::new(
        ApprovalDecisionOutcome::Deny,
        jarvis_core::ApprovalChannel::Desktop,
        at(minute),
        APPROVER,
    )
    .unwrap_or_else(|error| panic!("decision: {error}"))
}

#[tokio::test]
async fn an_approval_round_trips() {
    let (_directory, database) = seeded_database().await;
    let _run = live_run(&database).await;
    let request = approval_for("jarvis.mail.purge", &serde_json::json!({"all": true}));
    must(create_approval(&database, &request).await);

    let stored = must(find_approval(&database, &request.id().to_string()).await);
    assert_eq!(stored.id(), request.id());
    assert_eq!(stored.workspace_id(), request.workspace_id());
    assert_eq!(stored.run_id(), request.run_id());
    assert_eq!(stored.actor_id(), request.actor_id());
    assert_eq!(stored.tool(), request.tool());
    assert_eq!(stored.tool_version(), request.tool_version());
    assert_eq!(stored.intent(), request.intent());
    assert_eq!(stored.preview().as_str(), request.preview().as_str());
    assert_eq!(stored.risk_level(), request.risk_level());
    assert_eq!(stored.correlation_id(), request.correlation_id());
    assert_eq!(stored.created_at(), request.created_at());
    assert_eq!(stored.expires_at(), request.expires_at());
    assert_eq!(stored.stored_state(), ApprovalState::Pending);
    assert!(stored.decision().is_none());
}

#[tokio::test]
async fn unused_columns_are_written_with_fixed_placeholders() {
    let (_directory, database) = seeded_database().await;
    let _run = live_run(&database).await;
    let request = approval_for("jarvis.mail.purge", &serde_json::json!({"all": true}));
    must(create_approval(&database, &request).await);

    let row = must(
        sqlx::query("SELECT required_strength, nonce_hash FROM approvals WHERE id = ?1")
            .bind(request.id().to_string())
            .fetch_one(database.pool())
            .await,
    );
    let (unused_hash_value, unused_text_value) = unused_column_values();
    assert_eq!(
        must(row.try_get::<String, _>("required_strength")),
        unused_text_value
    );
    assert_eq!(
        must(row.try_get::<String, _>("nonce_hash")),
        unused_hash_value
    );
}

#[tokio::test]
async fn a_valid_decision_is_recorded() {
    let (_directory, database) = seeded_database().await;
    let _run = live_run(&database).await;
    let request = approval_for("jarvis.mail.purge", &serde_json::json!({"all": true}));
    must(create_approval(&database, &request).await);

    let decided = must(record_decision(&database, &request.id().to_string(), &approve(1)).await);
    assert_eq!(decided.stored_state(), ApprovalState::Approved);
    assert_eq!(
        decided.decision().map(ApprovalDecision::approver_id),
        Some(APPROVER)
    );

    let row = must(
        sqlx::query("SELECT decision_strength, nonce_hash FROM approvals WHERE id = ?1")
            .bind(request.id().to_string())
            .fetch_one(database.pool())
            .await,
    );
    let (unused_hash_value, unused_text_value) = unused_column_values();
    assert_eq!(
        must(row.try_get::<String, _>("decision_strength")),
        unused_text_value
    );
    assert_eq!(
        must(row.try_get::<String, _>("nonce_hash")),
        unused_hash_value
    );
}

#[tokio::test]
async fn a_decisions_attribution_survives_the_round_trip() {
    let (_directory, database) = seeded_database().await;
    let _run = live_run(&database).await;
    let request = approval_for("jarvis.mail.purge", &serde_json::json!({"all": true}));
    must(create_approval(&database, &request).await);
    must(record_decision(&database, &request.id().to_string(), &approve(1)).await);

    let stored = must(find_approval(&database, &request.id().to_string()).await);
    let recorded = stored
        .decision()
        .unwrap_or_else(|| panic!("a decision must be present"));
    assert_eq!(recorded.channel(), jarvis_core::ApprovalChannel::Desktop);
    assert_eq!(recorded.decided_at(), at(1));
    assert_eq!(recorded.approver_id(), APPROVER);
}

#[tokio::test]
async fn a_recorded_decision_cannot_be_replayed_or_replaced() {
    let (_directory, database) = seeded_database().await;
    let _run = live_run(&database).await;
    let request = approval_for("jarvis.mail.purge", &serde_json::json!({"all": true}));
    must(create_approval(&database, &request).await);

    let denied = must(record_decision(&database, &request.id().to_string(), &deny(1)).await);
    assert_eq!(denied.stored_state(), ApprovalState::Denied);

    let replayed = record_decision(&database, &request.id().to_string(), &deny(1)).await;
    assert!(
        matches!(replayed, Err(DatabaseError::ApprovalAlreadyDecided)),
        "a replay must be reported as already decided, got {replayed:?}"
    );

    let replacement = record_decision(&database, &request.id().to_string(), &approve(2)).await;
    assert!(
        matches!(replacement, Err(DatabaseError::ApprovalAlreadyDecided)),
        "a denial must not be replaceable, got {replacement:?}"
    );
}

#[tokio::test]
async fn a_decision_after_the_expiry_is_refused_and_an_approval_lapses_on_read() {
    let (_directory, database) = seeded_database().await;
    let _run = live_run(&database).await;
    let request = approval_expiring_at("jarvis.mail.purge", &serde_json::json!({"all": true}), 10);
    must(create_approval(&database, &request).await);

    for minute in [10, 11, 100] {
        let late = record_decision(&database, &request.id().to_string(), &approve(minute)).await;
        assert!(
            matches!(
                late,
                Err(DatabaseError::InvalidApprovalRequest {
                    field: "expires_at"
                })
            ),
            "a decision at minute {minute} must be refused as expired, got {late:?}"
        );
    }

    let decided = must(record_decision(&database, &request.id().to_string(), &approve(9)).await);
    assert!(decided.authorizes_at(at(9)));
    assert!(
        !decided.authorizes_at(at(10)),
        "an approval must stop authorizing at its expiry"
    );
    assert_eq!(decided.state_at(at(10)), ApprovalState::Expired);
}

#[tokio::test]
async fn a_lapsed_approval_is_not_pending() {
    let (_directory, database) = seeded_database().await;
    let _run = live_run(&database).await;
    let expired = approval_expiring_at("jarvis.mail.purge", &serde_json::json!({"all": true}), 10);
    must(create_approval(&database, &expired).await);

    let before = must(read_pending_approvals(&database, RUN, at(9)).await);
    assert_eq!(before.len(), 1, "it is pending before its expiry");

    let after = must(read_pending_approvals(&database, RUN, at(10)).await);
    assert!(
        after.is_empty(),
        "a lapsed approval must not be offered as awaiting a decision"
    );

    let stored = must(find_approval(&database, &expired.id().to_string()).await);
    assert_eq!(stored.state_at(at(10)), ApprovalState::Expired);
}

#[tokio::test]
async fn editing_the_action_produces_a_distinct_approval() {
    let (_directory, database) = seeded_database().await;
    let _run = live_run(&database).await;

    let original = approval_for("jarvis.mail.purge", &serde_json::json!({"folder": "inbox"}));
    must(create_approval(&database, &original).await);

    let edited = approval_for(
        "jarvis.mail.purge",
        &serde_json::json!({"folder": "archive"}),
    );
    assert_ne!(
        original.intent(),
        edited.intent(),
        "a changed argument must produce a different intent"
    );
    must(create_approval(&database, &edited).await);

    let stored = must(read_run_approvals(&database, RUN).await);
    assert_eq!(stored.len(), 2, "the edited action is a separate approval");
    let intents: Vec<String> = stored
        .iter()
        .map(|approval| approval.intent().to_hex())
        .collect();
    assert!(intents.contains(&original.intent().to_hex()));
    assert!(intents.contains(&edited.intent().to_hex()));
}

#[tokio::test]
async fn a_duplicate_intent_in_one_run_is_refused() {
    let (_directory, database) = seeded_database().await;
    let _run = live_run(&database).await;
    let arguments = serde_json::json!({"folder": "inbox"});

    let first = approval_for("jarvis.mail.purge", &arguments);
    must(create_approval(&database, &first).await);

    let duplicate = approval_for("jarvis.mail.purge", &arguments);
    assert_eq!(duplicate.intent(), first.intent());
    assert_ne!(duplicate.id(), first.id());
    let refused = create_approval(&database, &duplicate).await;
    assert!(
        matches!(refused, Err(DatabaseError::Sqlite { .. })),
        "a duplicate intent must be refused by the unique index, got {refused:?}"
    );

    let stored = must(read_run_approvals(&database, RUN).await);
    assert_eq!(stored.len(), 1);
}

#[tokio::test]
async fn a_decided_approval_still_blocks_the_same_intent() {
    let (_directory, database) = seeded_database().await;
    let _run = live_run(&database).await;
    let arguments = serde_json::json!({"folder": "inbox"});
    let first = approval_for("jarvis.mail.purge", &arguments);
    must(create_approval(&database, &first).await);
    must(record_decision(&database, &first.id().to_string(), &deny(1)).await);

    let second = approval_for("jarvis.mail.purge", &arguments);
    let refused = create_approval(&database, &second).await;
    assert!(
        matches!(refused, Err(DatabaseError::Sqlite { .. })),
        "a decided intent must not be re-requested in the same run"
    );
}

#[tokio::test]
async fn a_decided_request_cannot_be_created() {
    let (_directory, database) = seeded_database().await;
    let _run = live_run(&database).await;
    let request = approval_for("jarvis.mail.purge", &serde_json::json!({"all": true}));
    must(create_approval(&database, &request).await);
    let decided = must(record_decision(&database, &request.id().to_string(), &approve(1)).await);

    let refused = create_approval(&database, &decided).await;
    assert!(
        matches!(
            refused,
            Err(DatabaseError::InvalidApprovalRequest { field: "state" })
        ),
        "a decided request must not be creatable, got {refused:?}"
    );
}

#[tokio::test]
async fn a_missing_approval_is_not_found() {
    let (_directory, database) = seeded_database().await;
    let absent = "0198f000-0000-7000-8000-0000000000ff";
    let found = find_approval(&database, absent).await;
    assert!(
        matches!(found, Err(DatabaseError::ApprovalNotFound)),
        "expected not-found, got {found:?}"
    );

    let decided = record_decision(&database, absent, &approve(1)).await;
    assert!(
        matches!(decided, Err(DatabaseError::ApprovalNotFound)),
        "expected not-found, got {decided:?}"
    );
}

#[tokio::test]
async fn an_approval_for_a_missing_run_is_refused() {
    let (_directory, database) = seeded_database().await;
    let request = approval_for("jarvis.mail.purge", &serde_json::json!({"all": true}));
    let refused = create_approval(&database, &request).await;
    assert!(
        matches!(refused, Err(DatabaseError::Sqlite { .. })),
        "a dangling run reference must be refused, got {refused:?}"
    );
}

#[tokio::test]
async fn a_runs_approvals_are_listed_reproducibly() {
    let (_directory, database) = seeded_database().await;
    let _run = live_run(&database).await;
    for folder in ["inbox", "archive", "trash"] {
        let request = approval_for("jarvis.mail.purge", &serde_json::json!({"folder": folder}));
        must(create_approval(&database, &request).await);
    }

    let first: Vec<String> = must(read_run_approvals(&database, RUN).await)
        .iter()
        .map(|approval| approval.id().to_string())
        .collect();
    let second: Vec<String> = must(read_run_approvals(&database, RUN).await)
        .iter()
        .map(|approval| approval.id().to_string())
        .collect();
    assert_eq!(first.len(), 3);
    assert_eq!(first, second, "the order must be stable across reads");
}

#[tokio::test]
async fn a_row_whose_decision_attribution_is_missing_is_reported() {
    let (_directory, database) = seeded_database().await;
    let _run = live_run(&database).await;
    let request = approval_for("jarvis.mail.purge", &serde_json::json!({"all": true}));
    must(create_approval(&database, &request).await);
    must(record_decision(&database, &request.id().to_string(), &deny(1)).await);

    let rejected = sqlx::query("UPDATE approvals SET decided_by = NULL WHERE id = ?1")
        .bind(request.id().to_string())
        .execute(database.pool())
        .await;
    assert!(
        rejected.is_err(),
        "the CHECK must reject missing attribution"
    );

    let mut connection = must(database.pool().acquire().await);
    must(
        sqlx::query("PRAGMA ignore_check_constraints = ON")
            .execute(&mut *connection)
            .await
            .map(|_| ()),
    );
    must(
        sqlx::query("UPDATE approvals SET decided_by = NULL WHERE id = ?1")
            .bind(request.id().to_string())
            .execute(&mut *connection)
            .await
            .map(|_| ()),
    );

    let decoded = find_approval(&database, &request.id().to_string()).await;
    assert!(
        matches!(
            decoded,
            Err(DatabaseError::StoredApprovalInvalid {
                field: "decided_by"
            })
        ),
        "a row with missing attribution must not be trusted, got {decoded:?}"
    );
}

#[tokio::test]
async fn a_pending_approval_holds_its_arguments_until_it_is_decided() {
    let (_directory, database) = seeded_database().await;
    let _run = live_run(&database).await;
    let arguments = serde_json::json!({"url": "https://example.com"});
    let request = approval_for("jarvis.web.fetch", &arguments);
    let id = request.id().to_string();
    must(create_approval(&database, &request).await);

    assert_eq!(must(read_approval_arguments(&database, &id).await), None);
    assert!(must(
        attach_approval_arguments(&database, &id, &arguments.to_string()).await
    ));
    assert_eq!(
        must(read_approval_arguments(&database, &id).await),
        Some(arguments.to_string())
    );

    must(record_decision(&database, &id, &approve(1)).await);
    assert_eq!(
        must(read_approval_arguments(&database, &id).await),
        Some(arguments.to_string()),
        "an approved approval keeps its arguments until the call has run"
    );
    assert!(
        !must(attach_approval_arguments(&database, &id, &arguments.to_string()).await),
        "a payload can never be attached to an approval that was already decided"
    );
    must(clear_approval_arguments(&database, &id).await);
    assert_eq!(must(read_approval_arguments(&database, &id).await), None);
}

#[tokio::test]
async fn a_denied_approval_clears_its_arguments_at_once() {
    let (_directory, database) = seeded_database().await;
    let _run = live_run(&database).await;
    let arguments = serde_json::json!({"url": "https://example.com"});
    let request = approval_for("jarvis.web.fetch", &arguments);
    let id = request.id().to_string();
    must(create_approval(&database, &request).await);
    must(attach_approval_arguments(&database, &id, &arguments.to_string()).await);
    must(record_decision(&database, &id, &deny(1)).await);
    assert_eq!(must(read_approval_arguments(&database, &id).await), None);
}

#[tokio::test]
async fn arguments_too_large_to_show_are_not_held() {
    let (_directory, database) = seeded_database().await;
    let _run = live_run(&database).await;
    let request = approval_for("jarvis.web.fetch", &serde_json::json!({}));
    let id = request.id().to_string();
    must(create_approval(&database, &request).await);

    let huge = format!(
        "{{\"text\":\"{}\"}}",
        "x".repeat(MAX_APPROVAL_ARGUMENTS_BYTES)
    );
    assert!(!must(
        attach_approval_arguments(&database, &id, &huge).await
    ));
    assert_eq!(must(read_approval_arguments(&database, &id).await), None);
}

#[tokio::test]
async fn the_workspace_pending_list_excludes_decided_and_lapsed_approvals() {
    let (_directory, database) = seeded_database().await;
    let _run = live_run(&database).await;
    let pending = approval_expiring_at("jarvis.a.pending", &serde_json::json!({"n": 1}), 10);
    let decided = approval_expiring_at("jarvis.b.decided", &serde_json::json!({"n": 2}), 10);
    let lapsed = approval_expiring_at("jarvis.c.lapsed", &serde_json::json!({"n": 3}), 1);
    for request in [&pending, &decided, &lapsed] {
        must(create_approval(&database, request).await);
    }
    must(record_decision(&database, &decided.id().to_string(), &approve(1)).await);

    let listed = must(read_workspace_pending_approvals(&database, LOCAL_WORKSPACE_ID, at(5)).await);
    let tools: Vec<&str> = listed.iter().map(ApprovalRequest::tool).collect();
    assert_eq!(tools, vec!["jarvis.a.pending"], "got {tools:?}");

    let elsewhere = must(
        read_workspace_pending_approvals(&database, "0198f000-0000-7000-8000-00000000ffff", at(5))
            .await,
    );
    assert!(
        elsewhere.is_empty(),
        "another workspace's approvals must never be listed"
    );
}
