//! Tests for the skill revision repository.
//!
//! Weighted toward the rules that are **security properties** and the ones only a stored row can violate:
//! the promotion constraint, the provenance equality, and the fact that a decode re-applies the rules rather
//! than trusting the row.

use super::*;
use jarvis_core::{CorrelationId, UtcTimestamp};
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
    SkillRevision::new(SkillRevisionParts {
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
    })
    .unwrap_or_else(|error| panic!("fixture revision: {error}"))
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
    let usable_before = read_usable_skill_revisions(&database, &workspace_id, VALIDATOR)
        .await
        .unwrap_or_else(|error| panic!("read usable: {error}"));
    assert!(
        usable_before.is_empty(),
        "a proposal must not be offered as a usable revision"
    );

    let promoted = promote_skill_revision(
        &database,
        &proposed.revision_id().to_string(),
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

    let usable_after = read_usable_skill_revisions(&database, &workspace_id, VALIDATOR)
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
        "user-1",
        instant(20),
        VALIDATOR,
    )
    .await
    .unwrap_or_else(|error| panic!("first promote: {error}"));

    let second = promote_skill_revision(
        &database,
        &proposed.revision_id().to_string(),
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

/// **⭐⭐ A promotion with a blank approver is refused, and nothing is written.**
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
            id, skill_id, workspace_id, version, description, steps, source_kind, source_locator, \
            source_trust, source_excerpt_hash, state, dropped_fields, promoted_by_actor_id, \
            promoted_at, supersedes_revision_id, superseded_by_revision_id, run_id, \
            created_by_actor_id, correlation_id, created_at, updated_at, version_counter\
         ) VALUES (?1, ?2, ?3, '1', 'A procedure.', ?4, 'model_inference', 'run-9', 'derived', NULL, \
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
            id, skill_id, workspace_id, version, description, steps, source_kind, source_locator, \
            source_trust, source_excerpt_hash, state, dropped_fields, promoted_by_actor_id, \
            promoted_at, supersedes_revision_id, superseded_by_revision_id, run_id, \
            created_by_actor_id, correlation_id, created_at, updated_at, version_counter\
         ) VALUES (?1, ?2, ?3, '1', 'A procedure.', ?4, 'user_statement', 'session-1', \
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
                id, skill_id, workspace_id, version, description, steps, source_kind, source_locator, \
                source_trust, source_excerpt_hash, state, dropped_fields, promoted_by_actor_id, \
                promoted_at, supersedes_revision_id, superseded_by_revision_id, run_id, \
                created_by_actor_id, correlation_id, created_at, updated_at, version_counter\
             ) VALUES (?1, ?2, ?3, ?4, 'A procedure.', ?5, ?6, 'locator-1', ?7, NULL, 'proposed', '[]', \
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
        "user-1",
        instant(20),
        VALIDATOR,
    )
    .await
    .unwrap_or_else(|error| panic!("promote: {error}"));

    archive_skill_revision(
        &database,
        &proposed.revision_id().to_string(),
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
    let usable = read_usable_skill_revisions(&database, &workspace_id, VALIDATOR)
        .await
        .unwrap_or_else(|error| panic!("read usable: {error}"));
    assert!(usable.is_empty(), "an archived revision is not usable");

    let restored = restore_skill_revision(
        &database,
        &proposed.revision_id().to_string(),
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
        instant(30),
        VALIDATOR,
    )
    .await
    .unwrap_or_else(|error| panic!("archive: {error}"));
    assert!(
        archive_skill_revision(
            &database,
            &revision.revision_id().to_string(),
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

    let usable = read_usable_skill_revisions(&database, &workspace_id, VALIDATOR)
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
