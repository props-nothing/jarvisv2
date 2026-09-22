//! Storage adapters and migration infrastructure for JARVIS.

mod config;
mod credential;
mod database;
mod identity_repository;
mod inspect;
mod message_repository;
mod paths;
mod run_event_repository;
mod run_repository;
mod session_repository;

pub use config::{
    CURRENT_CONFIG_VERSION, Config, ConfigError, ConfigMigration, ConfigStore, DaemonConfig,
    LoadedConfig, LoggingConfig, ProfileConfig,
};
pub use credential::{CREDENTIAL_FILE_NAME, CredentialStore, CredentialStoreError};
pub use database::{
    CURRENT_SCHEMA_VERSION, DEFAULT_DATABASE_FILENAME, DaemonInstanceStart, DaemonStopReason,
    DatabaseError, SqliteDatabase,
};
pub use identity_repository::{
    LOCAL_USER_ID, LOCAL_WORKSPACE_ID, LocalIdentity, load_local_identity,
};
pub use inspect::{DaemonInstanceRow, DatabaseInspection, DatabaseState, inspect_database};
pub use message_repository::{
    MAX_MESSAGE_PAGE, StoredMessage, append_message, find_message, read_messages,
};
pub use paths::{
    AppPaths, PathError, PathKind, PathMode, RuntimePathSource, portable_layout,
    secure_private_file,
};
pub use run_event_repository::{
    NewRunEvent, StoredRunEvent, append_run_event, find_run_event, highest_run_event_sequence,
    read_run_events,
};
pub use run_repository::{
    MAX_OBJECTIVE_CHARS, NewRun, StoredRun, TerminalTransition, create_run, find_run,
    request_run_cancellation, settle_run, transition_run,
};
pub use session_repository::{
    API_SESSION_CHANNEL, NewSession, StartRunInput, StartedRun, StoredSession, find_session,
    start_run,
};
