//! Tests for durable memory: the persistence rules, and the four acceptance invariants this slice can
//! prove at the storage layer.
//!
//! # Why these tests are shaped as *adversarial* cases
//!
//! `memory-and-context.md` states its invariants as things that must not happen: an inferred preference must
//! never appear as confirmed fact, a correction must remove the old claim from current retrieval, deletion
//! must remove the text, and one workspace's memory must not enter another's context. Each test below is the
//! attack that invariant describes rather than the happy path, because a happy-path test passes against an
//! implementation that does the wrong thing in every case the invariant is about.

use std::path::{Path, PathBuf};

use super::*;
use crate::{DEFAULT_DATABASE_FILENAME, LOCAL_USER_ID, SqliteDatabase};

const WORKSPACE: &str = "0198f000-0000-7000-8000-000000000001";
const OTHER_WORKSPACE: &str = "0198f000-0000-7000-8000-000000000002";

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("jarvis-memory-{}", jarvis_core::scratch_tag()));
        must(std::fs::create_dir_all(&path));
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

/// A timestamp `minute` minutes from a fixed, non-zero-fraction base.
///
/// The non-zero fraction is deliberate: `UtcTimestamp`'s text form omits the fraction when it is zero, and
/// the omitted form sorts after the fractional one within a second — so a base of exactly `.000000000` would
/// put every value this produces one tick away from mixing widths.
fn at(minute: i128) -> UtcTimestamp {
    const BASE: i128 = 1_774_000_000_500_000_000;
    must(UtcTimestamp::from_unix_nanos(
        BASE + minute * 60_000_000_000,
    ))
}

async fn seeded_database() -> (TestDirectory, SqliteDatabase) {
    let directory = TestDirectory::new();
    let database =
        must(SqliteDatabase::open(&directory.path().join(DEFAULT_DATABASE_FILENAME)).await);
    // The second workspace exists so a boundary test can name a real one rather than a fabricated identifier
    // — a foreign key would refuse the latter, and the test would pass for the wrong reason.
    for (id, name) in [(WORKSPACE, "first"), (OTHER_WORKSPACE, "second")] {
        must(
            sqlx::query(
                "INSERT INTO workspaces \
                    (id, name, mode, data_policy, status, created_at, updated_at, version) \
                 VALUES (?1, ?2, 'local', 'standard', 'active', ?3, ?3, 1)",
            )
            .bind(id)
            .bind(name)
            .bind(at(0).to_string())
            .execute(database.pool())
            .await
            .map(|_| ()),
        );
    }
    (directory, database)
}

fn workspace() -> WorkspaceId {
    must(WORKSPACE.parse())
}

fn other_workspace() -> WorkspaceId {
    must(OTHER_WORKSPACE.parse())
}

fn entity_id() -> EntityId {
    EntityId::new()
}

fn user_source() -> MemorySource {
    must(MemorySource::of_kind(
        MemorySourceKind::UserStatement,
        "session:0198f000-0000-7000-8000-000000000003",
    ))
}

/// A confirmed preference, which is the fixture the acceptance invariants are about.
fn preference(subject: EntityId) -> MemoryRecord {
    record_of(
        MemoryType::Preference,
        "Prefers dark roast coffee",
        user_source(),
        MemoryConfidence::Confirmed,
        subject,
    )
}

/// A memory with the arguments a test varies.
fn record_of(
    memory_type: MemoryType,
    content: &str,
    source: MemorySource,
    confidence: MemoryConfidence,
    subject: EntityId,
) -> MemoryRecord {
    record_at(memory_type, content, source, confidence, subject, at(0))
}

/// The same fixture with an explicit instant, for the tests that order by one.
///
/// A parameter rather than a field the caller mutates, because `MemoryRecord` has no `parts` accessor: a
/// test needing a chosen instant has to build one, and doing that by hand would be a second copy of the
/// fixture body that could drift from this one.
fn record_at(
    memory_type: MemoryType,
    content: &str,
    source: MemorySource,
    confidence: MemoryConfidence,
    subject: EntityId,
    created_at: UtcTimestamp,
) -> MemoryRecord {
    must(MemoryRecord::new(MemoryRecordParts {
        id: MemoryId::new(),
        workspace_id: workspace(),
        memory_type,
        content: content.to_owned(),
        structured_claim: None,
        source,
        confidence,
        importance: 1,
        sensitivity: Sensitivity::Internal,
        entities: vec![EntityRef::confirmed(subject)],
        valid_from: None,
        valid_until: None,
        supersedes: None,
        run_id: None,
        created_by_actor_id: LOCAL_USER_ID.to_owned(),
        correlation_id: CorrelationId::new(),
        created_at,
    }))
}

fn key_for(record: &MemoryRecord) -> MemorySearchKey {
    must(MemorySearchKey::new(
        record.memory_type(),
        record.entities(),
        record.content(),
    ))
}

/// A subject entity must exist before a memory links to it, because the link carries a foreign key.
async fn a_subject(database: &SqliteDatabase) -> EntityId {
    let id = entity_id();
    must(
        record_entity(
            database,
            &NewEntity {
                id,
                workspace_id: workspace(),
                kind: EntityKind::Person,
                label: "Test Person".to_owned(),
                attributes: None,
                confidence: MemoryConfidence::Confirmed,
                created_at: at(0),
            },
        )
        .await,
    );
    id
}

/// Returns the field a decode refused, for the tests that write a broken row by hand.
///
/// Reads through the error rather than comparing a whole `Result`, because `StoredMemory` carries a decoded
/// record and is deliberately not `PartialEq` — comparing two of them would be a test of the decoder's output
/// rather than of the rule under test.
async fn refusal_field(database: &SqliteDatabase, id: &str) -> &'static str {
    match find_memory(database, id).await {
        Err(DatabaseError::StoredMemoryInvalid { field }) => field,
        other => panic!("expected a stored-memory refusal, got {other:?}"),
    }
}

// ------------------------------------------------------------------------------------------------
// Invariant: admission names its approver, and the row holds the decision
// ------------------------------------------------------------------------------------------------

/// **A confirmation records the approver and the moment, and both come back from the row.**
///
/// `P4-014` through the whole write path, which is the claim the requirement actually makes: the acceptance is
/// durable rather than a reply field. Asserted against a **subsequent read** rather than the returned value,
/// because the returned value is what an implementation that never wrote the columns would still produce.
///
/// The pair is asserted together and separately: a write that stored only one of the two would satisfy a check
/// for either alone, and the domain refuses half a decision on the way back in — so a row carrying one would be
/// unreadable, which is the failure this catches.
#[tokio::test]
async fn a_confirmation_records_its_approver_and_survives_the_round_trip() {
    let (_directory, database) = seeded_database().await;
    let subject = a_subject(&database).await;
    let memory = preference(subject);
    let key = key_for(&memory);
    must(record_memory(&database, &memory, &key).await);

    // A preference from a user statement starts `active` and names nobody: it needed no accepting.
    let read = must(find_memory(&database, &memory.id().to_string()).await);
    assert_eq!(read.record().admitted_by_actor_id(), None);
    assert_eq!(read.record().admitted_at(), None);

    // A relationship claim is the type the domain makes a proposal, so the fixture goes through the real
    // constructor rather than being handed a status.
    let proposal = record_of(
        MemoryType::Relationship,
        "Works with Dana",
        must(MemorySource::of_kind(
            MemorySourceKind::UserStatement,
            "session:0198f000-0000-7000-8000-000000000003",
        )),
        MemoryConfidence::Confirmed,
        subject,
    );
    assert_eq!(proposal.status(), MemoryStatus::Proposed);
    let proposal_key = key_for(&proposal);
    must(record_memory(&database, &proposal, &proposal_key).await);

    let confirmed = must(
        apply_memory_transition(
            &database,
            &proposal.id().to_string(),
            MemoryTransition::Confirm {
                approver_actor_id: "0198f000-0000-7000-8000-0000000000b2".to_owned(),
            },
            at(5),
        )
        .await,
    );

    let read_back = must(find_memory(&database, &proposal.id().to_string()).await);
    assert_eq!(read_back.record().status(), MemoryStatus::Active);
    assert_eq!(
        read_back.record().admitted_by_actor_id(),
        Some("0198f000-0000-7000-8000-0000000000b2"),
        "the approver must be in the row, not only in the returned value"
    );
    assert_eq!(
        read_back.record().admitted_at(),
        Some(at(5)),
        "and the moment, so the pair travels together"
    );
    assert_eq!(confirmed.version(), read_back.version());

    // **A later correction keeps the admission.** Correcting archives the claim, and an archive yields `None`
    // for both fields — so the write has to take them from the record rather than from the transition, or the
    // record of who accepted a claim would be erased at the moment it was superseded, which is when an audit
    // would want it.
    let replacement = record_of(
        MemoryType::Relationship,
        "Works closely with Dana",
        must(MemorySource::of_kind(
            MemorySourceKind::UserStatement,
            "session:0198f000-0000-7000-8000-000000000003",
        )),
        MemoryConfidence::Confirmed,
        subject,
    );
    let replacement = must(replacement.replace_with(proposal.id(), at(6)));
    let replacement_key = key_for(&replacement);
    must(record_memory(&database, &replacement, &replacement_key).await);
    let archived = must(
        apply_memory_transition(
            &database,
            &proposal.id().to_string(),
            MemoryTransition::ReplaceWith(replacement.id()),
            at(6),
        )
        .await,
    );
    assert_eq!(archived.record().status(), MemoryStatus::Archived);
    assert_eq!(
        archived.record().admitted_by_actor_id(),
        Some("0198f000-0000-7000-8000-0000000000b2"),
        "archiving a confirmed claim must not erase who confirmed it"
    );
}

/// **A row whose admission columns disagree with each other or with its status is refused on read.**
///
/// The schema cannot carry this rule for `memories`: `ALTER TABLE ADD COLUMN` takes only a column-def, so a
/// `CHECK` mentioning `status` is rejected as soon as it is added. The decode is therefore the only enforcer,
/// and this writes the three broken shapes directly to be sure it fires — a row that arrived from another
/// build, a restored backup, or a hand edit is exactly what the rule exists for.
#[tokio::test]
async fn an_inconsistent_admission_row_is_refused_on_read() {
    let (_directory, database) = seeded_database().await;
    let subject = a_subject(&database).await;
    let memory = preference(subject);
    let key = key_for(&memory);
    must(record_memory(&database, &memory, &key).await);
    let id = memory.id().to_string();

    // A proposed row naming an approver: accepted and awaiting acceptance at once. Written by hand because no
    // code path can produce it -- `confirm_by` also sets the status.
    let written = sqlx::query(
        "UPDATE memories SET status = 'proposed', admitted_by_actor_id = 'approver', \
            admitted_at = '2026-01-01T00:00:00Z' WHERE id = ?1",
    )
    .bind(&id)
    .execute(database.pool())
    .await;
    assert!(
        written.is_ok(),
        "the fixture must bypass the domain to write this"
    );
    assert_eq!(
        refusal_field(&database, &id).await,
        "admitted_at",
        "a proposal must not name an approver"
    );

    // Half a decision: an approver with no moment.
    let written = sqlx::query(
        "UPDATE memories SET status = 'active', admitted_by_actor_id = 'approver', admitted_at = NULL \
         WHERE id = ?1",
    )
    .bind(&id)
    .execute(database.pool())
    .await;
    assert!(written.is_ok());
    assert_eq!(
        refusal_field(&database, &id).await,
        "admitted_at",
        "an approver with no acceptance time is half a decision"
    );

    // And the other half, which is the direction a write that stored only the timestamp would produce.
    let written = sqlx::query(
        "UPDATE memories SET status = 'active', admitted_by_actor_id = NULL, \
            admitted_at = '2026-01-01T00:00:00Z' WHERE id = ?1",
    )
    .bind(&id)
    .execute(database.pool())
    .await;
    assert!(written.is_ok());
    assert_eq!(
        refusal_field(&database, &id).await,
        "admitted_at",
        "an acceptance time with no approver is half a decision"
    );

    // The control: a complete, consistent admission decodes, so the rule is not "refuse any row with these
    // columns set".
    let written = sqlx::query(
        "UPDATE memories SET status = 'active', admitted_by_actor_id = 'approver', \
            admitted_at = '2026-01-01T00:00:00Z' WHERE id = ?1",
    )
    .bind(&id)
    .execute(database.pool())
    .await;
    assert!(written.is_ok());
    let decoded = must(find_memory(&database, &id).await);
    assert_eq!(decoded.record().admitted_by_actor_id(), Some("approver"));
}

// ------------------------------------------------------------------------------------------------
// The canonical round trip
// ------------------------------------------------------------------------------------------------

/// **A memory round-trips: it is recorded, read back, and every field agrees.**
///
/// The positive control. Without it, every refusal test below could pass on a path that refused everything,
/// and the persistence claim would be untested.
#[tokio::test]
async fn a_memory_round_trips() {
    let (_directory, database) = seeded_database().await;
    let subject = a_subject(&database).await;
    let memory = preference(subject);
    let key = key_for(&memory);

    let stored_id = must(record_memory(&database, &memory, &key).await);
    assert_eq!(stored_id, memory.id().to_string());

    let stored = must(find_memory(&database, &stored_id).await);
    let read = stored.record();
    assert_eq!(read.id(), memory.id());
    assert_eq!(read.workspace_id(), workspace());
    assert_eq!(read.memory_type(), MemoryType::Preference);
    assert_eq!(read.content(), "Prefers dark roast coffee");
    assert_eq!(read.confidence(), MemoryConfidence::Confirmed);
    assert_eq!(read.status(), MemoryStatus::Active);
    assert_eq!(read.source().kind(), MemorySourceKind::UserStatement);
    assert_eq!(read.source().trust(), MemoryTrust::Authoritative);
    assert_eq!(
        read.source().locator(),
        "session:0198f000-0000-7000-8000-000000000003"
    );
    assert_eq!(read.entities().len(), 1);
    assert_eq!(read.entities()[0].entity_id(), subject);
    assert_eq!(read.entities()[0].matched_by(), EntityMatch::Confirmed);
    assert_eq!(stored.search_key(), Some(key.as_str()));
    assert_eq!(stored.version(), 1);
    assert_eq!(read.retrieval_count(), 0);
    assert!(read.last_accessed_at().is_none());
}

/// **A structured claim survives the round trip.**
///
/// It is stored as a JSON document, so the decode is a parse rather than a column read — which is a place a
/// field could silently go missing.
#[tokio::test]
async fn a_structured_claim_round_trips() {
    let (_directory, database) = seeded_database().await;
    let subject = a_subject(&database).await;
    let mut memory = preference(subject);
    let mut parts = deconstruct(&memory);
    parts.structured_claim = Some(must(StructuredClaim::new("user", "prefers", "dark roast")));
    memory = must(MemoryRecord::new(parts));
    let key = key_for(&memory);

    must(record_memory(&database, &memory, &key).await);
    let read = must(find_memory(&database, &memory.id().to_string()).await);
    let claim = read
        .record()
        .structured_claim()
        .unwrap_or_else(|| panic!("the claim must survive"));
    assert_eq!(claim.subject(), "user");
    assert_eq!(claim.predicate(), "prefers");
    assert_eq!(claim.object(), "dark roast");
    assert_eq!(claim.as_key(), "user|prefers|dark roast");
}

/// Rebuilds parts from a record, so a test can vary one field without restating fifteen.
fn deconstruct(record: &MemoryRecord) -> MemoryRecordParts {
    MemoryRecordParts {
        id: record.id(),
        workspace_id: record.workspace_id(),
        memory_type: record.memory_type(),
        content: record.content().to_owned(),
        structured_claim: record.structured_claim().cloned(),
        source: record.source().clone(),
        confidence: record.confidence(),
        importance: record.importance(),
        sensitivity: record.sensitivity(),
        entities: record.entities().to_vec(),
        valid_from: Some(record.valid_from()),
        valid_until: record.valid_until(),
        supersedes: record.supersedes(),
        run_id: record.run_id(),
        created_by_actor_id: record.created_by_actor_id().to_owned(),
        correlation_id: record.correlation_id(),
        created_at: record.created_at(),
    }
}

// ------------------------------------------------------------------------------------------------
// Invariant: an inferred preference never appears as confirmed fact
// ------------------------------------------------------------------------------------------------

/// **A model-produced claim is stored as a proposal, and can never be read back as a fact.**
///
/// The first acceptance invariant, as the write path a caller would actually take. `P4-001` refuses a raised
/// confidence at construction, and this asserts the refusal is still in force when a row is **written and
/// read back** — because a decode that re-derived the confidence from somewhere else would undo the
/// constructor's rule.
///
/// It asserts the **status** as well as the confidence, and the two are different claims: bounding the
/// confidence is what stops a model asserting a level, and deriving `Proposed` is what stops the claim being
/// current truth at the one level that is permitted. This test passed for as long as only the first held.
#[tokio::test]
async fn a_model_inference_cannot_be_recorded_as_a_fact() {
    let (_directory, database) = seeded_database().await;
    let subject = a_subject(&database).await;

    let inference = must(MemorySource::of_kind(
        MemorySourceKind::ModelInference,
        "run:0198f000-0000-7000-8000-0000000000c3",
    ));
    // The domain refuses it, so no row can be attempted.
    let refused = MemoryRecord::new(MemoryRecordParts {
        id: MemoryId::new(),
        workspace_id: workspace(),
        memory_type: MemoryType::Preference,
        content: "Probably prefers tea".to_owned(),
        structured_claim: None,
        source: inference.clone(),
        confidence: MemoryConfidence::Confirmed,
        importance: 1,
        sensitivity: Sensitivity::Internal,
        entities: vec![EntityRef::confirmed(subject)],
        valid_from: None,
        valid_until: None,
        supersedes: None,
        run_id: None,
        created_by_actor_id: LOCAL_USER_ID.to_owned(),
        correlation_id: CorrelationId::new(),
        created_at: at(0),
    });
    assert_eq!(refused, Err(jarvis_core::InvalidMemory::Source));

    // At `Unverified` it stores, and it comes back as a claim that is not stated as fact — so the invariant
    // holds end to end rather than only at construction.
    let unverified = record_of(
        MemoryType::Preference,
        "Probably prefers tea",
        inference,
        MemoryConfidence::Unverified,
        subject,
    );
    let key = key_for(&unverified);
    must(record_memory(&database, &unverified, &key).await);
    let read = must(find_memory(&database, &unverified.id().to_string()).await);
    assert_eq!(read.record().confidence(), MemoryConfidence::Unverified);
    assert!(
        !read.record().is_stateable_as_fact_at(at(1)),
        "an inferred preference must not be stateable as fact, even when it is the current claim"
    );
    // **And it is not the current claim either.** An earlier version of this line asserted the *opposite*,
    // and its comment justified the divergence: the presentation predicate was what kept an inference from
    // being asserted, "which is why the two are separate" from the status.
    //
    // Two layers disagreeing about one question is not a separation of concerns, and the status is the
    // load-bearing one: the comparison stage reports a candidate whose text differs from the current claim at
    // its key as a `Correction`, and a correction **supersedes**. So an inference that was `Active` silently
    // retired an earlier claim — a real effect produced by the disagreement, not a labelling nicety.
    //
    // Both layers now agree, and this test's name is finally true of its body.
    assert!(
        !read.record().effective_status_at(at(1)).is_current_truth(),
        "an inferred preference must not be the current claim, not merely unstateable"
    );
}

// ------------------------------------------------------------------------------------------------
// Invariant: correcting removes the old claim from current retrieval
// ------------------------------------------------------------------------------------------------

/// **A correction supersedes the old claim, which leaves retrieval while remaining for audit.**
///
/// The second acceptance invariant. Both halves are asserted: the old claim is no longer **current**, and it
/// is still **present** — because "retaining an allowed audit trail" and "removes the old claim from current
/// retrieval" are two requirements and a test that checked only one would pass against an implementation that
/// deleted it or one that left it current.
#[tokio::test]
async fn a_correction_supersedes_the_old_claim() {
    let (_directory, database) = seeded_database().await;
    let subject = a_subject(&database).await;

    let old = preference(subject);
    let old_key = key_for(&old);
    must(record_memory(&database, &old, &old_key).await);

    // The correction: different content, so a different key and a distinct row.
    let new = record_of(
        MemoryType::Preference,
        "Prefers light roast coffee",
        user_source(),
        MemoryConfidence::Confirmed,
        subject,
    );
    let new_key = key_for(&new);
    must(record_memory(&database, &new, &new_key).await);
    assert_ne!(
        old_key, new_key,
        "the fixture depends on a reworded claim being a distinct key"
    );

    // The link is recorded in **both** directions, so "is this current" is not a scan.
    let updated = must(
        apply_memory_transition(
            &database,
            &old.id().to_string(),
            MemoryTransition::ReplaceWith(new.id()),
            at(1),
        )
        .await,
    );
    assert_eq!(updated.record().superseded_by(), Some(new.id()));
    assert_eq!(
        updated.record().status(),
        MemoryStatus::Archived,
        "a replaced claim must stop being the current one"
    );
    assert_eq!(
        updated.record().effective_status_at(at(2)),
        jarvis_core::EffectiveMemoryStatus::Superseded
    );
    assert!(
        !updated
            .record()
            .effective_status_at(at(2))
            .is_current_truth()
    );
    // The audit trail: the text is still there, because the superseded claim is the record of what was
    // believed before the correction.
    assert_eq!(updated.record().content(), "Prefers dark roast coffee");

    // Both rows are readable, so nothing was lost.
    let all = must(read_workspace_memories(&database, workspace(), 16).await);
    assert_eq!(all.len(), 2);
    let current: Vec<&str> = all
        .iter()
        .filter(|memory| {
            memory
                .record()
                .effective_status_at(at(2))
                .is_current_truth()
        })
        .map(|memory| memory.record().content())
        .collect();
    assert_eq!(
        current,
        vec!["Prefers light roast coffee"],
        "only the correction may be current"
    );
}

/// **A correction trail cannot be rewritten: a second, different replacement is refused.**
#[tokio::test]
async fn a_replacement_cannot_be_rewritten_in_storage() {
    let (_directory, database) = seeded_database().await;
    let subject = a_subject(&database).await;
    let memory = preference(subject);
    let key = key_for(&memory);
    must(record_memory(&database, &memory, &key).await);

    let first = MemoryId::new();
    let second = MemoryId::new();
    // The two replacements must exist, because the column is a foreign key — which is itself a property worth
    // having: a supersession cannot point at a memory that does not exist. Their identifiers are therefore
    // **chosen** rather than generated: an earlier cut recorded rows with fresh identifiers and then linked
    // the two above, so the fixture asserted a link to nothing and the write failed on the foreign key.
    for replacement in [first, second] {
        let template = record_of(
            MemoryType::Preference,
            &format!("Replacement {replacement}"),
            user_source(),
            MemoryConfidence::Confirmed,
            subject,
        );
        let mut parts = deconstruct(&template);
        parts.id = replacement;
        let row = must(MemoryRecord::new(parts));
        let row_key = key_for(&row);
        must(record_memory(&database, &row, &row_key).await);
        assert_eq!(
            row.id(),
            replacement,
            "the fixture's replacement must carry the identifier the link names"
        );
    }

    must(
        apply_memory_transition(
            &database,
            &memory.id().to_string(),
            MemoryTransition::ReplaceWith(first),
            at(1),
        )
        .await,
    );
    let refused = apply_memory_transition(
        &database,
        &memory.id().to_string(),
        MemoryTransition::ReplaceWith(second),
        at(2),
    )
    .await;
    assert!(
        refused.is_err(),
        "a correction trail must not be rewritable, got {refused:?}"
    );
}

// ------------------------------------------------------------------------------------------------
// Invariant: deletion removes text and derived indexes, and blocks resurrection
// ------------------------------------------------------------------------------------------------

/// **Deleting a memory removes its text and its derived search key, keeps the row, and blocks a re-ingest.**
///
/// The third acceptance invariant, and the two requirements inside it pull against each other: the text and
/// the key must go, and the claim must not come back. The tombstone's **hash** is what satisfies both — it
/// recognises the same claim on a re-ingest without holding the words.
///
/// The four assertions are the four things that could each be wrong:
///
/// - the text is gone from the row and from the decoded value;
/// - the derived key is gone, which is the "derived indexes" half;
/// - the row still exists, so a source link resolves and an operator can see the deletion;
/// - the identical claim is **refused** on re-ingest rather than inserted.
#[tokio::test]
async fn deletion_removes_text_and_blocks_a_re_ingest() {
    let (_directory, database) = seeded_database().await;
    let subject = a_subject(&database).await;
    let memory = preference(subject);
    let key = key_for(&memory);
    must(record_memory(&database, &memory, &key).await);

    let deleted = must(
        apply_memory_transition(
            &database,
            &memory.id().to_string(),
            MemoryTransition::Delete,
            at(5),
        )
        .await,
    );
    assert_eq!(deleted.record().status(), MemoryStatus::Deleted);
    assert!(
        deleted.record().content().is_empty(),
        "the deleted text must not remain in the row"
    );
    assert!(
        deleted.search_key().is_none(),
        "the derived key must go with the text, because it holds the same words"
    );
    assert_eq!(deleted.version(), 2, "the row still exists and was updated");

    // The claim is tombstoned, so the same observation ingested again is refused.
    assert!(must(is_tombstoned(&database, workspace(), &key).await));

    let replacement = preference(subject);
    let same_key = MemorySearchKey::new(
        replacement.memory_type(),
        replacement.entities(),
        replacement.content(),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    let refused = record_memory(&database, &replacement, &same_key).await;
    assert!(
        matches!(refused, Err(DatabaseError::MemoryTombstoned)),
        "a deleted claim must not be re-recorded, got {refused:?}"
    );

    // A *different* claim about the same subject stores normally, so the tombstone blocks the claim rather
    // than the entity: forgetting one preference must not make the person unmemorable.
    let different = record_of(
        MemoryType::Preference,
        "Prefers a window seat",
        user_source(),
        MemoryConfidence::Confirmed,
        subject,
    );
    let different_key = key_for(&different);
    must(record_memory(&database, &different, &different_key).await);
}

/// **⭐⭐ The retrieval window returns the NEWEST rows, ordered by time rather than by timestamp text.**
///
/// The window is what `P4-004`'s selection draws candidates from, so a window that does not return the newest
/// rows silently removes recent claims from what the model may be told. The assertion is on **which** row is
/// absent rather than how many came back: a count-only assertion is satisfied by any wrong set of the right
/// size, and this test's own first version passed its count while dropping the wrong row.
///
/// # ⚠ The rows are a whole second apart, and that is a limitation rather than a convenience
///
/// These timestamps are RFC 3339 text with the fraction **omitted when it is zero** (`.`, 0x2E, sorts before
/// `Z`, 0x5A), so ordering by the raw text is wrong **within one second** — the `ADR-0034` trap. The read now
/// orders by `unixepoch(created_at)`, which parses the string into seconds. **That fixes the second boundary
/// and cannot fix anything finer**, because `unixepoch` resolves to whole seconds: two memories written in one
/// second tie, and `id ASC` decides arbitrarily. So this fixture uses distinct seconds, and **there is no test
/// here for the sub-second case because the platform cannot currently express one** — genuine nanosecond
/// ordering needs an integer-nanoseconds column, which is a migration across every timestamp column and
/// therefore `ADR-0034`'s decision rather than a drive-by. The defect within a second is thus **recorded as
/// open, not silently claimed fixed**.
#[tokio::test]
async fn the_retrieval_window_returns_the_newest_memories() {
    const BASE: i128 = 1_774_000_000_000_000_000;

    let (_directory, database) = seeded_database().await;
    let subject = a_subject(&database).await;

    let instants = [BASE, BASE + 1_000_000_000, BASE + 2_000_000_000]
        .map(|nanos| must(UtcTimestamp::from_unix_nanos(nanos)));

    let mut oldest = MemoryId::new();
    for (offset, created_at) in instants.into_iter().enumerate() {
        // The content varies because the **search key** deduplicates: three identical claims collide on the
        // unique index (`MemoryDuplicate`), which is the deduplication rule doing its job rather than a
        // fixture problem — the first version of this test used one text and could not store its own rows.
        let content = format!("Prefers dark roast coffee, variant {offset}");
        let record = record_at(
            MemoryType::Preference,
            &content,
            user_source(),
            MemoryConfidence::Confirmed,
            subject,
            created_at,
        );
        let key = key_for(&record);
        must(record_memory(&database, &record, &key).await);
        if offset == 0 {
            oldest = record.id();
        }
    }

    // Every row is returned when the window is at least the row count, so a read that returned nothing
    // cannot pass the next assertion.
    assert_eq!(
        must(read_retrievable_memories(&database, workspace(), 3).await).len(),
        3
    );

    let windowed = must(read_retrievable_memories(&database, workspace(), 2).await);
    assert_eq!(windowed.len(), 2, "the window must bound the read");
    assert!(
        !windowed.iter().any(|memory| memory.record().id() == oldest),
        "the oldest row must be the one the window drops; if it survives, the read is not ordering by time"
    );
}

/// **A deleted memory is not returned by the workspace read.**
#[tokio::test]
async fn a_deleted_memory_is_not_read_back() {
    let (_directory, database) = seeded_database().await;
    let subject = a_subject(&database).await;
    let memory = preference(subject);
    let key = key_for(&memory);
    must(record_memory(&database, &memory, &key).await);
    assert_eq!(
        must(read_workspace_memories(&database, workspace(), 16).await).len(),
        1
    );

    must(
        apply_memory_transition(
            &database,
            &memory.id().to_string(),
            MemoryTransition::Delete,
            at(1),
        )
        .await,
    );
    assert!(
        must(read_workspace_memories(&database, workspace(), 16).await).is_empty(),
        "a deleted memory must not be offered to retrieval"
    );
    // It is still readable by identifier, because the row is retained and an operator asking "what happened
    // to this" must get an answer.
    let by_id = must(find_memory(&database, &memory.id().to_string()).await);
    assert_eq!(by_id.record().status(), MemoryStatus::Deleted);
}

// ------------------------------------------------------------------------------------------------
// Invariant: one workspace's memory cannot enter another's context
// ------------------------------------------------------------------------------------------------

/// **A memory is readable only through its own workspace's read.**
///
/// The fourth acceptance invariant. The row is written against the first workspace and the second workspace's
/// read is asserted to return nothing — which is the shape of the leak rather than a check on the record's own
/// accessor, because a scoping bug would live in the query.
#[tokio::test]
async fn a_memory_cannot_be_read_from_another_workspace() {
    let (_directory, database) = seeded_database().await;
    let subject = a_subject(&database).await;
    let memory = preference(subject);
    let key = key_for(&memory);
    must(record_memory(&database, &memory, &key).await);

    assert_eq!(
        must(read_workspace_memories(&database, workspace(), 16).await).len(),
        1
    );
    assert!(
        must(read_workspace_memories(&database, other_workspace(), 16).await).is_empty(),
        "a memory must not be readable through another workspace"
    );
    // And the entity-scoped read is scoped too, which is the second query a bug could scope wrongly.
    assert_eq!(
        must(read_entity_memories(&database, workspace(), subject, 16).await).len(),
        1
    );
    assert!(
        must(read_entity_memories(&database, other_workspace(), subject, 16).await).is_empty(),
        "the entity-scoped read must be workspace-scoped as well"
    );
    assert!(memory.is_visible_in(workspace()));
    assert!(!memory.is_visible_in(other_workspace()));
}

// ------------------------------------------------------------------------------------------------
// Deduplication and reinforcement
// ------------------------------------------------------------------------------------------------

/// **A re-ingested identical claim is a duplicate carrying the existing identifier, not a second row.**
///
/// The admission lifecycle's deduplication step. The unique index decides rather than a read-then-write,
/// because a check followed by an insert is a race.
#[tokio::test]
async fn an_identical_claim_is_a_duplicate_not_a_second_row() {
    let (_directory, database) = seeded_database().await;
    let subject = a_subject(&database).await;
    let first = preference(subject);
    let key = key_for(&first);
    must(record_memory(&database, &first, &key).await);

    // A second row with the same claim and a different identifier, which is what a re-ingest produces.
    let second = record_of(
        MemoryType::Preference,
        // Deliberately the same claim with different case and spacing, so the normalization is what collapses
        // them rather than an accidental string equality.
        "  prefers   DARK roast coffee ",
        user_source(),
        MemoryConfidence::Confirmed,
        subject,
    );
    let second_key = key_for(&second);
    assert_eq!(
        key, second_key,
        "the fixture depends on two spellings producing one key"
    );

    let refused = record_memory(&database, &second, &second_key).await;
    match refused {
        Err(DatabaseError::MemoryDuplicate { existing_memory_id }) => {
            assert_eq!(
                existing_memory_id,
                first.id().to_string(),
                "the existing identifier must be returned so the caller reinforces"
            );
        }
        other => panic!("expected a duplicate, got {other:?}"),
    }
    assert_eq!(
        must(read_workspace_memories(&database, workspace(), 16).await).len(),
        1
    );
}

/// **Reinforcement records that a memory was useful, without pretending it was edited.**
#[tokio::test]
async fn reinforcement_records_use_and_leaves_the_edit_time_alone() {
    let (_directory, database) = seeded_database().await;
    let subject = a_subject(&database).await;
    let memory = preference(subject);
    let key = key_for(&memory);
    must(record_memory(&database, &memory, &key).await);

    let reinforced = must(reinforce_memory(&database, &memory.id().to_string(), at(3)).await);
    assert_eq!(reinforced.record().retrieval_count(), 1);
    assert_eq!(reinforced.record().last_accessed_at(), Some(at(3)));
    assert_eq!(
        reinforced.record().updated_at(),
        memory.updated_at(),
        "being read is not being edited"
    );

    let again = must(reinforce_memory(&database, &memory.id().to_string(), at(4)).await);
    assert_eq!(again.record().retrieval_count(), 2);

    // A deleted memory cannot be reinforced: it retains no text, so "this was used" would be a fact about a
    // claim that no longer exists.
    must(
        apply_memory_transition(
            &database,
            &memory.id().to_string(),
            MemoryTransition::Delete,
            at(5),
        )
        .await,
    );
    let refused = reinforce_memory(&database, &memory.id().to_string(), at(6)).await;
    assert!(refused.is_err(), "a deleted memory must not be reinforced");
}

// ------------------------------------------------------------------------------------------------
// Entities, aliases, and merging
// ------------------------------------------------------------------------------------------------

/// **A verified alias resolves to exactly one entity; two entities cannot both hold one.**
///
/// The architecture's identity rule read two ways: "Ambiguous aliases remain separate candidates" applies to
/// *probabilistic* matches, while a verified alias is an identity claim — so the second verified claim is a
/// contradiction to surface rather than a row to store.
#[tokio::test]
async fn a_verified_alias_resolves_to_one_entity() {
    let (_directory, database) = seeded_database().await;
    let first = a_subject(&database).await;
    let second = entity_id();
    must(
        record_entity(
            &database,
            &NewEntity {
                id: second,
                workspace_id: workspace(),
                kind: EntityKind::Person,
                label: "Other Person".to_owned(),
                attributes: None,
                confidence: MemoryConfidence::Likely,
                created_at: at(0),
            },
        )
        .await,
    );

    must(
        record_alias(
            &database,
            first,
            "email",
            "person@example.invalid",
            EntityMatch::ExactIdentifier,
            MemoryConfidence::Confirmed,
            MemorySourceKind::UserStatement,
            at(0),
        )
        .await,
    );
    let resolved =
        must(resolve_alias(&database, workspace(), "email", "person@example.invalid").await)
            .unwrap_or_else(|| panic!("a verified alias must resolve"));
    assert_eq!(resolved.entity_id(), first);

    // The same identity claimed for another entity is refused, and the existing holder is named.
    let refused = record_alias(
        &database,
        second,
        "email",
        "Person@Example.Invalid",
        EntityMatch::ExactIdentifier,
        MemoryConfidence::Confirmed,
        MemorySourceKind::UserStatement,
        at(1),
    )
    .await;
    match refused {
        Err(DatabaseError::AliasAlreadyVerified { existing_entity_id }) => {
            assert_eq!(existing_entity_id, first.to_string());
        }
        other => panic!("expected a verified-alias conflict, got {other:?}"),
    }

    // Case and whitespace are the same alias, which the normalization is what achieves.
    let same =
        must(resolve_alias(&database, workspace(), "email", "  PERSON@example.invalid ").await);
    assert_eq!(
        same.map(|alias| alias.entity_id()),
        Some(first),
        "folding must make one alias of both spellings"
    );
}

/// **A probabilistic alias does not resolve, and several entities may each be guessed from one name.**
///
/// The other half of the identity rule: candidates are kept, and a guess is never an identity. This is the
/// test that would fail if a resolver returned the first matching row regardless of verification.
#[tokio::test]
async fn a_probabilistic_alias_is_a_candidate_and_not_an_identity() {
    let (_directory, database) = seeded_database().await;
    let first = a_subject(&database).await;
    let second = entity_id();
    must(
        record_entity(
            &database,
            &NewEntity {
                id: second,
                workspace_id: workspace(),
                kind: EntityKind::Person,
                label: "Maybe The Same".to_owned(),
                attributes: None,
                confidence: MemoryConfidence::Uncertain,
                created_at: at(0),
            },
        )
        .await,
    );

    for entity in [first, second] {
        must(
            record_alias(
                &database,
                entity,
                "name",
                "J. Smith",
                EntityMatch::Probabilistic,
                MemoryConfidence::Uncertain,
                MemorySourceKind::ModelInference,
                at(0),
            )
            .await,
        );
    }

    // Both candidates are readable, which is what "remain separate candidates" requires.
    let candidates = must(read_alias_candidates(&database, workspace(), "name", "j. smith").await);
    assert_eq!(candidates.len(), 2, "both guesses must be visible");
    // ...and neither resolves, so a caller cannot turn a guess into an identity.
    assert!(
        must(resolve_alias(&database, workspace(), "name", "J. Smith").await).is_none(),
        "a probabilistic alias must not resolve to an entity"
    );
}

/// **Merging an entity is auditable and reversible, and a chain is refused.**
#[tokio::test]
async fn merging_an_entity_is_auditable_and_reversible() {
    let (_directory, database) = seeded_database().await;
    let winner = a_subject(&database).await;
    let loser = entity_id();
    must(
        record_entity(
            &database,
            &NewEntity {
                id: loser,
                workspace_id: workspace(),
                kind: EntityKind::Person,
                label: "Duplicate".to_owned(),
                attributes: None,
                confidence: MemoryConfidence::Likely,
                created_at: at(0),
            },
        )
        .await,
    );

    let merged = must(merge_entities(&database, loser, winner, at(1)).await);
    assert_eq!(merged.status(), EntityStatus::Merged);
    assert_eq!(
        merged.merged_into(),
        Some(winner),
        "the losing row must say where it went, or the merge is unreadable"
    );
    assert!(
        !merged.is_usable(),
        "a merged entity must not take new claims"
    );

    // Reversal is one update, which is why the row is retained rather than deleted.
    must(
        sqlx::query("UPDATE entities SET status = 'active', merged_into = NULL WHERE id = ?1")
            .bind(loser.to_string())
            .execute(database.pool())
            .await
            .map(|_| ()),
    );
    let restored = must(find_entity(&database, &loser.to_string()).await);
    assert_eq!(restored.status(), EntityStatus::Active);
    assert!(restored.is_usable());

    // A chain is refused: a merge target must be the current winner, so resolution is one hop and a cycle is
    // unrepresentable rather than merely unlikely.
    must(merge_entities(&database, loser, winner, at(2)).await);
    let chained = merge_entities(&database, winner, loser, at(3)).await;
    assert!(
        matches!(chained, Err(DatabaseError::InvalidEntityMerge { .. })),
        "a merge must not chain, got {chained:?}"
    );
    // And an entity cannot be merged into itself.
    let self_merge = merge_entities(&database, winner, winner, at(4)).await;
    assert!(matches!(
        self_merge,
        Err(DatabaseError::InvalidEntityMerge { .. })
    ));
}

// ------------------------------------------------------------------------------------------------
// Relations
// ------------------------------------------------------------------------------------------------

/// **A relation carries provenance and refuses a self-relation.**
#[tokio::test]
async fn a_relation_carries_provenance_and_refuses_itself() {
    let (_directory, database) = seeded_database().await;
    let first = a_subject(&database).await;
    let second = entity_id();
    must(
        record_entity(
            &database,
            &NewEntity {
                id: second,
                workspace_id: workspace(),
                kind: EntityKind::Organization,
                label: "An Organization".to_owned(),
                attributes: None,
                confidence: MemoryConfidence::Confirmed,
                created_at: at(0),
            },
        )
        .await,
    );

    must(
        record_relation(
            &database,
            workspace(),
            first,
            "works_at",
            second,
            &user_source(),
            MemoryConfidence::Confirmed,
            Sensitivity::Internal,
            LOCAL_USER_ID,
            CorrelationId::new(),
            at(0),
        )
        .await,
    );
    let relations = must(read_subject_relations(&database, workspace(), first).await);
    assert_eq!(relations.len(), 1);
    assert_eq!(relations[0].predicate(), "works_at");
    assert_eq!(relations[0].subject_id(), first);
    assert_eq!(relations[0].object_id(), second);

    // A self-relation is refused, so a traversal cannot loop on a data error.
    let self_relation = record_relation(
        &database,
        workspace(),
        first,
        "knows",
        first,
        &user_source(),
        MemoryConfidence::Confirmed,
        Sensitivity::Internal,
        LOCAL_USER_ID,
        CorrelationId::new(),
        at(1),
    )
    .await;
    assert!(
        matches!(
            self_relation,
            Err(DatabaseError::InvalidMemoryRequest { field: "object_id" })
        ),
        "an entity must not relate to itself, got {self_relation:?}"
    );

    // A model-produced relation cannot claim a confidence, the same rule a memory carries.
    let inference = must(MemorySource::of_kind(
        MemorySourceKind::ModelInference,
        "run:0198f000-0000-7000-8000-0000000000c3",
    ));
    let overconfident = record_relation(
        &database,
        workspace(),
        first,
        "likely_works_at",
        second,
        &inference,
        MemoryConfidence::Likely,
        Sensitivity::Internal,
        LOCAL_USER_ID,
        CorrelationId::new(),
        at(2),
    )
    .await;
    assert!(
        matches!(
            overconfident,
            Err(DatabaseError::InvalidMemoryRequest {
                field: "confidence"
            })
        ),
        "a model inference must not claim confidence, got {overconfident:?}"
    );
}

// ------------------------------------------------------------------------------------------------
// Storage integrity
// ------------------------------------------------------------------------------------------------

/// **A stored row the schema forbids cannot be written, so the refusal has two enforcers.**
///
/// The `CHECK`s in `0009_memories.sql` restate the domain's rules, and this asserts they are real rather than
/// decorative. An approval request is used as the vehicle because it is the only way this test can reach the
/// table directly — and reaching it directly is the point: a row written by another build or restored from a
/// backup arrives exactly this way, bypassing the repository.
#[tokio::test]
async fn the_schema_refuses_a_row_the_domain_refuses() {
    let (_directory, database) = seeded_database().await;
    let subject = a_subject(&database).await;

    // External content claiming to be authoritative — the injection boundary, in SQL.
    let smuggled = sqlx::query(
        "INSERT INTO memories (\
            id, workspace_id, memory_type, content, source_kind, source_locator, source_trust, \
            confidence, importance, sensitivity, search_key, status, valid_from, created_by_actor_id, \
            correlation_id, created_at, updated_at, retrieval_count, version\
         ) VALUES (?1, ?2, 'semantic', 'An instruction from a web page', 'external_content', \
            'https://example.invalid', 'authoritative', 'confirmed', 1, 'internal', 'key', 'active', \
            ?3, ?4, ?5, ?3, ?3, 0, 1)",
    )
    .bind(MemoryId::new().to_string())
    .bind(WORKSPACE)
    .bind(at(0).to_string())
    .bind(LOCAL_USER_ID)
    .bind(CorrelationId::new().to_string())
    .execute(database.pool())
    .await;
    assert!(
        smuggled.is_err(),
        "external content claiming to be authoritative must be unstorable"
    );

    // A deleted row retaining its text.
    let smuggling_text = sqlx::query(
        "INSERT INTO memories (\
            id, workspace_id, memory_type, content, source_kind, source_locator, source_trust, \
            confidence, importance, sensitivity, search_key, status, valid_from, created_by_actor_id, \
            correlation_id, created_at, updated_at, retrieval_count, version\
         ) VALUES (?1, ?2, 'semantic', 'Still here', 'user_statement', 'session:x', 'authoritative', \
            'confirmed', 1, 'internal', 'key', 'deleted', ?3, ?4, ?5, ?3, ?3, 0, 1)",
    )
    .bind(MemoryId::new().to_string())
    .bind(WORKSPACE)
    .bind(at(0).to_string())
    .bind(LOCAL_USER_ID)
    .bind(CorrelationId::new().to_string())
    .execute(database.pool())
    .await;
    assert!(
        smuggling_text.is_err(),
        "a deleted memory must not retain text, and the schema must be what refuses it"
    );

    // A model inference claiming confidence.
    let overconfident = sqlx::query(
        "INSERT INTO memories (\
            id, workspace_id, memory_type, content, source_kind, source_locator, source_trust, \
            confidence, importance, sensitivity, search_key, status, valid_from, created_by_actor_id, \
            correlation_id, created_at, updated_at, retrieval_count, version\
         ) VALUES (?1, ?2, 'semantic', 'Inferred', 'model_inference', 'run:x', 'derived', \
            'confirmed', 1, 'internal', 'key2', 'active', ?3, ?4, ?5, ?3, ?3, 0, 1)",
    )
    .bind(MemoryId::new().to_string())
    .bind(WORKSPACE)
    .bind(at(0).to_string())
    .bind(LOCAL_USER_ID)
    .bind(CorrelationId::new().to_string())
    .execute(database.pool())
    .await;
    assert!(
        overconfident.is_err(),
        "a model inference must not claim confidence, and the schema must be what refuses it"
    );

    // The control: a well-formed row through the same statement stores, so the refusals above are about the
    // values rather than about the statement being wrong.
    let accepted = sqlx::query(
        "INSERT INTO memories (\
            id, workspace_id, memory_type, content, source_kind, source_locator, source_trust, \
            confidence, importance, sensitivity, search_key, status, valid_from, created_by_actor_id, \
            correlation_id, created_at, updated_at, retrieval_count, version\
         ) VALUES (?1, ?2, 'semantic', 'A city', 'user_statement', 'session:x', 'authoritative', \
            'confirmed', 1, 'internal', 'key3', 'active', ?3, ?4, ?5, ?3, ?3, 0, 1)",
    )
    .bind(MemoryId::new().to_string())
    .bind(WORKSPACE)
    .bind(at(0).to_string())
    .bind(LOCAL_USER_ID)
    .bind(CorrelationId::new().to_string())
    .execute(database.pool())
    .await;
    assert!(accepted.is_ok(), "the control row must store: {accepted:?}");
    let _ = subject;
}

/// **Every entity kind round-trips through storage.**
#[tokio::test]
async fn every_entity_kind_round_trips() {
    let (_directory, database) = seeded_database().await;
    for kind in EntityKind::all() {
        let id = entity_id();
        must(
            record_entity(
                &database,
                &NewEntity {
                    id,
                    workspace_id: workspace(),
                    kind,
                    label: format!("A {kind}"),
                    attributes: None,
                    confidence: MemoryConfidence::Confirmed,
                    created_at: at(0),
                },
            )
            .await,
        );
        assert_eq!(
            must(find_entity(&database, &id.to_string()).await).kind(),
            kind
        );
    }
}
