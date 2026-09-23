//! Read-only inspection of the canonical SQLite database.
//!
//! `jarvis doctor` must describe a broken installation without repairing it and
//! without mutating anything (`docs/architecture/storage.md` refuses to run
//! against a newer schema and expects diagnostics to explain recovery). Opening
//! the database for real would migrate it, create it if absent, and enable WAL,
//! so this module never uses [`crate::SqliteDatabase::open`]. It opens read-only
//! with `create_if_missing(false)` and reports what it finds as data.
//!
//! A database that is newer than this binary is a **normal finding**, not an
//! error: the whole point is to explain that state rather than fail on it.

use std::{
    fs,
    path::{Path, PathBuf},
};

use sqlx::{ConnectOptions, Connection, sqlite::SqliteConnectOptions};

use crate::{CURRENT_SCHEMA_VERSION, DatabaseError};

const JARVIS_APPLICATION_ID: i64 = 1_245_794_902;

/// Classification of the on-disk database, derived without mutation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DatabaseState {
    /// No database file exists yet, which is normal before first start.
    Missing,
    /// The file exists but holds no schema objects.
    Empty,
    /// The markers agree and the schema matches this build.
    Current {
        /// Schema version found on disk.
        version: i64,
    },
    /// A supported older schema that this build would migrate upward.
    NeedsMigration {
        /// Schema version found on disk.
        found: i64,
        /// Schema version this build owns.
        supported: i64,
    },
    /// A newer schema that this build must not touch.
    FutureSchema {
        /// Schema version found on disk.
        found: i64,
        /// Schema version this build owns.
        supported: i64,
    },
    /// The file has no JARVIS application marker.
    Unmanaged,
    /// The file belongs to a different SQLite application.
    Foreign {
        /// Application ID found in the header.
        application_id: i64,
    },
    /// The schema markers disagree with each other.
    Inconsistent {
        /// Version in SQLite's application-owned header field.
        user_version: i64,
        /// Highest successful `SQLx` migration version.
        migration_version: i64,
        /// Version recorded in JARVIS's metadata row, when present.
        metadata_version: Option<i64>,
    },
    /// The file could not be read as a SQLite database at all.
    Unreadable {
        /// Bounded, secret-free explanation.
        reason: String,
    },
}

impl DatabaseState {
    /// Reports whether this state requires attention.
    #[must_use]
    pub const fn is_healthy(&self) -> bool {
        matches!(self, Self::Missing | Self::Empty | Self::Current { .. })
    }
}

/// One persisted daemon lifecycle row, for read-only evidence.
///
/// The acceptance gate needs to prove that daemon state survives a restart, which
/// means counting durable records rather than trusting a process exit code. The
/// table is read directly because this path must not open the database for real.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DaemonInstanceRow {
    /// Lifecycle identity.
    pub id: String,
    /// `starting`, `ready`, or `stopped`.
    pub state: String,
    /// Operating-system process ID recorded at startup.
    pub process_id: i64,
    /// Build version recorded at startup.
    pub version: String,
    /// Stop reason, when recorded.
    pub stop_reason: Option<String>,
}

/// Everything read-only inspection learned about the database.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DatabaseInspection {
    /// Absolute database path that was inspected.
    pub path: PathBuf,
    /// File size in bytes, when the file exists.
    pub size_bytes: Option<u64>,
    /// Classification derived from the file header and markers.
    pub state: DatabaseState,
    /// Whether `PRAGMA integrity_check` returned exactly `ok`.
    pub integrity_ok: bool,
    /// Whether `PRAGMA foreign_key_check` returned any violating row.
    pub foreign_key_violations: bool,
    /// Journal mode reported by SQLite, when it could be read.
    pub journal_mode: Option<String>,
    /// Persisted daemon lifecycle rows, oldest first.
    pub daemon_instances: Vec<DaemonInstanceRow>,
}

/// Inspects the database at a path without mutating it.
///
/// # Errors
///
/// Returns [`DatabaseError`] only for a failure that prevented inspection, such
/// as an unusable path. A corrupt, foreign, or newer database is reported as a
/// [`DatabaseState`] rather than an error.
pub async fn inspect_database(path: &Path) -> Result<DatabaseInspection, DatabaseError> {
    if !path.is_absolute() {
        return Err(DatabaseError::RelativePath);
    }

    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(DatabaseError::SymbolicLink);
        }
        Ok(metadata) if !metadata.is_file() => return Err(DatabaseError::NotFile),
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(source) => return Err(DatabaseError::Inspect { source }),
    };

    let Some(metadata) = metadata else {
        return Ok(DatabaseInspection {
            path: path.to_path_buf(),
            size_bytes: None,
            state: DatabaseState::Missing,
            integrity_ok: true,
            foreign_key_violations: false,
            journal_mode: None,
            daemon_instances: Vec::new(),
        });
    };
    let size_bytes = Some(metadata.len());

    // Read-only with creation disabled: inspection must not create or migrate.
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(false)
        .read_only(true)
        .disable_statement_logging();

    let mut connection = match options.connect().await {
        Ok(connection) => connection,
        Err(source) => {
            return Ok(DatabaseInspection {
                path: path.to_path_buf(),
                size_bytes,
                state: DatabaseState::Unreadable {
                    reason: bounded_reason(&source.to_string()),
                },
                integrity_ok: false,
                foreign_key_violations: false,
                journal_mode: None,
                daemon_instances: Vec::new(),
            });
        }
    };

    let inspection = describe(&mut connection, path, size_bytes).await;
    let _ = connection.close().await;
    inspection
}

async fn describe(
    connection: &mut sqlx::SqliteConnection,
    path: &Path,
    size_bytes: Option<u64>,
) -> Result<DatabaseInspection, DatabaseError> {
    let application_id = match query_integer(connection, "PRAGMA application_id").await {
        Ok(value) => value,
        Err(reason) => return Ok(unreadable(path, size_bytes, reason)),
    };
    let user_version = match query_integer(connection, "PRAGMA user_version").await {
        Ok(value) => value,
        Err(reason) => return Ok(unreadable(path, size_bytes, reason)),
    };
    let object_count = match query_integer(
        connection,
        "SELECT COUNT(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
    )
    .await
    {
        Ok(value) => value,
        Err(reason) => return Ok(unreadable(path, size_bytes, reason)),
    };

    let integrity_ok = integrity_ok(connection).await.unwrap_or(false);
    let foreign_key_violations = has_foreign_key_violation(connection).await.unwrap_or(false);
    let journal_mode = query_text(connection, "PRAGMA journal_mode").await.ok();
    let daemon_instances = read_daemon_instances(connection).await.unwrap_or_default();

    let state = if application_id == 0 && user_version == 0 && object_count == 0 {
        DatabaseState::Empty
    } else if application_id == 0 {
        DatabaseState::Unmanaged
    } else if application_id != JARVIS_APPLICATION_ID {
        DatabaseState::Foreign { application_id }
    } else if user_version > CURRENT_SCHEMA_VERSION {
        DatabaseState::FutureSchema {
            found: user_version,
            supported: CURRENT_SCHEMA_VERSION,
        }
    } else {
        let migration_version = migration_version(connection).await.unwrap_or(0);
        if migration_version > CURRENT_SCHEMA_VERSION {
            DatabaseState::FutureSchema {
                found: migration_version,
                supported: CURRENT_SCHEMA_VERSION,
            }
        } else {
            let metadata_version = metadata_version(connection).await.unwrap_or(None);
            let markers_match = migration_version == user_version
                && (user_version == 0 || metadata_version == Some(user_version));
            if !markers_match {
                DatabaseState::Inconsistent {
                    user_version,
                    migration_version,
                    metadata_version,
                }
            } else if user_version < CURRENT_SCHEMA_VERSION {
                DatabaseState::NeedsMigration {
                    found: user_version,
                    supported: CURRENT_SCHEMA_VERSION,
                }
            } else {
                DatabaseState::Current {
                    version: user_version,
                }
            }
        }
    };

    Ok(DatabaseInspection {
        path: path.to_path_buf(),
        size_bytes,
        state,
        integrity_ok,
        foreign_key_violations,
        journal_mode,
        daemon_instances,
    })
}

/// Reads persisted daemon lifecycle rows, returning none when the table is absent.
async fn read_daemon_instances(
    connection: &mut sqlx::SqliteConnection,
) -> Result<Vec<DaemonInstanceRow>, String> {
    let exists = query_integer(
        connection,
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = 'daemon_instances')",
    )
    .await?;
    if exists == 0 {
        return Ok(Vec::new());
    }

    let rows: Vec<(String, String, i64, String, Option<String>)> = sqlx::query_as(
        "SELECT id, state, process_id, version, stop_reason \
         FROM daemon_instances ORDER BY started_at asc",
    )
    .fetch_all(&mut *connection)
    .await
    .map_err(|error| bounded_reason(&error.to_string()))?;

    Ok(rows
        .into_iter()
        .map(
            |(id, state, process_id, version, stop_reason)| DaemonInstanceRow {
                id,
                state,
                process_id,
                version,
                stop_reason,
            },
        )
        .collect())
}

fn unreadable(path: &Path, size_bytes: Option<u64>, reason: String) -> DatabaseInspection {
    DatabaseInspection {
        path: path.to_path_buf(),
        size_bytes,
        state: DatabaseState::Unreadable { reason },
        integrity_ok: false,
        foreign_key_violations: false,
        journal_mode: None,
        daemon_instances: Vec::new(),
    }
}

async fn integrity_ok(connection: &mut sqlx::SqliteConnection) -> Result<bool, String> {
    let rows: Vec<String> = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_all(&mut *connection)
        .await
        .map_err(|error| bounded_reason(&error.to_string()))?;
    Ok(rows.len() == 1 && rows.first().is_some_and(|value| value == "ok"))
}

async fn has_foreign_key_violation(
    connection: &mut sqlx::SqliteConnection,
) -> Result<bool, String> {
    let violation = sqlx::query("PRAGMA foreign_key_check")
        .fetch_optional(&mut *connection)
        .await
        .map_err(|error| bounded_reason(&error.to_string()))?;
    Ok(violation.is_some())
}

async fn migration_version(connection: &mut sqlx::SqliteConnection) -> Result<i64, String> {
    let exists = query_integer(
        connection,
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = '_sqlx_migrations')",
    )
    .await?;
    if exists == 0 {
        return Ok(0);
    }
    query_integer(
        connection,
        "SELECT COALESCE(MAX(version), 0) FROM _sqlx_migrations WHERE success = TRUE",
    )
    .await
}

async fn metadata_version(connection: &mut sqlx::SqliteConnection) -> Result<Option<i64>, String> {
    let exists = query_integer(
        connection,
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = 'jarvis_storage_metadata')",
    )
    .await?;
    if exists == 0 {
        return Ok(None);
    }
    sqlx::query_scalar::<_, i64>(
        "SELECT schema_version FROM jarvis_storage_metadata WHERE singleton = 1",
    )
    .fetch_optional(&mut *connection)
    .await
    .map_err(|error| bounded_reason(&error.to_string()))
}

async fn query_integer(
    connection: &mut sqlx::SqliteConnection,
    statement: &'static str,
) -> Result<i64, String> {
    sqlx::query_scalar::<_, i64>(statement)
        .fetch_one(&mut *connection)
        .await
        .map_err(|error| bounded_reason(&error.to_string()))
}

async fn query_text(
    connection: &mut sqlx::SqliteConnection,
    statement: &'static str,
) -> Result<String, String> {
    sqlx::query_scalar::<_, String>(statement)
        .fetch_one(&mut *connection)
        .await
        .map_err(|error| bounded_reason(&error.to_string()))
}

/// Bounds a driver message so diagnostics stay single-line and secret-free.
///
/// SQLite messages name the file and the failure, not parameter values, but the
/// bound is applied anyway so an unexpected driver change cannot widen evidence.
fn bounded_reason(message: &str) -> String {
    let flattened: String = message
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .take(200)
        .collect();
    if flattened.trim().is_empty() {
        "the database could not be read".to_owned()
    } else {
        flattened.trim().to_owned()
    }
}

#[cfg(test)]
mod tests {

    use sqlx::{ConnectOptions, Executor, sqlite::SqliteConnectOptions};

    use super::*;

    struct TempDirectory(PathBuf);

    impl TempDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "jarvis-storage-inspect-{}",
                jarvis_core::scratch_tag()
            ));
            fs::create_dir_all(&path).unwrap_or_else(|error| panic!("create temp dir: {error}"));
            Self(path)
        }

        fn file(&self) -> PathBuf {
            self.0.join("jarvis.sqlite3")
        }
    }

    impl Drop for TempDirectory {
        fn drop(&mut self) {
            jarvis_core::remove_scratch_dir(&self.0);
        }
    }

    async fn write_database(path: &Path, statements: &[&'static str]) {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .disable_statement_logging();
        let mut connection = options
            .connect()
            .await
            .unwrap_or_else(|error| panic!("open fixture: {error}"));
        for statement in statements {
            connection
                .execute(*statement)
                .await
                .unwrap_or_else(|error| panic!("execute {statement:?}: {error}"));
        }
        connection
            .close()
            .await
            .unwrap_or_else(|error| panic!("close fixture: {error}"));
    }

    /// Returns a `'static` statement derived from the current schema version.
    ///
    /// `sqlx::query` requires `'static` SQL text, so a version number that must track
    /// [`CURRENT_SCHEMA_VERSION`] has to be produced at runtime. Leaking the string is
    /// bounded and test-only, and the alternative is worse: the previous fixture
    /// hard-coded `3`, so the bump to 4 would have left it silently describing a schema
    /// that no longer exists while every assertion still passed.
    fn schema_statement(template: &str) -> &'static str {
        Box::leak(
            template
                .replace("{v}", &CURRENT_SCHEMA_VERSION.to_string())
                .into_boxed_str(),
        )
    }

    #[tokio::test]
    async fn a_missing_database_is_reported_without_creating_it() {
        let directory = TempDirectory::new();
        let path = directory.file();

        let inspection = inspect_database(&path)
            .await
            .unwrap_or_else(|error| panic!("inspect: {error}"));
        assert_eq!(inspection.state, DatabaseState::Missing);
        assert!(
            !path.exists(),
            "inspection must not create the database file"
        );
    }

    #[tokio::test]
    async fn a_current_database_is_classified_as_healthy() {
        let directory = TempDirectory::new();
        let path = directory.file();
        write_database(
            &path,
            &[
                "PRAGMA application_id = 1245794902",
                schema_statement("PRAGMA user_version = {v}"),
                "CREATE TABLE jarvis_storage_metadata (singleton INTEGER PRIMARY KEY, schema_version INTEGER NOT NULL)",
                schema_statement("INSERT INTO jarvis_storage_metadata VALUES (1, {v})"),
                "CREATE TABLE _sqlx_migrations (version BIGINT PRIMARY KEY, description TEXT NOT NULL, installed_on TEXT NOT NULL, success BOOLEAN NOT NULL, checksum BLOB NOT NULL, execution_time BIGINT NOT NULL)",
                schema_statement("INSERT INTO _sqlx_migrations VALUES ({v}, 'fixture', '2026-01-01T00:00:00Z', TRUE, x'00', 0)"),
            ],
        )
        .await;

        let before = fs::metadata(&path)
            .unwrap_or_else(|error| panic!("metadata: {error}"))
            .len();
        let inspection = inspect_database(&path)
            .await
            .unwrap_or_else(|error| panic!("inspect: {error}"));

        assert_eq!(
            inspection.state,
            DatabaseState::Current {
                version: CURRENT_SCHEMA_VERSION
            }
        );
        assert!(inspection.integrity_ok);
        assert!(!inspection.foreign_key_violations);
        assert!(inspection.state.is_healthy());

        // The file must be byte-for-byte the same size afterwards.
        let after = fs::metadata(&path)
            .unwrap_or_else(|error| panic!("metadata: {error}"))
            .len();
        assert_eq!(before, after, "inspection must not modify the database");
    }

    #[tokio::test]
    async fn a_newer_schema_is_a_finding_not_an_error() {
        let directory = TempDirectory::new();
        let path = directory.file();
        write_database(
            &path,
            &[
                "PRAGMA application_id = 1245794902",
                "PRAGMA user_version = 99",
            ],
        )
        .await;

        let inspection = inspect_database(&path)
            .await
            .unwrap_or_else(|error| panic!("inspect: {error}"));
        assert_eq!(
            inspection.state,
            DatabaseState::FutureSchema {
                found: 99,
                supported: CURRENT_SCHEMA_VERSION
            }
        );
        assert!(!inspection.state.is_healthy());
    }

    #[tokio::test]
    async fn inconsistent_markers_are_reported_with_their_values() {
        let directory = TempDirectory::new();
        let path = directory.file();
        write_database(
            &path,
            &[
                "PRAGMA application_id = 1245794902",
                schema_statement("PRAGMA user_version = {v}"),
                "CREATE TABLE jarvis_storage_metadata (singleton INTEGER PRIMARY KEY, schema_version INTEGER NOT NULL)",
                "INSERT INTO jarvis_storage_metadata VALUES (1, 1)",
                "CREATE TABLE _sqlx_migrations (version BIGINT PRIMARY KEY, description TEXT NOT NULL, installed_on TEXT NOT NULL, success BOOLEAN NOT NULL, checksum BLOB NOT NULL, execution_time BIGINT NOT NULL)",
                schema_statement("INSERT INTO _sqlx_migrations VALUES ({v}, 'fixture', '2026-01-01T00:00:00Z', TRUE, x'00', 0)"),
            ],
        )
        .await;

        let inspection = inspect_database(&path)
            .await
            .unwrap_or_else(|error| panic!("inspect: {error}"));
        // Derived from the constant, not written as a literal. The `metadata_version`
        // stays deliberately at 1 so the marker disagreement this test is about remains.
        assert_eq!(
            inspection.state,
            DatabaseState::Inconsistent {
                user_version: CURRENT_SCHEMA_VERSION,
                migration_version: CURRENT_SCHEMA_VERSION,
                metadata_version: Some(1)
            }
        );
    }

    #[tokio::test]
    async fn a_foreign_and_unreadable_file_are_distinguished() {
        let directory = TempDirectory::new();
        let foreign = directory.0.join("foreign.sqlite3");
        write_database(
            &foreign,
            &["PRAGMA application_id = 99", "CREATE TABLE t (a INTEGER)"],
        )
        .await;
        let inspection = inspect_database(&foreign)
            .await
            .unwrap_or_else(|error| panic!("inspect: {error}"));
        assert_eq!(
            inspection.state,
            DatabaseState::Foreign { application_id: 99 }
        );

        let garbage = directory.0.join("garbage.sqlite3");
        fs::write(&garbage, b"this is not a database at all")
            .unwrap_or_else(|error| panic!("write fixture: {error}"));
        let inspection = inspect_database(&garbage)
            .await
            .unwrap_or_else(|error| panic!("inspect: {error}"));
        assert!(
            matches!(inspection.state, DatabaseState::Unreadable { .. }),
            "expected unreadable, got {:?}",
            inspection.state
        );
    }
}
