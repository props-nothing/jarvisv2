//! Durable storage for skill revisions: a procedure that names already-granted tools.
//!
//! `ADR-0117` fixes the trust boundary this repository stores, and three of its rules are visible here:
//!
//! 1. **A step is a request, not a grant.** Nothing in this table carries a scope, an approval, or a
//!    granted tool set, so there is no column a wider authority could arrive in. Whether a step may run is
//!    decided when it runs, by `jarvis_tools`'s pipeline, not by anything read here.
//! 2. **Promotion is an attributable decision.** [`promote_skill_revision`] requires an approver and an
//!    instant, and the schema refuses an active model-authored row that names neither.
//! 3. **Replacement is declared.** Both supersession directions are columns written by explicit calls, so
//!    which procedure ran is readable rather than inferred.
//!
//! # Why the tool-identifier rule arrives as a parameter
//!
//! A step names a tool, and the rule for a valid tool identifier lives in `jarvis_tools` — which this crate
//! cannot depend on, because `jarvis-tools` depends on `jarvis-core` and the arrow only goes one way
//! (`docs/architecture/repository-layout.md`: an adapter may depend on core and **not on another adapter**).
//!
//! So every function that builds a [`SkillStep`] takes a `validate_tool` function. That keeps the rule in
//! **one** place — the crate that owns it — rather than restating its shape here as a second pattern that
//! could disagree. It also means a **decode re-applies the rule**, which is this crate's convention
//! (`MemoryRecord::from_stored` re-applies every rule it can), instead of trusting that the row was written
//! by a build that checked: a hand-edited or restored row is refused on read exactly as on write.
//!
//! # Why the JSON columns are decoded rather than left as text
//!
//! `steps` and `dropped_fields` are JSON because a step list is only meaningful inside its revision (the
//! migration records the reasoning). Decoding them here means the domain's bounds and cross-field rules are
//! re-applied on read, so the `CHECK` constraints are the first enforcer and a `SkillRevision` is the
//! second.

use jarvis_core::{
    DropReason, InvalidSkill, MemorySource, MemorySourceKind, MemoryTrust, Sensitivity,
    SkillDroppedField, SkillId, SkillRevision, SkillRevisionParts, SkillState, SkillStep,
    UtcTimestamp, WorkspaceId,
};
use serde::{Deserialize, Serialize};
use sqlx::Row;

use crate::database::{DatabaseError, SqliteDatabase};

/// The tool-identifier rule, supplied by the caller.
///
/// A `dyn Fn` rather than a generic parameter so the public signatures stay readable: nearly every function
/// here needs it, and a generic would appear in each. The callers are the daemon (which passes
/// `jarvis_tools::ToolId::new`) and this crate's tests (which pass a function mirroring the same rule).
///
/// # Why it is also `Send + Sync`
///
/// These functions are `async` and the daemon calls them from a **spawned task**, so the validator is held
/// across an `await` and must therefore be shareable across threads. A bare `dyn Fn` is not, and the
/// omission is invisible until a caller spawns — which is why the bound belongs in the alias rather than
/// being discovered as a call-site error.
pub type ToolValidator<'a> = &'a (dyn Fn(&str) -> bool + Send + Sync);

/// A step as it is stored.
///
/// A separate shape from [`SkillStep`] because the stored form is this crate's schema and the domain type is
/// the validated value — the same split `rest.rs` states for wire types. It is `pub` only to the extent that
/// `pub(crate)` would not compile in a `#[derive]`d shape used across functions in this module.
#[derive(Deserialize, Serialize)]
struct StoredStep {
    position: u16,
    tool: String,
    tool_version: String,
    instruction: String,
}

/// A dropped field as it is stored.
#[derive(Deserialize, Serialize)]
struct StoredDrop {
    name: String,
    reason: String,
}

/// Encodes the steps as the JSON array the column holds.
fn encode_steps(revision: &SkillRevision) -> Result<String, DatabaseError> {
    let steps: Vec<StoredStep> = revision
        .steps()
        .iter()
        .map(|step| StoredStep {
            position: step.position(),
            tool: step.tool().to_owned(),
            tool_version: step.tool_version().to_owned(),
            instruction: step.instruction().to_owned(),
        })
        .collect();
    serde_json::to_string(&steps).map_err(|_| DatabaseError::StoredSkillInvalid { field: "steps" })
}

/// Encodes the dropped fields as the JSON array the column holds.
fn encode_drops(revision: &SkillRevision) -> Result<String, DatabaseError> {
    let drops: Vec<StoredDrop> = revision
        .dropped_fields()
        .iter()
        .map(|dropped| StoredDrop {
            name: dropped.name().to_owned(),
            reason: dropped.reason().as_str().to_owned(),
        })
        .collect();
    serde_json::to_string(&drops).map_err(|_| DatabaseError::StoredSkillInvalid {
        field: "dropped_fields",
    })
}

/// Records a new skill revision.
///
/// # Errors
///
/// - [`DatabaseError::StoredSkillInvalid`] when the revision cannot be encoded, which is an authoring fault
///   rather than a caller one.
/// - [`DatabaseError::Sqlite`] when the workspace reference is missing, when the `(skill, version)` pair is
///   already stored, or when the schema's own constraints refuse the row.
pub async fn record_skill_revision(
    database: &SqliteDatabase,
    revision: &SkillRevision,
) -> Result<(), DatabaseError> {
    let steps = encode_steps(revision)?;
    let drops = encode_drops(revision)?;

    sqlx::query(
        "INSERT INTO skill_revisions (\
            id, skill_id, workspace_id, version, sensitivity, description, steps, source_kind, \
            source_locator, source_trust, source_excerpt_hash, state, dropped_fields, \
            promoted_by_actor_id, promoted_at, supersedes_revision_id, superseded_by_revision_id, \
            run_id, created_by_actor_id, correlation_id, created_at, updated_at, version_counter\
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, \
            ?18, ?19, ?20, ?21, ?22, 1)",
    )
    .bind(revision.revision_id().to_string())
    .bind(revision.skill_id().to_string())
    .bind(revision.workspace_id().to_string())
    .bind(revision.version())
    .bind(revision.sensitivity().as_str())
    .bind(revision.description())
    .bind(steps)
    .bind(revision.source().kind().as_str())
    .bind(revision.source().locator())
    .bind(revision.source().trust().as_str())
    .bind(revision.source().excerpt_hash())
    .bind(revision.state().as_str())
    .bind(drops)
    .bind(revision.promoted_by_actor_id())
    .bind(revision.promoted_at().map(|at| at.to_string()))
    .bind(revision.supersedes().map(|id| id.to_string()))
    .bind(revision.superseded_by().map(|id| id.to_string()))
    .bind(revision.run_id().map(|id| id.to_string()))
    .bind(revision.created_by_actor_id())
    .bind(revision.correlation_id().to_string())
    .bind(revision.created_at().to_string())
    .bind(revision.updated_at().to_string())
    .execute(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "record a skill revision",
        source,
    })?;
    Ok(())
}

/// A revision together with the optimistic-locking counter the row holds.
///
/// # Why this is a separate type rather than a field on [`SkillRevision`]
///
/// `version_counter` is not part of the **procedure** — it is the platform's guard against a lost update, and
/// it changes for reasons the procedure's content has nothing to do with (a promotion, an archive, a
/// supersession link). Putting it on the domain value would make it a field a caller could set, and the
/// control path's whole requirement is that the counter a write presents is one the **store** issued and the
/// caller merely observed.
///
/// # Why the retrieval reads do not return it
///
/// `P4-012`'s selection and the executor's context assembly read revisions to **offer** them, and a counter has
/// no meaning there — the selection is a pure function of the revision set. Widening those reads would put a
/// value with no reader on a hot path, which is the shape `ADR-0092` records. So the counter arrives through
/// this type, used by the control surface where the guard is actually presented.
#[derive(Clone, Debug)]
pub struct StoredSkillRevision {
    /// The decoded revision.
    revision: SkillRevision,
    /// The counter a control verb must present.
    version_counter: i64,
}

impl StoredSkillRevision {
    /// Returns the decoded revision.
    #[must_use]
    pub const fn revision(&self) -> &SkillRevision {
        &self.revision
    }

    /// Returns the counter a control verb must present.
    #[must_use]
    pub const fn version_counter(&self) -> i64 {
        self.version_counter
    }

    /// Consumes the wrapper and returns the revision.
    #[must_use]
    pub fn into_revision(self) -> SkillRevision {
        self.revision
    }
}

/// Reads one revision by its own identifier.
///
/// `validate_tool` is the rule a step's tool identifier is checked against — see the module documentation
/// for why it arrives as a parameter.
///
/// # Errors
///
/// Returns [`DatabaseError::SkillRevisionNotFound`] when no revision has that identifier, and
/// [`DatabaseError::StoredSkillInvalid`] when a row cannot be decoded or violates a rule on read.
pub async fn find_skill_revision(
    database: &SqliteDatabase,
    revision_id: &str,
    validate_tool: ToolValidator<'_>,
) -> Result<SkillRevision, DatabaseError> {
    Ok(
        find_skill_revision_state(database, revision_id, validate_tool)
            .await?
            .into_revision(),
    )
}

/// Reads one revision **with its optimistic-locking counter**, for the control surface.
///
/// The read every control verb begins from: a promotion, an archive, or a deletion must present the counter
/// the caller observed, and this is where it comes from. A missing counter would leave a client no way to
/// obtain the value it must send, which is the lost update the guard exists to prevent.
///
/// # Errors
///
/// Returns [`DatabaseError::SkillRevisionNotFound`] when no revision has that identifier, and
/// [`DatabaseError::StoredSkillInvalid`] when a row cannot be decoded.
pub async fn find_skill_revision_state(
    database: &SqliteDatabase,
    revision_id: &str,
    validate_tool: ToolValidator<'_>,
) -> Result<StoredSkillRevision, DatabaseError> {
    let row = sqlx::query(
        "SELECT id, skill_id, workspace_id, version, sensitivity, description, steps, source_kind, \
                source_locator, source_trust, source_excerpt_hash, state, dropped_fields, \
                promoted_by_actor_id, promoted_at, supersedes_revision_id, superseded_by_revision_id, \
                run_id, created_by_actor_id, correlation_id, created_at, updated_at, version_counter \
         FROM skill_revisions WHERE id = ?1",
    )
    .bind(revision_id)
    .fetch_optional(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "read a skill revision",
        source,
    })?;
    let row = row.ok_or(DatabaseError::SkillRevisionNotFound)?;
    let version_counter = row.try_get::<i64, _>("version_counter").map_err(|_| {
        DatabaseError::StoredSkillInvalid {
            field: "version_counter",
        }
    })?;
    Ok(StoredSkillRevision {
        revision: decode_revision(&row, validate_tool)?,
        version_counter,
    })
}

/// Reads a workspace's revisions **with their counters**, newest first, bounded by `limit`.
///
/// The listing and export read of the control surface. Ordered by `unixepoch(created_at) DESC, id DESC` for
/// the reason [`read_usable_skill_revisions`] records in full: the stored form is RFC 3339 text whose
/// fraction is omitted at a whole second, so byte order and time order disagree, and a tie-break must share
/// the primary key's direction or it inverts every tie.
///
/// # Errors
///
/// Returns [`DatabaseError::StoredSkillInvalid`] when any row cannot be decoded.
pub async fn read_workspace_skill_revision_states(
    database: &SqliteDatabase,
    workspace_id: &str,
    limit: u32,
    validate_tool: ToolValidator<'_>,
) -> Result<Vec<StoredSkillRevision>, DatabaseError> {
    read_skill_states(
        database,
        "SELECT id, skill_id, workspace_id, version, sensitivity, description, steps, source_kind, \
                source_locator, source_trust, source_excerpt_hash, state, dropped_fields, \
                promoted_by_actor_id, promoted_at, supersedes_revision_id, superseded_by_revision_id, \
                run_id, created_by_actor_id, correlation_id, created_at, updated_at, version_counter \
         FROM skill_revisions WHERE workspace_id = ?1 \
         ORDER BY unixepoch(created_at) DESC, id DESC LIMIT ?2",
        workspace_id,
        limit,
        validate_tool,
    )
    .await
}

/// The runner the counter-carrying reads share, so one statement's parameter positions stay in one place.
async fn read_skill_states(
    database: &SqliteDatabase,
    statement: &'static str,
    workspace_id: &str,
    limit: u32,
    validate_tool: ToolValidator<'_>,
) -> Result<Vec<StoredSkillRevision>, DatabaseError> {
    let rows = sqlx::query(statement)
        .bind(workspace_id)
        .bind(i64::from(limit))
        .fetch_all(database.pool())
        .await
        .map_err(|source| DatabaseError::Sqlite {
            operation: "read skill revisions with their counters",
            source,
        })?;
    rows.iter()
        .map(|row| {
            let version_counter = row.try_get::<i64, _>("version_counter").map_err(|_| {
                DatabaseError::StoredSkillInvalid {
                    field: "version_counter",
                }
            })?;
            Ok(StoredSkillRevision {
                revision: decode_revision(row, validate_tool)?,
                version_counter,
            })
        })
        .collect()
}

/// Reads the revisions of one skill, newest first.
///
/// The order is `unixepoch(created_at)`, not `created_at`, for the reason [`read_usable_skill_revisions`]
/// records: the stored form is RFC 3339 text whose fraction is **omitted when zero**, so byte order and time
/// order disagree within a second.
///
/// # Errors
///
/// Returns [`DatabaseError::StoredSkillInvalid`] when any row cannot be decoded, so one corrupt row is
/// reported rather than silently skipped — a skipped revision would be a procedure an operator cannot see.
pub async fn read_skill_revisions(
    database: &SqliteDatabase,
    workspace_id: &str,
    skill_id: &str,
    validate_tool: ToolValidator<'_>,
) -> Result<Vec<SkillRevision>, DatabaseError> {
    let rows = sqlx::query(
        "SELECT id, skill_id, workspace_id, version, sensitivity, description, steps, source_kind, \
                source_locator, source_trust, source_excerpt_hash, state, dropped_fields, \
                promoted_by_actor_id, promoted_at, supersedes_revision_id, superseded_by_revision_id, \
                run_id, created_by_actor_id, correlation_id, created_at, updated_at \
         FROM skill_revisions WHERE workspace_id = ?1 AND skill_id = ?2 \
         ORDER BY unixepoch(created_at) DESC, id DESC",
    )
    .bind(workspace_id)
    .bind(skill_id)
    .fetch_all(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "read a skill's revisions",
        source,
    })?;
    rows.iter()
        .map(|row| decode_revision(row, validate_tool))
        .collect()
}

/// Reads the **usable** revisions of a workspace: those that are `active`.
///
/// The surface `P4-012`'s selection reads. A proposal and an archived revision are deliberately absent
/// rather than flagged, for the reason `ToolRegistry::discover` gives about an unavailable tool: something a
/// caller cannot use is not a choice, and listing it invites a use that policy would refuse. An operator who
/// wants the whole picture reads [`read_workspace_skill_revisions`].
///
/// # Why `limit` is a parameter rather than the newest rows being read unbounded
///
/// A caller assembling a prompt runs this on **every model call**, so an unbounded read grows with the
/// user's procedure library and makes the cost of one request a function of their history. The bound is what
/// makes that cost a constant, and it is a **candidate window** rather than a result set: the selection rule
/// — not this read — decides which candidates are usable, so the window is deliberately larger than the
/// number of skills that may be offered.
///
/// # ⭐ Why the order is `unixepoch(created_at) DESC, id DESC` and not `created_at DESC`
///
/// Two defects, and the second was invisible until the first was fixed.
///
/// **1. Byte order and time order disagree.** `created_at` is RFC 3339 text and [`UtcTimestamp`] **omits the
/// fraction at a whole second**, so in byte order `'…:20Z'` sorts *after* `'…:20.5Z'` (`'Z'` is 0x5A, `'.'`
/// is 0x2E). A windowed `ORDER BY created_at DESC` therefore does **not** return the newest rows: this read's
/// own test saw it return the oldest and the newest of three and **drop the middle one**. This is the
/// lexicographic trap `ADR-0034` records, whose conclusion is that these timestamps must not be compared as
/// text in SQL. `unixepoch()` parses the string into seconds, so ties are compared as instants.
///
/// **2. ⭐ A DESC primary with an ASC tie-break orders a tie OLDEST-first.** `unixepoch` resolves to whole
/// seconds, so rows inside one second tie, and `id ASC` then put the **earliest** of them first — the exact
/// opposite of what a newest-first read is for. Adding the tie-break to make the order *total* introduced a
/// second ordering defect, and only a fixture with two rows in one second could see it. The tie-break
/// descends because **it is part of the same ordering**, not a separate concern: an ordering has one
/// direction, and mixing directions within one key inverts every tie.
///
/// # The sub-second limit, recorded rather than hidden
///
/// `unixepoch()` resolves to **whole seconds**, so two revisions created within one second tie on this key and
/// the `id` tie-break decides. These identifiers are `UUIDv7`, whose first 48 bits are a millisecond
/// timestamp, so `id DESC` orders two same-second rows by their creation **millisecond** — and a real write
/// gives the identifier and `created_at` the same instant, so the tie-break agrees with the row rather than
/// overriding it. Two revisions differing by less than a millisecond would be ordered by the id's random tail,
/// which is arbitrary; that is the residual imprecision, and genuine nanosecond ordering needs an **integer
/// nanoseconds column** — a migration across every timestamp column, which is `ADR-0034`'s decision rather
/// than something to slip into this read. What matters for a candidate window is that it keeps the newest rows
/// and drops the oldest; it does that.
///
/// # Errors
///
/// Returns [`DatabaseError::StoredSkillInvalid`] when any row cannot be decoded.
pub async fn read_usable_skill_revisions(
    database: &SqliteDatabase,
    workspace_id: &str,
    limit: u32,
    validate_tool: ToolValidator<'_>,
) -> Result<Vec<SkillRevision>, DatabaseError> {
    let rows = sqlx::query(
        "SELECT id, skill_id, workspace_id, version, sensitivity, description, steps, source_kind, \
                source_locator, source_trust, source_excerpt_hash, state, dropped_fields, \
                promoted_by_actor_id, promoted_at, supersedes_revision_id, superseded_by_revision_id, \
                run_id, created_by_actor_id, correlation_id, created_at, updated_at \
         FROM skill_revisions WHERE workspace_id = ?1 AND state = 'active' \
         ORDER BY unixepoch(created_at) DESC, id DESC LIMIT ?2",
    )
    .bind(workspace_id)
    .bind(i64::from(limit))
    .fetch_all(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "read a workspace's usable skill revisions",
        source,
    })?;
    rows.iter()
        .map(|row| decode_revision(row, validate_tool))
        .collect()
}

/// Reads **every** revision of a workspace, whatever its state.
///
/// The inspection surface (`P4-013`): an operator reviewing skills needs the proposals and the archived
/// ones, because "why is this not being used" is the question this view exists to answer.
///
/// # Errors
///
/// Returns [`DatabaseError::StoredSkillInvalid`] when any row cannot be decoded.
pub async fn read_workspace_skill_revisions(
    database: &SqliteDatabase,
    workspace_id: &str,
    validate_tool: ToolValidator<'_>,
) -> Result<Vec<SkillRevision>, DatabaseError> {
    let rows = sqlx::query(
        "SELECT id, skill_id, workspace_id, version, sensitivity, description, steps, source_kind, \
                source_locator, source_trust, source_excerpt_hash, state, dropped_fields, \
                promoted_by_actor_id, promoted_at, supersedes_revision_id, superseded_by_revision_id, \
                run_id, created_by_actor_id, correlation_id, created_at, updated_at \
         FROM skill_revisions WHERE workspace_id = ?1 ORDER BY unixepoch(created_at) DESC, id DESC",
    )
    .bind(workspace_id)
    .fetch_all(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "read a workspace's skill revisions",
        source,
    })?;
    rows.iter()
        .map(|row| decode_revision(row, validate_tool))
        .collect()
}

/// Promotes a proposal to active, recording who decided and the `version_counter`.
///
/// # Why the write is guarded by `state = 'proposed'`
///
/// The same shape [`crate::record_decision`] uses for an approval: the `WHERE` clause requires the state the
/// transition starts from, so a second promotion affects **zero rows** and is reported rather than silently
/// overwriting the approver a first decision named. `ADR-0043` requires a decision to be attributable, and
/// replacing an attribution loses that.
///
/// # Why the domain transition runs first
///
/// [`SkillRevision::promote`] is where the rule lives — a promotion names its approver and a blank one is
/// refused. Calling it before the write means the domain is the enforcer and the SQL guard is defence in
/// depth against concurrency, rather than a second copy of a rule that could disagree.
///
/// # Why `expected_version` is required
///
/// A promotion is an **attributable decision** (`ADR-0117` §4, `ADR-0043`), and a decision taken against a
/// revision the approver has not read is not one — the approver would be approving text that has since
/// changed. The counter is what makes that expressible, and it is required rather than optional because a
/// caller that has not read the revision has nothing to decide about.
///
/// # Errors
///
/// Returns [`DatabaseError::SkillRevisionNotFound`] when no revision has that identifier,
/// [`DatabaseError::SkillConflict`] when the counter does not match (a concurrent writer changed the row),
/// [`DatabaseError::SkillPromotionRefused`] when the revision is not a proposal, and
/// [`DatabaseError::StoredSkillInvalid`] when the approver is unusable.
pub async fn promote_skill_revision(
    database: &SqliteDatabase,
    revision_id: &str,
    expected_version: i64,
    approver_actor_id: &str,
    at: UtcTimestamp,
    validate_tool: ToolValidator<'_>,
) -> Result<SkillRevision, DatabaseError> {
    let current = find_skill_revision(database, revision_id, validate_tool).await?;
    let promoted =
        current
            .promote(approver_actor_id, at)
            .map_err(|error: InvalidSkill| match error {
                // **The two promotion refusals keep their own reasons**, because the remedies differ: an
                // unattributed promotion needs an approver named, while a self-approval needs a *different*
                // actor to decide. Collapsing them into one message would send an operator looking for a
                // missing field that is not missing.
                InvalidSkill::PromotionUnattributed => DatabaseError::SkillPromotionRefused {
                    reason: "no approver was named",
                },
                InvalidSkill::PromotionSelfApproval => DatabaseError::SkillPromotionRefused {
                    reason: "the approver is the revision's own author",
                },
                InvalidSkill::ApproverTooLong => DatabaseError::StoredSkillInvalid {
                    field: "approver_actor_id",
                },
                // ⭐ The state guard keeps its **own reason**, because the remedy is a transition rather than a
                // field: an `Active` revision needs no decision, and an `Archived` one returns through
                // `restore`. Reporting `StoredSkillInvalid` here would send an operator to inspect a row that
                // is perfectly well formed.
                InvalidSkill::WrongState { state, .. } => DatabaseError::SkillPromotionRefused {
                    reason: match state {
                        SkillState::Active => "the revision is already active",
                        SkillState::Archived => "the revision is archived; restore it instead",
                        // Unreachable: `promote` refuses everything that is not `Proposed`, so the state
                        // carried here is never a proposal. Spelled out rather than wildcarded so adding a
                        // state forces this arm to be reconsidered.
                        SkillState::Proposed => "the revision is not a proposal",
                    },
                },
                other => DatabaseError::StoredSkillInvalid {
                    field: match other {
                        InvalidSkill::ModelAuthoredTrust => "state",
                        _ => "promotion",
                    },
                },
            })?;

    let updated = sqlx::query(
        "UPDATE skill_revisions SET state = 'active', promoted_by_actor_id = ?1, promoted_at = ?2, \
            updated_at = ?2, version_counter = version_counter + 1 \
         WHERE id = ?3 AND state = 'proposed' AND version_counter = ?4",
    )
    .bind(promoted.promoted_by_actor_id())
    .bind(at.to_string())
    .bind(revision_id)
    .bind(expected_version)
    .execute(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "promote a skill revision",
        source,
    })?;

    // Zero rows has two causes, and the caller has to be told which: a stale counter is a re-read and a
    // retry, while a revision that is no longer a proposal is not. Resolved by a read rather than by
    // guessing, which is the rule the run repository's guarded writes follow.
    if updated.rows_affected() == 0 {
        return Err(classify_skill_write_failure(database, revision_id, expected_version).await);
    }
    Ok(promoted)
}

/// Resolves why a guarded skill write affected no rows.
///
/// The two causes have different remedies, so collapsing them into one error sends an operator to the wrong
/// place: `SkillConflict` means the row **exists and changed**, so the answer is to re-read it, while
/// `SkillRevisionNotFound` means the caller named something that is not there. `SkillTransitionRefused`
/// covers the third case — the row is present, unchanged, and in a state that does not accept the transition
/// — which is a fact about the revision rather than about a race.
async fn classify_skill_write_failure(
    database: &SqliteDatabase,
    revision_id: &str,
    expected_version: i64,
) -> DatabaseError {
    // The validator is permissive here on purpose: this function asks about a **counter**, and a row whose
    // steps fail to decode still answers that question. A stricter validator would report every such write as
    // a conflict rather than as the decoding problem it is, which is the defect this avoids.
    match find_skill_revision_state(database, revision_id, &|_| true).await {
        Ok(stored) if stored.version_counter() != expected_version => DatabaseError::SkillConflict,
        // Present, unchanged, and refusing the transition: the state guard is the reason, and the caller's
        // own verb reports which transition it wanted.
        Ok(_) => DatabaseError::SkillTransitionRefused {
            reason: "the revision is not in the state this transition requires",
        },
        Err(error) => error,
    }
}

/// Sets a revision aside, retaining it for audit.
///
/// # Errors
///
/// Returns [`DatabaseError::SkillRevisionNotFound`], [`DatabaseError::SkillConflict`] when the counter does
/// not match, or [`DatabaseError::SkillTransitionRefused`] when the revision is already archived — a redundant
/// transition is refused rather than reported as success.
pub async fn archive_skill_revision(
    database: &SqliteDatabase,
    revision_id: &str,
    expected_version: i64,
    at: UtcTimestamp,
    validate_tool: ToolValidator<'_>,
) -> Result<SkillRevision, DatabaseError> {
    let current = find_skill_revision(database, revision_id, validate_tool).await?;
    let archived = current
        .archive(at)
        .map_err(|_| DatabaseError::SkillTransitionRefused {
            reason: "the revision is already archived",
        })?;
    write_state(database, revision_id, "archived", expected_version, at).await?;
    Ok(archived)
}

/// Restores an archived revision to the state its promotion record implies.
///
/// # Errors
///
/// Returns [`DatabaseError::SkillRevisionNotFound`], [`DatabaseError::SkillConflict`] when the counter does
/// not match, or [`DatabaseError::SkillTransitionRefused`] when the revision is not archived.
pub async fn restore_skill_revision(
    database: &SqliteDatabase,
    revision_id: &str,
    expected_version: i64,
    at: UtcTimestamp,
    validate_tool: ToolValidator<'_>,
) -> Result<SkillRevision, DatabaseError> {
    let current = find_skill_revision(database, revision_id, validate_tool).await?;
    let restored = current
        .restore(at)
        .map_err(|_| DatabaseError::SkillTransitionRefused {
            reason: "the revision is not archived",
        })?;
    write_state(
        database,
        revision_id,
        restored.state().as_str(),
        expected_version,
        at,
    )
    .await?;
    Ok(restored)
}

/// Writes one revision's state and the counter, guarded by the state it must be leaving.
///
/// Shared by archive and restore, which differ only in the target state. The guard is the same one
/// `promote_skill_revision` uses: the **state** the transition starts from, so a concurrent writer that got
/// there first is refused, plus the **counter** the caller observed, so a writer that changed something else
/// about the row is refused too. Both are in the `WHERE`, so the decision and the write cannot disagree.
async fn write_state(
    database: &SqliteDatabase,
    revision_id: &str,
    state: &str,
    expected_version: i64,
    at: UtcTimestamp,
) -> Result<(), DatabaseError> {
    let updated = sqlx::query(
        "UPDATE skill_revisions SET state = ?1, updated_at = ?2, version_counter = version_counter + 1 \
         WHERE id = ?3 AND version_counter = ?4",
    )
    .bind(state)
    .bind(at.to_string())
    .bind(revision_id)
    .bind(expected_version)
    .execute(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "write a skill revision's state",
        source,
    })?;
    if updated.rows_affected() == 0 {
        return Err(classify_skill_write_failure(database, revision_id, expected_version).await);
    }
    Ok(())
}

/// Records that one revision replaced another, writing **both** legs of the declared pair.
///
/// `ADR-0117` §5 and `ADR-0045`: replacement is declared, never inferred. The successor already names what
/// it replaced through its `supersedes` at insert time; this writes the forward direction so a read of the
/// predecessor finds its replacement without scanning every later revision.
///
/// # ⭐ Both ends must belong to the SAME skill, and this is the only layer that can say so
///
/// A supersession is a statement about **one procedure's history**, so a chain that leaves its own procedure
/// is not a longer chain — it is a wrong answer to "what replaced this". The rule needs **both rows**, which is
/// why it cannot live in [`SkillRevision`]: the domain holds one revision and has no way to ask whether a
/// given `SkillId` names a revision of the same procedure. Its own test records that limit explicitly
/// (`the_domain_cannot_vouch_for_a_predecessor_and_says_so`).
///
/// The existing successor check is what made this the **missing** half rather than an absent one:
/// `acknowledge_skill_revision` confirms the successor **exists**, and nothing confirmed it was **related**.
/// A dangling pointer is refused; a pointer to another procedure is accepted and looks identical from
/// either end.
///
/// # Errors
///
/// Returns [`DatabaseError::SkillRevisionNotFound`] when either revision is missing — **both** are checked,
/// because a dangling `superseded_by` would make the chain unwalkable and the failure would only surface
/// when something tried to follow it — and [`DatabaseError::SkillTransitionRefused`] when the two revisions
/// belong to different skills, or when they are the same revision.
pub async fn supersede_skill_revision(
    database: &SqliteDatabase,
    replaced_revision_id: &str,
    successor_revision_id: &str,
) -> Result<(), DatabaseError> {
    if replaced_revision_id == successor_revision_id {
        return Err(DatabaseError::SkillTransitionRefused {
            reason: "a revision cannot be replaced by itself",
        });
    }
    // The successor is checked first, so a missing successor does not leave the predecessor already
    // pointing at a row that does not exist.
    acknowledge_skill_revision(database, successor_revision_id).await?;

    // **The relationship, read as one statement.** Both rows must share a `skill_id`, and this is a single
    // query rather than a read of each and a comparison in Rust: the rule is a property of the **pair**, so
    // expressing it as two round trips would leave a window in which either row could be written, and the
    // comparison would decide something the database no longer agrees with.
    //
    // The count is **2** — both rows, not one. The predicate selects the rows whose skill is the
    // *predecessor's*, so a successor from another procedure is simply absent from that count. Comparing
    // against 1 would accept exactly the cross-procedure pair this exists to refuse: the predecessor always
    // matches its own skill, so a count of at least 1 is guaranteed and decides nothing.
    let related = sqlx::query(
        "SELECT COUNT(*) AS matching FROM skill_revisions \
         WHERE id IN (?1, ?2) AND skill_id = (SELECT skill_id FROM skill_revisions WHERE id = ?1)",
    )
    .bind(replaced_revision_id)
    .bind(successor_revision_id)
    .fetch_one(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "check a skill supersession relates one procedure",
        source,
    })?;
    let matching: i64 = related
        .try_get("matching")
        .map_err(|_| DatabaseError::Sqlite {
            operation: "check a skill supersession relates one procedure",
            source: sqlx::Error::RowNotFound,
        })?;
    if matching != 2 {
        return Err(DatabaseError::SkillTransitionRefused {
            reason: "the two revisions belong to different procedures, so the chain would leave its skill",
        });
    }

    let updated = sqlx::query(
        "UPDATE skill_revisions SET superseded_by_revision_id = ?1, version_counter = version_counter + 1 \
         WHERE id = ?2",
    )
    .bind(successor_revision_id)
    .bind(replaced_revision_id)
    .execute(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "record a skill supersession",
        source,
    })?;
    if updated.rows_affected() == 0 {
        return Err(DatabaseError::SkillRevisionNotFound);
    }
    Ok(())
}

/// Confirms a revision exists without decoding it.
///
/// Decoding would repeat validation the caller already performed, and the identifier read back is the proof
/// the row exists — the shape [`crate::approval_repository`]'s own acknowledgement uses.
async fn acknowledge_skill_revision(
    database: &SqliteDatabase,
    revision_id: &str,
) -> Result<(), DatabaseError> {
    let row = sqlx::query("SELECT id FROM skill_revisions WHERE id = ?1")
        .bind(revision_id)
        .fetch_optional(database.pool())
        .await
        .map_err(|source| DatabaseError::Sqlite {
            operation: "confirm a skill revision exists",
            source,
        })?;
    row.ok_or(DatabaseError::SkillRevisionNotFound).map(|_| ())
}

/// Deletes one revision outright, guarded by the counter the caller observed.
///
/// # Why a skill's deletion is final, unlike a memory's
///
/// A memory's deletion writes a **tombstone** so a later automatic ingest cannot resurrect the claim. A skill
/// has no ingest: it is written by an explicit request, so there is no process the user did not run that could
/// bring it back. A tombstone here would be a row nothing consults — the "value with a producer and no reader"
/// shape this repository removes wherever it finds it — so the row is deleted and the receipt says so.
///
/// # Why the guard is on the counter rather than on a state
///
/// `archive` is the reversible verb and `forget` is the final one, so a deletion is allowed from **any** state:
/// a proposal nobody wants, an active procedure being removed, and an archived revision being discarded are
/// all legitimate. What is not legitimate is deleting a revision the caller has not read, which is what the
/// counter guards — the same rule a correction follows, and the reason it is required rather than optional.
///
/// # Errors
///
/// Returns [`DatabaseError::SkillRevisionNotFound`] when no revision has that identifier,
/// [`DatabaseError::SkillConflict`] when the counter does not match, and
/// [`DatabaseError::SkillTransitionRefused`] when the revision cannot be removed without breaking a chain it
/// is part of.
pub async fn delete_skill_revision(
    database: &SqliteDatabase,
    revision_id: &str,
    expected_version: i64,
) -> Result<(), DatabaseError> {
    // ⭐ **A revision another revision points at cannot simply vanish.** Both supersession columns are
    // `REFERENCES ... ON DELETE SET NULL`, so the foreign keys would let the delete succeed and quietly blank
    // the surviving row's link — leaving a successor that declares nothing about what it replaced, or a
    // predecessor with no way to find its replacement. `ADR-0117` §5's rule is that replacement is
    // **declared**, so erasing one leg of a declaration is worse than refusing the deletion: the caller gets a
    // reason and can delete the pair deliberately.
    //
    // Read in the same statement as the guard so the decision and the delete cannot disagree about what
    // existed.
    let blocked = sqlx::query(
        "SELECT COUNT(*) AS linked FROM skill_revisions \
         WHERE (supersedes_revision_id = ?1 OR superseded_by_revision_id = ?1) AND id <> ?1",
    )
    .bind(revision_id)
    .fetch_one(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "check a skill revision's supersession links",
        source,
    })?;
    let linked: i64 = blocked
        .try_get("linked")
        .map_err(|_| DatabaseError::StoredSkillInvalid {
            field: "supersession_links",
        })?;
    if linked > 0 {
        return Err(DatabaseError::SkillTransitionRefused {
            reason: "another revision declares a supersession with this one, so deleting it would erase one leg of that declaration",
        });
    }

    let deleted = sqlx::query("DELETE FROM skill_revisions WHERE id = ?1 AND version_counter = ?2")
        .bind(revision_id)
        .bind(expected_version)
        .execute(database.pool())
        .await
        .map_err(|source| DatabaseError::Sqlite {
            operation: "delete a skill revision",
            source,
        })?;

    if deleted.rows_affected() == 1 {
        return Ok(());
    }
    // Zero rows has two causes and the caller must be told which: a stale counter is a retry, while a missing
    // row is not. Resolved by a read rather than by guessing, which is the rule the run repository's guarded
    // writes follow — a caller that lost a race must re-read rather than conclude deletion.
    match find_skill_revision_state(database, revision_id, &|_| true).await {
        Ok(_) => Err(DatabaseError::SkillConflict),
        Err(DatabaseError::SkillRevisionNotFound) => Err(DatabaseError::SkillRevisionNotFound),
        Err(other) => Err(other),
    }
}

/// Decodes one stored row, **re-applying every rule the domain can state**.
///
/// The `CHECK` constraints are the first enforcer and this is the second, so a row that arrived some other
/// way — another build, a restored backup, a hand edit — cannot decode into a value the domain forbids. That
/// is the reason this goes through [`SkillRevision`]'s parts rather than building fields directly.
fn decode_revision(
    row: &sqlx::sqlite::SqliteRow,
    validate_tool: ToolValidator<'_>,
) -> Result<SkillRevision, DatabaseError> {
    let text = |field: &'static str| -> Result<String, DatabaseError> {
        row.try_get::<String, _>(field)
            .map_err(|_| DatabaseError::StoredSkillInvalid { field })
    };

    let steps_json = text("steps")?;
    let steps = decode_steps(&steps_json, validate_tool)?;

    let drops_json = text("dropped_fields")?;
    let dropped_fields = decode_dropped_fields(&drops_json)?;

    let source = decode_source(row)?;

    let sensitivity: Sensitivity =
        text("sensitivity")?
            .parse()
            .map_err(|_| DatabaseError::StoredSkillInvalid {
                field: "sensitivity",
            })?;

    let state: SkillState = text("state")?
        .parse()
        .map_err(|_| DatabaseError::StoredSkillInvalid { field: "state" })?;
    // `from_stored` rather than `new`: a promoted model-authored revision legitimately decodes as active,
    // and the schema's "an active model-authored row names its approver" constraint is what refuses a row
    // that reached that state some other way — the one rule a decode cannot state itself.
    let revision = SkillRevision::from_stored(SkillRevisionParts {
        skill_id: parse_id(&text("skill_id")?)?,
        workspace_id: parse_workspace_id(&text("workspace_id")?)?,
        revision_id: parse_id(&text("id")?)?,
        version: text("version")?,
        description: text("description")?,
        steps,
        source,
        sensitivity,
        state,
        supersedes: row
            .try_get::<Option<String>, _>("supersedes_revision_id")
            .map_err(|_| DatabaseError::StoredSkillInvalid {
                field: "supersedes_revision_id",
            })?
            .map(|value| parse_id(&value))
            .transpose()?,
        dropped_fields,
        run_id: row
            .try_get::<Option<String>, _>("run_id")
            .map_err(|_| DatabaseError::StoredSkillInvalid { field: "run_id" })?
            .map(|value| {
                value
                    .parse()
                    .map_err(|_| DatabaseError::StoredSkillInvalid { field: "run_id" })
            })
            .transpose()?,
        created_by_actor_id: text("created_by_actor_id")?,
        correlation_id: text("correlation_id")?.parse().map_err(|_| {
            DatabaseError::StoredSkillInvalid {
                field: "correlation_id",
            }
        })?,
        created_at: parse_instant(&text("created_at")?)?,
    })
    // A decode passes the promotion requirement through, because an active model-authored revision is
    // exactly what a promotion produces — the domain refuses *constructing* one, and the schema's own
    // constraint is what refuses an active model-authored row that names no approver.
    .map_err(|error| DatabaseError::StoredSkillInvalid {
        field: match error {
            InvalidSkill::ModelAuthoredTrust => "state",
            InvalidSkill::Steps => "steps",
            InvalidSkill::Description => "description",
            _ => "skill",
        },
    })?;

    // The optional stored state is applied after construction: `promoted_by` and `promoted_at` are not part
    // of the declared parts, because a caller does not supply them — a promotion does. Applying them here is
    // what makes a read round-trip the attribution.
    let promoted_by = row
        .try_get::<Option<String>, _>("promoted_by_actor_id")
        .map_err(|_| DatabaseError::StoredSkillInvalid {
            field: "promoted_by_actor_id",
        })?;
    let promoted_at = row
        .try_get::<Option<String>, _>("promoted_at")
        .map_err(|_| DatabaseError::StoredSkillInvalid {
            field: "promoted_at",
        })?
        .map(|value| parse_instant(&value))
        .transpose()?;
    let superseded_by = row
        .try_get::<Option<String>, _>("superseded_by_revision_id")
        .map_err(|_| DatabaseError::StoredSkillInvalid {
            field: "superseded_by_revision_id",
        })?
        .map(|value| parse_id(&value))
        .transpose()?;
    let updated_at = parse_instant(&text("updated_at")?)?;

    Ok(revision.with_stored_lifecycle(promoted_by, promoted_at, superseded_by, updated_at))
}

/// Decodes the stored step array, **re-applying the tool-identifier rule**.
///
/// The rule arrives as a parameter (see the module documentation), so a row written by a build that used a
/// different rule is refused on read rather than loaded.
fn decode_steps(
    json: &str,
    validate_tool: ToolValidator<'_>,
) -> Result<Vec<SkillStep>, DatabaseError> {
    let stored: Vec<StoredStep> = serde_json::from_str(json)
        .map_err(|_| DatabaseError::StoredSkillInvalid { field: "steps" })?;
    stored
        .into_iter()
        .map(|step| {
            SkillStep::new(
                step.position,
                step.tool,
                step.tool_version,
                step.instruction,
                validate_tool,
            )
            .map_err(|_| DatabaseError::StoredSkillInvalid { field: "steps" })
        })
        .collect()
}

/// Decodes the stored dropped-field array, re-applying the name bound and the reason vocabulary.
fn decode_dropped_fields(json: &str) -> Result<Vec<SkillDroppedField>, DatabaseError> {
    let stored: Vec<StoredDrop> =
        serde_json::from_str(json).map_err(|_| DatabaseError::StoredSkillInvalid {
            field: "dropped_fields",
        })?;
    stored
        .into_iter()
        .map(|dropped| {
            let reason: DropReason =
                dropped
                    .reason
                    .parse()
                    .map_err(|_| DatabaseError::StoredSkillInvalid {
                        field: "dropped_fields",
                    })?;
            SkillDroppedField::new(dropped.name, reason).map_err(|_| {
                DatabaseError::StoredSkillInvalid {
                    field: "dropped_fields",
                }
            })
        })
        .collect()
}

/// Decodes the provenance columns, **re-applying the kind/trust equality**.
///
/// The same rule the memories table carries and the schema restates: external content cannot be
/// authoritative, and a model inference is derived. Applied here as well, so a row that arrived some other
/// way cannot decode into a source the domain forbids.
fn decode_source(row: &sqlx::sqlite::SqliteRow) -> Result<MemorySource, DatabaseError> {
    let kind: MemorySourceKind = row
        .try_get::<String, _>("source_kind")
        .map_err(|_| DatabaseError::StoredSkillInvalid {
            field: "source_kind",
        })?
        .parse()
        .map_err(|_| DatabaseError::StoredSkillInvalid {
            field: "source_kind",
        })?;
    let trust: MemoryTrust = row
        .try_get::<String, _>("source_trust")
        .map_err(|_| DatabaseError::StoredSkillInvalid {
            field: "source_trust",
        })?
        .parse()
        .map_err(|_| DatabaseError::StoredSkillInvalid {
            field: "source_trust",
        })?;
    MemorySource::new(
        kind,
        row.try_get::<String, _>("source_locator").map_err(|_| {
            DatabaseError::StoredSkillInvalid {
                field: "source_locator",
            }
        })?,
        trust,
        row.try_get::<Option<String>, _>("source_excerpt_hash")
            .map_err(|_| DatabaseError::StoredSkillInvalid {
                field: "source_excerpt_hash",
            })?,
    )
    .map_err(|_| DatabaseError::StoredSkillInvalid {
        field: "source_kind",
    })
}

fn parse_id(value: &str) -> Result<SkillId, DatabaseError> {
    value
        .parse()
        .map_err(|_| DatabaseError::StoredSkillInvalid { field: "id" })
}

fn parse_workspace_id(value: &str) -> Result<WorkspaceId, DatabaseError> {
    value
        .parse()
        .map_err(|_| DatabaseError::StoredSkillInvalid {
            field: "workspace_id",
        })
}

fn parse_instant(value: &str) -> Result<UtcTimestamp, DatabaseError> {
    value
        .parse()
        .map_err(|_| DatabaseError::StoredSkillInvalid {
            field: "created_at",
        })
}

#[cfg(test)]
mod tests;
