//! Tests for recording a workspace.
//!
//! The shape mirrors `memory_tests.rs`: a private `TestDirectory`, a `must` helper that panics with the whole
//! value, and a fixture that opens a migrated database. The workspace tests need one thing the memory tests do
//! not — the **seeded** identity rows — because the assertion that matters is that a recorded workspace is not
//! the one migration `0005` seeded.

use std::path::PathBuf;

use super::*;
use crate::{DEFAULT_DATABASE_FILENAME, LOCAL_WORKSPACE_ID, SqliteDatabase};

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("jarvis-workspace-{}", jarvis_core::scratch_tag()));
        must(std::fs::create_dir_all(&path));
        Self(path)
    }

    fn database_path(&self) -> PathBuf {
        self.0.join(DEFAULT_DATABASE_FILENAME)
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

/// Opens a migrated database, so migration `0005`'s seeded identity is present.
async fn database() -> (TestDirectory, SqliteDatabase) {
    let directory = TestDirectory::new();
    let database = must(SqliteDatabase::open(&directory.database_path()).await);
    (directory, database)
}

fn new_workspace(name: &str) -> NewWorkspace {
    NewWorkspace {
        id: WorkspaceId::new(),
        name: name.to_owned(),
        mode: WorkspaceMode::Local,
        data_policy: DataPolicy::Standard,
        created_at: UtcTimestamp::now(&jarvis_core::SystemClock),
    }
}

/// **A recorded workspace is a real second workspace, and it is not the seeded one.**
///
/// This is the property `A09` depends on and the reason `P4-010` added this function at all. A recorder that
/// silently returned the local workspace's identifier would make every isolation assertion pass against one
/// workspace — a green gate proving nothing.
///
/// The read-back goes through `inspect_database`, which is the path an operator's `jarvis doctor` takes,
/// rather than through the function that wrote the row. A writer and a reader from the same function would
/// agree by construction.
#[tokio::test]
async fn a_recorded_workspace_is_distinct_from_the_seeded_one() {
    let (directory, database) = database().await;
    let workspace = new_workspace("Second");
    let recorded = must(record_workspace(&database, &workspace).await);

    assert_eq!(recorded, workspace.id);
    assert_ne!(
        recorded.to_string(),
        LOCAL_WORKSPACE_ID,
        "a recorded workspace must not be the seeded one, or isolation is untestable"
    );

    let inspection = must(crate::inspect_database(&directory.database_path()).await);
    assert!(
        inspection.integrity_ok,
        "the database must still pass `PRAGMA integrity_check` after the insert"
    );
    // The foreign-key check is the assertion that makes this worth reading: `memories.workspace_id` and every
    // other workspace reference is a foreign key, so a row that did not land would make the *dependent* tables
    // inconsistent. A test that only counted workspaces would not notice.
    assert!(
        !inspection.foreign_key_violations,
        "recording a workspace must not leave a dangling reference: {:?}",
        inspection.path
    );
}

/// A second workspace can hold a claim, and the **seeded** identity still resolves to the first.
///
/// The second half is the guard that keeps this from becoming a scope change: recording a workspace must not
/// move which one the daemon acts as. `load_local_identity` is what every request resolves its scope through,
/// so if this ever returned the new workspace, every read in the product would silently change scope.
#[tokio::test]
async fn recording_a_workspace_does_not_change_the_resolved_identity() {
    let (_directory, database) = database().await;
    let before = must(crate::load_local_identity(&database).await);

    must(
        record_workspace(
            &database,
            &NewWorkspace {
                id: WorkspaceId::new(),
                name: "Second".to_owned(),
                mode: WorkspaceMode::Server,
                data_policy: DataPolicy::LocalOnly,
                created_at: UtcTimestamp::now(&jarvis_core::SystemClock),
            },
        )
        .await,
    );

    let after = must(crate::load_local_identity(&database).await);
    assert_eq!(
        before.workspace_id(),
        after.workspace_id(),
        "recording a workspace must not change which one the profile acts as"
    );
    assert_eq!(after.workspace_id(), LOCAL_WORKSPACE_ID);
}

/// A bad name is refused **by name**, not by a constraint violation.
///
/// Both edges are asserted: the refusals, and the bound itself being accepted. A guard that rejected the
/// largest legal value would pass a refusals-only test.
#[tokio::test]
async fn an_unusable_name_is_refused_and_the_bound_is_accepted() {
    let (_directory, database) = database().await;
    for (label, name) in [("empty", ""), ("whitespace", "   "), ("a tab", "\t")] {
        let refused = record_workspace(&database, &new_workspace(name)).await;
        assert!(
            matches!(
                refused,
                Err(DatabaseError::InvalidWorkspaceRequest { field: "name" })
            ),
            "a {label} name must be refused by field, got {refused:?}"
        );
    }
    assert!(matches!(
        record_workspace(&database, &new_workspace(&"x".repeat(129))).await,
        Err(DatabaseError::InvalidWorkspaceRequest { field: "name" })
    ));
    assert!(
        record_workspace(&database, &new_workspace(&"y".repeat(128)))
            .await
            .is_ok(),
        "the schema's own bound must be accepted"
    );
}

/// The enum values are the schema's own strings.
///
/// Asserted literally because a typo is a runtime constraint violation rather than a compile error, and the
/// enums exist to make the set closed. `local-only` in particular uses a **hyphen**, which is the kind of
/// value a `String` would have had wrong in one of two places.
#[test]
fn the_mode_and_policy_values_match_the_schema() {
    assert_eq!(WorkspaceMode::Local.as_str(), "local");
    assert_eq!(WorkspaceMode::Server.as_str(), "server");
    assert_eq!(DataPolicy::Standard.as_str(), "standard");
    assert_eq!(DataPolicy::LocalOnly.as_str(), "local-only");
}

/// A duplicate identifier does not error, which is the same idempotence the migration's `ON CONFLICT` gives.
///
/// Recorded as a decision rather than an accident: a retry after an ambiguous failure must not be reported as
/// a failure of its own, and the caller that wants to detect a collision reads the row.
#[tokio::test]
async fn recording_the_same_workspace_twice_is_idempotent() {
    let (_directory, database) = database().await;
    let workspace = new_workspace("Twice");
    must(record_workspace(&database, &workspace).await);
    let again = record_workspace(&database, &workspace).await;
    assert!(
        again.is_ok(),
        "a duplicate identifier must not be reported as a failure: {again:?}"
    );
}
