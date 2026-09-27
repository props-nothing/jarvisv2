//! Storage adapters and migration infrastructure for JARVIS.

mod config;
mod credential;
mod database;
mod identity_repository;
mod inspect;
mod memory_repository;
mod message_repository;
mod paths;
mod pgvector;
mod run_event_repository;
mod run_repository;
mod session_repository;

mod approval_repository;
mod secret_store;
mod tool_call_repository;
mod workspace_repository;

pub use approval_repository::{
    create_approval, find_approval, read_pending_approvals, read_run_approvals, record_decision,
};
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
pub use memory_repository::{
    EntityKind, EntityStatus, MemoryTransition, NewEntity, StoredAlias, StoredEntity, StoredMemory,
    StoredRelation, TASK_LIKE_PREDICATES, apply_memory_transition, count_memory_entity_links,
    find_entity, find_memory, find_memory_including_deleted, is_tombstoned, link_memory_entity,
    merge_entities, purge_memory, read_alias_candidates, read_all_memories, read_entity_memories,
    read_retrievable_memories, read_subject_relations, read_workspace_memories, record_alias,
    record_entity, record_memory, record_relation, record_tombstone, reinforce_memory,
    resolve_alias,
};
pub use message_repository::{
    MAX_MESSAGE_PAGE, StoredMessage, append_message, count_messages, find_message, read_messages,
    read_recent_messages,
};
pub use paths::{
    AppPaths, PathError, PathKind, PathMode, RuntimePathSource, portable_layout,
    secure_private_file,
};
pub use pgvector::{
    DistanceMetric, MAX_INDEXED_DIMENSIONS, PgVectorError, decode as decode_embedding,
    encode as encode_embedding, is_indexable,
};
pub use run_event_repository::{
    NewRunEvent, StoredRunEvent, append_run_event, find_run_event, highest_run_event_sequence,
    read_run_events,
};
pub use run_repository::{
    INTERRUPTED_ERROR_CODE, MAX_OBJECTIVE_CHARS, NewRun, StoredRun, TerminalTransition, create_run,
    find_run, recover_interrupted_runs, request_run_cancellation, settle_run, transition_run,
};
pub use secret_store::{APPROVAL_NONCE_DIRECTORY, SecretStore, SecretStoreError};
pub use session_repository::{
    API_SESSION_CHANNEL, NewSession, SessionTarget, StartRunInput, StartedRun, StoredSession,
    find_session, start_run,
};
pub use tool_call_repository::{
    CallBinding, CallOrigin, CallTarget, NewToolCall, StoredToolCall, admit_tool_call,
    advance_tool_call, find_tool_call, link_tool_call_approval, read_run_tool_calls,
    read_unrepeatable_calls, record_tool_outcome,
};
pub use workspace_repository::{DataPolicy, NewWorkspace, WorkspaceMode, record_workspace};
