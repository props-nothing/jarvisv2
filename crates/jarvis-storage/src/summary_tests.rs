//! Tests for durable session summaries (`P4-015`).
//!
//! # Why these are shaped as the attacks the slice names
//!
//! `P4-015` requires that a summary is "stored as a `Derived` claim with provenance and a retention rule, and
//! is **never presented as user-authored fact**". Each test below is one way that requirement can be broken:
//! the trust upgraded, the span fabricated, the coverage claimed twice, the provenance lost. A happy-path test
//! passes against an implementation wrong in every one of those cases.
//!
//! The overlap and complement tests are the ones that matter most, because they are the rules with **no schema
//! behind them**: SQLite can hold a span; it cannot know which turns a session has, and it cannot decide that
//! two intervals in one session must not intersect. Those rules live in this adapter and in `jarvis-core`, and
//! an assertion is the only thing that checks them.

use std::path::{Path, PathBuf};

use super::*;
use crate::{
    DEFAULT_DATABASE_FILENAME, LOCAL_USER_ID, NewEntity, SqliteDatabase, append_message,
    count_messages, find_memory, record_entity, record_memory,
};
use jarvis_core::{
    CorrelationId, EntityId, EntityRef, MemoryId, MemorySearchKey, MemorySource, MemorySourceKind,
    MessageRole, MessageSource, NewMessage, Sensitivity, SessionSummaryParts, WorkspaceId,
};

const WORKSPACE: &str = "0198f000-0000-7000-8000-000000000001";

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("jarvis-summary-{}", jarvis_core::scratch_tag()));
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

/// Unpacks a `Result` that must have failed, or panics naming the rule that was being checked.
///
/// The workspace denies `clippy::expect_used` in test code too, so this is the crate's idiom rather than
/// `.expect_err(..)` — and it carries the expectation into the message, which is what makes a failure name the
/// rule rather than only that something went wrong.
fn must_err<T, E>(result: Result<T, E>, what: &str) -> E {
    match result {
        Ok(_) => panic!("expected a failure: {what}"),
        Err(error) => error,
    }
}

fn at(minute: i128) -> UtcTimestamp {
    const BASE: i128 = 1_774_000_000_500_000_000;
    must(UtcTimestamp::from_unix_nanos(
        BASE + minute * 60_000_000_000,
    ))
}

fn workspace() -> WorkspaceId {
    must(WORKSPACE.parse())
}

async fn insert_session(database: &SqliteDatabase, session_id: SessionId, messages: i64) {
    must(
        sqlx::query(
            "INSERT INTO sessions (id, workspace_id, user_id, channel, status, created_at, \
             updated_at, version) VALUES (?1, ?2, ?3, 'cli', 'active', ?4, ?4, 1)",
        )
        .bind(session_id.to_string())
        .bind(WORKSPACE)
        .bind(LOCAL_USER_ID)
        .bind(at(0).to_string())
        .execute(database.pool())
        .await
        .map(|_| ()),
    );
    for index in 0..messages {
        must(
            append_message(
                database,
                &session_id.to_string(),
                None,
                &must(NewMessage::new(
                    format!("message {index}"),
                    MessageRole::User,
                    MessageSource::User,
                    Sensitivity::Internal,
                    None,
                )),
                CorrelationId::new(),
                at(0),
            )
            .await,
        );
    }
}

/// A workspace, a session with `message_count` messages, and a conversation entity to hang the summary on.
async fn seeded_database(
    message_count: i64,
) -> (TestDirectory, SqliteDatabase, SessionId, EntityId) {
    let directory = TestDirectory::new();
    let database =
        must(SqliteDatabase::open(&directory.path().join(DEFAULT_DATABASE_FILENAME)).await);
    must(
        sqlx::query(
            "INSERT INTO workspaces \
                (id, name, mode, data_policy, status, created_at, updated_at, version) \
             VALUES (?1, 'summary', 'local', 'standard', 'active', ?2, ?2, 1)",
        )
        .bind(WORKSPACE)
        .bind(at(0).to_string())
        .execute(database.pool())
        .await
        .map(|_| ()),
    );
    let session_id = SessionId::new();
    insert_session(&database, session_id, message_count).await;

    // The conversation entity, which is what a summary is *about*. Created through the real repository so the
    // foreign key this write depends on is real rather than assumed.
    let entity = EntityId::new();
    must(
        record_entity(
            &database,
            &NewEntity {
                id: entity,
                workspace_id: workspace(),
                kind: crate::EntityKind::Conversation,
                label: "Session 1".to_owned(),
                attributes: None,
                confidence: jarvis_core::MemoryConfidence::Confirmed,
                created_at: at(0),
            },
        )
        .await,
    );
    (directory, database, session_id, entity)
}

fn summary(
    session_id: SessionId,
    entity: EntityId,
    first: i64,
    last: i64,
    text: &str,
) -> SessionSummary {
    let span = must(SummarySpan::new(session_id, first, last));
    must(SessionSummary::new(SessionSummaryParts {
        summary_id: MemoryId::new(),
        workspace_id: workspace(),
        session_id,
        text: text.to_owned(),
        span,
        loss: must(SummaryLoss::new(
            u32::try_from(span.message_count()).unwrap_or(u32::MAX),
            Some(900),
        )),
        created_by_actor_id: LOCAL_USER_ID.to_owned(),
        correlation_id: CorrelationId::new(),
        created_at: at(1),
        entities: vec![EntityRef::confirmed(entity)],
    }))
}

/// **A stored summary is derived, attributed to its span, and linked to its subject.**
///
/// The three claims "never presented as user-authored fact" decomposes into, read back through the **real
/// decoder** rather than asserted on the value that went in. A test of the input passes with a writer that
/// dropped every column and returned the summary it was handed.
#[tokio::test]
async fn a_stored_summary_is_derived_and_names_its_span_and_subject() {
    let (_directory, database, session_id, entity) = seeded_database(4).await;
    let stored = must(
        record_summary(
            &database,
            &summary(session_id, entity, 1, 3, "Three turns."),
            4,
        )
        .await,
    );

    assert_eq!(stored.span().first_sequence, 1);
    assert_eq!(stored.span().last_sequence, 3);

    let record = must(find_memory(&database, stored.memory_id()).await)
        .record()
        .clone();
    assert_eq!(record.source().kind(), MemorySourceKind::Document);
    assert_eq!(record.source().trust(), jarvis_core::MemoryTrust::Derived);
    assert_eq!(record.memory_type(), jarvis_core::MemoryType::Conversation);
    assert!(
        record.source().locator().contains("summary:session/"),
        "the locator must name the span: {}",
        record.source().locator()
    );
    // The link is the "source links" requirement, asserted through the read path so a writer that skipped
    // `link_entities_on` fails here rather than passing on the value it returned.
    assert_eq!(record.entities().len(), 1);
    assert_eq!(record.entities()[0].entity_id(), entity);
}

/// **A summary cannot be built with authoritative trust, and the constructor is the reason.**
///
/// The control for the test above. The provenance is not something the writer chose; it is what
/// `SessionSummary::into_record` produces. This asserts the rule at the one place that decides it, so a change
/// there is a failure here rather than a silent weakening of every caller.
#[tokio::test]
async fn a_summary_can_never_be_stored_with_authoritative_trust() {
    let (_directory, database, session_id, entity) = seeded_database(2).await;
    let value = summary(session_id, entity, 0, 1, "Two turns.");
    let record = must(value.clone().into_record());
    assert_eq!(record.source().trust(), jarvis_core::MemoryTrust::Derived);
    assert_ne!(
        record.source().trust(),
        jarvis_core::MemoryTrust::Authoritative
    );

    // And the stored row agrees, so the writer is not overriding the constructor on the way in.
    let stored = must(record_summary(&database, &value, 2).await);
    let read_back = must(find_memory(&database, stored.memory_id()).await)
        .record()
        .clone();
    assert_eq!(read_back.source().trust(), record.source().trust());
}

/// **A span naming turns the session does not have is refused, and a real span is accepted.**
///
/// The fabrication this closes: a summary claiming to cover message 300 of a 4-message session. Nothing in SQL
/// can refuse it — `session_summaries` cannot see the transcript — so the check is here. Both directions are
/// asserted, because a check that refused *every* span would satisfy the refusal alone.
#[tokio::test]
async fn a_span_beyond_the_transcript_is_refused_and_a_real_span_is_accepted() {
    let (_directory, database, session_id, entity) = seeded_database(4).await;

    // 0..3 is the whole transcript: accepted.
    must(
        record_summary(
            &database,
            &summary(session_id, entity, 0, 3, "All four."),
            4,
        )
        .await,
    );

    // 2..4 names message 4, which a 4-message session does not have. The boundary is the point: `last` must be
    // strictly less than the count, since sequences run 0..count-1.
    let error = must_err(
        record_summary(&database, &summary(session_id, entity, 2, 4, "Too far."), 4).await,
        "a span naming an absent message must be refused",
    );
    assert_eq!(
        format!("{error}"),
        "the summary span is invalid",
        "the refusal must name the span, not the content"
    );

    // A session with no messages accepts no span, because there is no message 0 to summarize.
    let (_empty_dir, empty, empty_session, empty_entity) = seeded_database(0).await;
    assert_eq!(
        must(count_messages(&empty, &empty_session.to_string()).await),
        0
    );
    let error = must_err(
        record_summary(
            &empty,
            &summary(empty_session, empty_entity, 0, 0, "Nothing."),
            0,
        )
        .await,
        "an empty session has no turn to summarize",
    );
    assert_eq!(format!("{error}"), "the summary span is invalid");
}

/// **One session is not summarized twice over the same turns, and adjacent turns are not an overlap.**
///
/// Two summaries of one passage are two claims about it and a reader cannot tell which is current. The
/// boundary is what makes the rule usable: a summary of 0..3 must still allow one of 4..7, or a long session
/// could never be summarized in pieces.
#[tokio::test]
async fn an_overlapping_span_is_refused_and_an_adjacent_one_is_accepted() {
    let (_directory, database, session_id, entity) = seeded_database(8).await;
    must(
        record_summary(
            &database,
            &summary(session_id, entity, 0, 3, "First four."),
            8,
        )
        .await,
    );

    // Contained, containing, and identical — three ways two spans in one session overlap.
    for (first, last) in [(3_i64, 6_i64), (1, 2), (0, 3)] {
        let error = must_err(
            record_summary(
                &database,
                &summary(session_id, entity, first, last, "Overlap."),
                8,
            )
            .await,
            "an overlapping span must be refused",
        );
        assert_eq!(
            format!("{error}"),
            "turns 0..3 of this session are already summarized",
            "the refusal for {first}..{last} must name the span already stored"
        );
    }

    // 4..7 begins where 0..3 ends: not an overlap.
    must(
        record_summary(
            &database,
            &summary(session_id, entity, 4, 7, "Second four."),
            8,
        )
        .await,
    );
}

/// **The same sequences in another session do not overlap.**
///
/// Sequences are per-session, so two sessions both covering `0..3` are not two claims about one passage. This
/// asserts the **storage** half of that rule, and mutation confirmed which half it is: removing the `session_id`
/// filter from the overlap query fails here, while making `spans_overlap` itself ignore session identity does
/// **not** — that mutant is caught by `jarvis_core`'s `overlapping_spans_are_detected_in_both_directions`.
///
/// Both guards are needed and each has its own detector, which is the reason the split is worth recording: a
/// test named for the wrong one reads as coverage of a rule nothing checks.
#[tokio::test]
async fn the_same_sequences_in_another_session_do_not_overlap() {
    let (_directory, database, session_id, entity) = seeded_database(4).await;
    must(
        record_summary(
            &database,
            &summary(session_id, entity, 0, 3, "First session."),
            4,
        )
        .await,
    );

    let other = SessionId::new();
    insert_session(&database, other, 4).await;
    must(
        record_summary(
            &database,
            &summary(other, entity, 0, 3, "Second session."),
            4,
        )
        .await,
    );
}

/// **A turn count that disagrees with the span is refused rather than stored.**
///
/// The loss figures are what a compression ratio is divided by, so a producer that read one range and reported
/// another would store a plausible figure computed from the wrong denominator — a wrong number that looks like
/// a measurement.
#[tokio::test]
async fn a_turn_count_that_disagrees_with_the_span_is_refused() {
    let (_directory, database, session_id, entity) = seeded_database(8).await;
    let span = must(SummarySpan::new(session_id, 0, 3));
    let value = must(SessionSummary::new(SessionSummaryParts {
        summary_id: MemoryId::new(),
        workspace_id: workspace(),
        session_id,
        text: "Four turns claimed as two.".to_owned(),
        span,
        // The span holds 4; this claims 2.
        loss: must(SummaryLoss::new(2, Some(900))),
        created_by_actor_id: LOCAL_USER_ID.to_owned(),
        correlation_id: CorrelationId::new(),
        created_at: at(1),
        entities: vec![EntityRef::confirmed(entity)],
    }));
    let error = must_err(
        record_summary(&database, &value, 8).await,
        "a count that disagrees with the span must be refused",
    );
    assert_eq!(format!("{error}"), "the summary turns_covered is invalid");
}

/// **The unsummarized complement is the turns that remain, at every boundary.**
///
/// A producer deciding whether to compress more needs the ranges **not** done, and deriving them by subtracting
/// two sorted lists is the arithmetic that omits the range between two adjacent covered spans. The gap case is
/// the one that catches it.
#[tokio::test]
async fn the_unsummarized_complement_excludes_what_is_covered() {
    let (_directory, database, session_id, entity) = seeded_database(10).await;
    let ranges = |gaps: Vec<SummarySpan>| {
        gaps.iter()
            .map(|span| (span.first_sequence, span.last_sequence))
            .collect::<Vec<_>>()
    };

    // Nothing covered: the complement is the whole transcript.
    assert_eq!(
        ranges(must(
            read_unsummarized_ranges(&database, session_id, 10).await
        )),
        vec![(0, 9)]
    );

    // 0..3 done: the remainder starts **at 4**, not 3.
    must(record_summary(&database, &summary(session_id, entity, 0, 3, "First."), 10).await);
    assert_eq!(
        ranges(must(
            read_unsummarized_ranges(&database, session_id, 10).await
        )),
        vec![(4, 9)]
    );

    // A gap: 0..3 and 6..9 done leaves 4..5, which is the range a subtraction misses.
    must(record_summary(&database, &summary(session_id, entity, 6, 9, "Last."), 10).await);
    assert_eq!(
        ranges(must(
            read_unsummarized_ranges(&database, session_id, 10).await
        )),
        vec![(4, 5)]
    );

    // Fully covered: no ranges remain, the state in which offering a summary stops being a duplicate.
    must(record_summary(&database, &summary(session_id, entity, 4, 5, "Middle."), 10).await);
    assert!(must(read_unsummarized_ranges(&database, session_id, 10).await).is_empty());
}

/// **A deleted summary stops being read as a summary but still blocks an overlap.**
///
/// `read_session_summaries` filters on `active`; `read_session_summary_spans` does **not**. That asymmetry is
/// deliberate rather than an oversight: the overlap check must still see a set-aside summary's span, or
/// removing a summary would re-open its turns to a second claim about the same passage. Both halves are
/// asserted, because moving the filter to the wrong one of the two reads is exactly what this catches.
///
/// The `active` filter covers deletion **and** retention archiving — the archived case is asserted in
/// `archiving_a_session_collects_its_summaries_and_no_durable_memory`, and it is the one that failed first,
/// because the read originally filtered only on `deleted`.
#[tokio::test]
async fn a_deleted_summary_is_not_readable_but_still_blocks_an_overlap() {
    let (_directory, database, session_id, entity) = seeded_database(4).await;
    let stored =
        must(record_summary(&database, &summary(session_id, entity, 0, 3, "Gone."), 4).await);
    // A deletion clears the text **and** the search key, which the schema enforces: `deleted` is exactly the
    // status whose content is empty and whose key is gone, so a row that kept its key would be refused by the
    // table's own `CHECK` rather than by this test.
    must(
        sqlx::query(
            "UPDATE memories SET status = 'deleted', content = '', search_key = NULL, \
             claim_subject = NULL, claim_predicate = NULL, claim_object = NULL WHERE id = ?1",
        )
        .bind(stored.memory_id())
        .execute(database.pool())
        .await
        .map(|_| ()),
    );

    assert!(must(read_session_summaries(&database, session_id, 8).await).is_empty());
    assert_eq!(
        must(read_session_summary_spans(&database, session_id).await).len(),
        1,
        "the span must still be visible to the overlap check"
    );
    let error = must_err(
        record_summary(&database, &summary(session_id, entity, 0, 3, "Again."), 4).await,
        "a deleted summary's span must still block",
    );
    assert_eq!(
        format!("{error}"),
        "turns 0..3 of this session are already summarized"
    );
}

/// **A summary's memory follows the duplicate rule, so identical words are not stored twice.**
///
/// The search key is derived from the summary text and its subject, so two summaries of *different* spans that
/// say the same words collide. That is intended: it is the rule every memory follows, and a caller that wants
/// both keeps the span rather than the text.
#[tokio::test]
async fn two_summaries_of_the_same_words_are_a_duplicate() {
    let (_directory, database, session_id, entity) = seeded_database(8).await;
    let text = "The same words every time.";
    let first = must(record_summary(&database, &summary(session_id, entity, 0, 3, text), 8).await);
    let error = must_err(
        record_summary(&database, &summary(session_id, entity, 4, 7, text), 8).await,
        "the same words and subject are one claim",
    );
    assert!(
        format!("{error}").contains("already recorded"),
        "expected a duplicate naming the existing memory, got {error}"
    );
    // And the identifier is the first summary's, so a caller can reinforce rather than guess.
    let existing = must(find_memory(&database, first.memory_id()).await)
        .record()
        .clone();
    assert_eq!(existing.id().to_string(), first.memory_id());
}

/// **A refused summary leaves no half-written row behind.**
///
/// The write is two statements in one transaction, and the assertion is on the **tables**, not on the returned
/// error: an implementation that inserted the memory and then failed on the span would leave a memory claiming
/// to be a summary with no span — reachable only by a read that joins to nothing, which is the shape that
/// survives review because it looks like data.
#[tokio::test]
async fn nothing_is_left_behind_when_a_summary_is_refused() {
    let (_directory, database, session_id, entity) = seeded_database(8).await;
    // One ordinary memory besides the summary, so the count is not trivially the summary itself.
    let ordinary_id = must(record_ordinary_memory(&database, entity).await);
    let ordinary = must(find_memory(&database, &ordinary_id).await)
        .record()
        .clone();
    assert_eq!(ordinary.memory_type(), jarvis_core::MemoryType::Semantic);

    must(
        record_summary(
            &database,
            &summary(session_id, entity, 0, 3, "First four."),
            8,
        )
        .await,
    );
    let memories_before = count_rows(&database, CountedTable::Memories).await;
    let spans_before = count_rows(&database, CountedTable::SessionSummaries).await;

    assert!(
        record_summary(&database, &summary(session_id, entity, 1, 3, "Overlap."), 8)
            .await
            .is_err()
    );

    assert_eq!(
        count_rows(&database, CountedTable::Memories).await,
        memories_before
    );
    assert_eq!(
        count_rows(&database, CountedTable::SessionSummaries).await,
        spans_before
    );
}

/// Records one ordinary semantic memory through the real repository, returning its identifier.
async fn record_ordinary_memory(
    database: &SqliteDatabase,
    entity: EntityId,
) -> Result<String, DatabaseError> {
    let record = must(jarvis_core::MemoryRecord::new(
        jarvis_core::MemoryRecordParts {
            id: MemoryId::new(),
            workspace_id: workspace(),
            memory_type: jarvis_core::MemoryType::Semantic,
            content: "The user lives in Utrecht.".to_owned(),
            structured_claim: None,
            source: must(MemorySource::of_kind(
                MemorySourceKind::UserStatement,
                "cli:1",
            )),
            confidence: jarvis_core::MemoryConfidence::Confirmed,
            importance: 3,
            sensitivity: Sensitivity::Internal,
            // A memory must name what it is about — the rule that caught this fixture on its first run — so
            // the ordinary memory names the same entity the summary does.
            entities: vec![EntityRef::confirmed(entity)],
            valid_from: None,
            valid_until: None,
            supersedes: None,
            run_id: None,
            created_by_actor_id: LOCAL_USER_ID.to_owned(),
            correlation_id: CorrelationId::new(),
            created_at: at(0),
        },
    ));
    let key = must(MemorySearchKey::new(
        record.memory_type(),
        record.entities(),
        record.content(),
    ));
    record_memory(database, &record, &key).await
}

/// Counts the rows of one of the two tables this slice writes.
///
/// Two explicit queries rather than a table name interpolated into SQL: a `format!`-built statement is the
/// shape that turns a test helper into an injection site, and sqlx refuses to build one without an explicit
/// escape hatch that would then hide that fact from a reviewer.
async fn count_rows(database: &SqliteDatabase, table: CountedTable) -> i64 {
    let sql = match table {
        CountedTable::Memories => "SELECT COUNT(*) FROM memories",
        CountedTable::SessionSummaries => "SELECT COUNT(*) FROM session_summaries",
    };
    must(sqlx::query_scalar(sql).fetch_one(database.pool()).await)
}

/// The two tables this slice writes.
enum CountedTable {
    Memories,
    SessionSummaries,
}

/// **`record_memory` still resolves its duplicate after the insert was extracted for the summary writer.**
///
/// The refactor's control. `insert_memory_on` now serves two callers, and a change that suited the summary's
/// path — returning the affected count without resolving a duplicate, say — would be invisible to every test
/// above. This exercises the caller that was there first, including the duplicate path the summary writer
/// reaches differently.
#[tokio::test]
async fn recording_an_ordinary_memory_still_resolves_its_duplicate() {
    let (_directory, database, _session_id, entity) = seeded_database(1).await;
    let id = must(record_ordinary_memory(&database, entity).await);
    assert_eq!(id.len(), 36);
    // The same words and subject again is a second claim about one thing, which the search key refuses.
    let error = must_err(
        record_ordinary_memory(&database, entity).await,
        "a second write of one claim must be refused",
    );
    assert!(
        matches!(error, DatabaseError::MemoryDuplicate { .. }),
        "expected a duplicate naming the existing memory, got {error:?}"
    );
}

/// **A summary page outside the bound is refused, so a client cannot ask for everything.**
#[tokio::test]
async fn a_summary_page_above_the_bound_is_refused() {
    let (_directory, database, session_id, _entity) = seeded_database(2).await;
    for limit in [0, MAX_SUMMARY_PAGE + 1] {
        let error = must_err(
            read_session_summaries(&database, session_id, limit).await,
            "a page outside the bound must be refused",
        );
        assert_eq!(format!("{error}"), "the summary limit is invalid");
    }
}

/// **The retention rule archives a session's summaries and leaves every durable memory alone.**
///
/// `P4-015` requires a retention rule; `memory-and-context.md` names it for `Conversation` ("session retention
/// applies"); and `MemoryType::is_durable` is where "which types that covers" is decided. This asserts the
/// *effect* of the rule and — the half that catches an inverted predicate — that a durable memory in the same
/// workspace is **untouched**. Without that second assertion, a sweep that archived everything would pass.
#[tokio::test]
async fn archiving_a_session_collects_its_summaries_and_no_durable_memory() {
    let (_directory, database, session_id, entity) = seeded_database(4).await;
    // A durable memory (a confirmed fact) and a session-scoped one of each kind.
    let durable_id = must(record_durable_memory(&database, entity).await);
    let summary =
        must(record_summary(&database, &summary(session_id, entity, 0, 3, "Gone."), 4).await);

    assert_eq!(
        must(archive_session_summaries(&database, session_id, at(5)).await),
        1,
        "exactly the one summary is archived"
    );

    // The summary is retained but no longer read as a current claim.
    let stored = must(find_memory(&database, summary.memory_id()).await);
    assert_eq!(
        stored.record().status(),
        jarvis_core::MemoryStatus::Archived
    );
    assert!(!stored.record().status().is_current_claim());
    assert!(must(read_session_summaries(&database, session_id, 8).await).is_empty());

    // The control: a durable memory is not collected, because the predicate is the domain's `is_durable`.
    let untouched = must(find_memory(&database, &durable_id).await);
    assert_eq!(
        untouched.record().status(),
        jarvis_core::MemoryStatus::Active
    );

    // Idempotent: a second sweep changes nothing, because it filters on `active`.
    assert_eq!(
        must(archive_session_summaries(&database, session_id, at(6)).await),
        0
    );
}

/// Records one durable semantic memory, returning its identifier.
async fn record_durable_memory(
    database: &SqliteDatabase,
    entity: EntityId,
) -> Result<String, DatabaseError> {
    let record = must(jarvis_core::MemoryRecord::new(
        jarvis_core::MemoryRecordParts {
            id: MemoryId::new(),
            workspace_id: workspace(),
            memory_type: jarvis_core::MemoryType::Semantic,
            content: "A durable fact that a sweep must not touch.".to_owned(),
            structured_claim: None,
            source: must(MemorySource::of_kind(
                MemorySourceKind::UserStatement,
                "cli:9",
            )),
            confidence: jarvis_core::MemoryConfidence::Confirmed,
            importance: 3,
            sensitivity: Sensitivity::Internal,
            entities: vec![EntityRef::confirmed(entity)],
            valid_from: None,
            valid_until: None,
            supersedes: None,
            run_id: None,
            created_by_actor_id: LOCAL_USER_ID.to_owned(),
            correlation_id: CorrelationId::new(),
            created_at: at(0),
        },
    ));
    let key = must(MemorySearchKey::new(
        record.memory_type(),
        record.entities(),
        record.content(),
    ));
    record_memory(database, &record, &key).await
}

/// **The collection predicate is generated from the domain's durability rule, not restated.**
///
/// The falsification for the sweep above. The predicate names exactly the types `is_durable` **rejects** — no
/// more and no fewer — so adding a session-scoped type makes it collectable without editing the query, and
/// adding a durable one cannot be swept. A hand-written list would pass the sweep test and drift silently the
/// first time the enum changes, which is the class of defect a derived predicate exists to remove.
#[test]
fn the_collection_predicate_names_exactly_the_non_durable_types() {
    let predicate = super::collection_predicate();
    let expected: Vec<&str> = jarvis_core::MemoryType::all()
        .iter()
        .filter(|memory_type| memory_type.is_durable())
        .map(|memory_type| memory_type.as_str())
        .collect();
    for name in expected {
        assert!(
            !predicate.contains(&format!("'{name}'")),
            "the predicate {predicate} must not collect the durable type {name}"
        );
    }
    // And the session-scoped ones are present, so the predicate is not empty and vacuously safe.
    assert!(
        predicate.contains("'conversation'"),
        "a summary must be collectable: {predicate}"
    );
}
