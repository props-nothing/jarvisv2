//! Durable storage for canonical memory: records, entities, aliases, relations, and tombstones.
//!
//! `docs/architecture/memory-and-context.md` is the contract, `crates/jarvis-core/src/memory.rs` is the typed
//! form, and this module is the durable one. The schema's own notes (`0009_memories.sql`) argue why each
//! column is shaped as it is; what follows is why the *operations* are shaped as they are.
//!
//! # The write path enforces the admission lifecycle rather than trusting a caller
//!
//! The architecture's lifecycle has a deduplication step and a rejection step, and both are enforced here
//! rather than left to whoever calls:
//!
//! 1. **A tombstoned claim is refused.** The search key is hashed and looked up in `memory_tombstones`
//!    *before* the insert. This is what makes "forget" durable against the same source being read again â€”
//!    without it, deleting a memory would only last until the next sync.
//! 2. **A duplicate claim is refused with the existing identifier.** The unique index decides, so a
//!    read-then-write race cannot produce two rows; the caller's response is to *reinforce* the existing
//!    memory, which is why [`reinforce_memory`] exists.
//!
//! # The order of the two checks is load-bearing
//!
//! The tombstone check runs **before** the duplicate check, and the order matters in one direction: a
//! deleted memory's row still exists (with its `search_key` cleared), so it cannot collide on the unique
//! index â€” which means a re-ingest would otherwise *insert* rather than report a duplicate. Checking the
//! tombstone first makes the answer "this was deleted" rather than "here is a fresh memory", and the two are
//! very different answers to a user who asked for it to be forgotten.
//!
//! # Why the unique index is the deduplication mechanism
//!
//! A `SELECT` to check for an existing row followed by an `INSERT` is a race: two concurrent ingests both
//! read nothing and both insert. The `UNIQUE` index cannot race, so the insert is attempted and the
//! constraint violation is reported with the existing identifier â€” the same shape the tool-call ledger uses.
//!
//! # Why timestamps are never compared in SQL
//!
//! `jarvis_core::UtcTimestamp`'s text form is not lexicographically sortable (it omits the fraction when it
//! is zero, so `"...00Z"` sorts after `"...00.5Z"`). No predicate here compares a timestamp string. Ordering
//! is by `created_at` with the identifier as a tie-break, and every validity question is answered in Rust.

use jarvis_core::{
    CorrelationId, EntityId, EntityMatch, EntityRef, MemoryConfidence, MemoryId, MemoryRecord,
    MemoryRecordParts, MemorySearchKey, MemorySource, MemorySourceKind, MemoryStatus, MemoryTrust,
    MemoryType, Sensitivity, StoredMemoryState, StructuredClaim, UtcTimestamp, WorkspaceId,
};
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

use crate::database::{DatabaseError, SqliteDatabase};

/// The kinds of thing an entity can be.
///
/// Duplicated from the schema's `CHECK` as a Rust enum because a decode has to name a value it read, and
/// `String` everywhere would make "which kind is this" a string comparison at every call site.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EntityKind {
    /// A person.
    Person,
    /// An organization.
    Organization,
    /// A project.
    Project,
    /// A document.
    Document,
    /// An account: an email account, a service account.
    Account,
    /// A device.
    Device,
    /// A location.
    Location,
    /// An event.
    Event,
    /// A task.
    Task,
    /// A conversation.
    Conversation,
}

impl EntityKind {
    /// Returns the stable storage name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Person => "person",
            Self::Organization => "organization",
            Self::Project => "project",
            Self::Document => "document",
            Self::Account => "account",
            Self::Device => "device",
            Self::Location => "location",
            Self::Event => "event",
            Self::Task => "task",
            Self::Conversation => "conversation",
        }
    }

    /// Returns every kind, so a round-trip test can enumerate them.
    #[must_use]
    pub const fn all() -> [Self; 10] {
        [
            Self::Person,
            Self::Organization,
            Self::Project,
            Self::Document,
            Self::Account,
            Self::Device,
            Self::Location,
            Self::Event,
            Self::Task,
            Self::Conversation,
        ]
    }
}

impl std::str::FromStr for EntityKind {
    type Err = DatabaseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "person" => Ok(Self::Person),
            "organization" => Ok(Self::Organization),
            "project" => Ok(Self::Project),
            "document" => Ok(Self::Document),
            "account" => Ok(Self::Account),
            "device" => Ok(Self::Device),
            "location" => Ok(Self::Location),
            "event" => Ok(Self::Event),
            "task" => Ok(Self::Task),
            "conversation" => Ok(Self::Conversation),
            _ => Err(DatabaseError::StoredMemoryInvalid { field: "kind" }),
        }
    }
}

impl std::fmt::Display for EntityKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Where an entity stands.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EntityStatus {
    /// Current and usable.
    Active,
    /// Merged into another entity, which `merged_into` names.
    Merged,
    /// Set aside without being merged.
    Archived,
    /// Removed.
    Deleted,
}

impl EntityStatus {
    /// Returns the stable storage name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Merged => "merged",
            Self::Archived => "archived",
            Self::Deleted => "deleted",
        }
    }
}

impl std::str::FromStr for EntityStatus {
    type Err = DatabaseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "active" => Ok(Self::Active),
            "merged" => Ok(Self::Merged),
            "archived" => Ok(Self::Archived),
            "deleted" => Ok(Self::Deleted),
            _ => Err(DatabaseError::StoredMemoryInvalid { field: "status" }),
        }
    }
}

/// One stored entity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredEntity {
    id: EntityId,
    workspace_id: WorkspaceId,
    kind: EntityKind,
    label: String,
    attributes: Option<String>,
    confidence: MemoryConfidence,
    status: EntityStatus,
    merged_into: Option<EntityId>,
    created_at: UtcTimestamp,
    updated_at: UtcTimestamp,
    version: i64,
}

impl StoredEntity {
    /// Returns the entity identifier.
    #[must_use]
    pub const fn id(&self) -> EntityId {
        self.id
    }

    /// Returns the owning workspace.
    #[must_use]
    pub const fn workspace_id(&self) -> WorkspaceId {
        self.workspace_id
    }

    /// Returns what kind of thing the entity is.
    #[must_use]
    pub const fn kind(&self) -> EntityKind {
        self.kind
    }

    /// Returns the canonical label.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Returns the normalized attributes document, when one was recorded.
    #[must_use]
    pub fn attributes(&self) -> Option<&str> {
        self.attributes.as_deref()
    }

    /// Returns how confidently the entity was established.
    #[must_use]
    pub const fn confidence(&self) -> MemoryConfidence {
        self.confidence
    }

    /// Returns the stored status.
    #[must_use]
    pub const fn status(&self) -> EntityStatus {
        self.status
    }

    /// Returns the entity this one was merged into, when it was.
    #[must_use]
    pub const fn merged_into(&self) -> Option<EntityId> {
        self.merged_into
    }

    /// Returns when the entity was created.
    #[must_use]
    pub const fn created_at(&self) -> UtcTimestamp {
        self.created_at
    }

    /// Returns the optimistic-concurrency version.
    #[must_use]
    pub const fn version(&self) -> i64 {
        self.version
    }

    /// Returns whether the entity may be used as a subject or object in a new relation.
    ///
    /// Only an `Active` entity. A merged entity's claims belong to the winner, and a deleted one's to nobody,
    /// so a new relation against either would attach a claim to a name that no longer denotes anything.
    #[must_use]
    pub const fn is_usable(&self) -> bool {
        matches!(self.status, EntityStatus::Active)
    }
}

/// The fields needed to record an entity.
#[derive(Clone, Debug)]
pub struct NewEntity {
    /// The entity identifier.
    pub id: EntityId,
    /// The owning workspace.
    pub workspace_id: WorkspaceId,
    /// What kind of thing it is.
    pub kind: EntityKind,
    /// The canonical label.
    pub label: String,
    /// Normalized attributes, when available.
    pub attributes: Option<String>,
    /// How confidently it was established.
    pub confidence: MemoryConfidence,
    /// When it was created.
    pub created_at: UtcTimestamp,
}

/// One stored alias.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredAlias {
    id: String,
    entity_id: EntityId,
    alias_kind: String,
    alias_value: String,
    normalized: String,
    verification: EntityMatch,
    confidence: MemoryConfidence,
}

impl StoredAlias {
    /// Returns the alias identifier.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the entity the alias resolves to.
    #[must_use]
    pub const fn entity_id(&self) -> EntityId {
        self.entity_id
    }

    /// Returns the alias kind, such as `email` or `handle`.
    #[must_use]
    pub fn alias_kind(&self) -> &str {
        &self.alias_kind
    }

    /// Returns the alias as written.
    #[must_use]
    pub fn alias_value(&self) -> &str {
        &self.alias_value
    }

    /// Returns how the alias was established.
    #[must_use]
    pub const fn verification(&self) -> EntityMatch {
        self.verification
    }

    /// Returns how confidently the alias was established.
    #[must_use]
    pub const fn confidence(&self) -> MemoryConfidence {
        self.confidence
    }
}

/// A stored memory, decoded and re-validated.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredMemory {
    record: MemoryRecord,
    search_key: Option<String>,
    version: i64,
}

impl StoredMemory {
    /// Returns the decoded memory.
    #[must_use]
    pub const fn record(&self) -> &MemoryRecord {
        &self.record
    }

    /// Returns the deduplication key, which is `None` for a deleted memory.
    ///
    /// `None` rather than an empty string, because the key was **removed** with the content rather than
    /// becoming empty â€” and a caller seeing `None` learns that the row is a tombstone-shaped remainder
    /// rather than a memory with a degenerate key.
    #[must_use]
    pub fn search_key(&self) -> Option<&str> {
        self.search_key.as_deref()
    }

    /// Returns the optimistic-concurrency version.
    #[must_use]
    pub const fn version(&self) -> i64 {
        self.version
    }

    /// Consumes the wrapper, returning the domain value.
    #[must_use]
    pub fn into_record(self) -> MemoryRecord {
        self.record
    }
}

/// Records a memory, refusing a duplicate claim and a re-ingest of a deleted one.
///
/// # Errors
///
/// - [`DatabaseError::InvalidMemoryRequest`] when a field fails the domain's own validation.
/// - [`DatabaseError::MemoryTombstoned`] when the claim was deleted and must not return. Checked **before**
///   the insert, and before the duplicate check, because a deleted memory's row has no search key and so
///   cannot collide â€” see the module doc on why the order matters.
/// - [`DatabaseError::MemoryDuplicate`] carrying the existing identifier, so the caller reinforces rather
///   than retries.
/// - [`DatabaseError::Sqlite`] when a referenced workspace, run, or superseded memory is missing.
pub async fn record_memory(
    database: &SqliteDatabase,
    record: &MemoryRecord,
    search_key: &MemorySearchKey,
) -> Result<String, DatabaseError> {
    // 1. The resurrection check, first. See the module doc: a deleted row has no search key, so it cannot be
    //    caught by the unique index below, and without this check a re-ingest would insert a fresh row for a
    //    claim the user removed.
    if is_tombstoned(database, record.workspace_id(), search_key).await? {
        return Err(DatabaseError::MemoryTombstoned);
    }

    let affected = insert_memory_on(database.pool(), record, search_key).await?;

    if affected == 0 {
        // The conflict clause suppressed the insert, so a memory already exists for this key. Its identifier
        // is returned so the caller reinforces rather than guesses.
        let existing =
            find_memory_id_by_key(database, record.workspace_id(), search_key.as_str()).await?;
        return Err(DatabaseError::MemoryDuplicate {
            existing_memory_id: existing.ok_or(DatabaseError::Sqlite {
                operation: "resolve a memory duplicate",
                source: sqlx::Error::RowNotFound,
            })?,
        });
    }

    // The links are written after the row, in one transaction with it in principle; here they follow, and a
    // failure leaves a memory with fewer entity links rather than a link to a memory that does not exist.
    for entity in record.entities() {
        link_memory_entity(database, record.id(), entity.clone()).await?;
    }

    Ok(record.id().to_string())
}

/// Inserts one memory row on `executor`, reporting whether it was written.
///
/// # Why this is a helper rather than a statement inside [`record_memory`]
///
/// The summary writer records memories too, and it must do so **inside a transaction** with its own row. Two
/// statements listing the same twenty-six columns in the same order are two chances to omit one, and an
/// omitted column is not a compile error — it is a memory stored with that field defaulted, which for
/// `source_trust` would silently change whether its content may instruct. Extracting the statement makes the
/// two writers name the columns once.
///
/// It deliberately does **not** do the tombstone check, resolve a duplicate, or write entity links: those need
/// the workspace and the pool, and a caller inside a transaction must do them on that same connection or fail
/// with `SQLITE_BUSY_SNAPSHOT` when it reads and then writes. Each caller sequences them for its own case.
///
/// Returns the number of rows affected — zero means the `ON CONFLICT` clause suppressed the insert.
async fn insert_memory_on<'c, E>(
    executor: E,
    record: &MemoryRecord,
    search_key: &MemorySearchKey,
) -> Result<u64, DatabaseError>
where
    E: sqlx::Executor<'c, Database = sqlx::Sqlite>,
{
    let structured = claim_columns(record.structured_claim());

    let result = sqlx::query(
        "INSERT INTO memories (\
            id, workspace_id, memory_type, content, \
            claim_subject, claim_predicate, claim_object, \
            source_kind, source_locator, source_trust, source_excerpt_hash, \
            confidence, importance, sensitivity, search_key, status, \
            valid_from, valid_until, supersedes_memory_id, superseded_by_memory_id, run_id, \
            created_by_actor_id, correlation_id, created_at, updated_at, last_accessed_at, \
            retrieval_count, version\
         ) VALUES (\
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, \
            ?21, ?22, ?23, ?24, ?24, ?25, ?26, 1\
         ) ON CONFLICT (workspace_id, search_key) DO NOTHING",
    )
    .bind(record.id().to_string())
    .bind(record.workspace_id().to_string())
    .bind(record.memory_type().as_str())
    .bind(record.content())
    .bind(structured.0)
    .bind(structured.1)
    .bind(structured.2)
    .bind(record.source().kind().as_str())
    .bind(record.source().locator())
    .bind(record.source().trust().as_str())
    .bind(record.source().excerpt_hash())
    .bind(record.confidence().as_str())
    .bind(i64::from(record.importance()))
    .bind(record.sensitivity().as_str())
    .bind(search_key.as_str())
    .bind(record.status().as_str())
    .bind(record.valid_from().to_string())
    .bind(record.valid_until().map(|until| until.to_string()))
    .bind(record.supersedes().map(|id| id.to_string()))
    .bind(record.superseded_by().map(|id| id.to_string()))
    .bind(record.run_id().map(|id| id.to_string()))
    .bind(record.created_by_actor_id())
    .bind(record.correlation_id().to_string())
    .bind(record.created_at().to_string())
    .bind(record.last_accessed_at().map(|at| at.to_string()))
    .bind(i64::from(record.retrieval_count()))
    .execute(executor)
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "record a memory",
        source,
    })?;

    Ok(result.rows_affected())
}

/// Links a memory to an entity, recording **how** the entity was matched.
///
/// # Errors
///
/// Returns [`DatabaseError::Sqlite`] when the memory or entity is missing, which the foreign keys refuse.
pub async fn link_memory_entity(
    database: &SqliteDatabase,
    memory_id: MemoryId,
    entity: EntityRef,
) -> Result<(), DatabaseError> {
    sqlx::query(
        "INSERT INTO memory_entities (memory_id, entity_id, matched_by) VALUES (?1, ?2, ?3) \
         ON CONFLICT (memory_id, entity_id) DO UPDATE SET matched_by = \
            CASE \
                WHEN excluded.matched_by = 'probabilistic' THEN 'probabilistic' \
                WHEN memory_entities.matched_by = 'probabilistic' THEN 'probabilistic' \
                WHEN excluded.matched_by = 'exact_identifier' THEN 'exact_identifier' \
                WHEN memory_entities.matched_by = 'exact_identifier' THEN 'exact_identifier' \
                WHEN excluded.matched_by = 'provider_id' THEN 'provider_id' \
                WHEN memory_entities.matched_by = 'provider_id' THEN 'provider_id' \
                ELSE memory_entities.matched_by \
            END",
    )
    .bind(memory_id.to_string())
    .bind(entity.entity_id().to_string())
    .bind(entity.matched_by().as_str())
    .execute(database.pool())
    .await
    .map(|_| ())
    .map_err(|source| DatabaseError::Sqlite {
        operation: "link a memory to an entity",
        source,
    })?;
    Ok(())
}

/// Reads one memory, refusing to decode a row it did not record.
///
/// # Errors
///
/// - [`DatabaseError::MemoryNotFound`] when no memory has that identifier.
/// - [`DatabaseError::StoredMemoryInvalid`] when a stored row contradicts the domain's closed sets or its own
///   invariants, which a row written by another build or restored from a backup can do.
pub async fn find_memory(
    database: &SqliteDatabase,
    id: &str,
) -> Result<StoredMemory, DatabaseError> {
    let row = sqlx::query(
        "SELECT id, workspace_id, memory_type, content, claim_subject, claim_predicate, claim_object, source_kind, source_locator, \
                source_trust, source_excerpt_hash, confidence, importance, sensitivity, search_key, \
                status, valid_from, valid_until, supersedes_memory_id, superseded_by_memory_id, run_id, \
                created_by_actor_id, correlation_id, created_at, updated_at, last_accessed_at, \
                retrieval_count, admitted_by_actor_id, admitted_at, version \
         FROM memories WHERE id = ?1",
    )
    .bind(id)
    .fetch_optional(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "read a memory",
        source,
    })?;
    let row = row.ok_or(DatabaseError::MemoryNotFound)?;

    let search_key = row
        .try_get::<Option<String>, _>("search_key")
        .map_err(|_| DatabaseError::StoredMemoryInvalid {
            field: "search_key",
        })?;
    let record = decode_memory(&row)?;

    // The links are read **after** the record so a decode failure does not leave a half-built value. The
    // order also means the entity set is the store's, not one a caller supplied.
    let mut record = record;
    load_linked_entities(database, &mut record).await?;

    let version = row
        .try_get::<i64, _>("version")
        .map_err(|_| DatabaseError::StoredMemoryInvalid { field: "version" })?;
    Ok(StoredMemory {
        record,
        search_key,
        version,
    })
}

/// Reads the memories of one workspace, newest first, bounded by `limit`.
///
/// # Why this takes an explicit limit
///
/// A retrieval that reads every memory and filters in Rust is a scan that grows with the user's history, and
/// the bound forces the caller to state what it needs. `P4-004` will pass a candidate window rather than
/// everything, and this signature makes that the only shape available.
///
/// # ⭐ Why the order is `unixepoch(created_at)` and not `created_at`
///
/// A retrieval read is **windowed**, and these timestamps are RFC 3339 text with the fraction **omitted when
/// it is zero**, so `'…:20Z'` sorts *after* `'…:20.5Z'` in byte order (`.` is 0x2E, `Z` is 0x5A). A windowed
/// `ORDER BY created_at DESC` therefore does not return the newest rows: the read that prompted this fix
/// returned the oldest and newest of three and **dropped the middle one**. This is the `ADR-0034` trap, and
/// this module already refuses `valid_until` comparisons in SQL for exactly this reason — the ordering needed
/// the same treatment. `unixepoch()` parses the string into seconds, and `id` breaks a tie so the order is
/// total. Sub-second ordering is not recovered (`unixepoch` resolves to seconds; `julianday` only to
/// milliseconds), which is a recorded limit owned by `ADR-0034` rather than something a read can fix.
///
/// # Errors
///
/// Returns [`DatabaseError::StoredMemoryInvalid`] when a stored row cannot be decoded, so one corrupt row is
/// reported rather than silently skipped — a skipped memory would look like a memory that does not exist.
pub async fn read_workspace_memories(
    database: &SqliteDatabase,
    workspace_id: WorkspaceId,
    limit: u32,
) -> Result<Vec<StoredMemory>, DatabaseError> {
    read_memories(
        database,
        "SELECT id, workspace_id, memory_type, content, claim_subject, claim_predicate, claim_object, source_kind, source_locator, \
                source_trust, source_excerpt_hash, confidence, importance, sensitivity, search_key, \
                status, valid_from, valid_until, supersedes_memory_id, superseded_by_memory_id, run_id, \
                created_by_actor_id, correlation_id, created_at, updated_at, last_accessed_at, \
                retrieval_count, admitted_by_actor_id, admitted_at, version \
         FROM memories WHERE workspace_id = ?1 AND status <> 'deleted' \
         ORDER BY unixepoch(created_at) DESC, id DESC LIMIT ?2",
        workspace_id,
        None,
        limit,
    )
    .await
}

/// The claim predicates that assert a **task** rather than a durable fact.
///
/// # Why a predicate list and not a memory type
///
/// The retrieval path for a model request must not offer the model its own unfinished work — an objective it
/// was previously given, a planned step, a pending call — because a plan replayed into a prompt reads as an
/// instruction to continue it. `P4-001` models that as [`MemoryType::Working`], which is the *retention*
/// question ("is this expected to survive the run that produced it"), and the two are not the same set: a
/// user statement about a task ("I am working on the tax return") is a durable fact that happens to mention
/// one.
///
/// So this is a list of predicates, not a type filter, and it is deliberately **narrow**. Each entry names a
/// claim whose object is the work itself. A predicate this list does not know is treated as a fact, which is
/// the permissive direction for *offering* and the safe one for the failure mode that matters here: offering
/// a fact the model did not need costs budget, while omitting a fact costs the answer.
///
/// The list lives in the storage half rather than in `jarvis-core` because it is a **query predicate** — it
/// is what the SQL below filters on — and `jarvis-core`'s retrieval module operates on records a caller
/// already holds.
pub const TASK_LIKE_PREDICATES: [&str; 6] = [
    "objective",
    "current_objective",
    "pending_call",
    "planned_step",
    "plan",
    "next_action",
];

/// Reads the memories eligible to be offered to a model for one request.
///
/// # Why this is not `read_workspace_memories` with a `WHERE` clause added by the caller
///
/// The eligibility rules here are not a convenience filter; each one closes a path that would otherwise put
/// content into a prompt that must not be there, and every one of them is **in SQL rather than in the
/// assembler** so a caller cannot forget it:
///
/// - **`status <> 'deleted'`** — a tombstone has no content, but the row exists and decoding one is only
///   refused by the domain. Excluding it here means the candidate set never contains a claim the user
///   removed.
/// - **`status = 'active'`** — a proposal is awaiting review, and `ADR-0045`'s whole point is that a
///   candidate is not a fact until it is admitted. An **archived** claim is either superseded by a
///   correction or set aside by the user, and `MemoryStatus::Archived`'s own doc says it is "retained for
///   audit, not retrieved as current truth". It is excluded here rather than left to the conversion's
///   currency check, because a row that can only ever be dropped should not consume the candidate window.
///   `P4-007`'s recorded claim that this read filtered superseded rows was **wrong** — it filtered
///   `deleted` and `proposed` and admitted `archived`, relying on the conversion to refuse it. The
///   behaviour was correct and the statement about it was not, which is the same class of defect as a test
///   whose name and fixture disagree.
/// - **A task-like predicate is excluded** — see [`TASK_LIKE_PREDICATES`].
/// - **A model inference is excluded** — the rule `P4-003` established, enforced at the read rather than
///   only at conversion. A model's own previous output re-entering a prompt as evidence is the self-feeding
///   loop the inference boundary exists to prevent, and `status <> 'proposed'` does not cover it: an
///   inference the user *confirmed* is active by status, and is still the model's claim.
///
/// `valid_until` is deliberately **not** filtered. Expiry is a read-side fact evaluated against the clock
/// the caller holds (`MemoryRecord::effective_status_at`), and comparing a stored RFC 3339 string against a
/// clock in SQL is the lexicographic-comparison trap `ADR-0034` recorded — the fraction is omitted when it
/// is zero, so an expired row can sort as unexpired. The domain check is the one that decides.
///
/// # Errors
///
/// Returns [`DatabaseError::StoredMemoryInvalid`] when a stored row cannot be decoded.
pub async fn read_retrievable_memories(
    database: &SqliteDatabase,
    workspace_id: WorkspaceId,
    limit: u32,
) -> Result<Vec<StoredMemory>, DatabaseError> {
    read_memories(
        database,
        "SELECT id, workspace_id, memory_type, content, claim_subject, claim_predicate, claim_object, source_kind, source_locator, \
                source_trust, source_excerpt_hash, confidence, importance, sensitivity, search_key, \
                status, valid_from, valid_until, supersedes_memory_id, superseded_by_memory_id, run_id, \
                created_by_actor_id, correlation_id, created_at, updated_at, last_accessed_at, \
                retrieval_count, admitted_by_actor_id, admitted_at, version \
         FROM memories \
         WHERE workspace_id = ?1 \
           AND status = 'active' \
           AND source_kind <> 'model_inference' \
           AND (claim_predicate IS NULL OR lower(claim_predicate) NOT IN ( \
                 'objective', 'current_objective', 'pending_call', 'planned_step', 'plan', 'next_action')) \
         ORDER BY unixepoch(created_at) DESC, id DESC LIMIT ?2",
        workspace_id,
        None,
        limit,
    )
    .await
}

/// Reads the memories linked to one entity, newest first.
///
/// The entity-scoped read `P4-004`'s ranking needs, and it goes through `memory_entities` rather than a
/// content match so "what do I know about this person" is an index seek rather than a text search.
///
/// # Errors
///
/// Returns [`DatabaseError::StoredMemoryInvalid`] when a stored row cannot be decoded.
pub async fn read_entity_memories(
    database: &SqliteDatabase,
    workspace_id: WorkspaceId,
    entity_id: EntityId,
    limit: u32,
) -> Result<Vec<StoredMemory>, DatabaseError> {
    read_memories(
        database,
        "SELECT m.id, m.workspace_id, m.memory_type, m.content, m.claim_subject, m.claim_predicate, m.claim_object, m.source_kind, \
                m.source_locator, m.source_trust, m.source_excerpt_hash, m.confidence, m.importance, \
                m.sensitivity, m.search_key, m.status, m.valid_from, m.valid_until, \
                m.supersedes_memory_id, m.superseded_by_memory_id, m.run_id, m.created_by_actor_id, \
                m.correlation_id, m.created_at, m.updated_at, m.last_accessed_at, m.retrieval_count, \
                m.admitted_by_actor_id, m.admitted_at, \
                m.version \
         FROM memories m JOIN memory_entities me ON me.memory_id = m.id \
         WHERE m.workspace_id = ?1 AND me.entity_id = ?2 AND m.status <> 'deleted' \
         ORDER BY unixepoch(m.created_at) DESC, m.id DESC LIMIT ?3",
        workspace_id,
        Some(entity_id),
        limit,
    )
    .await
}

/// Reads every memory in a workspace, including deleted ones, for an export.
///
/// # Why this is not `read_workspace_memories`
///
/// An export and a listing answer different questions, and the difference is what each excludes.
/// `read_workspace_memories` is a **retrieval** read: it omits deleted rows, because a claim the user removed
/// must not be offered. An export is a **portability** read: `docs/architecture/memory-and-context.md`
/// requires "each memory type has configurable retention and export behavior", and an export that silently
/// omitted archived and superseded claims would be an incomplete picture of what this platform holds, which
/// is exactly what a portability request exists to reveal.
///
/// Deleted rows are included as their **tombstone form**: the content is empty and the search key is gone, so
/// the export shows that a claim existed and was removed rather than either hiding it or resurrecting its
/// text. That is `ADR-0044`'s design read back out — a deleted row is a tombstone-shaped remainder, and an
/// export should say so.
///
/// # Why this has its own statement rather than joining `read_memories`
///
/// The shared runner binds a positional `limit` and an optional entity, and this read needs a second
/// positional parameter (`offset`) *after* the limit. Adding a fourth shape to a runner whose whole purpose
/// is to keep one statement's parameter positions in one place would put the divergence back where the
/// runner exists to remove it. The decoder below is shared, which is where the real duplication would be.
///
/// # Errors
///
/// Returns [`DatabaseError::StoredMemoryInvalid`] when a stored row cannot be decoded, so a corrupt row is
/// reported rather than silently absent from a user's own data.
pub async fn read_all_memories(
    database: &SqliteDatabase,
    workspace_id: WorkspaceId,
    limit: u32,
    offset: u32,
) -> Result<Vec<StoredMemory>, DatabaseError> {
    // **`unixepoch`, and it matters here as much as anywhere.** These timestamps are RFC 3339 text with the
    // fraction **omitted when it is zero**, so `'…:20Z'` sorts *after* `'…:20.5Z'` in byte order: an export
    // ordered by `created_at` lists a user's memories in a sequence that is not time order. `id ASC` makes the
    // text order total, so the pages still **tile** — this is not a row-skipping defect, and claiming one would
    // overstate it — but the sequence a portability read produces is wrong, which is the same `ADR-0034` trap
    // the other reads in this file were corrected for. Sub-second order is not recovered: `unixepoch` resolves
    // to seconds, so two memories written in one second order by `id`, which is arbitrary but stable.
    let rows = sqlx::query(
        "SELECT id, workspace_id, memory_type, content, claim_subject, claim_predicate, claim_object, source_kind, source_locator, \
                source_trust, source_excerpt_hash, confidence, importance, sensitivity, search_key, \
                status, valid_from, valid_until, supersedes_memory_id, superseded_by_memory_id, run_id, \
                created_by_actor_id, correlation_id, created_at, updated_at, last_accessed_at, \
                retrieval_count, admitted_by_actor_id, admitted_at, version \
         FROM memories WHERE workspace_id = ?1 \
         ORDER BY unixepoch(created_at) DESC, id DESC LIMIT ?2 OFFSET ?3",
    )
    .bind(workspace_id.to_string())
    .bind(i64::from(limit))
    .bind(i64::from(offset))
    .fetch_all(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "read all memories",
        source,
    })?;

    let mut memories = Vec::with_capacity(rows.len());
    for row in &rows {
        memories.push(decode_stored_memory(row)?);
    }
    attach_entity_links(database, &mut memories).await?;
    Ok(memories)
}

/// Reads one memory by identifier **without** the deleted status exclusion.
///
/// `find_memory` refuses a deleted row because a retrieval should not see one; an export, a receipt, and a
/// purge all need to. Separate rather than a flag on `find_memory`, because a boolean parameter at a call
/// site reads as `find_memory(db, id, true)` and the `true` says nothing about what it permits.
///
/// # Errors
///
/// Returns [`DatabaseError::MemoryNotFound`] when no memory has that identifier, and
/// [`DatabaseError::StoredMemoryInvalid`] when the row cannot be decoded.
pub async fn find_memory_including_deleted(
    database: &SqliteDatabase,
    id: &str,
) -> Result<StoredMemory, DatabaseError> {
    let row = sqlx::query(
        "SELECT id, workspace_id, memory_type, content, claim_subject, claim_predicate, claim_object, source_kind, source_locator, \
                source_trust, source_excerpt_hash, confidence, importance, sensitivity, search_key, \
                status, valid_from, valid_until, supersedes_memory_id, superseded_by_memory_id, run_id, \
                created_by_actor_id, correlation_id, created_at, updated_at, last_accessed_at, \
                retrieval_count, admitted_by_actor_id, admitted_at, version \
         FROM memories WHERE id = ?1",
    )
    .bind(id)
    .fetch_optional(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "find a memory including deleted",
        source,
    })?;

    let Some(row) = row else {
        return Err(DatabaseError::MemoryNotFound);
    };
    let mut stored = decode_stored_memory(&row)?;
    // The links are loaded here too, and that is not cosmetic: this is the read `load_scoped` uses, so a
    // correction that inherited its entities from here would otherwise inherit an **empty** set and be
    // refused for naming no entity. The failure would be a correct operation rejected.
    load_linked_entities(database, &mut stored.record).await?;
    Ok(stored)
}

/// Counts the entity links attached to one memory, before a purge removes them.
///
/// A count read **before** the delete, because the cascade removes the rows and an `AFTER` count would always
/// be zero — a receipt field that can only ever report one value is a field that reports nothing.
///
/// # Errors
///
/// Returns [`DatabaseError::Sqlite`] when the read fails.
pub async fn count_memory_entity_links(
    database: &SqliteDatabase,
    memory_id: &str,
) -> Result<u32, DatabaseError> {
    let row = sqlx::query("SELECT COUNT(*) AS total FROM memory_entities WHERE memory_id = ?1")
        .bind(memory_id)
        .fetch_one(database.pool())
        .await
        .map_err(|source| DatabaseError::Sqlite {
            operation: "count memory entity links",
            source,
        })?;
    let total = row
        .try_get::<i64, _>("total")
        .map_err(|_| DatabaseError::StoredMemoryInvalid {
            field: "memory_entities_count",
        })?;
    u32::try_from(total).map_err(|_| DatabaseError::StoredMemoryInvalid {
        field: "memory_entities_count",
    })
}

/// Writes the tombstone that blocks a purged claim from returning, **on the purge's own transaction**.
///
/// # Why this takes a transaction rather than the database
///
/// The tombstone must be written on the same connection as the delete, and the first version of
/// [`purge_memory`] did not do that: it called the pool-level [`record_tombstone`], which takes a second
/// connection, while the purge's transaction held a read. SQLite in WAL mode then fails the transaction's
/// own write with `SQLITE_BUSY_SNAPSHOT` — the snapshot it read from is no longer current — and the caller
/// sees a bare "failed to purge a memory". Accepting the transaction makes writing through the pool a
/// *compile* error rather than a subtle runtime one.
///
/// It also means the row's fields are read from the transaction's own `SELECT` rather than through a second
/// read, so the type and provenance written into the tombstone are the ones the delete acts on.
///
/// # Errors
///
/// Returns [`DatabaseError::StoredMemoryInvalid`] for a row whose columns do not decode, and
/// [`DatabaseError::Sqlite`] when the insert fails.
async fn write_tombstone_on(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    row: &SqliteRow,
    memory_id: &str,
    at: UtcTimestamp,
) -> Result<(), DatabaseError> {
    let search_key = row
        .try_get::<Option<String>, _>("search_key")
        .map_err(|_| DatabaseError::StoredMemoryInvalid {
            field: "search_key",
        })?;
    // A row with no search key is already a tombstone-shaped remainder, so there is nothing to block: the
    // deletion that cleared the key wrote its own tombstone. Not an error, and not a silent skip either —
    // the caller's receipt reports `removed_search_key: false` for the same row.
    let Some(search_key) = search_key else {
        return Ok(());
    };
    let text = |field: &'static str| -> Result<String, DatabaseError> {
        row.try_get::<String, _>(field)
            .map_err(|_| DatabaseError::StoredMemoryInvalid { field })
    };
    sqlx::query(
        "INSERT INTO memory_tombstones (\
            id, workspace_id, memory_type, search_key_hash, memory_id, deleted_by_actor_id, \
            correlation_id, deleted_at\
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
         ON CONFLICT (workspace_id, search_key_hash) DO NOTHING",
    )
    .bind(jarvis_core::MemoryId::new().to_string())
    .bind(text("workspace_id")?)
    .bind(text("memory_type")?)
    .bind(search_key_hash(&search_key))
    .bind(memory_id)
    .bind(text("created_by_actor_id")?)
    .bind(text("correlation_id")?)
    .bind(at.to_string())
    .execute(&mut **transaction)
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "record a memory tombstone",
        source,
    })?;
    Ok(())
}

/// Removes a memory row outright, with its search key, tombstone, and entity links.
///
/// # This is the full user deletion, and it is a different operation from `Delete`
///
/// `apply_memory_transition`'s `Delete` clears the **text** and keeps the row, which is what
/// `docs/architecture/memory-and-context.md`'s acceptance invariant needs ("deleting it removes text and
/// derived indexes") and what a source link continues to resolve against. It is not a full deletion: the
/// row, its actor, its correlation identity, and its provenance remain.
///
/// A **full user deletion** is the stronger request — "remove this and do not keep a record that says
/// anything" — and it is what `docs/architecture/memory-and-context.md` describes as covering "source links
/// where owned, embeddings, full-text indexes, caches, and relation edges". So this really deletes the row,
/// and the foreign keys (`memory_entities`, and `memories.supersedes_memory_id` /
/// `superseded_by_memory_id` via `ON DELETE SET NULL`) do the rest.
///
/// # Why the tombstone is written *before* the delete
///
/// The tombstone is the only durable record that blocks a re-ingest, and it deliberately has **no** foreign
/// key to the memory so it can outlive it. Writing it first means a crash between the two leaves a tombstone
/// for a memory that still exists, which is recoverable by deleting again — whereas the reverse order would
/// leave a deleted memory with no tombstone, so a later ingest would resurrect a claim the user removed. The
/// safe failure direction is the one that over-blocks.
///
/// `allow_relearn` is the deliberate exception: it skips the tombstone **and** removes any existing one, which
/// is an undo of a deletion rather than a cleanup, and it is reported in the receipt so the audit trail
/// distinguishes the two.
///
/// # Errors
///
/// - [`DatabaseError::MemoryNotFound`] when no memory has that identifier.
/// - [`DatabaseError::MemoryConflict`] when a concurrent writer advanced the version.
/// - [`DatabaseError::Sqlite`] when any statement fails.
pub async fn purge_memory(
    database: &SqliteDatabase,
    id: &str,
    expected_version: i64,
    allow_relearn: bool,
    at: UtcTimestamp,
) -> Result<(), DatabaseError> {
    // The version guard is a `SELECT`-then-`DELETE` inside **one transaction**, so a concurrent edit cannot
    // slip between the check and the removal. A bare `DELETE ... WHERE version = ?` would be simpler and
    // would report the same conflict, but it would not let the caller distinguish "the version moved" from
    // "the row is gone" — and those need different answers: the first is retryable, the second is not.
    //
    // Every statement below runs on this one connection, including the tombstone write. That is not a style
    // choice: SQLite in WAL mode fails a **deferred** transaction that reads and then writes with
    // `SQLITE_BUSY_SNAPSHOT` if another connection committed in between, so a tombstone written through the
    // pool while this transaction held a read made the delete fail with a bare "failed to purge a memory".
    // The first version of this function did exactly that, and the failure was invisible until a purge ran
    // against a memory whose tombstone had to be written.
    let mut transaction =
        database
            .pool()
            .begin()
            .await
            .map_err(|source| DatabaseError::Sqlite {
                operation: "begin a memory purge",
                source,
            })?;

    let row = sqlx::query(
        "SELECT version, workspace_id, memory_type, search_key, created_by_actor_id, correlation_id \
         FROM memories WHERE id = ?1",
    )
    .bind(id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "read a memory to purge",
        source,
    })?;
    let Some(row) = row else {
        transaction
            .commit()
            .await
            .map_err(|source| DatabaseError::Sqlite {
                operation: "commit a memory purge",
                source,
            })?;
        return Err(DatabaseError::MemoryNotFound);
    };
    let stored_version = row
        .try_get::<i64, _>("version")
        .map_err(|_| DatabaseError::StoredMemoryInvalid { field: "version" })?;
    if stored_version != expected_version {
        // Committed rather than dropped so the read is not left holding a write lock. Nothing was written,
        // so this is only releasing the snapshot.
        transaction
            .commit()
            .await
            .map_err(|source| DatabaseError::Sqlite {
                operation: "commit a memory purge",
                source,
            })?;
        return Err(DatabaseError::MemoryConflict);
    }

    if allow_relearn {
        sqlx::query("DELETE FROM memory_tombstones WHERE memory_id = ?1")
            .bind(id)
            .execute(&mut *transaction)
            .await
            .map_err(|source| DatabaseError::Sqlite {
                operation: "remove a memory tombstone",
                source,
            })?;
    } else {
        write_tombstone_on(&mut transaction, &row, id, at).await?;
    }

    // `memory_entities` cascades, and the two supersession columns are `ON DELETE SET NULL`, so a claim that
    // pointed at this one stops pointing at a row that no longer exists. `sqlite3` enforces that only when
    // `PRAGMA foreign_keys` is on; `SqliteDatabase::open` sets it, which is what makes the cascade real
    // rather than decorative.
    let result = sqlx::query("DELETE FROM memories WHERE id = ?1 AND version = ?2")
        .bind(id)
        .bind(expected_version)
        .execute(&mut *transaction)
        .await
        .map_err(|source| DatabaseError::Sqlite {
            operation: "purge a memory",
            source,
        })?;

    if result.rows_affected() == 0 {
        transaction
            .commit()
            .await
            .map_err(|source| DatabaseError::Sqlite {
                operation: "commit a memory purge",
                source,
            })?;
        return Err(DatabaseError::MemoryConflict);
    }

    transaction
        .commit()
        .await
        .map_err(|source| DatabaseError::Sqlite {
            operation: "commit a memory purge",
            source,
        })
}

/// Decodes one row from any of the memory reads into a `StoredMemory`.
///
/// Shared by every read so the column list and the decode cannot drift apart between them — the defect this
/// module's `read_memories` doc records, where a parameter went unbound and the query silently ignored a
/// filter. A second decode would be a second chance to name the wrong column.
fn decode_stored_memory(row: &sqlx::sqlite::SqliteRow) -> Result<StoredMemory, DatabaseError> {
    let search_key = row
        .try_get::<Option<String>, _>("search_key")
        .map_err(|_| DatabaseError::StoredMemoryInvalid {
            field: "search_key",
        })?;
    let version = row
        .try_get::<i64, _>("version")
        .map_err(|_| DatabaseError::StoredMemoryInvalid { field: "version" })?;
    Ok(StoredMemory {
        record: decode_memory(row)?,
        search_key,
        version,
    })
}

/// Runs one of the workspace-scoped memory reads that take no entity.
///
/// # Why the entity filter is an `Option` bound conditionally rather than a statement per shape
///
/// The entity-scoped statement takes a third parameter, so the `limit` is `?2` in one shape and `?3` in the
/// other. Binding the entity when it is present keeps that divergence in one place, and a caller cannot
/// supply the entity-scoped statement without its entity. An earlier cut gave this function an `entity_id`
/// parameter but never bound it: the query then still ran, silently ignoring the filter, which is exactly the
/// failure the shared runner exists to prevent.
async fn read_memories(
    database: &SqliteDatabase,
    statement: &'static str,
    workspace_id: WorkspaceId,
    entity_id: Option<EntityId>,
    limit: u32,
) -> Result<Vec<StoredMemory>, DatabaseError> {
    let mut query = sqlx::query(statement).bind(workspace_id.to_string());
    if let Some(entity_id) = entity_id {
        query = query.bind(entity_id.to_string());
    }

    let rows = query
        .bind(i64::from(limit))
        .fetch_all(database.pool())
        .await
        .map_err(|source| DatabaseError::Sqlite {
            operation: "read memories",
            source,
        })?;

    let mut memories = Vec::with_capacity(rows.len());
    for row in &rows {
        memories.push(decode_stored_memory(row)?);
    }
    attach_entity_links(database, &mut memories).await?;
    Ok(memories)
}
/// Records that a memory was selected into a context, reinforcing it.
///
/// # Errors
///
/// - [`DatabaseError::MemoryNotFound`] when no memory has that identifier.
/// - [`DatabaseError::InvalidMemoryStatus`] when the memory is deleted, because a deleted memory retains no
///   text and reinforcing it would make "was this used" a fact about a claim that no longer exists.
/// - [`DatabaseError::MemoryConflict`] when a concurrent writer advanced the version.
pub async fn reinforce_memory(
    database: &SqliteDatabase,
    id: &str,
    at: UtcTimestamp,
) -> Result<StoredMemory, DatabaseError> {
    let existing = find_memory(database, id).await?;
    if existing.record().status().is_terminal() {
        return Err(DatabaseError::InvalidMemoryStatus {
            from: existing.record().status().as_str(),
            to: "reinforced",
        });
    }

    let result = sqlx::query(
        "UPDATE memories SET last_accessed_at = ?3, retrieval_count = retrieval_count + 1, \
            version = version + 1 \
         WHERE id = ?1 AND version = ?2 AND status <> 'deleted'",
    )
    .bind(id)
    .bind(existing.version)
    .bind(at.to_string())
    .execute(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "reinforce a memory",
        source,
    })?;

    if result.rows_affected() == 0 {
        return Err(DatabaseError::MemoryConflict);
    }
    find_memory(database, id).await
}

/// Applies a domain transition to a stored memory and writes it back.
///
/// # Why the transition is applied by the **domain** and then written
///
/// The legal edges live in `MemoryStatus::can_advance_to`, and re-implementing them in SQL would be a second
/// copy of a rule that decides whether a claim is current. So the domain value is transitioned first â€”
/// which is what refuses an illegal edge â€” and the resulting fields are written under a version guard. A
/// storage layer that decided the edges itself would be the layer that let a superseded claim stay current.
///
/// # Errors
///
/// - [`DatabaseError::MemoryNotFound`] when no memory has that identifier.
/// - [`DatabaseError::InvalidMemoryRequest`] when the domain refuses the transition, carrying the field the
///   domain named.
/// - [`DatabaseError::MemoryConflict`] when a concurrent writer advanced the version.
pub async fn apply_memory_transition(
    database: &SqliteDatabase,
    id: &str,
    transition: MemoryTransition,
    at: UtcTimestamp,
) -> Result<StoredMemory, DatabaseError> {
    let existing = find_memory(database, id).await?;
    let current = existing.record();

    let next = match transition {
        // **The approver is a parameter of the transition, not a field of the request beside it.**
        //
        // `P4-014` is "admission is a decision that names its approver", and a decision is one fact: a
        // `Confirm` that carried no approver would be a state change with nobody behind it, which is the shape
        // the requirement removes. Folding it into the variant means the two cannot be supplied out of step,
        // and `InvalidApprovalField`'s own reasoning in `jarvis-core::approval` records the same conclusion
        // reached from the other direction -- a separate argument for one fact had already cost this repository
        // a discarded value once.
        MemoryTransition::Confirm { approver_actor_id } => {
            current.confirm_by(&approver_actor_id, at)
        }
        MemoryTransition::Archive => current.archive(at),
        MemoryTransition::Delete => current.delete(at),
        MemoryTransition::ReplaceWith(replacement) => current.replace_with(replacement, at),
    }
    .map_err(|error| memory_error(&error))?;

    // A `Delete` clears the content and the key, which is where the acceptance invariant "deleting it removes
    // text and derived indexes" is actually satisfied: the row remains so a source link still resolves, and
    // the text and its derived key do not.
    let search_key = match next.status() {
        MemoryStatus::Deleted => None,
        _ => existing.search_key.clone(),
    };

    // The claim triple is split into its three columns, and the pair of `CHECK`s in `0009` makes a partly
    // populated claim unstorable.
    let claim = claim_columns(next.structured_claim());

    let result = sqlx::query(
        "UPDATE memories SET status = ?3, content = ?4, claim_subject = ?5, claim_predicate = ?6, \
            claim_object = ?7, search_key = ?8, superseded_by_memory_id = ?9, updated_at = ?10, \
            admitted_by_actor_id = ?11, admitted_at = ?12, \
            version = version + 1 \
         WHERE id = ?1 AND version = ?2",
    )
    .bind(id)
    .bind(existing.version)
    .bind(next.status().as_str())
    .bind(next.content())
    .bind(claim.0)
    .bind(claim.1)
    .bind(claim.2)
    .bind(search_key.as_deref())
    .bind(
        next.superseded_by()
            .map(|replacement| replacement.to_string()),
    )
    .bind(next.updated_at().to_string())
    // **Written from the transition's output, not from the request.** `confirm_by` is the only thing that
    // sets either field, and an archive, delete, or replacement yields `None` for both -- so a correction of a
    // confirmed claim, which archives it, keeps the admission it had. Writing `None` there would erase the
    // record of who accepted the claim at the moment it was superseded, which is when an audit would want it.
    .bind(next.admitted_by_actor_id())
    .bind(next.admitted_at().map(|value| value.to_string()))
    .execute(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "apply a memory transition",
        source,
    })?;

    if result.rows_affected() == 0 {
        return Err(DatabaseError::MemoryConflict);
    }

    // **The tombstone is written after the row**, and the order is the safe one: a crash between them leaves
    // a memory that is deleted and not yet tombstoned, which is recoverable by deleting it again â€” while the
    // reverse order would leave a tombstone blocking a claim whose deletion never happened, which a user
    // could not see or undo.
    if next.status() == MemoryStatus::Deleted
        && let Some(key) = &existing.search_key
    {
        record_tombstone(database, existing.record(), key, at).await?;
    }

    find_memory(database, id).await
}

/// Which lifecycle change to apply.
///
/// Not `Copy`, because the confirmation variant carries an owned approver identity. The three that carry no
/// value could have been a separate unit-only enum, and that was rejected: `apply_memory_transition` takes one
/// value and matches it once, so splitting the type would buy a cheaper discriminant in exchange for a second
/// match arm on every caller and a shape where "a confirmation without an approver" is expressible again.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MemoryTransition {
    /// Accept a proposal, recording who accepted it.
    ///
    /// The approver is part of the variant rather than an argument beside the transition, so a state change
    /// cannot be separated from the decision that justifies it — see the note in `apply_memory_transition`.
    Confirm {
        /// The identity accepting the claim.
        approver_actor_id: String,
    },
    /// Set aside without deleting.
    Archive,
    /// Remove the text and block a re-ingest.
    Delete,
    /// Record that another memory has replaced this one.
    ReplaceWith(MemoryId),
}

/// Returns whether a claim was deleted in this workspace.
///
/// # Errors
///
/// Returns [`DatabaseError::Sqlite`] when the read fails.
pub async fn is_tombstoned(
    database: &SqliteDatabase,
    workspace_id: WorkspaceId,
    search_key: &MemorySearchKey,
) -> Result<bool, DatabaseError> {
    let row = sqlx::query(
        "SELECT id FROM memory_tombstones WHERE workspace_id = ?1 AND search_key_hash = ?2",
    )
    .bind(workspace_id.to_string())
    .bind(search_key_hash(search_key.as_str()))
    .fetch_optional(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "check a memory tombstone",
        source,
    })?;
    Ok(row.is_some())
}

/// Writes the tombstone that blocks a deleted claim from returning.
///
/// # Errors
///
/// Returns [`DatabaseError::Sqlite`] when the write fails.
pub async fn record_tombstone(
    database: &SqliteDatabase,
    memory: &MemoryRecord,
    search_key: &str,
    at: UtcTimestamp,
) -> Result<(), DatabaseError> {
    sqlx::query(
        "INSERT INTO memory_tombstones (\
            id, workspace_id, memory_type, search_key_hash, memory_id, deleted_by_actor_id, \
            correlation_id, deleted_at\
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
         ON CONFLICT (workspace_id, search_key_hash) DO NOTHING",
    )
    .bind(jarvis_core::MemoryId::new().to_string())
    .bind(memory.workspace_id().to_string())
    .bind(memory.memory_type().as_str())
    .bind(search_key_hash(search_key))
    // The identifier, but **not** a foreign key: a full user-deletion removes the memory row and the
    // tombstone must outlive it. Writing the identifier lets an operator line the two up while the memory
    // still exists, and costs nothing once it does not.
    .bind(memory.id().to_string())
    .bind(memory.created_by_actor_id())
    .bind(memory.correlation_id().to_string())
    .bind(at.to_string())
    .execute(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "record a memory tombstone",
        source,
    })?;
    Ok(())
}

/// Records an entity, or returns the existing one for the same label and kind.
///
/// # Errors
///
/// Returns [`DatabaseError::Sqlite`] when the row cannot be written.
pub async fn record_entity(
    database: &SqliteDatabase,
    entity: &NewEntity,
) -> Result<EntityId, DatabaseError> {
    if entity.label.trim().is_empty() || entity.label.chars().count() > 256 {
        return Err(DatabaseError::InvalidMemoryRequest { field: "label" });
    }
    sqlx::query(
        "INSERT INTO entities (\
            id, workspace_id, kind, label, attributes, confidence, status, merged_into, created_at, \
            updated_at, version\
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'active', NULL, ?7, ?7, 1) \
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(entity.id.to_string())
    .bind(entity.workspace_id.to_string())
    .bind(entity.kind.as_str())
    .bind(entity.label.trim())
    .bind(entity.attributes.as_deref())
    .bind(entity.confidence.as_str())
    .bind(entity.created_at.to_string())
    .execute(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "record an entity",
        source,
    })?;
    Ok(entity.id)
}

/// Reads one entity.
///
/// # Errors
///
/// Returns [`DatabaseError::EntityNotFound`] when no entity has that identifier, and
/// [`DatabaseError::StoredMemoryInvalid`] when a stored row cannot be decoded.
pub async fn find_entity(
    database: &SqliteDatabase,
    id: &str,
) -> Result<StoredEntity, DatabaseError> {
    let row = sqlx::query(
        "SELECT id, workspace_id, kind, label, attributes, confidence, status, merged_into, created_at, \
                updated_at, version \
         FROM entities WHERE id = ?1",
    )
    .bind(id)
    .fetch_optional(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "read an entity",
        source,
    })?;
    let row = row.ok_or(DatabaseError::EntityNotFound)?;
    decode_entity(&row)
}

/// Merges one entity into another, making the merge auditable and reversible.
///
/// # Why the losing row is retained
///
/// A merge is "auditable and reversible" per the architecture, so the merged entity is **not** deleted: it is
/// marked `merged` and points at the winner. Reversal is then one `UPDATE`, and an audit of "what did we
/// believe before the merge" is a row rather than a log line.
///
/// # Why a chain is refused
///
/// A merge whose target is itself merged would make resolution a chain to follow, and a cycle would make it
/// infinite. So the target must be `active`: merging is always *into* the current winner, which keeps the
/// resolution depth at exactly one hop.
///
/// # Errors
///
/// - [`DatabaseError::EntityNotFound`] when either entity is missing.
/// - [`DatabaseError::InvalidEntityMerge`] for a self-merge, a target that is not active, or a source that is
///   not active.
pub async fn merge_entities(
    database: &SqliteDatabase,
    source_id: EntityId,
    target_id: EntityId,
    at: UtcTimestamp,
) -> Result<StoredEntity, DatabaseError> {
    if source_id == target_id {
        return Err(DatabaseError::InvalidEntityMerge {
            reason: "an entity cannot be merged into itself",
        });
    }
    let source = find_entity(database, &source_id.to_string()).await?;
    let target = find_entity(database, &target_id.to_string()).await?;
    if source.workspace_id() != target.workspace_id() {
        return Err(DatabaseError::InvalidEntityMerge {
            reason: "both entities must belong to one workspace",
        });
    }
    if !source.is_usable() {
        return Err(DatabaseError::InvalidEntityMerge {
            reason: "only an active entity can be merged away",
        });
    }
    if !target.is_usable() {
        return Err(DatabaseError::InvalidEntityMerge {
            reason: "an entity may only be merged into the current winner",
        });
    }

    let result = sqlx::query(
        "UPDATE entities SET status = 'merged', merged_into = ?3, updated_at = ?4, version = version + 1 \
         WHERE id = ?1 AND version = ?2 AND status = 'active'",
    )
    .bind(source_id.to_string())
    .bind(source.version())
    .bind(target_id.to_string())
    .bind(at.to_string())
    .execute(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "merge entities",
        source,
    })?;

    if result.rows_affected() == 0 {
        return Err(DatabaseError::InvalidEntityMerge {
            reason: "the source entity changed before the merge could be recorded",
        });
    }
    find_entity(database, &source_id.to_string()).await
}

/// Records a verified alias for an entity.
///
/// # Errors
///
/// - [`DatabaseError::EntityNotFound`] when the entity is missing.
/// - [`DatabaseError::AliasAlreadyVerified`] when another entity already holds this verified alias in the
///   workspace. A **verified** alias is an identity claim, so two entities holding one is a contradiction to
///   surface; a `probabilistic` alias is a candidate, and the partial index lets several entities carry one.
// The eight parameters are the alias's own fields, one per column the table has, plus the database. Grouping
// them into a struct would move the same eight fields one level down and give a caller a place to omit one,
// which is the opposite of what this write needs.
#[allow(clippy::too_many_arguments)]
pub async fn record_alias(
    database: &SqliteDatabase,
    entity_id: EntityId,
    alias_kind: &str,
    alias_value: &str,
    verification: EntityMatch,
    confidence: MemoryConfidence,
    source_kind: MemorySourceKind,
    at: UtcTimestamp,
) -> Result<(), DatabaseError> {
    let entity = find_entity(database, &entity_id.to_string()).await?;
    let alias_value = alias_value.trim();
    if alias_value.is_empty() || alias_value.chars().count() > 256 {
        return Err(DatabaseError::InvalidMemoryRequest {
            field: "alias_value",
        });
    }

    let result = sqlx::query(
        "INSERT INTO entity_aliases (\
            id, entity_id, workspace_id, alias_kind, alias_value, normalized, source_kind, \
            verification, confidence, created_at, updated_at\
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10) \
         ON CONFLICT (workspace_id, alias_kind, normalized) WHERE verification <> 'probabilistic' \
         DO NOTHING",
    )
    .bind(jarvis_core::MemoryId::new().to_string())
    .bind(entity_id.to_string())
    .bind(entity.workspace_id().to_string())
    .bind(alias_kind)
    .bind(alias_value)
    .bind(normalize_alias(alias_value))
    .bind(source_kind.as_str())
    .bind(verification.as_str())
    .bind(confidence.as_str())
    .bind(at.to_string())
    .execute(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "record an entity alias",
        source,
    })?;

    if result.rows_affected() == 0 && verification.may_merge() {
        // A verified alias collided, so another entity holds this identity. The existing holder is returned
        // so the caller can surface the contradiction rather than guess which entity is right.
        let existing = read_verified_alias(
            database,
            entity.workspace_id(),
            alias_kind,
            &normalize_alias(alias_value),
        )
        .await?
        .ok_or(DatabaseError::Sqlite {
            operation: "resolve a verified alias",
            source: sqlx::Error::RowNotFound,
        })?;
        return Err(DatabaseError::AliasAlreadyVerified {
            existing_entity_id: existing.entity_id().to_string(),
        });
    }
    Ok(())
}

/// Resolves an alias to the entity it names, when it resolves.
///
/// # Why only a verified alias resolves
///
/// The architecture: "Ambiguous aliases remain separate candidates." A resolution that returned a
/// `probabilistic` match would turn a guess into an identity, and every later claim would inherit it. So this
/// reads only verified aliases, and a caller wanting candidates reads [`read_alias_candidates`] instead.
///
/// # Errors
///
/// Returns [`DatabaseError::StoredMemoryInvalid`] when a stored row cannot be decoded.
pub async fn resolve_alias(
    database: &SqliteDatabase,
    workspace_id: WorkspaceId,
    alias_kind: &str,
    alias_value: &str,
) -> Result<Option<StoredAlias>, DatabaseError> {
    read_verified_alias(
        database,
        workspace_id,
        alias_kind,
        &normalize_alias(alias_value),
    )
    .await
}

/// Reads every alias matching a value, verified or not.
///
/// What "remain separate candidates" requires: several entities may be guessed from one name, so a resolver
/// that must decide has to see all of them rather than the first.
///
/// # Errors
///
/// Returns [`DatabaseError::StoredMemoryInvalid`] when a stored row cannot be decoded.
pub async fn read_alias_candidates(
    database: &SqliteDatabase,
    workspace_id: WorkspaceId,
    alias_kind: &str,
    alias_value: &str,
) -> Result<Vec<StoredAlias>, DatabaseError> {
    let rows = sqlx::query(
        "SELECT id, entity_id, alias_kind, alias_value, normalized, verification, confidence \
         FROM entity_aliases WHERE workspace_id = ?1 AND alias_kind = ?2 AND normalized = ?3 \
         ORDER BY id ASC",
    )
    .bind(workspace_id.to_string())
    .bind(alias_kind)
    .bind(normalize_alias(alias_value))
    .fetch_all(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "read alias candidates",
        source,
    })?;

    rows.iter().map(decode_alias).collect()
}

/// Reads one verified alias, or `None`.
async fn read_verified_alias(
    database: &SqliteDatabase,
    workspace_id: WorkspaceId,
    alias_kind: &str,
    normalized: &str,
) -> Result<Option<StoredAlias>, DatabaseError> {
    let row = sqlx::query(
        "SELECT id, entity_id, alias_kind, alias_value, normalized, verification, confidence \
         FROM entity_aliases \
         WHERE workspace_id = ?1 AND alias_kind = ?2 AND normalized = ?3 AND verification <> 'probabilistic'",
    )
    .bind(workspace_id.to_string())
    .bind(alias_kind)
    .bind(normalized)
    .fetch_optional(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "read a verified alias",
        source,
    })?;
    match row {
        Some(row) => Ok(Some(decode_alias(&row)?)),
        None => Ok(None),
    }
}

/// Records a relation between two entities.
///
/// # Errors
///
/// - [`DatabaseError::EntityNotFound`] when either entity is missing.
/// - [`DatabaseError::InvalidMemoryRequest`] when the predicate is unusable, the source is unusable, or the
///   two entities are the same â€” which the schema also refuses, so this reports it before the write.
// The ten parameters are the relation's own attributes and provenance, one per column the table has, plus
// the database. Grouping them into a struct would move the same fields one level down and give a caller a
// place to omit one, which is the opposite of what an auditable relation write needs.
#[allow(clippy::too_many_arguments)]
pub async fn record_relation(
    database: &SqliteDatabase,
    workspace_id: WorkspaceId,
    subject_id: EntityId,
    predicate: &str,
    object_id: EntityId,
    source: &MemorySource,
    confidence: MemoryConfidence,
    sensitivity: Sensitivity,
    actor_id: &str,
    correlation_id: CorrelationId,
    at: UtcTimestamp,
) -> Result<String, DatabaseError> {
    if subject_id == object_id {
        return Err(DatabaseError::InvalidMemoryRequest { field: "object_id" });
    }
    let predicate = predicate.trim();
    if predicate.is_empty() || predicate.chars().count() > 256 {
        return Err(DatabaseError::InvalidMemoryRequest { field: "predicate" });
    }
    // A relation is a claim, so the two rules a memory carries apply here too. Checked before the write so
    // the refusal names the field rather than arriving as a `CHECK` violation.
    if source.kind().is_model_produced() && confidence != MemoryConfidence::Unverified {
        return Err(DatabaseError::InvalidMemoryRequest {
            field: "confidence",
        });
    }

    let id = jarvis_core::MemoryId::new().to_string();
    sqlx::query(
        "INSERT INTO entity_relations (\
            id, workspace_id, subject_id, predicate, object_id, source_kind, source_locator, source_trust, \
            source_excerpt_hash, confidence, sensitivity, status, valid_from, valid_until, \
            created_by_actor_id, correlation_id, created_at, updated_at, version\
         ) VALUES (\
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 'active', ?12, NULL, ?13, ?14, ?12, ?12, 1\
         )",
    )
    .bind(&id)
    .bind(workspace_id.to_string())
    .bind(subject_id.to_string())
    .bind(predicate)
    .bind(object_id.to_string())
    .bind(source.kind().as_str())
    .bind(source.locator())
    .bind(source.trust().as_str())
    .bind(source.excerpt_hash())
    .bind(confidence.as_str())
    .bind(sensitivity.as_str())
    .bind(at.to_string())
    .bind(actor_id)
    .bind(correlation_id.to_string())
    .execute(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "record an entity relation",
        source,
    })?;
    Ok(id)
}

/// Reads the relations of one subject.
///
/// # Errors
///
/// Returns [`DatabaseError::Sqlite`] when the read fails.
pub async fn read_subject_relations(
    database: &SqliteDatabase,
    workspace_id: WorkspaceId,
    subject_id: EntityId,
) -> Result<Vec<StoredRelation>, DatabaseError> {
    let rows = sqlx::query(
        "SELECT id, workspace_id, subject_id, predicate, object_id, confidence, sensitivity, status, \
                valid_from, valid_until \
         FROM entity_relations \
         WHERE workspace_id = ?1 AND subject_id = ?2 AND status <> 'deleted' \
         ORDER BY predicate ASC, id ASC",
    )
    .bind(workspace_id.to_string())
    .bind(subject_id.to_string())
    .fetch_all(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "read entity relations",
        source,
    })?;
    rows.iter().map(decode_relation).collect()
}

/// One stored relation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredRelation {
    id: String,
    workspace_id: WorkspaceId,
    subject_id: EntityId,
    predicate: String,
    object_id: EntityId,
    confidence: MemoryConfidence,
    sensitivity: Sensitivity,
    valid_from: UtcTimestamp,
    valid_until: Option<UtcTimestamp>,
}

impl StoredRelation {
    /// Returns the relation identifier.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the owning workspace.
    #[must_use]
    pub const fn workspace_id(&self) -> WorkspaceId {
        self.workspace_id
    }

    /// Returns the subject entity.
    #[must_use]
    pub const fn subject_id(&self) -> EntityId {
        self.subject_id
    }

    /// Returns the predicate.
    #[must_use]
    pub fn predicate(&self) -> &str {
        &self.predicate
    }

    /// Returns the object entity.
    #[must_use]
    pub const fn object_id(&self) -> EntityId {
        self.object_id
    }

    /// Returns how confidently the relation was established.
    #[must_use]
    pub const fn confidence(&self) -> MemoryConfidence {
        self.confidence
    }

    /// Returns how widely the relation may be disclosed.
    #[must_use]
    pub const fn sensitivity(&self) -> Sensitivity {
        self.sensitivity
    }

    /// Returns the instant the relation became true.
    #[must_use]
    pub const fn valid_from(&self) -> UtcTimestamp {
        self.valid_from
    }

    /// Returns the instant it stops being true, when one is recorded.
    #[must_use]
    pub const fn valid_until(&self) -> Option<UtcTimestamp> {
        self.valid_until
    }
}

/// The instant a memory should be considered current at, from a stored clock.
///
/// A free function rather than a method so a caller supplying a fixed instant is not tempted to read a clock
/// inside the repository â€” the same rule `record_decision` follows, and the reason every validity question is
/// answerable at a chosen time in a test.
///
/// # Errors
///
/// Returns [`DatabaseError::StoredMemoryInvalid`] when the timestamp text is unusable.
pub fn parse_timestamp(value: &str, field: &'static str) -> Result<UtcTimestamp, DatabaseError> {
    value
        .parse()
        .map_err(|_| DatabaseError::StoredMemoryInvalid { field })
}

/// Returns whether a value is a memory's stored search key for a deleted row.
///
/// Used by the decode path to decide whether a cleared key is expected, so a live memory with no key is
/// reported rather than silently accepted.
fn search_key_hash(search_key: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(search_key.as_bytes());
    let mut rendered = String::with_capacity(64);
    for byte in hasher.finalize() {
        rendered.push(char::from(b"0123456789abcdef"[usize::from(byte >> 4)]));
        rendered.push(char::from(b"0123456789abcdef"[usize::from(byte & 0x0f)]));
    }
    rendered
}

/// Folds an alias for comparison: trimmed, lowercased, and whitespace-collapsed.
///
/// Stored rather than derived at each comparison, so the uniqueness rule and a lookup cannot apply different
/// folding. Whitespace is collapsed rather than only trimmed, because `"a b"` and `"a  b"` are one alias as a
/// person reads them, and a unique index that treated them as two would let a duplicate identity in.
fn normalize_alias(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Splits a structured claim into its three columns.
///
/// A tuple rather than one string, because the columns are three: the schema stores a claim as
/// subject/predicate/object so a lookup by predicate is an index seek and a reader does not depend on a
/// document's shape being stable.
fn claim_columns(claim: Option<&StructuredClaim>) -> (Option<&str>, Option<&str>, Option<&str>) {
    match claim {
        Some(claim) => (
            Some(claim.subject()),
            Some(claim.predicate()),
            Some(claim.object()),
        ),
        None => (None, None, None),
    }
}

/// Finds a memory's identifier by its search key.
async fn find_memory_id_by_key(
    database: &SqliteDatabase,
    workspace_id: WorkspaceId,
    search_key: &str,
) -> Result<Option<String>, DatabaseError> {
    let row = sqlx::query("SELECT id FROM memories WHERE workspace_id = ?1 AND search_key = ?2")
        .bind(workspace_id.to_string())
        .bind(search_key)
        .fetch_optional(database.pool())
        .await
        .map_err(|source| DatabaseError::Sqlite {
            operation: "find a memory by its search key",
            source,
        })?;
    match row {
        Some(row) => {
            Ok(Some(row.try_get::<String, _>("id").map_err(|_| {
                DatabaseError::StoredMemoryInvalid { field: "id" }
            })?))
        }
        None => Ok(None),
    }
}

/// Reads a memory's entity links into the domain value.
async fn load_linked_entities(
    database: &SqliteDatabase,
    record: &mut MemoryRecord,
) -> Result<(), DatabaseError> {
    let rows = sqlx::query(
        "SELECT entity_id, matched_by FROM memory_entities WHERE memory_id = ?1 ORDER BY entity_id ASC",
    )
    .bind(record.id().to_string())
    .fetch_all(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "read a memory's entities",
        source,
    })?;

    let mut entities = Vec::with_capacity(rows.len());
    for row in &rows {
        let entity_id: EntityId = row
            .try_get::<String, _>("entity_id")
            .map_err(|_| DatabaseError::StoredMemoryInvalid { field: "entity_id" })?
            .parse()
            .map_err(|_| DatabaseError::StoredMemoryInvalid { field: "entity_id" })?;
        let matched_by: EntityMatch = row
            .try_get::<String, _>("matched_by")
            .map_err(|_| DatabaseError::StoredMemoryInvalid {
                field: "matched_by",
            })?
            .parse()
            .map_err(|_| DatabaseError::StoredMemoryInvalid {
                field: "matched_by",
            })?;
        entities.push(EntityRef::new(entity_id, matched_by));
    }

    // The entity set is replaced rather than merged: the store is the authority on what a memory is linked
    // to, and a decode that kept whatever a caller supplied would let the two disagree.
    record.replace_entities(entities);
    Ok(())
}

/// Loads entity links for a page of memories, so a listing and an export report the entities a claim is
/// about rather than an empty set.
///
/// One statement per row rather than a join, because the link rows carry a `matched_by` per entity and a
/// join would either fan the memory columns out or need a second decode path. The page is bounded by
/// `MAX_MEMORY_PAGE`, so the count is bounded too — which is the reason the bound exists rather than a
/// later optimisation.
async fn attach_entity_links(
    database: &SqliteDatabase,
    memories: &mut [StoredMemory],
) -> Result<(), DatabaseError> {
    for stored in memories.iter_mut() {
        load_linked_entities(database, &mut stored.record).await?;
    }
    Ok(())
}

/// Decodes one memory row, re-checking the domain's invariants.
///
/// The schema's `CHECK`s make a contradictory row unstorable through this repository, but a row written by
/// another build, restored from a backup, or hand-edited is not covered by that argument -- the same reasoning
/// the approval and tool-call repositories use for their own re-checks.
///
/// # Why a refusal here is a **stored-row** error and not an invalid request
///
/// The mapping is deliberately not `memory_error`, and the difference is who can act. The same
/// `InvalidMemory` value reaching this function came **from the row** rather than from an argument: nobody
/// supplied the admission columns, the status, or the content. Reporting it as
/// [`DatabaseError::InvalidMemoryRequest`] would name a request field for a value the caller never sent, which
/// sends an operator to look at their request instead of at their data -- and it would also make a corrupt row
/// answer `422`, telling the caller to change what they are asking for when what they need is a backup.
fn decode_memory(row: &SqliteRow) -> Result<MemoryRecord, DatabaseError> {
    let (parts, state) = read_memory_fields(row)?;
    MemoryRecord::from_stored(parts, &state).map_err(|error| DatabaseError::StoredMemoryInvalid {
        field: invalid_memory_field(&error),
    })
}

/// Reads every column a memory row carries.
///
/// # Why this is separate from the construction
///
/// Reading a row and applying the domain's rules to what was read are two jobs, and the second is the one
/// that can refuse. Keeping them apart means the reader has no opinion about validity, and — the practical
/// reason — neither function grows past the point where a reader loses the thread: the two together are well
/// over a hundred lines, which is the shape that hides a missing field.
// Twenty-eight columns, one line each, and the length is the table's rather than the logic's. Splitting
// further would mean a helper per column group, and the grouping would be arbitrary — every field here is read
// exactly once, in the order the statement selects it, which is what makes a missing column obvious.
#[allow(clippy::too_many_lines)]
fn read_memory_fields(
    row: &SqliteRow,
) -> Result<(MemoryRecordParts, StoredMemoryState), DatabaseError> {
    let invalid = |field: &'static str| DatabaseError::StoredMemoryInvalid { field };
    let text = |field: &'static str| -> Result<String, DatabaseError> {
        row.try_get::<String, _>(field).map_err(|_| invalid(field))
    };
    let optional = |field: &'static str| -> Result<Option<String>, DatabaseError> {
        row.try_get::<Option<String>, _>(field)
            .map_err(|_| invalid(field))
    };
    let parse_id = |value: String, field: &'static str| -> Result<MemoryId, DatabaseError> {
        value.parse().map_err(|_| invalid(field))
    };

    let id = parse_id(text("id")?, "id")?;
    let workspace_id: WorkspaceId = text("workspace_id")?
        .parse()
        .map_err(|_| invalid("workspace_id"))?;
    let memory_type: MemoryType = text("memory_type")?
        .parse()
        .map_err(|_| invalid("memory_type"))?;
    let content = text("content")?;
    let structured_claim = match optional("claim_subject")? {
        Some(subject) => Some(
            StructuredClaim::new(subject, text("claim_predicate")?, text("claim_object")?)
                .map_err(|_| invalid("structured_claim"))?,
        ),
        None => None,
    };
    let source_kind: MemorySourceKind = text("source_kind")?
        .parse()
        .map_err(|_| invalid("source_kind"))?;
    let source_locator = text("source_locator")?;
    let source_trust: MemoryTrust = text("source_trust")?
        .parse()
        .map_err(|_| invalid("source_trust"))?;
    let source_excerpt_hash = optional("source_excerpt_hash")?;
    // Built through the constructor rather than field by field, so a stored row whose trust disagrees with
    // its kind is **refused** here as well as at write time.
    let source = MemorySource::new(
        source_kind,
        source_locator,
        source_trust,
        source_excerpt_hash,
    )
    .map_err(|_| invalid("source_kind"))?;
    let confidence: MemoryConfidence = text("confidence")?
        .parse()
        .map_err(|_| invalid("confidence"))?;
    let importance = row
        .try_get::<i64, _>("importance")
        .map_err(|_| invalid("importance"))?;
    let sensitivity: Sensitivity = text("sensitivity")?
        .parse()
        .map_err(|_| invalid("sensitivity"))?;
    let status: MemoryStatus = text("status")?.parse().map_err(|_| invalid("status"))?;
    let valid_from = parse_timestamp(&text("valid_from")?, "valid_from")?;
    let valid_until = match optional("valid_until")? {
        Some(value) => Some(parse_timestamp(&value, "valid_until")?),
        None => None,
    };
    let supersedes = match optional("supersedes_memory_id")? {
        Some(value) => Some(parse_id(value, "supersedes_memory_id")?),
        None => None,
    };
    let superseded_by = match optional("superseded_by_memory_id")? {
        Some(value) => Some(parse_id(value, "superseded_by_memory_id")?),
        None => None,
    };
    let run_id = match optional("run_id")? {
        Some(value) => Some(value.parse().map_err(|_| invalid("run_id"))?),
        None => None,
    };
    let created_by_actor_id = text("created_by_actor_id")?;
    let correlation_id: CorrelationId = text("correlation_id")?
        .parse()
        .map_err(|_| invalid("correlation_id"))?;
    let created_at = parse_timestamp(&text("created_at")?, "created_at")?;
    let updated_at = parse_timestamp(&text("updated_at")?, "updated_at")?;
    let last_accessed_at = match optional("last_accessed_at")? {
        Some(value) => Some(parse_timestamp(&value, "last_accessed_at")?),
        None => None,
    };
    let retrieval_count = u32::try_from(
        row.try_get::<i64, _>("retrieval_count")
            .map_err(|_| invalid("retrieval_count"))?,
    )
    .unwrap_or(u32::MAX);
    // The admission pair, read from the row rather than derived. Both are `Option`, and `from_stored` is what
    // refuses a row where only one is present -- the rule this table's schema cannot express, because SQLite
    // will not add a column-level `CHECK` that mentions `status`.
    let admitted_by_actor_id = optional("admitted_by_actor_id")?;
    let admitted_at = match optional("admitted_at")? {
        Some(value) => Some(parse_timestamp(&value, "admitted_at")?),
        None => None,
    };

    // The parts are returned with the state rather than the parts alone, because the constructor applies the
    // two together: `deleted` is the one status whose content may be empty, so a caller that had to pass the
    // status in separately could pass it to the wrong row.
    Ok((
        MemoryRecordParts {
            id,
            workspace_id,
            memory_type,
            content,
            structured_claim,
            source,
            confidence,
            importance: u8::try_from(importance).unwrap_or(jarvis_core::MAX_MEMORY_IMPORTANCE),
            sensitivity,
            // Replaced once the links are read, so the constructor's entity check is satisfied without a second
            // query inside the decode.
            entities: Vec::new(),
            valid_from: Some(valid_from),
            valid_until,
            supersedes,
            run_id,
            created_by_actor_id,
            correlation_id,
            created_at,
        },
        StoredMemoryState {
            status,
            superseded_by,
            updated_at,
            last_accessed_at,
            retrieval_count,
            admitted_by_actor_id,
            admitted_at,
        },
    ))
}

/// Decodes one entity row.
fn decode_entity(row: &SqliteRow) -> Result<StoredEntity, DatabaseError> {
    let invalid = |field: &'static str| DatabaseError::StoredMemoryInvalid { field };
    let text = |field: &'static str| -> Result<String, DatabaseError> {
        row.try_get::<String, _>(field).map_err(|_| invalid(field))
    };

    Ok(StoredEntity {
        id: text("id")?.parse().map_err(|_| invalid("id"))?,
        workspace_id: text("workspace_id")?
            .parse()
            .map_err(|_| invalid("workspace_id"))?,
        kind: text("kind")?.parse()?,
        label: text("label")?,
        attributes: row
            .try_get::<Option<String>, _>("attributes")
            .map_err(|_| invalid("attributes"))?,
        confidence: text("confidence")?
            .parse()
            .map_err(|_| invalid("confidence"))?,
        status: text("status")?.parse()?,
        merged_into: match row
            .try_get::<Option<String>, _>("merged_into")
            .map_err(|_| invalid("merged_into"))?
        {
            Some(value) => Some(value.parse().map_err(|_| invalid("merged_into"))?),
            None => None,
        },
        created_at: parse_timestamp(&text("created_at")?, "created_at")?,
        updated_at: parse_timestamp(&text("updated_at")?, "updated_at")?,
        version: row
            .try_get::<i64, _>("version")
            .map_err(|_| invalid("version"))?,
    })
}

/// Decodes one alias row.
fn decode_alias(row: &SqliteRow) -> Result<StoredAlias, DatabaseError> {
    let invalid = |field: &'static str| DatabaseError::StoredMemoryInvalid { field };
    let text = |field: &'static str| -> Result<String, DatabaseError> {
        row.try_get::<String, _>(field).map_err(|_| invalid(field))
    };
    Ok(StoredAlias {
        id: text("id")?,
        entity_id: text("entity_id")?
            .parse()
            .map_err(|_| invalid("entity_id"))?,
        alias_kind: text("alias_kind")?,
        alias_value: text("alias_value")?,
        normalized: text("normalized")?,
        verification: text("verification")?
            .parse()
            .map_err(|_| invalid("verification"))?,
        confidence: text("confidence")?
            .parse()
            .map_err(|_| invalid("confidence"))?,
    })
}

/// Decodes one relation row.
fn decode_relation(row: &SqliteRow) -> Result<StoredRelation, DatabaseError> {
    let invalid = |field: &'static str| DatabaseError::StoredMemoryInvalid { field };
    let text = |field: &'static str| -> Result<String, DatabaseError> {
        row.try_get::<String, _>(field).map_err(|_| invalid(field))
    };
    Ok(StoredRelation {
        id: text("id")?,
        workspace_id: text("workspace_id")?
            .parse()
            .map_err(|_| invalid("workspace_id"))?,
        subject_id: text("subject_id")?
            .parse()
            .map_err(|_| invalid("subject_id"))?,
        predicate: text("predicate")?,
        object_id: text("object_id")?
            .parse()
            .map_err(|_| invalid("object_id"))?,
        confidence: text("confidence")?
            .parse()
            .map_err(|_| invalid("confidence"))?,
        sensitivity: text("sensitivity")?
            .parse()
            .map_err(|_| invalid("sensitivity"))?,
        valid_from: parse_timestamp(&text("valid_from")?, "valid_from")?,
        valid_until: match row
            .try_get::<Option<String>, _>("valid_until")
            .map_err(|_| invalid("valid_until"))?
        {
            Some(value) => Some(parse_timestamp(&value, "valid_until")?),
            None => None,
        },
    })
}

/// Maps a domain validation failure to a storage error, naming the field.
///
/// The field name is stable and the offending value is never included: a memory's content is user-authored
/// text and its source locator can name a provider identifier, so an error carrying either could put content
/// in a log. `Debug` of the domain error is deliberately not used for the same reason.
const fn memory_error(error: &jarvis_core::InvalidMemory) -> DatabaseError {
    DatabaseError::InvalidMemoryRequest {
        field: invalid_memory_field(error),
    }
}

/// Returns the column a domain rule is about.
///
/// Shared by the two mappings rather than duplicated, because the question -- which column does this rule
/// name -- has one answer whatever is being refused. Two copies would be two places to update when a rule is
/// added, and the repository has already been bitten by a pair of values that had to agree and nothing holding
/// both.
///
/// The field name is stable and the offending value is never included: a memory's content is user-authored
/// text and its source locator can name a provider identifier, so an error carrying either could put content
/// in a log. `Debug` of the domain error is deliberately not used for the same reason.
const fn invalid_memory_field(error: &jarvis_core::InvalidMemory) -> &'static str {
    use jarvis_core::InvalidMemory as Field;
    // Several variants name the same column, and that is the point rather than an accident: `Content` and
    // `DeletedRetainsText` are two rules about one column, and `SupersedesMissing`/`SupersedesSelf` are two
    // about another. Merging each pair into one arm would need a helper returning a name for a list of
    // unrelated variants, which reads as though the grouping were principled when it is only a coincidence of
    // both failures landing on the same field. The list stays exhaustive per variant, so adding a rule forces
    // a decision about which column it names.
    #[allow(clippy::match_same_arms)]
    match error {
        Field::Content => "content",
        Field::StructuredClaim => "structured_claim",
        Field::Source => "source",
        Field::SourceExcerptHash => "source_excerpt_hash",
        Field::Entities => "entities",
        Field::SearchKey => "search_key",
        Field::ClaimPart => "structured_claim",
        Field::Validity => "valid_until",
        Field::UnknownConfidence => "confidence",
        Field::UnknownType => "memory_type",
        Field::UnknownStatus => "status",
        Field::UnknownSourceKind => "source_kind",
        Field::UnknownTrust => "source_trust",
        Field::EntityReference => "entity_id",
        Field::SupersedesMissing => "supersedes_memory_id",
        Field::SupersedesSelf => "supersedes_memory_id",
        Field::AlreadyDeleted => "status",
        Field::DeletedRetainsText => "content",
        Field::IllegalStatus { .. } => "status",
        // The two admission rules name the columns they are about rather than a caller-supplied field, because
        // a caller supplies neither: both are decided from the stored row and from the identity a transition
        // carries. A refusal here is an operator's diagnostic, not a request the caller can correct.
        Field::ApproverUnusable => "admitted_by_actor_id",
        Field::AdmissionInconsistent => "admitted_at",
    }
}

#[cfg(test)]
#[path = "memory_tests.rs"]
mod tests;
