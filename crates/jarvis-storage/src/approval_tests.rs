//! In-crate approval tests: forgery, replay, expiry, mutation, and stale state.
//!
//! `docs/architecture/security.md` lists those five as required approval suites, and each has a test
//! named after its category below, because a suite that covers a category without naming it cannot
//! show which category is missing.
//!
//! # Why in-crate rather than in `tests/`
//!
//! For one concrete reason: the storage-integrity test reproduces a constraint-free writer to prove
//! the decode-time re-checks fire, and that needs the database's pool handle, which is crate-private.
//! Everything else writes through the repository, so the `CHECK` constraints and the guarded
//! statements are what is under test rather than a fixture's idea of them.

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
    ApprovalState, AuthenticationStrength, CanonicalIntentHash, CorrelationId, DecisionNonce,
    UtcTimestamp,
};

const SESSION: &str = "0198f000-0000-7000-8000-000000000003";
const RUN: &str = "0198f000-0000-7000-8000-0000000000c3";

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

/// A distinct instant per minute offset, so a stored timestamp is attributable to its writer.
fn at(minute: i128) -> UtcTimestamp {
    must(UtcTimestamp::from_unix_nanos(
        1_774_000_000_000_000_000 + minute * 60_000_000_000,
    ))
}

/// Opens a migrated database and seeds the session an approval needs.
///
/// The workspace and user come from the migration's own seeds rather than new rows, so the fixture
/// uses the same identities a real profile has — a fixture that invented its own would not exercise
/// the `REFERENCES` clauses against the seeded values.
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

/// An approval for a risk-3 destructive action needing presence, expiring at a chosen minute.
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
        required_strength: AuthenticationStrength::Present,
        nonce: must(DecisionNonce::generate()),
        correlation_id: CorrelationId::new(),
        created_at: at(0),
        expires_at: at(expires_minute),
    }))
}

/// The common fixture: an approval expiring at minute 10.
fn approval_for(tool: &str, arguments: &serde_json::Value) -> ApprovalRequest {
    approval_expiring_at(tool, arguments, 10)
}

fn approve(minute: i128) -> ApprovalDecision {
    ApprovalDecision::new(
        ApprovalDecisionOutcome::Approve,
        jarvis_core::ApprovalChannel::Desktop,
        AuthenticationStrength::Present,
        at(minute),
    )
}

fn deny(minute: i128) -> ApprovalDecision {
    ApprovalDecision::new(
        ApprovalDecisionOutcome::Deny,
        jarvis_core::ApprovalChannel::Desktop,
        AuthenticationStrength::Present,
        at(minute),
    )
}

/// A request round-trips: it is created, read back, and every field agrees.
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
    assert_eq!(stored.tool(), "jarvis.mail.purge");
    assert_eq!(stored.tool_version(), "1.0.0");
    assert_eq!(stored.intent(), request.intent());
    assert_eq!(stored.preview().as_str(), request.preview().as_str());
    assert_eq!(stored.risk_level(), 3);
    assert_eq!(stored.required_strength(), AuthenticationStrength::Present);
    assert_eq!(stored.correlation_id(), request.correlation_id());
    assert_eq!(stored.created_at(), request.created_at());
    assert_eq!(stored.expires_at(), request.expires_at());
    assert_eq!(stored.stored_state(), ApprovalState::Pending);
    assert!(stored.decision().is_none());
}

/// **The stored row holds a DIGEST of the nonce, never the nonce itself.**
///
/// `events-and-workflows.md`: "Approval is not a permanent bearer token." A row that stored the nonce
/// would be exactly that, so this reads the raw column and asserts the nonce's value is absent from
/// it.
#[tokio::test]
async fn the_stored_row_holds_a_digest_not_the_nonce() {
    let (_directory, database) = seeded_database().await;
    let _run = live_run(&database).await;
    let request = approval_for("jarvis.mail.purge", &serde_json::json!({"all": true}));
    must(create_approval(&database, &request).await);

    let stored = must(stored_nonce_digest(&database, &request.id().to_string()).await);
    assert_eq!(stored, digest(request.nonce_for_storage()));
    assert_ne!(stored, request.nonce_for_storage());
    assert_eq!(stored.len(), 64);
    assert!(
        !stored.contains(request.nonce_for_storage()),
        "the nonce must not appear anywhere in the stored value"
    );
}

/// **A decision with the correct nonce is recorded, and the nonce is then unusable.**
///
/// The nonce rotates to a digest that matches nothing, so the one-time property is a property of the
/// row rather than of caller discipline.
#[tokio::test]
async fn a_valid_decision_is_recorded_and_the_nonce_is_consumed() {
    let (_directory, database) = seeded_database().await;
    let _run = live_run(&database).await;
    let request = approval_for("jarvis.mail.purge", &serde_json::json!({"all": true}));
    must(create_approval(&database, &request).await);

    let decided = must(
        record_decision(
            &database,
            &request.id().to_string(),
            "approver-1",
            request.nonce_for_storage(),
            &approve(1),
        )
        .await,
    );
    assert_eq!(decided.stored_state(), ApprovalState::Approved);
    assert!(decided.authorizes_at(at(2)));
    let recorded = decided
        .decision()
        .unwrap_or_else(|| panic!("a decision must be present"));
    assert_eq!(recorded.outcome(), ApprovalDecisionOutcome::Approve);
    assert_eq!(recorded.channel(), jarvis_core::ApprovalChannel::Desktop);
    assert_eq!(recorded.strength(), AuthenticationStrength::Present);
    assert_eq!(recorded.decided_at(), at(1));

    // The stored digest no longer matches the value that decided it, so presenting it again cannot
    // match even if the state guard were somehow bypassed.
    let after = must(stored_nonce_digest(&database, &request.id().to_string()).await);
    assert_ne!(after, digest(request.nonce_for_storage()));
    assert_eq!(after, digest(""));
}

/// The decision's channel survives the round trip, so a decision is attributable.
#[tokio::test]
async fn a_decisions_attribution_survives_the_round_trip() {
    let (_directory, database) = seeded_database().await;
    let _run = live_run(&database).await;
    let request = approval_for("jarvis.mail.purge", &serde_json::json!({"all": true}));
    must(create_approval(&database, &request).await);
    must(
        record_decision(
            &database,
            &request.id().to_string(),
            "approver-1",
            request.nonce_for_storage(),
            &ApprovalDecision::new(
                ApprovalDecisionOutcome::Approve,
                jarvis_core::ApprovalChannel::Cli,
                AuthenticationStrength::Present,
                at(1),
            ),
        )
        .await,
    );

    let stored = must(find_approval(&database, &request.id().to_string()).await);
    let recorded = stored
        .decision()
        .unwrap_or_else(|| panic!("a decision must be present"));
    assert_eq!(recorded.channel(), jarvis_core::ApprovalChannel::Cli);
    assert_eq!(recorded.decided_at(), at(1));
}

/// **FORGERY: a wrong nonce is refused and nothing is recorded.**
#[tokio::test]
async fn a_forged_nonce_is_refused() {
    let (_directory, database) = seeded_database().await;
    let _run = live_run(&database).await;
    let request = approval_for("jarvis.mail.purge", &serde_json::json!({"all": true}));
    must(create_approval(&database, &request).await);

    let forged = "0".repeat(64);
    let refused = record_decision(
        &database,
        &request.id().to_string(),
        "approver-1",
        &forged,
        &approve(1),
    )
    .await;
    assert!(
        matches!(refused, Err(DatabaseError::ApprovalNonceMismatch)),
        "a forged nonce must be refused as such, got {refused:?}"
    );

    let stored = must(find_approval(&database, &request.id().to_string()).await);
    assert_eq!(
        stored.stored_state(),
        ApprovalState::Pending,
        "a refused decision must record nothing"
    );
    assert!(stored.decision().is_none());
}

/// **REPLAY: a denial cannot be replaced by a later approval, even with the recorded nonce.**
#[tokio::test]
async fn a_recorded_decision_cannot_be_replayed_or_replaced() {
    let (_directory, database) = seeded_database().await;
    let _run = live_run(&database).await;
    let request = approval_for("jarvis.mail.purge", &serde_json::json!({"all": true}));
    must(create_approval(&database, &request).await);

    let denied = must(
        record_decision(
            &database,
            &request.id().to_string(),
            "approver-1",
            request.nonce_for_storage(),
            &deny(1),
        )
        .await,
    );
    assert_eq!(denied.stored_state(), ApprovalState::Denied);

    // Replaying the decision, with the nonce that worked, must be refused.
    let replayed = record_decision(
        &database,
        &request.id().to_string(),
        "approver-1",
        request.nonce_for_storage(),
        &deny(1),
    )
    .await;
    assert!(
        matches!(replayed, Err(DatabaseError::ApprovalAlreadyDecided)),
        "a replay must be reported as already decided, got {replayed:?}"
    );

    // And replacing the denial with an approval must also be refused.
    let replacement = record_decision(
        &database,
        &request.id().to_string(),
        "approver-2",
        request.nonce_for_storage(),
        &approve(2),
    )
    .await;
    assert!(
        matches!(replacement, Err(DatabaseError::ApprovalAlreadyDecided)),
        "a denial must not be replaceable, got {replacement:?}"
    );

    let stored = must(find_approval(&database, &request.id().to_string()).await);
    assert_eq!(
        stored.stored_state(),
        ApprovalState::Denied,
        "the original denial must survive both attempts"
    );
    assert!(!stored.authorizes_at(at(3)));
}

/// **EXPIRY: a decision after the expiry is refused, and an approved row lapses on read.**
///
/// The lapse half is what makes an approval "not a permanent bearer token": a stored `approved` row
/// stops authorizing once its lifetime elapses, without any sweep job running.
#[tokio::test]
async fn a_decision_after_the_expiry_is_refused_and_an_approval_lapses_on_read() {
    let (_directory, database) = seeded_database().await;
    let _run = live_run(&database).await;
    let request = approval_expiring_at("jarvis.mail.purge", &serde_json::json!({"all": true}), 10);
    must(create_approval(&database, &request).await);

    for minute in [10, 11, 100] {
        let late = record_decision(
            &database,
            &request.id().to_string(),
            "approver-1",
            request.nonce_for_storage(),
            &approve(minute),
        )
        .await;
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

    // A decision inside the lifetime is accepted.
    let decided = must(
        record_decision(
            &database,
            &request.id().to_string(),
            "approver-1",
            request.nonce_for_storage(),
            &approve(9),
        )
        .await,
    );
    assert!(decided.authorizes_at(at(9)));
    assert!(
        !decided.authorizes_at(at(10)),
        "an approval must stop authorizing at its expiry"
    );
    assert_eq!(decided.state_at(at(10)), ApprovalState::Expired);
    // The stored state is still `approved`: a row cannot read a clock, so the lapse is a read-side
    // fact rather than a column that would go stale.
    assert_eq!(decided.stored_state(), ApprovalState::Approved);
}

/// **STALE STATE: an undecided approval that has lapsed is not reported as pending.**
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

    // It is still stored and still readable, so an operator can see that it lapsed.
    let stored = must(find_approval(&database, &expired.id().to_string()).await);
    assert_eq!(stored.state_at(at(10)), ApprovalState::Expired);
}

/// **MUTATION: editing the action produces a different intent, so the approval cannot be reused.**
///
/// `security.md`: "Editing the action invalidates the approval." The intent hash is what a resuming
/// execution revalidates, so this asserts the hash differs and that a second approval for the changed
/// action is a distinct row rather than a conflict with the first.
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
    let intents: Vec<String> = stored.iter().map(|a| a.intent().to_hex()).collect();
    assert!(intents.contains(&original.intent().to_hex()));
    assert!(intents.contains(&edited.intent().to_hex()));
}

/// **The identical intent cannot be approved twice in one run.**
///
/// Two pending approvals for one action would mean two prompts for one effect, and answering either
/// would appear to authorize it. The unique index is what makes that unrepresentable.
#[tokio::test]
async fn a_duplicate_intent_in_one_run_is_refused() {
    let (_directory, database) = seeded_database().await;
    let _run = live_run(&database).await;
    let arguments = serde_json::json!({"folder": "inbox"});

    let first = approval_for("jarvis.mail.purge", &arguments.clone());
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

/// **A decided approval still blocks the same intent**, so deciding the first is not a way to request
/// a second prompt for the same action.
#[tokio::test]
async fn a_decided_approval_still_blocks_the_same_intent() {
    let (_directory, database) = seeded_database().await;
    let _run = live_run(&database).await;
    let arguments = serde_json::json!({"folder": "inbox"});
    let first = approval_for("jarvis.mail.purge", &arguments.clone());
    must(create_approval(&database, &first).await);
    must(
        record_decision(
            &database,
            &first.id().to_string(),
            "approver-1",
            first.nonce_for_storage(),
            &deny(1),
        )
        .await,
    );

    let second = approval_for("jarvis.mail.purge", &arguments);
    assert!(
        create_approval(&database, &second).await.is_err(),
        "a decided intent must not be re-requested in the same run; a changed action must be"
    );
}

/// **An already-decided request cannot be created as a new row.**
///
/// A row created decided would bypass the nonce, the strength floor, and the self-approval refusal,
/// so the store refuses it rather than storing a decision that never passed a check.
#[tokio::test]
async fn a_decided_request_cannot_be_created() {
    let (_directory, database) = seeded_database().await;
    let _run = live_run(&database).await;
    let request = approval_for("jarvis.mail.purge", &serde_json::json!({"all": true}));
    must(create_approval(&database, &request).await);
    let decided = must(
        record_decision(
            &database,
            &request.id().to_string(),
            "approver-1",
            request.nonce_for_storage(),
            &approve(1),
        )
        .await,
    );

    // A second run, so the intent index is not what refuses it.
    let new = must(NewRun::new(
        "0198f000-0000-7000-8000-0000000000c4",
        SESSION,
        LOCAL_WORKSPACE_ID,
        LOCAL_USER_ID,
        "Second task",
        CorrelationId::new(),
        at(0),
    ));
    must(create_run(&database, &new).await);

    let refused = create_approval(&database, &decided).await;
    assert!(
        matches!(
            refused,
            Err(DatabaseError::InvalidApprovalRequest { field: "state" })
        ),
        "a decided request must not be creatable, got {refused:?}"
    );
}

/// **SELF-APPROVAL: the requesting actor cannot decide its own approval.**
#[tokio::test]
async fn the_requesting_actor_cannot_decide_its_own_approval() {
    let (_directory, database) = seeded_database().await;
    let _run = live_run(&database).await;
    let request = approval_for("jarvis.mail.purge", &serde_json::json!({"all": true}));
    must(create_approval(&database, &request).await);

    let refused = record_decision(
        &database,
        &request.id().to_string(),
        LOCAL_USER_ID,
        request.nonce_for_storage(),
        &approve(1),
    )
    .await;
    assert!(
        matches!(
            refused,
            Err(DatabaseError::InvalidApprovalRequest {
                field: "decided_by"
            })
        ),
        "self-approval must be refused, got {refused:?}"
    );

    let stored = must(find_approval(&database, &request.id().to_string()).await);
    assert_eq!(stored.stored_state(), ApprovalState::Pending);
}

/// **A decision below the required strength is refused**, so a voice confirmation cannot release a
/// risk-3 action.
#[tokio::test]
async fn a_weak_decision_is_refused() {
    let (_directory, database) = seeded_database().await;
    let _run = live_run(&database).await;
    let request = approval_for("jarvis.mail.purge", &serde_json::json!({"all": true}));
    must(create_approval(&database, &request).await);

    for strength in [
        AuthenticationStrength::Absent,
        AuthenticationStrength::ChannelEvidence,
    ] {
        let refused = record_decision(
            &database,
            &request.id().to_string(),
            "approver-1",
            request.nonce_for_storage(),
            &ApprovalDecision::new(
                ApprovalDecisionOutcome::Approve,
                jarvis_core::ApprovalChannel::Voice,
                strength,
                at(1),
            ),
        )
        .await;
        assert!(
            matches!(
                refused,
                Err(DatabaseError::InvalidApprovalRequest {
                    field: "decision_strength"
                })
            ),
            "{strength} must not release a presence-required approval, got {refused:?}"
        );
    }

    let stored = must(find_approval(&database, &request.id().to_string()).await);
    assert_eq!(stored.stored_state(), ApprovalState::Pending);
}

/// A missing approval is a not-found, distinct from a conflict.
#[tokio::test]
async fn a_missing_approval_is_not_found() {
    let (_directory, database) = seeded_database().await;
    let absent = "0198f000-0000-7000-8000-0000000000ff";
    let found = find_approval(&database, absent).await;
    assert!(
        matches!(found, Err(DatabaseError::ApprovalNotFound)),
        "expected not-found, got {found:?}"
    );

    let decided = record_decision(
        &database,
        absent,
        "approver-1",
        &"0".repeat(64),
        &approve(1),
    )
    .await;
    assert!(
        matches!(decided, Err(DatabaseError::ApprovalNotFound)),
        "expected not-found, got {decided:?}"
    );
}

/// An approval for a run that does not exist is refused by the foreign key.
#[tokio::test]
async fn an_approval_for_a_missing_run_is_refused() {
    let (_directory, database) = seeded_database().await;
    // No run is created, so the reference cannot resolve.
    let request = approval_for("jarvis.mail.purge", &serde_json::json!({"all": true}));
    let refused = create_approval(&database, &request).await;
    assert!(
        matches!(refused, Err(DatabaseError::Sqlite { .. })),
        "a dangling run reference must be refused, got {refused:?}"
    );
}

/// A run's approvals are listed reproducibly.
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
        .map(|a| a.id().to_string())
        .collect();
    let second: Vec<String> = must(read_run_approvals(&database, RUN).await)
        .iter()
        .map(|a| a.id().to_string())
        .collect();
    assert_eq!(first.len(), 3);
    assert_eq!(
        first, second,
        "the order must be a function of the rows, not of the read"
    );
}

/// **A row whose decision attribution is missing is reported, not trusted.**
///
/// The storage-integrity finding: the migration's `CHECK` makes a decided row with no attribution
/// unstorable through this repository, but a row written by another build or restored from a backup
/// is not covered by that argument, so the decoder re-checks. The plain update is asserted to be
/// rejected first, so this test also proves the `CHECK` exists.
///
/// Note what the `CHECK` does and does not enforce. It enforces the **grouping** — a decided row
/// carries all four attribution fields and an undecided one carries none. It cannot enforce *which*
/// decision was made, because `state` is the only column that records that, so there is no second
/// source to disagree with. An earlier version of the decoder compared `state` against the decision
/// and the comparison could never fail; this test is what found that.
#[tokio::test]
async fn a_row_whose_decision_attribution_is_missing_is_reported() {
    let (_directory, database) = seeded_database().await;
    let _run = live_run(&database).await;
    let request = approval_for("jarvis.mail.purge", &serde_json::json!({"all": true}));
    must(create_approval(&database, &request).await);
    must(
        record_decision(
            &database,
            &request.id().to_string(),
            "approver-1",
            request.nonce_for_storage(),
            &deny(1),
        )
        .await,
    );

    // The CHECK rejects a decided row whose attribution was cleared: the grouping is what it
    // enforces.
    let rejected = sqlx::query("UPDATE approvals SET decided_by = NULL WHERE id = ?1")
        .bind(request.id().to_string())
        .execute(database.pool())
        .await;
    assert!(
        rejected.is_err(),
        "the CHECK must reject a decided row with no attribution"
    );

    // A constraint-free writer, so a row another build could have written can be reproduced without
    // editing the schema. `PRAGMA` is per-connection, so one connection is acquired and held: issuing
    // it through the pool would apply it to whichever connection the pool happened to hand out, and
    // the update could land on a different one — which is exactly what happened when this test was
    // first written, and the failure looked like a refused write rather than a misplaced pragma.
    let mut connection = must(database.pool().acquire().await);
    must(
        sqlx::query("PRAGMA ignore_check_constraints = ON")
            .execute(&mut *connection)
            .await
            .map(|_| ()),
    );
    must(
        sqlx::query("UPDATE approvals SET decided_by = '' WHERE id = ?1")
            .bind(request.id().to_string())
            .execute(&mut *connection)
            .await
            .map(|_| ()),
    );
    must(connection.close().await);

    let decoded = find_approval(&database, &request.id().to_string()).await;
    assert!(
        matches!(decoded, Err(DatabaseError::StoredApprovalInvalid { .. })),
        "an empty approver must be reported, got {decoded:?}"
    );
}
