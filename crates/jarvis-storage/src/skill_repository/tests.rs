//! Tests for the skill revision repository.
//!
//! Weighted toward the rules that are **security properties** and the ones only a stored row can violate:
//! the promotion constraint, the provenance equality, and the fact that a decode re-applies the rules rather
//! than trusting the row.

use super::*;
use jarvis_core::{CorrelationId, Sensitivity, UtcTimestamp};
use std::path::PathBuf;

/// The tool-identifier rule the tests pass in, mirroring `jarvis_tools::ToolId`'s shape.
///
/// A local function because this crate cannot depend on `jarvis-tools`. The rule it mirrors is "a dotted
/// identifier with a non-empty namespace and name"; a vacuous `|_| true` would not exercise the rejection
/// path, which is why the repository takes a validator at all.
fn valid_tool(identifier: &str) -> bool {
    identifier
        .split_once('.')
        .is_some_and(|(namespace, name)| !namespace.is_empty() && !name.is_empty())
}

const VALIDATOR: ToolValidator<'_> = &valid_tool;

/// The counter a freshly recorded revision holds.
///
/// `record_skill_revision` inserts `version_counter = 1`, and a transition increments it. Named rather than
/// written as a bare `1` so a test reads as "the version it currently holds" rather than as a magic number,
/// and so a change to the insert cannot silently make every guard test pass for the wrong reason.
const RECORDED_VERSION: i64 = 1;

/// Reads the counter a revision currently holds, which is what a control verb must present.
///
/// # Why the tests read it rather than tracking it by hand
///
/// Every transition increments `version_counter`, so a sequence of verbs must present a **different** number
/// each time — and a test that hardcoded `RECORDED_VERSION` for the second verb in a chain would fail, which
/// is the guard working correctly rather than a fixture problem. Reading the current value is also what a real
/// client does: it reads the revision, then presents the counter it observed, which is the entire point of an
/// optimistic guard. Tracking the arithmetic by hand in each test would be a second implementation of
/// `version_counter = version_counter + 1` that could disagree with the SQL.
async fn current_version(database: &crate::SqliteDatabase, revision_id: &str) -> i64 {
    find_skill_revision_state(database, revision_id, VALIDATOR)
        .await
        .unwrap_or_else(|error| panic!("read the revision's counter: {error}"))
        .version_counter()
}

/// The candidate window the fixtures pass to [`read_usable_skill_revisions`].
///
/// Large enough that no fixture here is truncated by it, because these tests are about **which rows** the
/// read returns rather than how many: a bound that cut a fixture's rows would make the assertions pass for
/// the wrong reason. The bound's own behaviour is asserted separately, with a small limit.
const SKILL_READ_LIMIT: u32 = 64;

fn instant(offset: i128) -> UtcTimestamp {
    UtcTimestamp::from_unix_nanos(1_700_000_000_000_000_000 + offset)
        .unwrap_or_else(|error| panic!("fixture clock: {error}"))
}

/// A step naming a real-looking tool.
fn step(position: u16, tool: &str) -> SkillStep {
    SkillStep::new(
        position,
        tool,
        "1.0.0",
        "Carry out the described action.",
        valid_tool,
    )
    .unwrap_or_else(|error| panic!("fixture step: {error}"))
}

/// A user-authored, active revision.
fn revision(workspace_id: &str) -> SkillRevision {
    SkillRevision::new(revision_parts(workspace_id))
        .unwrap_or_else(|error| panic!("fixture revision: {error}"))
}

/// The parts of a user-authored active revision, exposed so a test can vary one field.
///
/// A parts builder rather than a second full fixture, because the ordering test needs the **same** revision
/// with a different `created_at` � and a duplicated literal would be a second statement of the fixture that
/// could drift from this one.
fn revision_parts(workspace_id: &str) -> SkillRevisionParts {
    SkillRevisionParts {
        skill_id: SkillId::new(),
        workspace_id: workspace_id
            .parse()
            .unwrap_or_else(|error| panic!("fixture workspace: {error}")),
        revision_id: SkillId::new(),
        version: "1".to_owned(),
        description: "Summarize the open items in the user's notes.".to_owned(),
        steps: vec![step(1, "jarvis.files.read"), step(2, "jarvis.files.list")],
        source: MemorySource::of_kind(MemorySourceKind::UserStatement, "session-1")
            .unwrap_or_else(|error| panic!("fixture source: {error}")),
        sensitivity: Sensitivity::Internal,
        state: SkillState::Active,
        supersedes: None,
        dropped_fields: vec![
            SkillDroppedField::new("allowed_tools", DropReason::ToolSelection)
                .unwrap_or_else(|error| panic!("fixture drop: {error}")),
        ],
        // **`None`, not a generated run.** `run_id` is a foreign key to `agent_runs`, so a fixture run that
        // was never stored violates it — which is how this fixture was wrong the first time. A skill the user
        // authored has no producing run, which is why the field is optional in the first place.
        run_id: None,
        created_by_actor_id: "user-1".to_owned(),
        correlation_id: CorrelationId::new(),
        created_at: instant(0),
    }
}

/// A **model-authored proposal**, which is what a promotion acts on.
fn proposal(workspace_id: &str) -> SkillRevision {
    SkillRevision::new(SkillRevisionParts {
        skill_id: SkillId::new(),
        workspace_id: workspace_id
            .parse()
            .unwrap_or_else(|error| panic!("fixture workspace: {error}")),
        revision_id: SkillId::new(),
        version: "1".to_owned(),
        description: "A procedure the model suggested.".to_owned(),
        steps: vec![step(1, "jarvis.files.read")],
        source: MemorySource::of_kind(MemorySourceKind::ModelInference, "run-9")
            .unwrap_or_else(|error| panic!("fixture source: {error}")),
        sensitivity: Sensitivity::Internal,
        state: SkillState::Proposed,
        supersedes: None,
        dropped_fields: Vec::new(),
        // See the note in `revision`: a generated run that was never stored violates the foreign key.
        run_id: None,
        created_by_actor_id: "run-9".to_owned(),
        correlation_id: CorrelationId::new(),
        created_at: instant(10),
    })
    .unwrap_or_else(|error| panic!("fixture proposal: {error}"))
}

/// Opens a database with a seeded workspace, returning the database and the workspace identifier.
///
/// # Why the database lives in its own directory
///
/// The first version put the file **directly** in the system temp directory, and every open took over a
/// minute: `SqliteDatabase::open` calls `prepare_private_directory` on the database's parent, so it was
/// re-securing `%TEMP%` — a shared directory with many entries and a broad ACL — on every one of sixteen
/// tests. A private subdirectory is what the approval fixture already does, and it is fast because the
/// directory it secures is one it just created.
///
/// A lesson worth stating: the cost was not in the code under test but in *where the fixture put its file*,
/// and it looked like a hang rather than a slowdown.
async fn database() -> (TestDirectory, crate::SqliteDatabase, String) {
    let directory = TestDirectory::new();
    let database = crate::SqliteDatabase::open(&directory.0.join(crate::DEFAULT_DATABASE_FILENAME))
        .await
        .unwrap_or_else(|error| panic!("open fixture database: {error}"));
    let identity = crate::load_local_identity(&database)
        .await
        .unwrap_or_else(|error| panic!("the fixture must be seeded: {error}"));
    (directory, database, identity.workspace_id().to_string())
}

/// A temporary directory holding one test's database, removed on drop.
///
/// The `Drop` matters rather than being tidiness: without it every run left a database behind, and the
/// directory is the thing `prepare_private_directory` secures.
struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("jarvis-skills-{}", jarvis_core::scratch_tag()));
        std::fs::create_dir_all(&path)
            .unwrap_or_else(|error| panic!("create fixture directory: {error}"));
        Self(path)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        jarvis_core::remove_scratch_dir(&self.0);
    }
}

/// Encodes one step as the stored JSON shape, for the tests that write SQL directly.
fn stored_steps(tool: &str) -> String {
    serde_json::to_string(&vec![StoredStep {
        position: 1,
        tool: tool.to_owned(),
        tool_version: "1.0.0".to_owned(),
        instruction: "Read it.".to_owned(),
    }])
    .unwrap_or_else(|error| panic!("encode steps: {error}"))
}

/// **A revision round-trips with its steps, provenance, drops, and lifecycle intact.**
///
/// The baseline the rest of the file depends on: without it, a failing assertion below could be about the
/// decode rather than about the rule under test.
#[tokio::test]
async fn a_skill_revision_round_trips() {
    let (_dir, database, workspace_id) = database().await;
    let original = revision(&workspace_id);
    record_skill_revision(&database, &original)
        .await
        .unwrap_or_else(|error| panic!("record: {error}"));

    let read = find_skill_revision(&database, &original.revision_id().to_string(), VALIDATOR)
        .await
        .unwrap_or_else(|error| panic!("read: {error}"));

    assert_eq!(read.skill_id(), original.skill_id());
    assert_eq!(read.revision_id(), original.revision_id());
    assert_eq!(read.version(), "1");
    assert_eq!(read.description(), original.description());
    assert_eq!(read.state(), SkillState::Active);
    assert!(read.is_usable());
    assert_eq!(read.steps().len(), 2);
    assert_eq!(read.steps()[0].tool(), "jarvis.files.read");
    assert_eq!(read.steps()[0].tool_version(), "1.0.0");
    assert_eq!(read.source().kind(), MemorySourceKind::UserStatement);
    assert_eq!(read.source().trust(), MemoryTrust::Authoritative);
    assert_eq!(read.dropped_fields().len(), 1);
    assert_eq!(read.dropped_fields()[0].name(), "allowed_tools");

    database.close().await;
}

/// **⭐⭐ A promotion records its approver, and the row round-trips the attribution.**
///
/// `ADR-0117` §4 and `ADR-0043`. The rule is enforced in three places — the domain transition, the SQL
/// `WHERE state = 'proposed'` guard, and the schema's "active model-authored requires an approver" check —
/// and this asserts the stored result rather than the value the call returned.
#[tokio::test]
async fn a_promotion_records_and_round_trips_its_approver() {
    let (_dir, database, workspace_id) = database().await;
    let proposed = proposal(&workspace_id);
    record_skill_revision(&database, &proposed)
        .await
        .unwrap_or_else(|error| panic!("record: {error}"));

    // A proposal is not usable, which is what makes the promotion meaningful.
    let usable_before =
        read_usable_skill_revisions(&database, &workspace_id, SKILL_READ_LIMIT, VALIDATOR)
            .await
            .unwrap_or_else(|error| panic!("read usable: {error}"));
    assert!(
        usable_before.is_empty(),
        "a proposal must not be offered as a usable revision"
    );

    let promoted = promote_skill_revision(
        &database,
        &proposed.revision_id().to_string(),
        RECORDED_VERSION,
        "user-1",
        instant(20),
        VALIDATOR,
    )
    .await
    .unwrap_or_else(|error| panic!("promote: {error}"));
    assert_eq!(promoted.state(), SkillState::Active);
    assert_eq!(promoted.promoted_by_actor_id(), Some("user-1"));

    // Read back, so the assertion is about the stored row.
    let read = find_skill_revision(&database, &proposed.revision_id().to_string(), VALIDATOR)
        .await
        .unwrap_or_else(|error| panic!("read: {error}"));
    assert_eq!(read.state(), SkillState::Active);
    assert_eq!(
        read.promoted_by_actor_id(),
        Some("user-1"),
        "the approver a decision named must survive the round trip"
    );
    assert_eq!(read.promoted_at(), Some(instant(20)));

    let usable_after =
        read_usable_skill_revisions(&database, &workspace_id, SKILL_READ_LIMIT, VALIDATOR)
            .await
            .unwrap_or_else(|error| panic!("read usable: {error}"));
    assert_eq!(usable_after.len(), 1, "the promoted revision is usable");

    database.close().await;
}

/// **⭐⭐ A second promotion is refused rather than overwriting the first decision's approver.**
///
/// The property `ADR-0043` needs. The guard is `WHERE state = 'proposed'`, so the second call affects zero
/// rows — and the test asserts the **refusal** as well as the surviving approver, because a silent no-op
/// reporting success is the failure mode.
#[tokio::test]
async fn a_second_promotion_is_refused() {
    let (_dir, database, workspace_id) = database().await;
    let proposed = proposal(&workspace_id);
    record_skill_revision(&database, &proposed)
        .await
        .unwrap_or_else(|error| panic!("record: {error}"));
    promote_skill_revision(
        &database,
        &proposed.revision_id().to_string(),
        RECORDED_VERSION,
        "user-1",
        instant(20),
        VALIDATOR,
    )
    .await
    .unwrap_or_else(|error| panic!("first promote: {error}"));

    let second = promote_skill_revision(
        &database,
        &proposed.revision_id().to_string(),
        RECORDED_VERSION,
        "user-2",
        instant(30),
        VALIDATOR,
    )
    .await;
    assert!(
        second.is_err(),
        "a second promotion must be refused rather than replacing the approver"
    );

    let read = find_skill_revision(&database, &proposed.revision_id().to_string(), VALIDATOR)
        .await
        .unwrap_or_else(|error| panic!("read: {error}"));
    assert_eq!(
        read.promoted_by_actor_id(),
        Some("user-1"),
        "the first decision's approver must stand"
    );

    database.close().await;
}

/// **⭐⭐⭐ A stale counter is refused, and it is reported as a CONFLICT rather than as a missing row.**
///
/// The guard that makes every control verb safe: a promotion, an archive, or a deletion presents the counter
/// the caller **observed**, so a verb applied to a revision the operator has not read is refused. The same
/// rule a memory's correction follows, and for the same reason — `ADR-0117` §4 makes promotion an attributable
/// decision, and a decision taken against unseen text is not one.
///
/// # Why the two zero-row causes must be distinguished
///
/// A guarded `UPDATE` affecting no rows has **two** causes: a stale counter (the row exists and changed) and a
/// revision that is not there. The remedies are opposite — re-read and retry, versus stop — so collapsing them
/// sends an operator looking for a deletion that never happened. This asserts the **conflict**, and the sibling
/// test below asserts the not-found, so an implementation reporting one for both fails one of them.
///
/// The stale value is `RECORDED_VERSION + 5` rather than `RECORDED_VERSION - 1`: a counter is `>= 1`, so a
/// value below the initial one could be refused by a range check rather than by the comparison, and the test
/// would pass for the wrong reason.
#[tokio::test]
async fn a_stale_counter_is_reported_as_a_conflict() {
    let (_dir, database, workspace_id) = database().await;
    let proposed = proposal(&workspace_id);
    record_skill_revision(&database, &proposed)
        .await
        .unwrap_or_else(|error| panic!("record: {error}"));

    let stale = promote_skill_revision(
        &database,
        &proposed.revision_id().to_string(),
        RECORDED_VERSION + 5,
        "user-1",
        instant(20),
        VALIDATOR,
    )
    .await;
    assert!(
        matches!(stale, Err(DatabaseError::SkillConflict)),
        "a stale counter must be reported as a conflict, because the remedy is a re-read rather than a \
         search for a row that is present"
    );

    // The refusal must leave the revision a proposal: a guard that reports an error *and* writes satisfies the
    // assertion above while having promoted the revision anyway.
    let read = find_skill_revision(&database, &proposed.revision_id().to_string(), VALIDATOR)
        .await
        .unwrap_or_else(|error| panic!("read: {error}"));
    assert_eq!(
        read.state(),
        SkillState::Proposed,
        "a refused promotion must not have promoted anything"
    );
    assert_eq!(read.promoted_by_actor_id(), None);

    // **The control:** the counter the revision actually holds is accepted.
    promote_skill_revision(
        &database,
        &proposed.revision_id().to_string(),
        RECORDED_VERSION,
        "user-1",
        instant(20),
        VALIDATOR,
    )
    .await
    .unwrap_or_else(|error| panic!("the observed counter must be accepted: {error}"));

    database.close().await;
}

/// **A guarded write on a revision that does not exist is a NOT-FOUND, not a conflict.**
///
/// The complement of the test above, asserted separately because the two share one zero-row path: an
/// implementation that reported `SkillConflict` for both would satisfy that test and leave a caller told to
/// re-read something that is not there.
#[tokio::test]
async fn a_guarded_write_on_a_missing_revision_is_not_found() {
    let (_dir, database, _workspace_id) = database().await;
    let missing = SkillId::new().to_string();

    let result = promote_skill_revision(
        &database,
        &missing,
        RECORDED_VERSION,
        "user-1",
        instant(20),
        VALIDATOR,
    )
    .await;
    assert!(
        matches!(result, Err(DatabaseError::SkillRevisionNotFound)),
        "a missing revision must be reported as missing, so the caller is not sent to re-read it"
    );

    database.close().await;
}

/// **⭐⭐ Deleting a revision clears it, and a revision another one points at cannot be deleted.**
///
/// # Why the linked case is a refusal rather than a cascade
///
/// Both supersession columns are `REFERENCES ... ON DELETE SET NULL`, so the schema would let the delete
/// succeed and quietly **blank the surviving row's link** — leaving a successor that declares nothing about
/// what it replaced, or a predecessor with no way to find its replacement. `ADR-0117` §5's rule is that
/// replacement is **declared**, so erasing one leg of a declaration destroys the fact the rule exists to make
/// readable. The caller gets a reason and can delete the pair deliberately.
#[tokio::test]
async fn deleting_a_revision_is_guarded_by_its_links() {
    let (_dir, database, workspace_id) = database().await;
    let original = revision(&workspace_id);
    record_skill_revision(&database, &original)
        .await
        .unwrap_or_else(|error| panic!("record: {error}"));

    // An unlinked revision deletes cleanly, so the refusal below is about the link rather than about deletion
    // itself.
    delete_skill_revision(
        &database,
        &original.revision_id().to_string(),
        RECORDED_VERSION,
    )
    .await
    .unwrap_or_else(|error| panic!("an unlinked revision must be deletable: {error}"));
    assert!(
        matches!(
            find_skill_revision(&database, &original.revision_id().to_string(), VALIDATOR).await,
            Err(DatabaseError::SkillRevisionNotFound)
        ),
        "the row must be gone, not merely unreadable"
    );

    // Now the linked pair: a successor that declares what it replaced.
    let first = revision(&workspace_id);
    record_skill_revision(&database, &first)
        .await
        .unwrap_or_else(|error| panic!("record first: {error}"));
    let mut successor_parts = revision_parts(&workspace_id);
    successor_parts.skill_id = first.skill_id();
    successor_parts.version = "2".to_owned();
    successor_parts.supersedes = Some(first.revision_id());
    let successor = SkillRevision::new(successor_parts)
        .unwrap_or_else(|error| panic!("fixture successor: {error}"));
    record_skill_revision(&database, &successor)
        .await
        .unwrap_or_else(|error| panic!("record successor: {error}"));
    supersede_skill_revision(
        &database,
        &first.revision_id().to_string(),
        &successor.revision_id().to_string(),
    )
    .await
    .unwrap_or_else(|error| panic!("supersede: {error}"));

    let refused = delete_skill_revision(
        &database,
        &first.revision_id().to_string(),
        current_version(&database, &first.revision_id().to_string()).await,
    )
    .await;
    assert!(
        matches!(refused, Err(DatabaseError::SkillTransitionRefused { .. })),
        "deleting a revision another one declares a supersession with would erase one leg of that declaration"
    );

    // The link must still be intact, so the refusal preserved the fact rather than half-erasing it.
    let read_first = find_skill_revision(&database, &first.revision_id().to_string(), VALIDATOR)
        .await
        .unwrap_or_else(|error| panic!("read first: {error}"));
    assert_eq!(read_first.superseded_by(), Some(successor.revision_id()));

    // Deleting the **successor** is equally refused: the declaration has two ends, and the successor itself
    // names what it replaced, so removing it blanks the other leg.
    let refused_successor = delete_skill_revision(
        &database,
        &successor.revision_id().to_string(),
        current_version(&database, &successor.revision_id().to_string()).await,
    )
    .await;
    assert!(
        matches!(
            refused_successor,
            Err(DatabaseError::SkillTransitionRefused { .. })
        ),
        "the successor's own `supersedes` is the other leg, so it is protected too"
    );

    database.close().await;
}

/// **A promotion with a blank approver is refused, and nothing is written.**
///
/// An unattributable promotion is exactly what `ADR-0043` forbids. Asserted on the row as well as the return
/// value, because a caller that wrote the row and then errored would leave an active revision whose approver
/// reads as "someone".
#[tokio::test]
async fn a_promotion_without_an_approver_writes_nothing() {
    let (_dir, database, workspace_id) = database().await;
    let proposed = proposal(&workspace_id);
    record_skill_revision(&database, &proposed)
        .await
        .unwrap_or_else(|error| panic!("record: {error}"));

    let refused = promote_skill_revision(
        &database,
        &proposed.revision_id().to_string(),
        RECORDED_VERSION,
        "   ",
        instant(20),
        VALIDATOR,
    )
    .await;
    assert!(refused.is_err(), "a blank approver must be refused");

    let read = find_skill_revision(&database, &proposed.revision_id().to_string(), VALIDATOR)
        .await
        .unwrap_or_else(|error| panic!("read: {error}"));
    assert_eq!(
        read.state(),
        SkillState::Proposed,
        "the refused promotion must leave the row a proposal"
    );
    assert_eq!(read.promoted_by_actor_id(), None);

    database.close().await;
}

/// **⭐⭐ The schema refuses an active model-authored row that names no approver.**
///
/// This is the constraint **only the schema can state**: the domain must let a decode read back an active
/// model-authored revision (that is what a promotion produces), so the domain's construction check cannot
/// cover a stored row. The test bypasses the repository and writes SQL directly, which is the only way to
/// reach the constraint — a repository-mediated write cannot produce this row.
#[tokio::test]
async fn the_schema_refuses_an_unattributed_active_model_authored_row() {
    let (_dir, database, workspace_id) = database().await;

    let result = sqlx::query(
        "INSERT INTO skill_revisions (\
            id, skill_id, workspace_id, version, sensitivity, description, steps, source_kind, source_locator, \
            source_trust, source_excerpt_hash, state, dropped_fields, promoted_by_actor_id, \
            promoted_at, supersedes_revision_id, superseded_by_revision_id, run_id, \
            created_by_actor_id, correlation_id, created_at, updated_at, version_counter\
         ) VALUES (?1, ?2, ?3, '1', 'internal', 'A procedure.', ?4, 'model_inference', 'run-9', 'derived', NULL, \
            'active', '[]', NULL, NULL, NULL, NULL, NULL, 'run-9', ?5, ?6, ?6, 1)",
    )
    .bind(SkillId::new().to_string())
    .bind(SkillId::new().to_string())
    .bind(&workspace_id)
    .bind(stored_steps("jarvis.files.read"))
    .bind(CorrelationId::new().to_string())
    .bind(instant(0).to_string())
    .execute(database.pool())
    .await;

    assert!(
        result.is_err(),
        "an active model-authored revision with no approver must be unstorable"
    );

    database.close().await;
}

/// **⭐⭐ A decode re-applies the tool-identifier rule rather than trusting the row.**
///
/// The reason `validate_tool` is a parameter: a row written by another build, restored from a backup, or
/// edited outside JARVIS must be refused on read exactly as on write. Asserted against the real validator,
/// which rejects an identifier with no namespace — so the failure is the rule and not a parse error.
#[tokio::test]
async fn a_decode_re_applies_the_tool_identifier_rule() {
    let (_dir, database, workspace_id) = database().await;

    sqlx::query(
        "INSERT INTO skill_revisions (\
            id, skill_id, workspace_id, version, sensitivity, description, steps, source_kind, source_locator, \
            source_trust, source_excerpt_hash, state, dropped_fields, promoted_by_actor_id, \
            promoted_at, supersedes_revision_id, superseded_by_revision_id, run_id, \
            created_by_actor_id, correlation_id, created_at, updated_at, version_counter\
         ) VALUES (?1, ?2, ?3, '1', 'internal', 'A procedure.', ?4, 'user_statement', 'session-1', \
            'authoritative', NULL, 'active', '[]', NULL, NULL, NULL, NULL, NULL, 'user-1', ?5, ?6, ?6, 1)",
    )
    .bind(SkillId::new().to_string())
    .bind(SkillId::new().to_string())
    .bind(&workspace_id)
    .bind(stored_steps("not-a-tool"))
    .bind(CorrelationId::new().to_string())
    .bind(instant(0).to_string())
    .execute(database.pool())
    .await
    .unwrap_or_else(|error| panic!("the fixture row must be storable: {error}"));

    let rows = read_workspace_skill_revisions(&database, &workspace_id, VALIDATOR).await;
    assert!(
        rows.is_err(),
        "a stored step naming an invalid tool must be refused on read, not loaded"
    );

    database.close().await;
}

/// **⭐⭐ The provenance equality is restated in SQL, so a mismatched row cannot exist.**
///
/// The injection boundary the memories table also carries: a model inference is `derived` and external
/// content is `untrusted`. Asserted by writing SQL directly, because the repository's own path cannot produce
/// the row — which is the point of the second enforcer (`ADR-0117` §3).
#[tokio::test]
async fn the_schema_refuses_a_provenance_that_contradicts_its_kind() {
    let (_dir, database, workspace_id) = database().await;

    for (kind, trust) in [
        ("model_inference", "authoritative"),
        ("external_content", "authoritative"),
        ("user_statement", "derived"),
    ] {
        let result = sqlx::query(
            "INSERT INTO skill_revisions (\
                id, skill_id, workspace_id, version, sensitivity, description, steps, source_kind, source_locator, \
                source_trust, source_excerpt_hash, state, dropped_fields, promoted_by_actor_id, \
                promoted_at, supersedes_revision_id, superseded_by_revision_id, run_id, \
                created_by_actor_id, correlation_id, created_at, updated_at, version_counter\
             ) VALUES (?1, ?2, ?3, ?4, 'internal', 'A procedure.', ?5, ?6, 'locator-1', ?7, NULL, 'proposed', '[]', \
                NULL, NULL, NULL, NULL, NULL, 'user-1', ?8, ?9, ?9, 1)",
        )
        .bind(SkillId::new().to_string())
        .bind(SkillId::new().to_string())
        .bind(&workspace_id)
        .bind(format!("v-{kind}"))
        .bind(stored_steps("jarvis.files.read"))
        .bind(kind)
        .bind(trust)
        .bind(CorrelationId::new().to_string())
        .bind(instant(0).to_string())
        .execute(database.pool())
        .await;

        assert!(
            result.is_err(),
            "{kind} claiming {trust} trust must be unstorable"
        );
    }

    database.close().await;
}

/// **Two revisions of one skill at the same author version are refused.**
///
/// The `(workspace, skill, version)` unique index. Two rows claiming one version of one procedure is a
/// contradiction to surface rather than a second row to store.
#[tokio::test]
async fn a_duplicate_version_of_one_skill_is_refused() {
    let (_dir, database, workspace_id) = database().await;
    let first = revision(&workspace_id);
    record_skill_revision(&database, &first)
        .await
        .unwrap_or_else(|error| panic!("record: {error}"));

    // The same skill and version, but a different revision identity.
    let second = SkillRevision::new(SkillRevisionParts {
        skill_id: first.skill_id(),
        workspace_id: first.workspace_id(),
        revision_id: SkillId::new(),
        version: first.version().to_owned(),
        sensitivity: Sensitivity::Internal,
        description: "A different body under the same version.".to_owned(),
        steps: vec![step(1, "jarvis.files.read")],
        source: first.source().clone(),
        state: SkillState::Active,
        supersedes: None,
        dropped_fields: Vec::new(),
        run_id: None,
        created_by_actor_id: "user-1".to_owned(),
        correlation_id: CorrelationId::new(),
        created_at: instant(5),
    })
    .unwrap_or_else(|error| panic!("fixture: {error}"));
    assert!(
        record_skill_revision(&database, &second).await.is_err(),
        "two revisions of one skill may not claim the same version"
    );

    database.close().await;
}

/// **Archive and restore round-trip, and a restore returns to the state the promotion implies.**
#[tokio::test]
async fn archiving_and_restoring_round_trip() {
    let (_dir, database, workspace_id) = database().await;
    let proposed = proposal(&workspace_id);
    record_skill_revision(&database, &proposed)
        .await
        .unwrap_or_else(|error| panic!("record: {error}"));
    promote_skill_revision(
        &database,
        &proposed.revision_id().to_string(),
        RECORDED_VERSION,
        "user-1",
        instant(20),
        VALIDATOR,
    )
    .await
    .unwrap_or_else(|error| panic!("promote: {error}"));

    archive_skill_revision(
        &database,
        &proposed.revision_id().to_string(),
        current_version(&database, &proposed.revision_id().to_string()).await,
        instant(30),
        VALIDATOR,
    )
    .await
    .unwrap_or_else(|error| panic!("archive: {error}"));

    let archived = find_skill_revision(&database, &proposed.revision_id().to_string(), VALIDATOR)
        .await
        .unwrap_or_else(|error| panic!("read: {error}"));
    assert_eq!(archived.state(), SkillState::Archived);
    assert_eq!(
        archived.promoted_by_actor_id(),
        Some("user-1"),
        "the promotion's attribution must survive archiving"
    );

    // An archived revision is not offered for use.
    let usable = read_usable_skill_revisions(&database, &workspace_id, SKILL_READ_LIMIT, VALIDATOR)
        .await
        .unwrap_or_else(|error| panic!("read usable: {error}"));
    assert!(usable.is_empty(), "an archived revision is not usable");

    let restored = restore_skill_revision(
        &database,
        &proposed.revision_id().to_string(),
        current_version(&database, &proposed.revision_id().to_string()).await,
        instant(40),
        VALIDATOR,
    )
    .await
    .unwrap_or_else(|error| panic!("restore: {error}"));
    assert_eq!(
        restored.state(),
        SkillState::Active,
        "a promoted revision returns to active"
    );
    assert_eq!(restored.promoted_by_actor_id(), Some("user-1"));

    database.close().await;
}

/// **? The candidate window bounds the read, which is what makes one model call's cost a constant.**
///
/// The read runs on every model call, so an unbounded one would make the cost of a question a function of
/// how many procedures the user has written. The assertion is the **boundary** from both sides: at the limit
/// every row is returned, and one past it the newest are returned and the oldest is absent. A read that
/// ignored the limit fails the second half; a read that returned nothing fails the first.
///
/// # ? This test found a real ordering defect, which is why the offsets are whole seconds
///
/// The first version of this fixture used offsets of `0, 10, 20` **nanoseconds**, and the middle row was
/// dropped rather than the oldest. The cause is not the window: these timestamps are RFC 3339 text and a
/// whole second omits its fraction, so `'�:20Z'` sorts *after* `'�:20.00000002Z'` in byte order � the
/// lexicographic trap `UtcTimestamp`'s own module documents. The read now orders by `unixepoch(created_at)`,
/// which is why the offsets below are whole **seconds**: `unixepoch` resolves to seconds, so a fixture that
/// varied only the nanoseconds would be testing the tie-break rather than the ordering.
#[tokio::test]
async fn the_usable_read_returns_at_most_the_limit_newest_active_revisions() {
    let (_dir, database, workspace_id) = database().await;

    // One whole second for the oldest, then two instants inside the **same** second, so the text order
    // (whole second last) disagrees with the time order (whole second first). That disagreement is the
    // `ADR-0034` trap, and a fixture of evenly-spaced whole seconds cannot produce it.
    //
    // The three rows are also **within one second of each other**, which is what makes the tie-break the thing
    // under test: `unixepoch(created_at)` ties all three, and only the identifier separates them. The
    // ordering is `unixepoch(created_at) DESC, id DESC` — **the tie-break descends too**, because a descending
    // primary with an ascending tie-break orders a tie oldest-first, which is what this test caught.
    //
    // The identifiers carry the instant's milliseconds in their first 48 bits (the same millisecond in the
    // id's low three hex digits, then 1, 2, 3), **matching `created_at` exactly**, which is what a real write
    // produces. A deliberately disagreeing pair would test that this read prefers the identifier, which is not
    // what is wanted here.
    let instants = [
        ("018bcfe5-6b80-7001-9000-000000000000", 0_i128),
        ("018bcfe5-6b81-7002-9000-000000000000", 1_000_000),
        ("018bcfe5-6b82-7003-9000-000000000000", 2_000_000),
    ];

    let mut oldest = String::new();
    for (offset, (revision_id, nanos)) in instants.into_iter().enumerate() {
        let mut parts = revision_parts(&workspace_id);
        parts.description = format!("Procedure number {offset}.");
        parts.created_at = instant(nanos);
        parts.revision_id = revision_id
            .parse()
            .unwrap_or_else(|error| panic!("fixture revision id: {error}"));
        let revision =
            SkillRevision::new(parts).unwrap_or_else(|error| panic!("fixture revision: {error}"));
        record_skill_revision(&database, &revision)
            .await
            .unwrap_or_else(|error| panic!("record: {error}"));
        if offset == 0 {
            oldest = revision.revision_id().to_string();
        }
    }

    let all = read_usable_skill_revisions(&database, &workspace_id, 3, VALIDATOR)
        .await
        .unwrap_or_else(|error| panic!("read at the limit: {error}"));
    assert_eq!(
        all.len(),
        3,
        "a limit at the row count must return every row, or this test would pass for a read that returns \
         nothing"
    );

    let newest_two = read_usable_skill_revisions(&database, &workspace_id, 2, VALIDATOR)
        .await
        .unwrap_or_else(|error| panic!("read below the limit: {error}"));
    assert_eq!(newest_two.len(), 2, "the window must bound the read");
    assert!(
        !newest_two
            .iter()
            .any(|revision| revision.revision_id().to_string() == oldest),
        "the read is newest-first, so the oldest row is the one the window drops"
    );

    database.close().await;
}

/// **A redundant transition is refused rather than reported as success.**
#[tokio::test]
async fn redundant_skill_transitions_are_refused() {
    let (_dir, database, workspace_id) = database().await;
    let revision = revision(&workspace_id);
    record_skill_revision(&database, &revision)
        .await
        .unwrap_or_else(|error| panic!("record: {error}"));

    assert!(
        restore_skill_revision(
            &database,
            &revision.revision_id().to_string(),
            RECORDED_VERSION,
            instant(30),
            VALIDATOR
        )
        .await
        .is_err(),
        "restoring a revision that was never archived must be refused"
    );
    archive_skill_revision(
        &database,
        &revision.revision_id().to_string(),
        RECORDED_VERSION,
        instant(30),
        VALIDATOR,
    )
    .await
    .unwrap_or_else(|error| panic!("archive: {error}"));
    assert!(
        archive_skill_revision(
            &database,
            &revision.revision_id().to_string(),
            RECORDED_VERSION,
            instant(40),
            VALIDATOR
        )
        .await
        .is_err(),
        "archiving twice must be refused"
    );

    database.close().await;
}

/// **⭐ A supersession writes both legs of the declared pair, and a missing successor is refused.**
///
/// `ADR-0117` §5 and `ADR-0045`: replacement is declared, never inferred. Both directions are asserted,
/// because a forward pointer with no successor row would be a chain nothing can walk.
#[tokio::test]
async fn a_supersession_writes_both_directions() {
    let (_dir, database, workspace_id) = database().await;
    let original = revision(&workspace_id);
    record_skill_revision(&database, &original)
        .await
        .unwrap_or_else(|error| panic!("record: {error}"));

    let successor = SkillRevision::new(SkillRevisionParts {
        skill_id: original.skill_id(),
        workspace_id: original.workspace_id(),
        revision_id: SkillId::new(),
        version: "2".to_owned(),
        sensitivity: Sensitivity::Internal,
        description: "A corrected procedure.".to_owned(),
        steps: vec![step(1, "jarvis.files.read")],
        source: original.source().clone(),
        state: SkillState::Active,
        supersedes: Some(original.revision_id()),
        dropped_fields: Vec::new(),
        run_id: None,
        created_by_actor_id: "user-1".to_owned(),
        correlation_id: CorrelationId::new(),
        created_at: instant(5),
    })
    .unwrap_or_else(|error| panic!("fixture: {error}"));
    record_skill_revision(&database, &successor)
        .await
        .unwrap_or_else(|error| panic!("record successor: {error}"));

    // A successor that does not exist is refused, so the forward pointer is never dangling.
    assert!(
        supersede_skill_revision(
            &database,
            &original.revision_id().to_string(),
            &SkillId::new().to_string()
        )
        .await
        .is_err(),
        "a missing successor must be refused before the forward pointer is written"
    );

    supersede_skill_revision(
        &database,
        &original.revision_id().to_string(),
        &successor.revision_id().to_string(),
    )
    .await
    .unwrap_or_else(|error| panic!("supersede: {error}"));

    let read_successor =
        find_skill_revision(&database, &successor.revision_id().to_string(), VALIDATOR)
            .await
            .unwrap_or_else(|error| panic!("read successor: {error}"));
    assert_eq!(
        read_successor.supersedes(),
        Some(original.revision_id()),
        "the successor declares what it replaced"
    );

    let read_original =
        find_skill_revision(&database, &original.revision_id().to_string(), VALIDATOR)
            .await
            .unwrap_or_else(|error| panic!("read original: {error}"));
    assert_eq!(
        read_original.superseded_by(),
        Some(successor.revision_id()),
        "the replaced revision points forward, so the chain is walkable"
    );

    database.close().await;
}

/// **A revision cannot be replaced by itself.**
#[tokio::test]
async fn a_supersession_cycle_is_refused() {
    let (_dir, database, workspace_id) = database().await;
    let revision = revision(&workspace_id);
    record_skill_revision(&database, &revision)
        .await
        .unwrap_or_else(|error| panic!("record: {error}"));

    assert!(
        supersede_skill_revision(
            &database,
            &revision.revision_id().to_string(),
            &revision.revision_id().to_string()
        )
        .await
        .is_err(),
        "a revision cannot be replaced by itself"
    );

    database.close().await;
}

/// **⭐⭐⭐ A revision may not be declared as replacing ANOTHER skill's revision.**
///
/// `ADR-0117` §5 and `ADR-0045` make replacement a **declared** correction: the successor names what it
/// replaced so "which procedure ran" is readable rather than reconstructed. The forward leg is written by
/// [`supersede_skill_revision`], and this is the **only** layer that can check the rule — the domain holds one
/// revision and cannot ask whether a given `SkillId` names a revision of the same procedure, which its own
/// test records.
///
/// The rule the chain needs is that **both ends belong to the same `skill_id`**. Without it, `superseded_by`
/// can point from one procedure to a revision of a **different** one, so walking the chain from A reaches a
/// revision belonging to B — and a reader answering "what replaced this procedure" is handed a different
/// procedure's revision. The dangling-successor case is already refused (`acknowledge_skill_revision`
/// confirms the row exists), which is what makes this the **missing** half: the successor is confirmed to
/// *exist* and never confirmed to be *related*.
///
/// The assertion is the refusal, and the control beneath it is the pair that must keep working — a successor
/// of the same skill — because an implementation that refused every supersession would satisfy the refusal
/// alone.
#[tokio::test]
async fn a_revision_may_not_be_replaced_by_another_skills_revision() {
    let (_dir, database, workspace_id) = database().await;
    let original = revision(&workspace_id);
    record_skill_revision(&database, &original)
        .await
        .unwrap_or_else(|error| panic!("record: {error}"));

    // A revision of a DIFFERENT skill, in the same workspace. Every other property is valid, so the only
    // thing this cross-links is which procedure the chain describes.
    let mut foreign_parts = revision_parts(&workspace_id);
    foreign_parts.skill_id = SkillId::new();
    foreign_parts.description = "A different procedure entirely.".to_owned();
    foreign_parts.supersedes = None;
    let foreign = SkillRevision::new(foreign_parts)
        .unwrap_or_else(|error| panic!("fixture foreign revision: {error}"));
    record_skill_revision(&database, &foreign)
        .await
        .unwrap_or_else(|error| panic!("record foreign: {error}"));

    let refused = supersede_skill_revision(
        &database,
        &original.revision_id().to_string(),
        &foreign.revision_id().to_string(),
    )
    .await;
    assert!(
        refused.is_err(),
        "a chain that leaves its own procedure would make 'what replaced this' answer with another skill's \
         revision"
    );

    // The refusal must not have written the forward pointer anyway: a check that reports an error and writes
    // the row satisfies the assertion above while leaving the chain broken.
    let read_original =
        find_skill_revision(&database, &original.revision_id().to_string(), VALIDATOR)
            .await
            .unwrap_or_else(|error| panic!("read original: {error}"));
    assert_eq!(
        read_original.superseded_by(),
        None,
        "a refused supersession must leave no forward pointer"
    );

    // **The control:** the same call with a successor of the SAME skill is accepted, so the refusal above is
    // the relationship rule rather than a supersession that never works.
    let mut same_parts = revision_parts(&workspace_id);
    same_parts.skill_id = original.skill_id();
    same_parts.version = "2".to_owned();
    same_parts.description = "A corrected procedure.".to_owned();
    same_parts.supersedes = Some(original.revision_id());
    let successor =
        SkillRevision::new(same_parts).unwrap_or_else(|error| panic!("fixture successor: {error}"));
    record_skill_revision(&database, &successor)
        .await
        .unwrap_or_else(|error| panic!("record successor: {error}"));
    supersede_skill_revision(
        &database,
        &original.revision_id().to_string(),
        &successor.revision_id().to_string(),
    )
    .await
    .unwrap_or_else(|error| panic!("a same-skill successor must be accepted: {error}"));

    database.close().await;
}

/// **A missing revision is reported as missing rather than as a malformed value.**
#[tokio::test]
async fn a_missing_revision_is_not_found() {
    let (_dir, database, _workspace_id) = database().await;
    let error = find_skill_revision(&database, &SkillId::new().to_string(), VALIDATOR)
        .await
        .err()
        .unwrap_or_else(|| panic!("a missing revision must be reported"));
    // Matched rather than compared because `DatabaseError` is deliberately not `PartialEq`: it carries
    // `sqlx::Error`, which has no equality, and deriving one would require comparing a driver error's text.
    assert!(
        matches!(error, crate::DatabaseError::SkillRevisionNotFound),
        "a missing revision must be reported as missing, got {error:?}"
    );
    database.close().await;
}

/// **The inspection read includes every state; the usable read does not.**
///
/// The two surfaces answer different questions: selection needs only what may be used, while an operator
/// reviewing skills needs "why is this not being used" — the reason `read_workspace_skill_revisions` exists.
#[tokio::test]
async fn the_inspection_read_includes_every_state() {
    let (_dir, database, workspace_id) = database().await;
    let active = revision(&workspace_id);
    record_skill_revision(&database, &active)
        .await
        .unwrap_or_else(|error| panic!("record: {error}"));
    let proposed = proposal(&workspace_id);
    record_skill_revision(&database, &proposed)
        .await
        .unwrap_or_else(|error| panic!("record: {error}"));

    let everything = read_workspace_skill_revisions(&database, &workspace_id, VALIDATOR)
        .await
        .unwrap_or_else(|error| panic!("read all: {error}"));
    assert_eq!(everything.len(), 2, "both revisions must be listed");

    let usable = read_usable_skill_revisions(&database, &workspace_id, SKILL_READ_LIMIT, VALIDATOR)
        .await
        .unwrap_or_else(|error| panic!("read usable: {error}"));
    assert_eq!(usable.len(), 1, "only the active revision is usable");
    assert_eq!(usable[0].revision_id(), active.revision_id());

    database.close().await;
}

/// **One skill's revisions are readable without another's.**
#[tokio::test]
async fn revisions_are_scoped_to_their_skill() {
    let (_dir, database, workspace_id) = database().await;
    let first = revision(&workspace_id);
    record_skill_revision(&database, &first)
        .await
        .unwrap_or_else(|error| panic!("record: {error}"));
    // A second skill, so the fixture has two independent procedures.
    let second = revision(&workspace_id);
    record_skill_revision(&database, &second)
        .await
        .unwrap_or_else(|error| panic!("record: {error}"));

    let first_revisions = read_skill_revisions(
        &database,
        &workspace_id,
        &first.skill_id().to_string(),
        VALIDATOR,
    )
    .await
    .unwrap_or_else(|error| panic!("read: {error}"));
    assert_eq!(first_revisions.len(), 1);
    assert_eq!(first_revisions[0].revision_id(), first.revision_id());

    database.close().await;
}

/// **Every closed set round-trips through the stored form.**
///
/// The stored name is a contract with a reader, so a round trip is asserted for the vocabularies a row
/// carries: state, provenance kind and trust, and every drop reason.
#[tokio::test]
async fn the_stored_vocabularies_round_trip() {
    let (_dir, database, workspace_id) = database().await;
    let revision = SkillRevision::new(SkillRevisionParts {
        skill_id: SkillId::new(),
        workspace_id: workspace_id
            .parse()
            .unwrap_or_else(|error| panic!("workspace: {error}")),
        revision_id: SkillId::new(),
        version: "1".to_owned(),
        description: "A procedure carrying every drop reason.".to_owned(),
        steps: vec![step(1, "jarvis.files.read")],
        source: MemorySource::of_kind(MemorySourceKind::Document, "doc-1")
            .unwrap_or_else(|error| panic!("fixture source: {error}")),
        sensitivity: Sensitivity::Internal,
        state: SkillState::Proposed,
        supersedes: None,
        dropped_fields: DropReason::all()
            .into_iter()
            .map(|reason| {
                SkillDroppedField::new(format!("field-{}", reason.as_str()), reason)
                    .unwrap_or_else(|error| panic!("fixture drop: {error}"))
            })
            .collect(),
        run_id: None,
        created_by_actor_id: "user-1".to_owned(),
        correlation_id: CorrelationId::new(),
        created_at: instant(0),
    })
    .unwrap_or_else(|error| panic!("fixture: {error}"));
    record_skill_revision(&database, &revision)
        .await
        .unwrap_or_else(|error| panic!("record: {error}"));

    let read = find_skill_revision(&database, &revision.revision_id().to_string(), VALIDATOR)
        .await
        .unwrap_or_else(|error| panic!("read: {error}"));
    assert_eq!(read.state(), SkillState::Proposed);
    assert_eq!(read.source().kind(), MemorySourceKind::Document);
    assert_eq!(read.source().trust(), MemoryTrust::Derived);
    assert_eq!(
        read.dropped_fields().len(),
        DropReason::all().len(),
        "every drop reason must round-trip"
    );
    for reason in DropReason::all() {
        assert!(
            read.dropped_fields()
                .iter()
                .any(|dropped| dropped.reason() == reason),
            "{reason} must survive the round trip"
        );
    }

    database.close().await;
}

/// **⭐⭐ The classification round-trips, and it is what selection refuses on.**
///
/// `P4-012`'s whole disclosure rule rests on this column: a skill's prose and step instructions reach a
/// model, so a procedure about a confidential workflow must not be sent to a third-party model. A round trip
/// is asserted first, because a column that silently defaulted would make the selection rule refuse nothing
/// — the value would read as `internal` however the row was written.
#[tokio::test]
async fn the_classification_round_trips() {
    let (_dir, database, workspace_id) = database().await;

    for sensitivity in Sensitivity::all() {
        let mut parts = fixture_parts(&workspace_id, sensitivity);
        parts.version = sensitivity.as_str().to_owned();
        let revision =
            SkillRevision::new(parts).unwrap_or_else(|error| panic!("fixture revision: {error}"));
        record_skill_revision(&database, &revision)
            .await
            .unwrap_or_else(|error| panic!("record {sensitivity}: {error}"));

        let read = find_skill_revision(&database, &revision.revision_id().to_string(), VALIDATOR)
            .await
            .unwrap_or_else(|error| panic!("read {sensitivity}: {error}"));
        assert_eq!(
            read.sensitivity(),
            sensitivity,
            "{sensitivity} must survive the round trip rather than defaulting"
        );
    }

    database.close().await;
}

/// **The schema refuses a classification outside the closed set.**
///
/// The column carries a `CHECK`, so a row written by another build or restored from a backup cannot decode
/// into a value `Sensitivity` does not have. Asserted by writing SQL directly, because the repository's own
/// path cannot produce the row.
#[tokio::test]
async fn the_schema_refuses_an_unknown_classification() {
    let (_dir, database, workspace_id) = database().await;
    let parts = fixture_parts(&workspace_id, Sensitivity::Internal);

    let result = sqlx::query(
        "INSERT INTO skill_revisions (\
            id, skill_id, workspace_id, version, sensitivity, description, steps, source_kind, \
            source_locator, source_trust, source_excerpt_hash, state, dropped_fields, \
            promoted_by_actor_id, promoted_at, supersedes_revision_id, superseded_by_revision_id, \
            run_id, created_by_actor_id, correlation_id, created_at, updated_at, version_counter\
         ) VALUES (?1, ?2, ?3, '1', 'secret', 'A procedure.', ?4, 'user_statement', 'session-1', \
            'authoritative', NULL, 'proposed', '[]', NULL, NULL, NULL, NULL, NULL, 'user-1', ?5, ?6, ?6, 1)",
    )
    .bind(SkillId::new().to_string())
    .bind(parts.skill_id.to_string())
    .bind(&workspace_id)
    .bind(stored_steps("jarvis.files.read"))
    .bind(CorrelationId::new().to_string())
    .bind(instant(0).to_string())
    .execute(database.pool())
    .await;

    assert!(
        result.is_err(),
        "a classification outside the closed set must be unstorable"
    );

    database.close().await;
}

/// A fixture revision's parts, so the classification tests state only what they vary.
fn fixture_parts(workspace_id: &str, sensitivity: Sensitivity) -> SkillRevisionParts {
    SkillRevisionParts {
        skill_id: SkillId::new(),
        workspace_id: workspace_id
            .parse()
            .unwrap_or_else(|error| panic!("fixture workspace: {error}")),
        revision_id: SkillId::new(),
        version: "1".to_owned(),
        description: "Read the user's notes.".to_owned(),
        steps: vec![step(1, "jarvis.files.read")],
        source: MemorySource::of_kind(MemorySourceKind::UserStatement, "session-1")
            .unwrap_or_else(|error| panic!("fixture source: {error}")),
        sensitivity,
        state: SkillState::Active,
        supersedes: None,
        dropped_fields: Vec::new(),
        run_id: None,
        created_by_actor_id: "user-1".to_owned(),
        correlation_id: CorrelationId::new(),
        created_at: instant(0),
    }
}
