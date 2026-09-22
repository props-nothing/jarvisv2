//! In-crate tool call tests: duplicate detection, bounded output, and honest outcomes.
//!
//! The properties worth naming, because each is a case where the obvious implementation is wrong:
//!
//! - **the idempotency ledger** (the unique index) makes a re-driven pipeline adopt the existing call
//!   rather than make a second one;
//! - **a deliberate second call with identical arguments is a second call**, because the key is
//!   generated at admission rather than derived from the intent;
//! - **a terminal outcome cannot be replaced**, which is what stops an `Unknown` becoming a `Failed`;
//! - **`must_not_repeat` is true for an in-flight or ambiguous call**, which is the predicate a
//!   recovery path decides with.
//!
//! In-crate rather than in `tests/` for the same reason the approval tests are: the storage-integrity
//! test needs the pool handle to reproduce a constraint-free writer, and that handle is crate-private.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use super::*;
use crate::{
    CallBinding, CallOrigin, CallTarget, DEFAULT_DATABASE_FILENAME, DatabaseError, LOCAL_USER_ID,
    LOCAL_WORKSPACE_ID, NewRun, NewToolCall, SqliteDatabase, create_run,
};
use jarvis_core::ToolOutcomeRecord;

static TEST_DIRECTORY_ID: AtomicU64 = AtomicU64::new(0);

const SESSION: &str = "0198f000-0000-7000-8000-000000000003";
const RUN: &str = "0198f000-0000-7000-8000-0000000000c3";
const CALL: &str = "0198f000-0000-7000-8000-0000000000d1";

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let sequence = TEST_DIRECTORY_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "jarvis-tool-calls-{}-{sequence}",
            std::process::id()
        ));
        must(fs::create_dir_all(&path));
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn must<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("expected success, got {error:?}"),
    }
}

/// Reaches `submitted` legally, which needs `authorized` first.
///
/// `requested -> submitted` is **not** a legal edge: a call must be authorized before an adapter
/// receives it. Reaching it directly is refused by the transition table, which is why every test that
/// wants a submitted call goes through this helper rather than one call.
async fn reach_submitted(database: &SqliteDatabase, id: &str, at: UtcTimestamp) -> StoredToolCall {
    must(advance_tool_call(database, id, ToolOutcome::Authorized, at).await);
    must(advance_tool_call(database, id, ToolOutcome::Submitted, at).await)
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

async fn live_run(database: &SqliteDatabase) -> String {
    let new = must(NewRun::new(
        RUN,
        SESSION,
        LOCAL_WORKSPACE_ID,
        LOCAL_USER_ID,
        "Send the quarterly report",
        CorrelationId::new(),
        at(0),
    ));
    let run = must(create_run(database, &new).await);
    run.id().to_owned()
}

/// A distinct key per call site, so a duplicate is deliberate rather than accidental.
fn key_for(discriminator: u8) -> String {
    format!("0123456789abcdef0123456789abcde{discriminator}")
}

fn call(id: &str, key: &str) -> NewToolCall {
    must(NewToolCall::new(
        id,
        CallOrigin::new(LOCAL_WORKSPACE_ID, RUN, None),
        CallTarget::new("jarvis.mail.send", "1.0.0"),
        CallBinding::new(
            "a".repeat(64),
            key,
            r#"{"receipt_id":"0198f000-0000-7000-8000-0000000000e1"}"#,
            "policy-3",
            None,
        ),
        CorrelationId::new(),
        at(0),
    ))
}

/// A call round-trips: it is admitted, read back, and every field agrees.
#[tokio::test]
async fn a_call_round_trips() {
    let (_directory, database) = seeded_database().await;
    let _run = live_run(&database).await;
    let admitted = must(admit_tool_call(&database, &call(CALL, &key_for(1))).await);
    assert_eq!(admitted, CALL);

    let stored = must(find_tool_call(&database, CALL).await);
    assert_eq!(stored.id(), CALL);
    assert_eq!(stored.run_id(), RUN);
    assert_eq!(stored.workspace_id(), LOCAL_WORKSPACE_ID);
    assert_eq!(stored.tool(), "jarvis.mail.send");
    assert_eq!(stored.tool_version(), "1.0.0");
    assert_eq!(stored.intent_hash(), "a".repeat(64));
    assert_eq!(stored.idempotency_key(), key_for(1));
    assert_eq!(stored.policy_version(), "policy-3");
    assert_eq!(stored.approval_id(), None);
    assert_eq!(stored.step_id(), None);
    assert_eq!(stored.outcome(), ToolOutcome::Requested);
    assert_eq!(stored.output(), None);
    assert!(!stored.output_truncated());
    assert_eq!(stored.reported_at(), None);
    assert_eq!(stored.version(), 1);
    assert_eq!(stored.created_at(), at(0));
    assert!(
        stored.must_not_repeat(),
        "an admitted but unrun call must not be repeated blindly"
    );
}

/// **The idempotency ledger: a re-drive finds the existing call instead of making a second one.**
///
/// The duplicate result carries the existing identifier, so the caller adopts it. This is the
/// "idempotency/duplicate check" step in the pipeline diagram.
#[tokio::test]
async fn a_duplicate_key_returns_the_existing_call() {
    let (_directory, database) = seeded_database().await;
    let _run = live_run(&database).await;
    must(admit_tool_call(&database, &call(CALL, &key_for(1))).await);

    // A different call identifier under the same key: the second admission must report the first.
    let duplicate_id = "0198f000-0000-7000-8000-0000000000d2";
    let refused = admit_tool_call(&database, &call(duplicate_id, &key_for(1))).await;
    match refused {
        Err(DatabaseError::ToolCallDuplicate { existing_call_id }) => {
            assert_eq!(
                existing_call_id, CALL,
                "the existing identifier must be returned so the caller adopts it"
            );
        }
        other => panic!("expected a duplicate, got {other:?}"),
    }

    let calls = must(read_run_tool_calls(&database, RUN).await);
    assert_eq!(calls.len(), 1, "no second call may be recorded");
}

/// **A deliberate second call with identical arguments is a second call.**
///
/// The reason the key is generated at admission rather than derived from the intent: "archive this
/// folder, then archive it again after a new message arrived" has one intent hash and two calls.
#[tokio::test]
async fn a_second_call_with_the_same_intent_is_a_second_call() {
    let (_directory, database) = seeded_database().await;
    let _run = live_run(&database).await;
    let first = "0198f000-0000-7000-8000-0000000000d1";
    let second = "0198f000-0000-7000-8000-0000000000d2";

    must(admit_tool_call(&database, &call(first, &key_for(1))).await);
    must(admit_tool_call(&database, &call(second, &key_for(2))).await);

    let calls = must(read_run_tool_calls(&database, RUN).await);
    assert_eq!(calls.len(), 2);
    // Both carry the SAME intent hash, which is the point: the intent does not distinguish them.
    assert_eq!(calls[0].intent_hash(), calls[1].intent_hash());
    assert_ne!(calls[0].idempotency_key(), calls[1].idempotency_key());
}

/// A run's calls are listed oldest first, reproducibly.
#[tokio::test]
async fn a_runs_calls_are_listed_reproducibly() {
    let (_directory, database) = seeded_database().await;
    let _run = live_run(&database).await;
    for discriminator in 1..=3 {
        let id = format!("0198f000-0000-7000-8000-0000000000d{discriminator}");
        must(admit_tool_call(&database, &call(&id, &key_for(discriminator))).await);
    }
    let first: Vec<String> = must(read_run_tool_calls(&database, RUN).await)
        .iter()
        .map(|call| call.id().to_owned())
        .collect();
    let second: Vec<String> = must(read_run_tool_calls(&database, RUN).await)
        .iter()
        .map(|call| call.id().to_owned())
        .collect();
    assert_eq!(first.len(), 3);
    assert_eq!(first, second, "the order must be a function of the rows");
}

/// **A confirmed outcome is stored with its evidence and output.**
#[tokio::test]
async fn a_confirmed_outcome_stores_evidence_and_output() {
    let (_directory, database) = seeded_database().await;
    let _run = live_run(&database).await;
    must(admit_tool_call(&database, &call(CALL, &key_for(1))).await);
    must(advance_tool_call(&database, CALL, ToolOutcome::Authorized, at(1)).await);
    must(advance_tool_call(&database, CALL, ToolOutcome::Submitted, at(2)).await);

    let record = must(ToolOutcomeRecord::confirmed("provider-message-id-12345"));
    let stored = must(
        record_tool_outcome(
            &database,
            CALL,
            &record,
            Some(("sent to 1 recipient", false)),
            at(3),
        )
        .await,
    );

    assert_eq!(stored.outcome(), ToolOutcome::Confirmed);
    assert_eq!(
        stored.record().evidence(),
        Some("provider-message-id-12345")
    );
    assert_eq!(stored.output(), Some("sent to 1 recipient"));
    assert!(!stored.output_truncated());
    assert_eq!(stored.reported_at(), Some(at(3)));
    assert_eq!(stored.version(), 4, "each write advances the version");
    assert!(
        !stored.must_not_repeat(),
        "a proven effect is not repeatable, and not in flight either"
    );
}

/// **A failed outcome says nothing happened, so a repeat is permitted from the outcome alone.**
#[tokio::test]
async fn a_failed_outcome_permits_a_repeat() {
    let (_directory, database) = seeded_database().await;
    let _run = live_run(&database).await;
    must(admit_tool_call(&database, &call(CALL, &key_for(1))).await);
    let record = must(ToolOutcomeRecord::failed("the provider returned 403"));
    let stored = must(record_tool_outcome(&database, CALL, &record, None, at(1)).await);

    assert_eq!(stored.outcome(), ToolOutcome::Failed);
    assert_eq!(stored.record().reason(), Some("the provider returned 403"));
    assert!(
        !stored.must_not_repeat(),
        "a dis-proven effect may be repeated"
    );
}

/// **An unknown outcome may have had an effect, so it must not be repeated.**
///
/// The distinction that decides a retry, asserted through the stored record rather than the enum.
#[tokio::test]
async fn an_unknown_outcome_must_not_be_repeated() {
    let (_directory, database) = seeded_database().await;
    let _run = live_run(&database).await;
    must(admit_tool_call(&database, &call(CALL, &key_for(1))).await);
    reach_submitted(&database, CALL, at(1)).await;
    let record = must(ToolOutcomeRecord::new(ToolOutcome::Unknown));
    let stored = must(record_tool_outcome(&database, CALL, &record, None, at(2)).await);

    assert_eq!(stored.outcome(), ToolOutcome::Unknown);
    assert!(stored.outcome().may_have_had_an_effect());
    assert!(stored.must_not_repeat());

    // A submitted call is also unrepeatable, because it is still in flight.
    let (_second_directory, second_database) = seeded_database().await;
    let _second_run = live_run(&second_database).await;
    let second_call = "0198f000-0000-7000-8000-0000000000d2";
    must(admit_tool_call(&second_database, &call(second_call, &key_for(2))).await);
    let submitted = reach_submitted(&second_database, second_call, at(1)).await;
    assert!(
        submitted.must_not_repeat(),
        "an in-flight call is unrepeatable"
    );
}

/// **A terminal outcome cannot be replaced by a different one.**
///
/// The case that matters: an `Unknown` overwritten by a `Failed` after a re-drive would claim no
/// effect happened when one may have. A repeat of the SAME outcome is a no-op, so a retried write is
/// not an error.
#[tokio::test]
async fn a_terminal_outcome_cannot_be_replaced() {
    let (_directory, database) = seeded_database().await;
    let _run = live_run(&database).await;
    must(admit_tool_call(&database, &call(CALL, &key_for(1))).await);
    reach_submitted(&database, CALL, at(1)).await;
    let unknown = must(ToolOutcomeRecord::new(ToolOutcome::Unknown));
    must(record_tool_outcome(&database, CALL, &unknown, None, at(2)).await);

    // Replacing it with a failure is refused.
    let failed = must(ToolOutcomeRecord::failed("the provider refused"));
    match record_tool_outcome(&database, CALL, &failed, None, at(3)).await {
        Err(DatabaseError::ToolCallAlreadyResolved { existing }) => {
            assert_eq!(existing, "unknown");
        }
        other => panic!("expected an already-resolved refusal, got {other:?}"),
    }

    // Replacing it with a confirmation is refused too, so a re-drive cannot invent proof.
    let confirmed = must(ToolOutcomeRecord::confirmed("invented-id"));
    assert!(matches!(
        record_tool_outcome(&database, CALL, &confirmed, None, at(3)).await,
        Err(DatabaseError::ToolCallAlreadyResolved { .. })
    ));

    // Writing the same outcome again is a harmless no-op.
    let unchanged = must(record_tool_outcome(&database, CALL, &unknown, None, at(4)).await);
    assert_eq!(unchanged.outcome(), ToolOutcome::Unknown);
    assert_eq!(
        unchanged.version(),
        4,
        "a no-op must not advance the version: admit, authorize, submit, report"
    );

    let stored = must(find_tool_call(&database, CALL).await);
    assert_eq!(
        stored.outcome(),
        ToolOutcome::Unknown,
        "the original outcome must survive every attempt"
    );
}

/// The transition table is enforced on the write path, so a skip is refused.
///
/// `requested -> submitted` is included deliberately: it is the edge that looks legal — the call was
/// authorized a moment ago in the pipeline diagram — and is refused, because a call that never passed
/// `authorized` has no receipt, and an adapter must never be handed one.
#[tokio::test]
async fn a_skipped_transition_is_refused() {
    let (_directory, database) = seeded_database().await;
    let _run = live_run(&database).await;
    must(admit_tool_call(&database, &call(CALL, &key_for(1))).await);

    // `requested -> confirmed` is not a legal edge: evidence cannot exist for an un-sent request.
    let confirmed = must(ToolOutcomeRecord::confirmed("provider-id-1"));
    assert!(matches!(
        record_tool_outcome(&database, CALL, &confirmed, None, at(1)).await,
        Err(DatabaseError::StoredToolCallInvalid { field: "outcome" })
    ));

    // `requested -> submitted` is refused too, so the authorization step cannot be skipped.
    assert!(matches!(
        advance_tool_call(&database, CALL, ToolOutcome::Submitted, at(1)).await,
        Err(DatabaseError::StoredToolCallInvalid { field: "outcome" })
    ));

    let stored = must(find_tool_call(&database, CALL).await);
    assert_eq!(stored.outcome(), ToolOutcome::Requested);
    assert_eq!(
        stored.version(),
        1,
        "a refused transition must not advance the row"
    );
}

/// A call's output is bounded by the migration, so an oversized one is refused.
#[tokio::test]
async fn an_oversized_output_is_refused_by_storage() {
    let (_directory, database) = seeded_database().await;
    let _run = live_run(&database).await;
    must(admit_tool_call(&database, &call(CALL, &key_for(1))).await);
    reach_submitted(&database, CALL, at(1)).await;
    let record = must(ToolOutcomeRecord::confirmed("provider-id-oversized"));
    let oversized = "x".repeat(32769);
    let refused =
        record_tool_outcome(&database, CALL, &record, Some((&oversized, true)), at(2)).await;
    assert!(
        matches!(refused, Err(DatabaseError::Sqlite { .. })),
        "an oversized output must be refused, got {refused:?}"
    );
}

/// **A truncation flag is stored, so a reader can tell a short output from a cut one.**
#[tokio::test]
async fn a_truncation_flag_survives_the_round_trip() {
    let (_directory, database) = seeded_database().await;
    let _run = live_run(&database).await;
    must(admit_tool_call(&database, &call(CALL, &key_for(1))).await);
    reach_submitted(&database, CALL, at(1)).await;
    // `submitted -> unknown` is the smallest legal step that can carry an output: an adapter that
    // answered with a bounded prefix and then could not be reached again reports `unknown`.
    let record = must(ToolOutcomeRecord::new(ToolOutcome::Unknown));
    let stored = must(
        record_tool_outcome(
            &database,
            CALL,
            &record,
            Some(("partial content", true)),
            at(2),
        )
        .await,
    );
    assert!(stored.output_truncated());
    assert_eq!(stored.output(), Some("partial content"));

    let reread = must(find_tool_call(&database, CALL).await);
    assert!(
        reread.output_truncated(),
        "the flag must survive, so the reader knows the content is partial"
    );
}

/// A missing call is a not-found.
#[tokio::test]
async fn a_missing_call_is_not_found() {
    let (_directory, database) = seeded_database().await;
    let absent = "0198f000-0000-7000-8000-0000000000ff";
    assert!(matches!(
        find_tool_call(&database, absent).await,
        Err(DatabaseError::ToolCallNotFound)
    ));
    let record = must(ToolOutcomeRecord::new(ToolOutcome::Submitted));
    assert!(matches!(
        record_tool_outcome(&database, absent, &record, None, at(1)).await,
        Err(DatabaseError::ToolCallNotFound)
    ));
}

/// A call for a run that does not exist is refused by the foreign key.
#[tokio::test]
async fn a_call_for_a_missing_run_is_refused() {
    let (_directory, database) = seeded_database().await;
    // No run is created, so the reference cannot resolve.
    let refused = admit_tool_call(&database, &call(CALL, &key_for(1))).await;
    assert!(
        matches!(refused, Err(DatabaseError::Sqlite { .. })),
        "a dangling run reference must be refused, got {refused:?}"
    );
}

/// A malformed digest or key is refused before it reaches storage.
///
/// Both are compared as text later, so a malformed value would be stored happily and then never match
/// — failing silently at the point where an approval is revalidated.
#[tokio::test]
async fn a_malformed_digest_or_key_is_refused() {
    let (_directory, database) = seeded_database().await;
    let _run = live_run(&database).await;

    for (field, hash, key) in [
        ("intent_hash", "A".repeat(64), key_for(1)),
        ("intent_hash", "a".repeat(63), key_for(1)),
        ("intent_hash", "z".repeat(64), key_for(1)),
        ("idempotency_key", "a".repeat(64), "A".repeat(32)),
        ("idempotency_key", "a".repeat(64), "a".repeat(31)),
    ] {
        let built = NewToolCall::new(
            CALL,
            CallOrigin::new(LOCAL_WORKSPACE_ID, RUN, None),
            CallTarget::new("jarvis.mail.send", "1.0.0"),
            CallBinding::new(hash, key, r#"{"receipt_id":"x"}"#, "policy-3", None),
            CorrelationId::new(),
            at(0),
        );
        match built {
            Err(DatabaseError::InvalidToolCallRequest { field: reported }) => {
                assert_eq!(reported, field);
            }
            other => panic!("expected {field} to be refused, got {other:?}"),
        }
    }
}

/// **A row whose outcome contradicts its evidence is reported, not trusted.**
///
/// The storage-integrity finding: the migration's `CHECK` makes a confirmed row without evidence
/// unstorable, and the decode re-checks in case a row arrived some other way. The plain update is
/// asserted to be rejected first, so this proves the `CHECK` exists too.
#[tokio::test]
async fn a_row_whose_outcome_contradicts_its_evidence_is_reported() {
    let (_directory, database) = seeded_database().await;
    let _run = live_run(&database).await;
    must(admit_tool_call(&database, &call(CALL, &key_for(1))).await);
    reach_submitted(&database, CALL, at(1)).await;
    let confirmed = must(ToolOutcomeRecord::confirmed("provider-id-1"));
    must(record_tool_outcome(&database, CALL, &confirmed, None, at(2)).await);

    // The CHECK refuses a confirmed row whose evidence was cleared.
    let rejected = sqlx::query("UPDATE tool_calls SET evidence = NULL WHERE id = ?1")
        .bind(CALL)
        .execute(database.pool())
        .await;
    assert!(
        rejected.is_err(),
        "the CHECK must refuse a confirmation with no evidence"
    );

    // A constraint-free writer, on ONE held connection: `PRAGMA` is per-connection, so issuing it
    // through the pool would apply it to whichever connection the pool handed out.
    let mut connection = must(database.pool().acquire().await);
    must(
        sqlx::query("PRAGMA ignore_check_constraints = ON")
            .execute(&mut *connection)
            .await
            .map(|_| ()),
    );
    must(
        sqlx::query("UPDATE tool_calls SET evidence = NULL WHERE id = ?1")
            .bind(CALL)
            .execute(&mut *connection)
            .await
            .map(|_| ()),
    );
    must(connection.close().await);

    let decoded = find_tool_call(&database, CALL).await;
    assert!(
        matches!(decoded, Err(DatabaseError::StoredToolCallInvalid { .. })),
        "a confirmation with no evidence must be reported, got {decoded:?}"
    );
}

/// The unrepeatable set contains exactly the calls a re-drive must skip.
#[tokio::test]
async fn the_unrepeatable_set_is_the_ambiguous_and_in_flight_calls() {
    let (_directory, database) = seeded_database().await;
    let _run = live_run(&database).await;

    // Three calls: one confirmed, one failed, one unknown.
    let confirmed_call = "0198f000-0000-7000-8000-0000000000d1";
    let failed_call = "0198f000-0000-7000-8000-0000000000d2";
    let unknown_call = "0198f000-0000-7000-8000-0000000000d3";
    for (id, discriminator) in [(confirmed_call, 1u8), (failed_call, 2), (unknown_call, 3)] {
        must(admit_tool_call(&database, &call(id, &key_for(discriminator))).await);
        reach_submitted(&database, id, at(1)).await;
    }
    must(
        record_tool_outcome(
            &database,
            confirmed_call,
            &must(ToolOutcomeRecord::confirmed("provider-id-1")),
            None,
            at(2),
        )
        .await,
    );
    must(
        record_tool_outcome(
            &database,
            failed_call,
            &must(ToolOutcomeRecord::failed("the provider refused")),
            None,
            at(2),
        )
        .await,
    );
    must(
        record_tool_outcome(
            &database,
            unknown_call,
            &must(ToolOutcomeRecord::new(ToolOutcome::Unknown)),
            None,
            at(2),
        )
        .await,
    );

    let unrepeatable = must(read_unrepeatable_calls(&database, RUN).await);
    let identifiers: Vec<&str> = unrepeatable.iter().map(StoredToolCall::id).collect();
    assert_eq!(
        identifiers,
        vec![unknown_call],
        "only the ambiguous call is unrepeatable: a proven effect is done, and a failed one is safe"
    );
}
