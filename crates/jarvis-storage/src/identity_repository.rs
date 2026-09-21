//! Reads the single local user and workspace a profile belongs to.
//!
//! Migration `0005_seed_local_identity.sql` inserts these rows with fixed identifiers,
//! and this module is how the rest of the system finds them.
//!
//! # Why a fixed identity rather than a lookup by name
//!
//! The identifiers are constants here, matching the migration exactly. Resolving "the
//! active user" by querying for `status = 'active' LIMIT 1` would work on a fresh
//! profile and then silently return a different row once a server profile or a guest
//! identity exists. A constant cannot drift that way, and a mismatch between the
//! migration and this module is a test failure (`the_seeded_identity_matches_this_module`)
//! rather than a runtime surprise.
//!
//! # Why this is not `ActorContext`
//!
//! `docs/api/contracts.md` requires every command to receive an `ActorContext` carrying
//! authenticated identity, scopes, and correlation. That type arrives when
//! authentication does. This module supplies only the *resolved* local identity that an
//! already-authenticated local client acts as, which is what `P2-007` needs to satisfy
//! `agent_runs`' foreign keys. It deliberately has no scopes: authorization is not its
//! job and a scope set here would be a second source of authority.

use crate::database::{DatabaseError, SqliteDatabase};

/// Identifier of the seeded local user.
///
/// Must match `0005_seed_local_identity.sql`. Duplicated in both places on purpose: a
/// migration is SQL and cannot import a Rust constant, so the pair is held together by
/// an assertion instead of by hope.
pub const LOCAL_USER_ID: &str = "0198f000-0000-7000-8000-0000000000b1";

/// Identifier of the seeded local workspace.
///
/// Must match `0005_seed_local_identity.sql`.
pub const LOCAL_WORKSPACE_ID: &str = "0198f000-0000-7000-8000-0000000000b2";

/// The local identity a profile acts as.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalIdentity {
    user_id: String,
    workspace_id: String,
}

impl LocalIdentity {
    /// Returns the seeded local user identifier.
    #[must_use]
    pub fn user_id(&self) -> &str {
        &self.user_id
    }

    /// Returns the seeded local workspace identifier.
    #[must_use]
    pub fn workspace_id(&self) -> &str {
        &self.workspace_id
    }
}

/// Reads the seeded local identity, failing if the seed is absent.
///
/// # Errors
///
/// Returns [`DatabaseError::LocalIdentityMissing`] when the seeded rows do not exist.
/// That is a genuine failure rather than an empty result: a profile without its local
/// identity cannot create a session or a run, and reporting `Ok(None)` here would push
/// the same failure to the first write, where the cause is much harder to see.
pub async fn load_local_identity(
    database: &SqliteDatabase,
) -> Result<LocalIdentity, DatabaseError> {
    let user = sqlx::query_scalar::<_, String>("SELECT id FROM users WHERE id = ?1")
        .bind(LOCAL_USER_ID)
        .fetch_optional(database.pool())
        .await
        .map_err(|source| DatabaseError::Sqlite {
            operation: "read the local user",
            source,
        })?;
    if user.is_none() {
        return Err(DatabaseError::LocalIdentityMissing { field: "user" });
    }

    let workspace = sqlx::query_scalar::<_, String>("SELECT id FROM workspaces WHERE id = ?1")
        .bind(LOCAL_WORKSPACE_ID)
        .fetch_optional(database.pool())
        .await
        .map_err(|source| DatabaseError::Sqlite {
            operation: "read the local workspace",
            source,
        })?;
    if workspace.is_none() {
        return Err(DatabaseError::LocalIdentityMissing { field: "workspace" });
    }

    Ok(LocalIdentity {
        user_id: LOCAL_USER_ID.to_owned(),
        workspace_id: LOCAL_WORKSPACE_ID.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    use sqlx::Row;

    use super::*;
    use crate::DEFAULT_DATABASE_FILENAME;

    static TEST_DIRECTORY_ID: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let sequence = TEST_DIRECTORY_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "jarvis-local-identity-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir_all(&path).unwrap_or_else(|error| panic!("temp dir: {error}"));
            Self(path)
        }

        fn path(&self) -> &PathBuf {
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

    /// Opens a fresh database with nothing but the migrations applied.
    ///
    /// Deliberately does NOT insert identity rows: the whole point of `0005` is that a
    /// real profile has them without any client writing them, so a fixture that seeds
    /// them would test the fixture instead of the migration.
    async fn migrated_database() -> (TestDirectory, SqliteDatabase) {
        let directory = TestDirectory::new();
        let path = directory.path().join(DEFAULT_DATABASE_FILENAME);
        let database = must(SqliteDatabase::open(&path).await);
        (directory, database)
    }

    /// **The regression test for the seeding gap.** A freshly migrated profile has a
    /// local identity without any client having created one.
    #[tokio::test]
    async fn a_migrated_profile_has_a_local_identity() {
        let (_directory, database) = migrated_database().await;

        let identity = must(load_local_identity(&database).await);
        assert_eq!(identity.user_id(), LOCAL_USER_ID);
        assert_eq!(identity.workspace_id(), LOCAL_WORKSPACE_ID);
    }

    /// The migration's identifiers and this module's constants must agree. They are
    /// duplicated because SQL cannot import a Rust constant, so the pair is asserted
    /// rather than assumed; a rename in one place is a failure here, not a mystery at
    /// the first write that references a user that does not exist.
    #[tokio::test]
    async fn the_seeded_identity_matches_this_module() {
        let (_directory, database) = migrated_database().await;

        // Static SQL joined in the database rather than a `format!`-built statement.
        // sqlx rejects a dynamically composed query, and a table name interpolated into
        // SQL is the exact shape that rejection exists to prevent, so the query is
        // written out and the two identifiers are bound.
        let (users, workspaces) = must(
            sqlx::query_as::<_, (i64, i64)>(
                "SELECT \
                    (SELECT COUNT(*) FROM users WHERE id = ?1), \
                    (SELECT COUNT(*) FROM workspaces WHERE id = ?2)",
            )
            .bind(LOCAL_USER_ID)
            .bind(LOCAL_WORKSPACE_ID)
            .fetch_one(database.pool())
            .await,
        );
        assert_eq!(
            users, 1,
            "users must contain the seeded row {LOCAL_USER_ID}"
        );
        assert_eq!(
            workspaces, 1,
            "workspaces must contain the seeded row {LOCAL_WORKSPACE_ID}"
        );
    }

    /// A missing seed is reported as a missing identity, not as an empty result. The
    /// local user is deleted here so the guard is exercised, because a guard that only
    /// runs on a healthy database proves nothing.
    #[tokio::test]
    async fn a_profile_without_its_seed_is_reported() {
        let (_directory, database) = migrated_database().await;
        must(
            sqlx::query("DELETE FROM users WHERE id = ?1")
                .bind(LOCAL_USER_ID)
                .execute(database.pool())
                .await
                .map(|_| ()),
        );

        assert!(matches!(
            load_local_identity(&database).await,
            Err(DatabaseError::LocalIdentityMissing { field: "user" })
        ));
    }

    /// The seeded workspace is created in the mode and policy a local profile actually
    /// has, so a client cannot infer that data leaves the machine when it does not.
    #[tokio::test]
    async fn the_seeded_workspace_is_local_only() {
        let (_directory, database) = migrated_database().await;

        let row = must(
            sqlx::query("SELECT mode, data_policy, status FROM workspaces WHERE id = ?1")
                .bind(LOCAL_WORKSPACE_ID)
                .fetch_one(database.pool())
                .await,
        );
        assert_eq!(
            must(row.try_get::<String, _>("mode")),
            "local",
            "a SQLite-only profile must not describe itself as a server workspace"
        );
        assert_eq!(must(row.try_get::<String, _>("data_policy")), "local-only");
        assert_eq!(must(row.try_get::<String, _>("status")), "active");
    }

    /// Upgrading a profile that already holds these rows must not fail or rewrite them.
    /// `INSERT OR IGNORE` is what makes that true, so it is asserted rather than trusted.
    #[tokio::test]
    async fn an_existing_identity_survives_re_migration() {
        let directory = TestDirectory::new();
        let path = directory.path().join(DEFAULT_DATABASE_FILENAME);
        let database = must(SqliteDatabase::open(&path).await);
        must(
            sqlx::query("UPDATE users SET display_name = 'Renamed' WHERE id = ?1")
                .bind(LOCAL_USER_ID)
                .execute(database.pool())
                .await
                .map(|_| ()),
        );
        database.close().await;

        // Reopening re-runs the migration plan; `INSERT OR IGNORE` must leave the edit.
        let reopened = must(SqliteDatabase::open(&path).await);
        let name = must(
            sqlx::query_scalar::<_, String>("SELECT display_name FROM users WHERE id = ?1")
                .bind(LOCAL_USER_ID)
                .fetch_one(reopened.pool())
                .await,
        );
        assert_eq!(
            name, "Renamed",
            "a re-run migration must not overwrite an existing identity row"
        );
    }
}
