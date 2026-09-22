use std::{
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use jarvis_core::{DaemonRunId, UtcTimestamp};
use sqlx::{
    ConnectOptions, Connection, SqlitePool,
    migrate::{MigrateError, Migrator},
    sqlite::{SqliteConnectOptions, SqlitePoolOptions, SqliteSynchronous},
};
use thiserror::Error;

use crate::{
    AppPaths, PathError, PathKind,
    paths::{prepare_private_directory, secure_private_file},
};

/// Current application-owned SQLite schema version.
pub const CURRENT_SCHEMA_VERSION: i64 = 7;
/// Default filename for the canonical local database.
pub const DEFAULT_DATABASE_FILENAME: &str = "jarvis.sqlite3";

const JARVIS_APPLICATION_ID: i64 = 1_245_794_902;
const MINIMUM_SQLITE_VERSION: &str = "3.51.3";
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_CONNECTIONS: u32 = 4;

static MIGRATOR: Migrator = sqlx::migrate!("./migrations/sqlite");

/// Immutable metadata written when one daemon lifecycle begins.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DaemonInstanceStart {
    id: DaemonRunId,
    process_id: u32,
    version: String,
    target_os: String,
    target_arch: String,
    started_at: UtcTimestamp,
}

impl DaemonInstanceStart {
    /// Constructs validated daemon startup metadata.
    ///
    /// # Errors
    ///
    /// Returns [`DatabaseError::InvalidDaemonMetadata`] when the process ID is
    /// zero or a build label is empty, oversized, or contains unsafe text.
    pub fn new(
        id: DaemonRunId,
        process_id: u32,
        version: impl Into<String>,
        target_os: impl Into<String>,
        target_arch: impl Into<String>,
        started_at: UtcTimestamp,
    ) -> Result<Self, DatabaseError> {
        let version = version.into();
        let target_os = target_os.into();
        let target_arch = target_arch.into();
        if process_id == 0 {
            return Err(DatabaseError::InvalidDaemonMetadata {
                field: "process id",
            });
        }
        validate_build_label("version", &version, 64)?;
        validate_build_label("target operating system", &target_os, 32)?;
        validate_build_label("target architecture", &target_arch, 32)?;
        Ok(Self {
            id,
            process_id,
            version,
            target_os,
            target_arch,
            started_at,
        })
    }

    /// Returns the durable daemon lifecycle ID.
    #[must_use]
    pub const fn id(&self) -> DaemonRunId {
        self.id
    }
}

/// Stable reasons a daemon lifecycle can settle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DaemonStopReason {
    /// A supported operating-system shutdown signal was received.
    Signal,
    /// Startup failed after the lifecycle record was created.
    StartupFailed,
    /// Graceful resource shutdown encountered an error.
    ShutdownFailed,
}

impl DaemonStopReason {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Signal => "signal",
            Self::StartupFailed => "startup-failed",
            Self::ShutdownFailed => "shutdown-failed",
        }
    }
}

/// Errors produced while opening, checking, backing up, or migrating SQLite.
#[derive(Debug, Error)]
pub enum DatabaseError {
    /// The requested database path was not absolute.
    #[error("the SQLite database path is not absolute")]
    RelativePath,
    /// The requested database path had no usable parent directory.
    #[error("the SQLite database path has no parent directory")]
    MissingParent,
    /// The database endpoint was a symbolic link.
    #[error("the SQLite database path is a symbolic link")]
    SymbolicLink,
    /// The database endpoint existed but was not a regular file.
    #[error("the SQLite database path is not a regular file")]
    NotFile,
    /// A managed path could not be prepared or secured.
    #[error(transparent)]
    Path(#[from] PathError),
    /// The database endpoint could not be inspected.
    #[error("failed to inspect the SQLite database path")]
    Inspect {
        /// The underlying operating-system error.
        #[source]
        source: io::Error,
    },
    /// `SQLx` could not open the database pool.
    #[error("failed to open the SQLite database")]
    Open {
        /// The underlying `SQLx` error.
        #[source]
        source: sqlx::Error,
    },
    /// A database operation failed.
    #[error("failed to {operation} the SQLite database")]
    Sqlite {
        /// A bounded description of the attempted operation.
        operation: &'static str,
        /// The underlying `SQLx` error.
        #[source]
        source: sqlx::Error,
    },
    /// The linked SQLite runtime is older than the corruption-fix baseline.
    #[error("SQLite {found} is older than the required {minimum}")]
    RuntimeTooOld {
        /// Version reported by `sqlite_version()`.
        found: String,
        /// Minimum accepted SQLite release.
        minimum: &'static str,
    },
    /// SQLite returned a version string that could not be compared safely.
    #[error("SQLite returned an invalid runtime version")]
    InvalidRuntimeVersion {
        /// The bounded value returned by SQLite.
        found: String,
    },
    /// The embedded migration sequence did not match the compiled schema constant.
    #[error("the embedded SQLite migration plan is inconsistent")]
    InvalidMigrationPlan {
        /// Highest embedded migration version.
        embedded: i64,
        /// Compiled application schema version.
        expected: i64,
    },
    /// A non-empty database had no JARVIS application marker.
    #[error("the SQLite file is non-empty but is not marked as a JARVIS database")]
    UnmanagedDatabase,
    /// A database belongs to a different SQLite application.
    #[error("the SQLite file belongs to application id {application_id}")]
    ForeignDatabase {
        /// SQLite application ID found in the file header.
        application_id: i64,
    },
    /// The database was created by a newer JARVIS schema.
    #[error("SQLite schema {found} is newer than supported schema {supported}")]
    FutureSchema {
        /// Application schema version found on disk.
        found: i64,
        /// Highest schema version understood by this binary.
        supported: i64,
    },
    /// Application and migration version markers disagreed.
    #[error("the SQLite schema markers are inconsistent")]
    InconsistentSchema {
        /// Version in SQLite's application-owned header field.
        user_version: i64,
        /// Highest successful `SQLx` migration version.
        migration_version: i64,
        /// Version in JARVIS's metadata row, when present.
        metadata_version: Option<i64>,
    },
    /// A backup filename could not be represented safely for SQLite.
    #[error("the SQLite backup path is not valid Unicode")]
    NonUnicodeBackupPath,
    /// The system clock could not produce a collision-resistant backup name.
    #[error("the system clock is earlier than the Unix epoch")]
    ClockBeforeUnixEpoch,
    /// A generated backup endpoint already existed.
    #[error("the generated SQLite backup path already exists")]
    BackupAlreadyExists,
    /// The backup failed an independent consistency check.
    #[error("the pre-migration SQLite backup failed {check}")]
    InvalidBackup {
        /// Name of the failed bounded verification check.
        check: &'static str,
    },
    /// `SQLx` rejected or failed an embedded migration.
    #[error("failed to migrate the SQLite database")]
    Migration {
        /// Verified pre-migration backup available for recovery, if one was needed.
        backup: Option<PathBuf>,
        /// The underlying migration error.
        #[source]
        source: MigrateError,
    },
    /// The selected VFS could not enable WAL mode.
    #[error("SQLite did not enable WAL mode")]
    WalUnavailable {
        /// Journal mode returned by SQLite.
        actual: String,
    },
    /// Daemon lifecycle metadata failed bounded validation.
    #[error("the daemon {field} metadata is invalid")]
    InvalidDaemonMetadata {
        /// Stable field name without the rejected value.
        field: &'static str,
    },
    /// A daemon lifecycle transition was duplicate or out of order.
    #[error("the daemon lifecycle cannot transition to {transition}")]
    DaemonLifecycleConflict {
        /// Stable target state name.
        transition: &'static str,
    },
    /// Fields required to create a run failed bounded validation.
    #[error("the agent run {field} is invalid")]
    InvalidRunRequest {
        /// Stable field name without the rejected value.
        field: &'static str,
    },
    /// No run exists for the requested identifier.
    #[error("no agent run exists for the requested identifier")]
    RunNotFound,
    /// No message exists for the requested identifier.
    #[error("no message exists for the requested identifier")]
    MessageNotFound,
    /// Fields required to store a message failed bounded validation.
    #[error("the message {field} is invalid")]
    InvalidMessageRequest {
        /// Stable field name without the rejected value.
        field: &'static str,
    },
    /// A stored message row contradicted the schema's invariants.
    ///
    /// A storage-integrity finding rather than a caller mistake: the row came from another build,
    /// a restored backup, or an edit outside JARVIS. Reported as data so the caller can decide.
    #[error("the stored message {field} is not internally consistent")]
    StoredMessageInvalid {
        /// Stable field name without the stored value.
        field: &'static str,
    },
    /// A run write lost an optimistic-concurrency race or hit a duplicate identifier.
    ///
    /// The stored run was **not** changed. The caller must re-read it and decide from the
    /// current state, because the version it expected is no longer current.
    #[error("the agent run was changed by another writer")]
    RunConflict,
    /// The run state machine refused the requested transition.
    #[error("the agent run transition was refused")]
    RunTransitionRefused {
        /// The specific rule that rejected the edge.
        #[source]
        source: jarvis_core::RunTransitionError,
    },
    /// A stored run row contradicted the state machine's row invariants.
    ///
    /// This is a storage-integrity finding, not a caller mistake: the row was written by
    /// another build, restored from a backup, or edited outside JARVIS. It is reported as
    /// data rather than silently repaired, so the caller can decide and the finding stays
    /// visible.
    #[error("the stored agent run has an invalid {field}")]
    StoredRunInvalid {
        /// Stable field name without the offending value.
        field: &'static str,
    },
    /// Fields required to append a run event failed bounded validation.
    #[error("the run event {field} is invalid")]
    InvalidRunEventRequest {
        /// Stable field name without the rejected value.
        field: &'static str,
    },
    /// No run event exists for the requested identifier.
    #[error("no run event exists for the requested identifier")]
    RunEventNotFound,
    /// A run event write lost a race for its sequence number.
    ///
    /// The stored stream was **not** renumbered. The caller must re-read the highest
    /// stored sequence and retry, because another writer allocated the position it read.
    #[error("the run event sequence was taken by another writer")]
    RunEventConflict,
    /// A run event was appended to a run that had already settled.
    ///
    /// A settled run emits nothing more, so accepting this would leave a stream whose
    /// terminal event is not last — the stream would claim the run settled twice.
    #[error("a settled agent run cannot emit another event")]
    RunEventAfterSettlement,
    /// A stored run event row contradicted the domain's own validity rules.
    ///
    /// A storage-integrity finding rather than a caller mistake: the row was written by
    /// another build, restored from a backup, or edited outside JARVIS. Reported as data
    /// rather than silently repaired or dropped, so the finding stays visible.
    #[error("the stored run event has an invalid {field}")]
    StoredRunEventInvalid {
        /// Stable field name without the offending value.
        field: &'static str,
    },
    /// A profile did not contain the local identity migration `0005` seeds.
    ///
    /// A real failure rather than an empty result: without this row no session and no
    /// run can be created, so reporting `Ok(None)` would defer the cause to the first
    /// write that trips a foreign key, where it is much harder to attribute.
    #[error("this profile is missing its seeded local {field}")]
    LocalIdentityMissing {
        /// Stable field name without the offending value.
        field: &'static str,
    },
    /// Fields required to create a session failed bounded validation.
    #[error("the session {field} is invalid")]
    InvalidSessionRequest {
        /// Stable field name without the rejected value.
        field: &'static str,
    },
    /// No session exists for the requested identifier.
    #[error("no session exists for the requested identifier")]
    SessionNotFound,
    /// The session exists but no new run may be attached to it.
    ///
    /// Distinct from [`Self::SessionNotFound`] because the two call for different actions: an
    /// archived session means "start a new conversation", while a session this identity may not
    /// write into means "this is not yours", and reporting the first for the second would confirm
    /// that somebody else's session exists.
    #[error("the session {field} does not accept new work")]
    SessionNotWritable {
        /// Stable field name without the offending value.
        field: &'static str,
    },
    /// A stored session row contradicted the domain's own closed sets.
    ///
    /// A storage-integrity finding rather than a caller mistake: the row was written by
    /// another build, restored from a backup, or edited outside JARVIS. Reported rather
    /// than defaulted, because a defaulted `channel` would produce a session evaluated
    /// against a policy nobody chose.
    #[error("the stored session has an invalid {field}")]
    StoredSessionInvalid {
        /// Stable field name without the offending value.
        field: &'static str,
    },
    /// An approval request failed the domain's own validation.
    ///
    /// Delegated from [`jarvis_core::ApprovalRequest::new`] rather than re-checked here, so the
    /// bounds live in one place. The field name is stable and the offending value is never echoed:
    /// the preview is user-visible text and the intent hash is a digest, and neither belongs in an
    /// error that could reach a log.
    #[error("the approval {field} is invalid")]
    InvalidApprovalRequest {
        /// Stable field name without the offending value.
        field: &'static str,
    },
    /// No approval exists for the requested identifier.
    #[error("no approval exists for the requested identifier")]
    ApprovalNotFound,
    /// An approval's state or intent no longer matches what the caller expected.
    ///
    /// A guarded write matched nothing. Two causes, and both are real: the approval was decided by
    /// someone else in the meantime, or it lapsed. The caller must re-read rather than conclude the
    /// row is gone, which is why this is a conflict and not a not-found.
    #[error("the approval state changed before this decision could be recorded")]
    ApprovalConflict,
    /// A decision was presented against an approval that already has one.
    ///
    /// A **replay**, not a conflict: the nonce is one-time, so a second presentation of a recorded
    /// decision is either a retried request or an attempt to replace a denial. Both must be refused,
    /// and distinguishing it from [`Self::ApprovalConflict`] lets the caller report which happened.
    #[error("this approval already has a recorded decision")]
    ApprovalAlreadyDecided,
    /// A decision's nonce did not match the stored digest.
    ///
    /// The forgery case. The presented value is never echoed, because an error that carried it would
    /// put a candidate secret into a log.
    #[error("the presented decision nonce does not match this approval")]
    ApprovalNonceMismatch,
    /// A stored approval row contradicted the domain's own closed sets.
    ///
    /// A storage-integrity finding: the row was written by another build, restored from a backup, or
    /// edited outside JARVIS. Reported rather than defaulted, because a defaulted decision channel
    /// would attribute a decision to a person through a channel they never used.
    #[error("the stored approval has an invalid {field}")]
    StoredApprovalInvalid {
        /// Stable field name without the offending value.
        field: &'static str,
    },
    /// A tool call request failed the domain's own validation.
    #[error("the tool call {field} is invalid")]
    InvalidToolCallRequest {
        /// Stable field name without the offending value.
        field: &'static str,
    },
    /// No tool call exists for the requested identifier.
    #[error("no tool call exists for the requested identifier")]
    ToolCallNotFound,
    /// A tool call record is already stored under this run's idempotency key.
    ///
    /// **This is the duplicate-detection result, not an error condition.** The key is generated once
    /// when a call is admitted, so a re-driven pipeline that reaches this point has found the existing
    /// call and must read it rather than create a second one. Reported as its own variant because the
    /// caller's response is to *use* the returned identifier, and a generic conflict would push it
    /// into a retry loop.
    #[error("a tool call is already recorded for this run and idempotency key")]
    ToolCallDuplicate {
        /// The identifier of the call already recorded.
        existing_call_id: String,
    },
    /// A tool call outcome write lost a race for the record's version.
    ///
    /// Nothing was recorded on the losing side. The caller must re-read rather than assume the row is
    /// gone, because the two causes — another writer advanced the version, or the call does not exist
    /// — need different responses.
    #[error("the tool call version changed before this outcome could be recorded")]
    ToolCallConflict,
    /// A call's outcome may already be recorded, so this one is refused.
    ///
    /// Distinct from [`Self::ToolCallConflict`] because the cause is different and so is the remedy: a
    /// conflict means re-read and retry, while this means the outcome is already known. Refused for a
    /// **terminal** existing outcome, since an effect that is proven or disproven cannot be re-reported
    /// — and reporting it again is how an `Unknown` gets quietly replaced by a `Failed`.
    #[error("this tool call already has a reported outcome of {existing}")]
    ToolCallAlreadyResolved {
        /// The outcome already recorded.
        existing: String,
    },
    /// A stored tool call row contradicted the domain's own closed sets or honesty rules.
    #[error("the stored tool call has an invalid {field}")]
    StoredToolCallInvalid {
        /// Stable field name without the offending value.
        field: &'static str,
    },
}

/// An initialized local SQLite database owned by JARVIS.
#[derive(Debug)]
pub struct SqliteDatabase {
    pool: SqlitePool,
    path: PathBuf,
    sqlite_version: String,
    pre_migration_backup: Option<PathBuf>,
}

impl SqliteDatabase {
    /// Opens the canonical database under the resolved JARVIS data directory.
    ///
    /// # Errors
    ///
    /// Returns [`DatabaseError`] if the path cannot be secured, the database is
    /// corrupt, foreign, or newer than this binary, a backup cannot be verified,
    /// or a migration or connection setting fails.
    pub async fn open_default(paths: &AppPaths) -> Result<Self, DatabaseError> {
        let path = paths.data().join(DEFAULT_DATABASE_FILENAME);
        Self::open(&path).await
    }

    /// Opens an absolute SQLite path and brings it to the current schema.
    ///
    /// Existing recognized databases are backed up and independently verified
    /// before any pending migration runs.
    ///
    /// # Errors
    ///
    /// Returns [`DatabaseError`] if the path cannot be secured, the database is
    /// corrupt, foreign, or newer than this binary, a backup cannot be verified,
    /// or a migration or connection setting fails.
    pub async fn open(path: &Path) -> Result<Self, DatabaseError> {
        validate_migration_plan()?;
        prepare_database_path(path)?;

        let options = connection_options(path, true);
        let pool = SqlitePoolOptions::new()
            .max_connections(MAX_CONNECTIONS)
            .connect_with(options)
            .await
            .map_err(|source| DatabaseError::Open { source })?;

        if let Err(error) = secure_private_file(PathKind::Data, path) {
            pool.close().await;
            return Err(error.into());
        }

        let initialized = initialize(&pool, path).await;
        match initialized {
            Ok((sqlite_version, pre_migration_backup)) => Ok(Self {
                pool,
                path: path.to_path_buf(),
                sqlite_version,
                pre_migration_backup,
            }),
            Err(error) => {
                pool.close().await;
                Err(error)
            }
        }
    }

    /// Returns the absolute database path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the SQLite runtime version verified during startup.
    #[must_use]
    pub fn sqlite_version(&self) -> &str {
        &self.sqlite_version
    }

    /// Returns the verified backup created before this open migrated the schema.
    #[must_use]
    pub fn pre_migration_backup(&self) -> Option<&Path> {
        self.pre_migration_backup.as_deref()
    }

    /// Borrows the connection pool for the repository modules in this crate.
    ///
    /// Crate-private because a `SqlitePool` is an infrastructure detail: handing one to a
    /// caller outside this crate is how SQL escapes an adapter and reaches a use case,
    /// which `docs/architecture/repository-layout.md` forbids.
    pub(crate) const fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Closes every pooled connection and waits for SQLite workers to stop.
    pub async fn close(&self) {
        self.pool.close().await;
    }

    /// Persists the beginning of one daemon process lifecycle.
    ///
    /// # Errors
    ///
    /// Returns [`DatabaseError`] when insertion fails or the ID already exists.
    pub async fn record_daemon_start(
        &self,
        instance: &DaemonInstanceStart,
    ) -> Result<(), DatabaseError> {
        sqlx::query(
            "INSERT INTO daemon_instances (\
                id, process_id, version, target_os, target_arch, state, started_at\
             ) VALUES (?1, ?2, ?3, ?4, ?5, 'starting', ?6)",
        )
        .bind(instance.id.to_string())
        .bind(i64::from(instance.process_id))
        .bind(&instance.version)
        .bind(&instance.target_os)
        .bind(&instance.target_arch)
        .bind(instance.started_at.to_string())
        .execute(&self.pool)
        .await
        .map_err(|source| DatabaseError::Sqlite {
            operation: "record daemon startup",
            source,
        })?;
        Ok(())
    }

    /// Marks a starting daemon lifecycle ready to serve clients.
    ///
    /// # Errors
    ///
    /// Returns [`DatabaseError::DaemonLifecycleConflict`] unless exactly one
    /// matching lifecycle is still in `starting`, or [`DatabaseError::Sqlite`]
    /// if persistence fails.
    pub async fn mark_daemon_ready(
        &self,
        id: DaemonRunId,
        ready_at: UtcTimestamp,
    ) -> Result<(), DatabaseError> {
        let result = sqlx::query(
            "UPDATE daemon_instances \
             SET state = 'ready', ready_at = ?2 \
             WHERE id = ?1 AND state = 'starting'",
        )
        .bind(id.to_string())
        .bind(ready_at.to_string())
        .execute(&self.pool)
        .await
        .map_err(|source| DatabaseError::Sqlite {
            operation: "mark daemon ready",
            source,
        })?;
        require_single_transition(result.rows_affected(), "ready")
    }

    /// Settles a starting or ready daemon lifecycle as stopped.
    ///
    /// # Errors
    ///
    /// Returns [`DatabaseError::DaemonLifecycleConflict`] unless exactly one
    /// matching lifecycle is active, or [`DatabaseError::Sqlite`] if
    /// persistence fails.
    pub async fn mark_daemon_stopped(
        &self,
        id: DaemonRunId,
        stopped_at: UtcTimestamp,
        reason: DaemonStopReason,
    ) -> Result<(), DatabaseError> {
        let result = sqlx::query(
            "UPDATE daemon_instances \
             SET state = 'stopped', stopped_at = ?2, stop_reason = ?3 \
             WHERE id = ?1 AND state IN ('starting', 'ready')",
        )
        .bind(id.to_string())
        .bind(stopped_at.to_string())
        .bind(reason.as_str())
        .execute(&self.pool)
        .await
        .map_err(|source| DatabaseError::Sqlite {
            operation: "mark daemon stopped",
            source,
        })?;
        require_single_transition(result.rows_affected(), "stopped")
    }
}

fn validate_build_label(
    field: &'static str,
    value: &str,
    maximum_length: usize,
) -> Result<(), DatabaseError> {
    let valid = !value.is_empty()
        && value.len() <= maximum_length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b'+'));
    if valid {
        Ok(())
    } else {
        Err(DatabaseError::InvalidDaemonMetadata { field })
    }
}

fn require_single_transition(
    rows_affected: u64,
    transition: &'static str,
) -> Result<(), DatabaseError> {
    if rows_affected == 1 {
        Ok(())
    } else {
        Err(DatabaseError::DaemonLifecycleConflict { transition })
    }
}

/// Requires that a version-guarded run write changed exactly one row.
///
/// Zero rows means the `WHERE` clause matched nothing, which has two causes: the
/// expectation was stale, or the identifier does not exist. It is reported as a
/// **conflict** rather than as "not found", because a caller that lost a race must
/// re-read rather than conclude the run was deleted. A genuinely missing identifier
/// surfaces as [`DatabaseError::RunNotFound`] from the follow-up read, which is where
/// identity is actually known.
pub(crate) fn require_run_transition(rows_affected: u64) -> Result<(), DatabaseError> {
    if rows_affected == 1 {
        Ok(())
    } else {
        Err(DatabaseError::RunConflict)
    }
}

async fn initialize(
    pool: &SqlitePool,
    path: &Path,
) -> Result<(String, Option<PathBuf>), DatabaseError> {
    let sqlite_version = query_text(pool, "SELECT sqlite_version()", "read SQLite version").await?;
    require_safe_sqlite_version(&sqlite_version)?;

    let state = inspect_schema(pool).await?;
    let backup = if state.recognized && state.version < CURRENT_SCHEMA_VERSION {
        Some(create_verified_backup(pool, path, state.version).await?)
    } else {
        None
    };

    MIGRATOR
        .run(pool)
        .await
        .map_err(|source| DatabaseError::Migration {
            backup: backup.clone(),
            source,
        })?;

    let migrated = inspect_schema(pool).await?;
    if migrated.version != CURRENT_SCHEMA_VERSION {
        return Err(DatabaseError::InconsistentSchema {
            user_version: migrated.version,
            migration_version: migrated.migration_version,
            metadata_version: migrated.metadata_version,
        });
    }

    let journal_mode = query_text(
        pool,
        "PRAGMA journal_mode = WAL",
        "enable write-ahead logging",
    )
    .await?;
    if !journal_mode.eq_ignore_ascii_case("wal") {
        return Err(DatabaseError::WalUnavailable {
            actual: journal_mode,
        });
    }

    sqlx::query("PRAGMA optimize")
        .execute(pool)
        .await
        .map_err(|source| DatabaseError::Sqlite {
            operation: "optimize the migrated schema",
            source,
        })?;
    secure_private_file(PathKind::Data, path)?;

    Ok((sqlite_version, backup))
}

fn validate_migration_plan() -> Result<(), DatabaseError> {
    let embedded = MIGRATOR
        .iter()
        .map(|migration| migration.version)
        .max()
        .unwrap_or_default();
    if embedded == CURRENT_SCHEMA_VERSION {
        Ok(())
    } else {
        Err(DatabaseError::InvalidMigrationPlan {
            embedded,
            expected: CURRENT_SCHEMA_VERSION,
        })
    }
}

fn prepare_database_path(path: &Path) -> Result<(), DatabaseError> {
    if !path.is_absolute() {
        return Err(DatabaseError::RelativePath);
    }
    let parent = path.parent().ok_or(DatabaseError::MissingParent)?;
    prepare_private_directory(PathKind::Data, parent)?;

    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(DatabaseError::SymbolicLink),
        Ok(metadata) if !metadata.is_file() => Err(DatabaseError::NotFile),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(DatabaseError::Inspect { source }),
    }
}

fn connection_options(path: &Path, create_if_missing: bool) -> SqliteConnectOptions {
    SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(create_if_missing)
        .foreign_keys(true)
        .busy_timeout(BUSY_TIMEOUT)
        .synchronous(SqliteSynchronous::Full)
        .pragma("trusted_schema", "OFF")
        .pragma("recursive_triggers", "ON")
        .disable_statement_logging()
}

#[derive(Clone, Copy, Debug)]
struct SchemaState {
    recognized: bool,
    version: i64,
    migration_version: i64,
    metadata_version: Option<i64>,
}

async fn inspect_schema(pool: &SqlitePool) -> Result<SchemaState, DatabaseError> {
    let application_id =
        query_integer(pool, "PRAGMA application_id", "read application id").await?;
    let user_version = query_integer(pool, "PRAGMA user_version", "read schema version").await?;
    let object_count = query_integer(
        pool,
        "SELECT COUNT(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
        "inspect schema objects",
    )
    .await?;

    if application_id == 0 && user_version == 0 && object_count == 0 {
        return Ok(SchemaState {
            recognized: false,
            version: 0,
            migration_version: 0,
            metadata_version: None,
        });
    }
    if application_id == 0 {
        return Err(DatabaseError::UnmanagedDatabase);
    }
    if application_id != JARVIS_APPLICATION_ID {
        return Err(DatabaseError::ForeignDatabase { application_id });
    }
    if user_version > CURRENT_SCHEMA_VERSION {
        return Err(DatabaseError::FutureSchema {
            found: user_version,
            supported: CURRENT_SCHEMA_VERSION,
        });
    }

    let migration_version = read_migration_version(pool).await?;
    if migration_version > CURRENT_SCHEMA_VERSION {
        return Err(DatabaseError::FutureSchema {
            found: migration_version,
            supported: CURRENT_SCHEMA_VERSION,
        });
    }

    let metadata_version = read_metadata_version(pool).await?;
    let markers_match = migration_version == user_version
        && (user_version == 0 || metadata_version == Some(user_version));
    if !markers_match {
        return Err(DatabaseError::InconsistentSchema {
            user_version,
            migration_version,
            metadata_version,
        });
    }

    Ok(SchemaState {
        recognized: true,
        version: user_version,
        migration_version,
        metadata_version,
    })
}

async fn read_migration_version(pool: &SqlitePool) -> Result<i64, DatabaseError> {
    let exists = query_integer(
        pool,
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = '_sqlx_migrations')",
        "inspect migration history",
    )
    .await?;
    if exists == 0 {
        return Ok(0);
    }

    query_integer(
        pool,
        "SELECT COALESCE(MAX(version), 0) FROM _sqlx_migrations WHERE success = TRUE",
        "read migration history",
    )
    .await
}

async fn read_metadata_version(pool: &SqlitePool) -> Result<Option<i64>, DatabaseError> {
    let exists = query_integer(
        pool,
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = 'jarvis_storage_metadata')",
        "inspect storage metadata",
    )
    .await?;
    if exists == 0 {
        return Ok(None);
    }

    sqlx::query_scalar::<_, i64>(
        "SELECT schema_version FROM jarvis_storage_metadata WHERE singleton = 1",
    )
    .fetch_optional(pool)
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "read storage metadata",
        source,
    })
}

async fn create_verified_backup(
    pool: &SqlitePool,
    source_path: &Path,
    source_version: i64,
) -> Result<PathBuf, DatabaseError> {
    let backup_path = backup_path(source_path, source_version)?;
    match fs::symlink_metadata(&backup_path) {
        Ok(_) => return Err(DatabaseError::BackupAlreadyExists),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(source) => return Err(DatabaseError::Inspect { source }),
    }

    let backup_text = backup_path
        .to_str()
        .ok_or(DatabaseError::NonUnicodeBackupPath)?;
    sqlx::query("VACUUM INTO ?1")
        .bind(backup_text)
        .execute(pool)
        .await
        .map_err(|source| DatabaseError::Sqlite {
            operation: "create a pre-migration backup",
            source,
        })?;
    secure_private_file(PathKind::Data, &backup_path)?;
    verify_backup(&backup_path, source_version).await?;
    Ok(backup_path)
}

fn backup_path(source: &Path, source_version: i64) -> Result<PathBuf, DatabaseError> {
    let parent = source.parent().ok_or(DatabaseError::MissingParent)?;
    let mut filename = source
        .file_name()
        .map_or_else(|| OsString::from(DEFAULT_DATABASE_FILENAME), OsString::from);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| DatabaseError::ClockBeforeUnixEpoch)?
        .as_nanos();
    filename.push(format!(
        ".schema-v{source_version}-to-v{CURRENT_SCHEMA_VERSION}.{timestamp}.bak"
    ));
    Ok(parent.join(filename))
}

async fn verify_backup(path: &Path, expected_version: i64) -> Result<(), DatabaseError> {
    let options = connection_options(path, false).read_only(true);
    let mut connection = options
        .connect()
        .await
        .map_err(|source| DatabaseError::Sqlite {
            operation: "open the pre-migration backup",
            source,
        })?;

    let integrity = sqlx::query_scalar::<_, String>("PRAGMA integrity_check")
        .fetch_all(&mut connection)
        .await
        .map_err(|source| DatabaseError::Sqlite {
            operation: "verify backup integrity",
            source,
        })?;
    if integrity.len() != 1 || integrity.first().is_none_or(|value| value != "ok") {
        return Err(DatabaseError::InvalidBackup {
            check: "integrity validation",
        });
    }

    let foreign_key_violation = sqlx::query("PRAGMA foreign_key_check")
        .fetch_optional(&mut connection)
        .await
        .map_err(|source| DatabaseError::Sqlite {
            operation: "verify backup foreign keys",
            source,
        })?;
    if foreign_key_violation.is_some() {
        return Err(DatabaseError::InvalidBackup {
            check: "foreign-key validation",
        });
    }

    let application_id = sqlx::query_scalar::<_, i64>("PRAGMA application_id")
        .fetch_one(&mut connection)
        .await
        .map_err(|source| DatabaseError::Sqlite {
            operation: "verify backup application id",
            source,
        })?;
    let user_version = sqlx::query_scalar::<_, i64>("PRAGMA user_version")
        .fetch_one(&mut connection)
        .await
        .map_err(|source| DatabaseError::Sqlite {
            operation: "verify backup schema version",
            source,
        })?;
    if application_id != JARVIS_APPLICATION_ID || user_version != expected_version {
        return Err(DatabaseError::InvalidBackup {
            check: "identity validation",
        });
    }

    connection
        .close()
        .await
        .map_err(|source| DatabaseError::Sqlite {
            operation: "close the pre-migration backup",
            source,
        })
}

async fn query_integer(
    pool: &SqlitePool,
    query: &'static str,
    operation: &'static str,
) -> Result<i64, DatabaseError> {
    sqlx::query_scalar::<_, i64>(query)
        .fetch_one(pool)
        .await
        .map_err(|source| DatabaseError::Sqlite { operation, source })
}

async fn query_text(
    pool: &SqlitePool,
    query: &'static str,
    operation: &'static str,
) -> Result<String, DatabaseError> {
    sqlx::query_scalar::<_, String>(query)
        .fetch_one(pool)
        .await
        .map_err(|source| DatabaseError::Sqlite { operation, source })
}

fn require_safe_sqlite_version(version: &str) -> Result<(), DatabaseError> {
    let found =
        parse_sqlite_version(version).ok_or_else(|| DatabaseError::InvalidRuntimeVersion {
            found: version.chars().take(32).collect(),
        })?;
    let minimum = parse_sqlite_version(MINIMUM_SQLITE_VERSION).ok_or(
        DatabaseError::InvalidMigrationPlan {
            embedded: 0,
            expected: CURRENT_SCHEMA_VERSION,
        },
    )?;
    if found < minimum {
        Err(DatabaseError::RuntimeTooOld {
            found: version.chars().take(32).collect(),
            minimum: MINIMUM_SQLITE_VERSION,
        })
    } else {
        Ok(())
    }
}

fn parse_sqlite_version(version: &str) -> Option<(u32, u32, u32)> {
    let mut parts = version.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

#[cfg(test)]
mod tests {
    use std::{
        fmt::Debug,
        sync::atomic::{AtomicU64, Ordering},
    };

    use sqlx::SqliteConnection;

    use super::*;

    static TEST_DIRECTORY_ID: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let sequence = TEST_DIRECTORY_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "jarvis-storage-database-{}-{sequence}",
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

    fn must<T, E: Debug>(result: Result<T, E>) -> T {
        match result {
            Ok(value) => value,
            Err(error) => panic!("expected success, got {error:?}"),
        }
    }

    async fn execute(connection: &mut SqliteConnection, statement: &'static str) {
        must(sqlx::query(statement).execute(&mut *connection).await);
    }

    /// Returns a `'static` statement built at test time.
    ///
    /// `sqlx::query` requires `'static` SQL text, and a version-dependent fixture must
    /// be derived from the schema constant or it silently goes stale on the next
    /// schema bump. Leaking the text is bounded and correct in a test.
    fn leaked(statement: String) -> &'static str {
        Box::leak(statement.into_boxed_str())
    }

    async fn create_version_zero_fixture(path: &Path) {
        let mut connection = must(connection_options(path, true).connect().await);
        execute(&mut connection, "PRAGMA application_id = 1245794902").await;
        execute(
            &mut connection,
            "CREATE TABLE legacy_sentinel (value TEXT NOT NULL) STRICT",
        )
        .await;
        execute(
            &mut connection,
            "INSERT INTO legacy_sentinel (value) VALUES ('before-migration')",
        )
        .await;
        execute(&mut connection, "PRAGMA user_version = 0").await;
        must(connection.close().await);
    }

    async fn create_future_fixture(path: &Path) {
        // Derived from the current version so that bumping the schema cannot quietly
        // turn this into a *current* database and stop testing the future case.
        let statement = leaked(format!(
            "PRAGMA user_version = {}",
            CURRENT_SCHEMA_VERSION + 1
        ));
        let mut connection = must(connection_options(path, true).connect().await);
        execute(&mut connection, "PRAGMA application_id = 1245794902").await;
        execute(&mut connection, statement).await;
        must(connection.close().await);
    }

    // --- Phase 2 schema constraints -------------------------------------------
    //
    // `docs/data/schema.md` requires tests for constraints, unique/idempotency
    // behaviour, and optimistic concurrency. These assert that the migration's
    // invariants are enforced by SQLite rather than merely documented, because a
    // constraint that only exists in prose is not a constraint.

    const USER: &str = "0198f000-0000-7000-8000-000000000001";
    const WORKSPACE: &str = "0198f000-0000-7000-8000-000000000002";
    const SESSION: &str = "0198f000-0000-7000-8000-000000000003";
    const RUN: &str = "0198f000-0000-7000-8000-000000000004";
    const CORRELATION: &str = "0198f000-0000-7000-8000-000000000005";
    const MESSAGE: &str = "0198f000-0000-7000-8000-000000000006";
    const AT: &str = "2026-09-21T10:00:00Z";
    const HASH: &str = "0123456789abcdef0123456789abcdef";

    /// Opens a fresh database and seeds the single local user and workspace.
    async fn seeded_database() -> (TestDirectory, SqliteDatabase) {
        let directory = TestDirectory::new();
        let path = directory.path().join(DEFAULT_DATABASE_FILENAME);
        let database = must(SqliteDatabase::open(&path).await);
        must(
            execute_sql(
                &database,
                "INSERT INTO users \
             (id, display_name, status, created_at, updated_at, version) \
             VALUES (?1, 'Local User', 'active', ?2, ?2, 1)",
                &[USER, AT],
            )
            .await,
        );
        must(
            execute_sql(
                &database,
                "INSERT INTO workspaces \
             (id, name, mode, data_policy, status, created_at, updated_at, version) \
             VALUES (?1, 'Local', 'local', 'local-only', 'active', ?2, ?2, 1)",
                &[WORKSPACE, AT],
            )
            .await,
        );
        must(
            execute_sql(
                &database,
                "INSERT INTO sessions \
             (id, workspace_id, user_id, channel, status, created_at, updated_at, version) \
             VALUES (?1, ?2, ?3, 'cli', 'active', ?4, ?4, 1)",
                &[SESSION, WORKSPACE, USER, AT],
            )
            .await,
        );
        (directory, database)
    }

    /// Runs a parameterized statement, binding each value as text.
    async fn execute_sql(
        database: &SqliteDatabase,
        statement: &'static str,
        binds: &[&str],
    ) -> Result<sqlx::sqlite::SqliteQueryResult, sqlx::Error> {
        let mut query = sqlx::query(statement);
        for value in binds {
            query = query.bind(*value);
        }
        query.execute(&database.pool).await
    }

    #[tokio::test]
    async fn phase_two_tables_exist_after_migration() {
        let (_directory, database) = seeded_database().await;
        for table in [
            "users",
            "workspaces",
            "sessions",
            "messages",
            "agent_runs",
            "agent_steps",
            "model_calls",
        ] {
            let exists = must(
                sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'table' AND name = ?1",
                )
                .bind(table)
                .fetch_one(&database.pool)
                .await,
            );
            assert_eq!(exists, 1, "expected table {table} to exist");
        }
    }

    #[tokio::test]
    async fn a_duplicate_message_sequence_within_a_session_is_rejected() {
        let (_directory, database) = seeded_database().await;
        let insert = "INSERT INTO messages \
             (id, session_id, sequence, author_kind, role, content, content_bytes, \
              sensitivity, source, created_at) \
             VALUES (?1, ?2, 0, 'user', 'user', 'hello', 5, 'internal', 'user', ?3)";
        must(execute_sql(&database, insert, &[MESSAGE, SESSION, AT]).await);

        // A second distinct message reusing sequence 0 must not be storable, or the
        // conversation order becomes ambiguous and non-replayable.
        let second = "0198f000-0000-7000-8000-000000000007";
        let outcome = execute_sql(&database, insert, &[second, SESSION, AT]).await;
        assert!(
            outcome.is_err(),
            "a duplicate sequence within one session must be rejected"
        );
    }

    #[tokio::test]
    async fn a_message_referencing_a_missing_session_is_rejected() {
        let (_directory, database) = seeded_database().await;
        let outcome = execute_sql(
            &database,
            "INSERT INTO messages \
             (id, session_id, sequence, author_kind, role, content, content_bytes, \
              sensitivity, source, created_at) \
             VALUES (?1, ?2, 0, 'user', 'user', 'orphan', 6, 'internal', 'user', ?3)",
            &[MESSAGE, CORRELATION, AT],
        )
        .await;
        assert!(
            outcome.is_err(),
            "foreign keys are enforced, so an orphan message must be rejected"
        );
    }

    #[tokio::test]
    async fn a_terminal_run_without_an_outcome_is_unrepresentable() {
        let (_directory, database) = seeded_database().await;
        let insert = "INSERT INTO agent_runs \
             (id, session_id, workspace_id, user_id, objective, state, started_at, \
              updated_at, correlation_id, version) \
             VALUES (?1, ?2, ?3, ?4, 'do a thing', ?5, ?6, ?6, ?7, 1)";

        // `completed` with no `completed_at`/`terminal_outcome` must fail.
        let outcome = execute_sql(
            &database,
            insert,
            &[RUN, SESSION, WORKSPACE, USER, "completed", AT, CORRELATION],
        )
        .await;
        assert!(
            outcome.is_err(),
            "a terminal state must carry an outcome and an end time"
        );

        // The same row in a non-terminal state is valid, which proves the CHECK
        // rejects the inconsistency rather than the insert itself.
        let accepted = execute_sql(
            &database,
            insert,
            &[RUN, SESSION, WORKSPACE, USER, "received", AT, CORRELATION],
        )
        .await;
        assert!(accepted.is_ok(), "a non-terminal run must be storable");
    }

    #[tokio::test]
    async fn an_unknown_run_state_is_rejected() {
        let (_directory, database) = seeded_database().await;
        let outcome = execute_sql(
            &database,
            "INSERT INTO agent_runs \
             (id, session_id, workspace_id, user_id, objective, state, started_at, \
              updated_at, correlation_id, version) \
             VALUES (?1, ?2, ?3, ?4, 'do a thing', 'thinking_hard', ?5, ?5, ?6, 1)",
            &[RUN, SESSION, WORKSPACE, USER, AT, CORRELATION],
        )
        .await;
        assert!(
            outcome.is_err(),
            "a state outside the run state machine must be rejected"
        );
    }

    #[tokio::test]
    async fn optimistic_concurrency_lets_exactly_one_writer_advance_a_run() {
        let (_directory, database) = seeded_database().await;
        must(
            execute_sql(
                &database,
                "INSERT INTO agent_runs \
             (id, session_id, workspace_id, user_id, objective, state, started_at, \
              updated_at, correlation_id, version) \
             VALUES (?1, ?2, ?3, ?4, 'do a thing', 'received', ?5, ?5, ?6, 1)",
                &[RUN, SESSION, WORKSPACE, USER, AT, CORRELATION],
            )
            .await,
        );

        let advance = "UPDATE agent_runs SET state = ?1, version = version + 1 \
                       WHERE id = ?2 AND version = ?3";

        // Two writers both read version 1. The first wins.
        let first = must(execute_sql(&database, advance, &["context_building", RUN, "1"]).await);
        assert_eq!(first.rows_affected(), 1);

        // The second presents the version it read and changes nothing.
        let second = must(execute_sql(&database, advance, &["planning", RUN, "1"]).await);
        assert_eq!(
            second.rows_affected(),
            0,
            "a stale version must not advance the run"
        );

        // A writer that re-reads the new version succeeds, so the guard is the
        // version and not a blanket refusal.
        let third = must(execute_sql(&database, advance, &["planning", RUN, "2"]).await);
        assert_eq!(third.rows_affected(), 1);
    }

    #[tokio::test]
    async fn a_failed_model_call_without_an_error_code_is_unrepresentable() {
        let (_directory, database) = seeded_database().await;
        must(
            execute_sql(
                &database,
                "INSERT INTO agent_runs \
             (id, session_id, workspace_id, user_id, objective, state, started_at, \
              updated_at, correlation_id, version) \
             VALUES (?1, ?2, ?3, ?4, 'do a thing', 'received', ?5, ?5, ?6, 1)",
                &[RUN, SESSION, WORKSPACE, USER, AT, CORRELATION],
            )
            .await,
        );

        // Two separate statements so each rejection is attributable.
        //
        // The failure case binds `NULL` explicitly rather than an empty string: an
        // empty string would also violate the column's length CHECK, so the insert
        // would be rejected for the wrong reason and the test would pass without ever
        // exercising the rule it claims to prove.
        let failed_id = "0198f000-0000-7000-8000-0000000000f1";
        let failed = execute_sql(
            &database,
            "INSERT INTO model_calls \
             (id, run_id, provider_id, model_id, attempt, status, streaming, \
              prompt_hash, started_at, ended_at, error_code) \
             VALUES (?1, ?2, 'openai-compatible', 'gpt-oss:20b', 1, 'failed', 0, ?3, ?4, ?4, NULL)",
            &[failed_id, RUN, HASH, AT],
        )
        .await;
        assert!(
            failed.is_err(),
            "a failed call must record a redacted error code, not null"
        );

        // A success omits the column, so it is genuinely null rather than empty, and
        // must be storable. This also proves the CHECK is not merely presence-based.
        let succeeded_id = "0198f000-0000-7000-8000-0000000000f2";
        let succeeded = execute_sql(
            &database,
            "INSERT INTO model_calls \
             (id, run_id, provider_id, model_id, attempt, status, streaming, \
              prompt_hash, started_at, ended_at) \
             VALUES (?1, ?2, 'openai-compatible', 'gpt-oss:20b', 1, 'succeeded', 0, ?3, ?4, ?4)",
            &[succeeded_id, RUN, HASH, AT],
        )
        .await;
        assert!(
            succeeded.is_ok(),
            "a successful call must be storable without an error code"
        );
    }

    #[tokio::test]
    async fn an_unknown_sensitivity_level_is_rejected_by_storage() {
        let (_directory, database) = seeded_database().await;
        let outcome = execute_sql(
            &database,
            "INSERT INTO messages \
             (id, session_id, sequence, author_kind, role, content, content_bytes, \
              sensitivity, source, created_at) \
             VALUES (?1, ?2, 0, 'user', 'user', 'hi', 2, ?3, 'user', ?4)",
            &[MESSAGE, SESSION, "top_secret", AT],
        )
        .await;
        assert!(
            outcome.is_err(),
            "the sensitivity vocabulary must match `jarvis_core::Sensitivity`"
        );
    }

    #[tokio::test]
    async fn a_stored_message_size_that_disagrees_with_its_text_is_rejected() {
        let (_directory, database) = seeded_database().await;

        // Sequence is a bind parameter because `UNIQUE (session_id, sequence)` would
        // otherwise reject the later inserts and the test would pass or fail for a
        // reason unrelated to the size constraint it claims to prove.
        let insert = "INSERT INTO messages \
             (id, session_id, sequence, author_kind, role, content, content_bytes, \
              sensitivity, source, created_at) \
             VALUES (?1, ?2, ?3, 'user', 'user', ?4, ?5, 'internal', 'user', ?6)";

        // Establish what SQLite actually measures. `length()` counts CHARACTERS for a
        // TEXT value and BYTES for a BLOB, so the migration's cast is load-bearing.
        let rust_bytes = i64::try_from("héllo".len()).unwrap_or_default();
        let (blob_len, text_len) = must(
            sqlx::query_as::<_, (i64, i64)>("SELECT length(CAST(?1 AS BLOB)), length(?1)")
                .bind("héllo")
                .fetch_one(&database.pool)
                .await,
        );
        assert_eq!(
            (blob_len, text_len),
            (rust_bytes, 5),
            "the cast must yield UTF-8 bytes ({rust_bytes}) while plain length() yields \
             the character count"
        );

        // A size inconsistent with the content cannot be stored. Without this, a
        // budget computed from `content_bytes` could disagree with the text actually
        // sent to a model.
        let wrong = execute_sql(
            &database,
            insert,
            &[MESSAGE, SESSION, "0", "hello", "999", AT],
        )
        .await;
        assert!(
            wrong.is_err(),
            "a stored size inconsistent with the content must be rejected"
        );

        // A consistent byte count is accepted.
        let right_id = "0198f000-0000-7000-8000-000000000008";
        let right = execute_sql(
            &database,
            insert,
            &[right_id, SESSION, "1", "hello", "5", AT],
        )
        .await;
        assert!(right.is_ok(), "a consistent size must be storable");

        // Non-ASCII text is measured in bytes, so byte accounting matches the domain.
        let multibyte_id = "0198f000-0000-7000-8000-000000000009";
        let multibyte = execute_sql(
            &database,
            insert,
            &[
                multibyte_id,
                SESSION,
                "2",
                "héllo",
                &rust_bytes.to_string(),
                AT,
            ],
        )
        .await;
        assert!(
            multibyte.is_ok(),
            "the size must be measured in UTF-8 bytes, matching the domain layer"
        );

        // And the character count must be rejected, which proves the cast is what makes
        // the constraint byte-accurate rather than character-accurate.
        let character_count_id = "0198f000-0000-7000-8000-00000000000a";
        let character_count = execute_sql(
            &database,
            insert,
            &[character_count_id, SESSION, "3", "héllo", "5", AT],
        )
        .await;
        assert!(
            character_count.is_err(),
            "a character count must not satisfy a byte-length constraint"
        );
    }

    #[tokio::test]
    async fn a_message_referencing_a_missing_run_is_rejected() {
        let (_directory, database) = seeded_database().await;
        // `run_id` is nullable, so a foreign key here is only exercised when it is
        // set. This proves the reference is actually enforced rather than silently
        // skipped because the parent table was declared later.
        let outcome = execute_sql(
            &database,
            "INSERT INTO messages \
             (id, session_id, sequence, author_kind, role, content, content_bytes, \
              sensitivity, source, run_id, created_at) \
             VALUES (?1, ?2, 0, 'assistant', 'assistant', 'hi', 2, 'internal', 'model', ?3, ?4)",
            &[MESSAGE, SESSION, RUN, AT],
        )
        .await;
        assert!(
            outcome.is_err(),
            "a message must not reference a run that does not exist"
        );
    }

    #[tokio::test]
    async fn deleting_a_run_detaches_its_messages_without_deleting_them() {
        let (_directory, database) = seeded_database().await;
        must(
            execute_sql(
                &database,
                "INSERT INTO agent_runs \
             (id, session_id, workspace_id, user_id, objective, state, started_at, \
              updated_at, correlation_id, version) \
             VALUES (?1, ?2, ?3, ?4, 'do a thing', 'received', ?5, ?5, ?6, 1)",
                &[RUN, SESSION, WORKSPACE, USER, AT, CORRELATION],
            )
            .await,
        );
        must(
            execute_sql(
                &database,
                "INSERT INTO messages \
             (id, session_id, sequence, author_kind, role, content, content_bytes, \
              sensitivity, source, run_id, created_at) \
             VALUES (?1, ?2, 0, 'assistant', 'assistant', 'hi', 2, 'internal', 'model', ?3, ?4)",
                &[MESSAGE, SESSION, RUN, AT],
            )
            .await,
        );

        must(execute_sql(&database, "DELETE FROM agent_runs WHERE id = ?1", &[RUN]).await);

        // `SET NULL`, not `CASCADE`: the message belongs to its session and must
        // outlive the run that produced it.
        let remaining = must(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM messages WHERE id = ?1 AND run_id IS NULL",
            )
            .bind(MESSAGE)
            .fetch_one(&database.pool)
            .await,
        );
        assert_eq!(
            remaining, 1,
            "deleting a run must detach its messages, not delete them"
        );
    }

    async fn create_unmanaged_fixture(path: &Path) {
        let mut connection = must(connection_options(path, true).connect().await);
        execute(
            &mut connection,
            "CREATE TABLE someone_elses_data (value TEXT NOT NULL) STRICT",
        )
        .await;
        must(connection.close().await);
    }

    async fn create_invalid_foreign_key_fixture(path: &Path) {
        let options = connection_options(path, true).foreign_keys(false);
        let mut connection = must(options.connect().await);
        execute(&mut connection, "PRAGMA application_id = 1245794902").await;
        execute(
            &mut connection,
            "CREATE TABLE parents (id INTEGER PRIMARY KEY) STRICT",
        )
        .await;
        execute(
            &mut connection,
            "CREATE TABLE children (parent_id INTEGER NOT NULL REFERENCES parents(id)) STRICT",
        )
        .await;
        execute(
            &mut connection,
            "INSERT INTO children (parent_id) VALUES (42)",
        )
        .await;
        execute(&mut connection, "PRAGMA user_version = 0").await;
        must(connection.close().await);
    }

    async fn read_only_connection(path: &Path) -> SqliteConnection {
        must(
            connection_options(path, false)
                .read_only(true)
                .connect()
                .await,
        )
    }

    #[tokio::test]
    async fn fresh_database_enforces_the_complete_connection_contract() {
        let directory = TestDirectory::new();
        let path = directory.path().join(DEFAULT_DATABASE_FILENAME);
        let database = must(SqliteDatabase::open(&path).await);

        assert_eq!(database.path(), path);
        assert!(database.pre_migration_backup().is_none());
        assert!(
            parse_sqlite_version(database.sqlite_version())
                >= parse_sqlite_version(MINIMUM_SQLITE_VERSION)
        );

        let application_id = must(
            sqlx::query_scalar::<_, i64>("PRAGMA application_id")
                .fetch_one(&database.pool)
                .await,
        );
        let user_version = must(
            sqlx::query_scalar::<_, i64>("PRAGMA user_version")
                .fetch_one(&database.pool)
                .await,
        );
        let metadata_version = must(
            sqlx::query_scalar::<_, i64>(
                "SELECT schema_version FROM jarvis_storage_metadata WHERE singleton = 1",
            )
            .fetch_one(&database.pool)
            .await,
        );
        let migration_count = must(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM _sqlx_migrations WHERE success = TRUE",
            )
            .fetch_one(&database.pool)
            .await,
        );
        assert_eq!(application_id, JARVIS_APPLICATION_ID);
        assert_eq!(user_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(metadata_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(migration_count, CURRENT_SCHEMA_VERSION);

        let mut connections = Vec::new();
        for _ in 0..MAX_CONNECTIONS {
            connections.push(must(database.pool.acquire().await));
        }
        for connection in &mut connections {
            let foreign_keys = must(
                sqlx::query_scalar::<_, i64>("PRAGMA foreign_keys")
                    .fetch_one(&mut **connection)
                    .await,
            );
            let trusted_schema = must(
                sqlx::query_scalar::<_, i64>("PRAGMA trusted_schema")
                    .fetch_one(&mut **connection)
                    .await,
            );
            let recursive_triggers = must(
                sqlx::query_scalar::<_, i64>("PRAGMA recursive_triggers")
                    .fetch_one(&mut **connection)
                    .await,
            );
            let synchronous = must(
                sqlx::query_scalar::<_, i64>("PRAGMA synchronous")
                    .fetch_one(&mut **connection)
                    .await,
            );
            let journal_mode = must(
                sqlx::query_scalar::<_, String>("PRAGMA journal_mode")
                    .fetch_one(&mut **connection)
                    .await,
            );
            assert_eq!(foreign_keys, 1);
            assert_eq!(trusted_schema, 0);
            assert_eq!(recursive_triggers, 1);
            assert_eq!(synchronous, 2);
            assert_eq!(journal_mode, "wal");
        }
        drop(connections);
        database.close().await;
        drop(database);

        let reopened = must(SqliteDatabase::open(&path).await);
        assert!(reopened.pre_migration_backup().is_none());
        reopened.close().await;
    }

    #[tokio::test]
    async fn upgrade_creates_a_verified_restorable_backup() {
        let directory = TestDirectory::new();
        let path = directory.path().join(DEFAULT_DATABASE_FILENAME);
        create_version_zero_fixture(&path).await;

        let database = must(SqliteDatabase::open(&path).await);
        let backup = match database.pre_migration_backup() {
            Some(path) => path.to_path_buf(),
            None => panic!("expected a pre-migration backup"),
        };
        assert!(backup.is_file());
        let sentinel = must(
            sqlx::query_scalar::<_, String>("SELECT value FROM legacy_sentinel")
                .fetch_one(&database.pool)
                .await,
        );
        assert_eq!(sentinel, "before-migration");
        database.close().await;
        drop(database);

        let restored_path = directory.path().join("restored.sqlite3");
        must(fs::copy(&backup, &restored_path));
        let mut restored = read_only_connection(&restored_path).await;
        let restored_sentinel = must(
            sqlx::query_scalar::<_, String>("SELECT value FROM legacy_sentinel")
                .fetch_one(&mut restored)
                .await,
        );
        let restored_version = must(
            sqlx::query_scalar::<_, i64>("PRAGMA user_version")
                .fetch_one(&mut restored)
                .await,
        );
        let metadata_exists = must(
            sqlx::query_scalar::<_, i64>(
                "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name = 'jarvis_storage_metadata')",
            )
            .fetch_one(&mut restored)
            .await,
        );
        assert_eq!(restored_sentinel, "before-migration");
        assert_eq!(restored_version, 0);
        assert_eq!(metadata_exists, 0);
        must(restored.close().await);
    }

    #[tokio::test]
    async fn future_schema_is_rejected_without_changing_the_database() {
        let directory = TestDirectory::new();
        let path = directory.path().join(DEFAULT_DATABASE_FILENAME);
        create_future_fixture(&path).await;
        let before = must(fs::read(&path));

        let error = match SqliteDatabase::open(&path).await {
            Ok(database) => {
                database.close().await;
                panic!("expected a future schema error")
            }
            Err(error) => error,
        };
        assert!(matches!(
            error,
            DatabaseError::FutureSchema {
                found,
                supported: CURRENT_SCHEMA_VERSION
            } if found == CURRENT_SCHEMA_VERSION + 1
        ));
        assert_eq!(must(fs::read(&path)), before);
    }

    #[tokio::test]
    async fn non_empty_unmanaged_database_is_rejected() {
        let directory = TestDirectory::new();
        let path = directory.path().join(DEFAULT_DATABASE_FILENAME);
        create_unmanaged_fixture(&path).await;

        let error = match SqliteDatabase::open(&path).await {
            Ok(database) => {
                database.close().await;
                panic!("expected an unmanaged database error")
            }
            Err(error) => error,
        };
        assert!(matches!(error, DatabaseError::UnmanagedDatabase));
    }

    #[tokio::test]
    async fn invalid_backup_blocks_migration() {
        let directory = TestDirectory::new();
        let path = directory.path().join(DEFAULT_DATABASE_FILENAME);
        create_invalid_foreign_key_fixture(&path).await;

        let error = match SqliteDatabase::open(&path).await {
            Ok(database) => {
                database.close().await;
                panic!("expected backup verification to fail")
            }
            Err(error) => error,
        };
        assert!(matches!(
            error,
            DatabaseError::InvalidBackup {
                check: "foreign-key validation"
            }
        ));

        let mut original = read_only_connection(&path).await;
        let user_version = must(
            sqlx::query_scalar::<_, i64>("PRAGMA user_version")
                .fetch_one(&mut original)
                .await,
        );
        let metadata_exists = must(
            sqlx::query_scalar::<_, i64>(
                "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name = 'jarvis_storage_metadata')",
            )
            .fetch_one(&mut original)
            .await,
        );
        assert_eq!(user_version, 0);
        assert_eq!(metadata_exists, 0);
        must(original.close().await);
    }

    #[tokio::test]
    async fn daemon_lifecycle_transitions_are_ordered_and_durable() {
        let directory = TestDirectory::new();
        let path = directory.path().join(DEFAULT_DATABASE_FILENAME);
        let database = must(SqliteDatabase::open(&path).await);
        let id = DaemonRunId::new();
        let started_at = must(UtcTimestamp::from_unix_nanos(1_700_000_000_000_000_000));
        let ready_at = must(UtcTimestamp::from_unix_nanos(1_700_000_001_000_000_000));
        let stopped_at = must(UtcTimestamp::from_unix_nanos(1_700_000_002_000_000_000));
        let instance = must(DaemonInstanceStart::new(
            id, 42, "0.1.0", "windows", "x86_64", started_at,
        ));

        must(database.record_daemon_start(&instance).await);
        must(database.mark_daemon_ready(id, ready_at).await);
        let duplicate_ready = database.mark_daemon_ready(id, ready_at).await;
        assert!(matches!(
            duplicate_ready,
            Err(DatabaseError::DaemonLifecycleConflict {
                transition: "ready"
            })
        ));
        must(
            database
                .mark_daemon_stopped(id, stopped_at, DaemonStopReason::Signal)
                .await,
        );
        let duplicate_stop = database
            .mark_daemon_stopped(id, stopped_at, DaemonStopReason::Signal)
            .await;
        assert!(matches!(
            duplicate_stop,
            Err(DatabaseError::DaemonLifecycleConflict {
                transition: "stopped"
            })
        ));

        let row = must(
            sqlx::query_as::<_, (String, String, String, String)>(
                "SELECT state, started_at, ready_at, stopped_at \
                 FROM daemon_instances WHERE id = ?1",
            )
            .bind(id.to_string())
            .fetch_one(&database.pool)
            .await,
        );
        let reason = must(
            sqlx::query_scalar::<_, String>(
                "SELECT stop_reason FROM daemon_instances WHERE id = ?1",
            )
            .bind(id.to_string())
            .fetch_one(&database.pool)
            .await,
        );
        assert_eq!(row.0, "stopped");
        assert_eq!(row.1, started_at.to_string());
        assert_eq!(row.2, ready_at.to_string());
        assert_eq!(row.3, stopped_at.to_string());
        assert_eq!(reason, "signal");
        database.close().await;
    }

    #[test]
    fn daemon_metadata_rejects_unsafe_labels_without_echoing_them() {
        let timestamp = must(UtcTimestamp::from_unix_nanos(1_700_000_000_000_000_000));
        let error = DaemonInstanceStart::new(
            DaemonRunId::new(),
            42,
            "0.1.0\nforged",
            "windows",
            "x86_64",
            timestamp,
        );
        assert!(matches!(
            error,
            Err(DatabaseError::InvalidDaemonMetadata { field: "version" })
        ));
        let display = match error {
            Ok(_) => panic!("expected invalid daemon metadata"),
            Err(error) => error.to_string(),
        };
        assert!(!display.contains("forged"));
    }

    #[test]
    fn sqlite_version_parser_rejects_ambiguous_values() {
        assert_eq!(parse_sqlite_version("3.51.3"), Some((3, 51, 3)));
        assert_eq!(parse_sqlite_version("3.51"), None);
        assert_eq!(parse_sqlite_version("3.51.3.1"), None);
        assert_eq!(parse_sqlite_version("3.51.x"), None);
    }
}
