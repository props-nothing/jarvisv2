//! Storage adapters and migration infrastructure for JARVIS.

mod config;
mod credential;
mod database;
mod inspect;
mod paths;
mod run_event_repository;
mod run_repository;

pub use config::{
    CURRENT_CONFIG_VERSION, Config, ConfigError, ConfigMigration, ConfigStore, DaemonConfig,
    LoadedConfig, LoggingConfig, ProfileConfig,
};
pub use credential::{CREDENTIAL_FILE_NAME, CredentialStore, CredentialStoreError};
pub use database::{
    CURRENT_SCHEMA_VERSION, DEFAULT_DATABASE_FILENAME, DaemonInstanceStart, DaemonStopReason,
    DatabaseError, SqliteDatabase,
};
pub use inspect::{DaemonInstanceRow, DatabaseInspection, DatabaseState, inspect_database};
pub use paths::{
    AppPaths, PathError, PathKind, PathMode, RuntimePathSource, portable_layout,
    secure_private_file,
};
pub use run_event_repository::{
    NewRunEvent, StoredRunEvent, append_run_event, find_run_event, highest_run_event_sequence,
    read_run_events,
};
pub use run_repository::{
    MAX_OBJECTIVE_CHARS, NewRun, StoredRun, create_run, find_run, request_run_cancellation,
    transition_run,
};
