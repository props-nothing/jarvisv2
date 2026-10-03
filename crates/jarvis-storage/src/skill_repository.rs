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
    DropReason, InvalidSkill, MemorySource, MemorySourceKind, MemoryTrust, SkillDroppedField,
    SkillId, SkillRevision, SkillRevisionParts, SkillState, SkillStep, UtcTimestamp, WorkspaceId,
};
use serde::{Deserialize, Serialize};
use sqlx::Row;

use crate::database::{DatabaseError, SqliteDatabase};

/// The tool-identifier rule, supplied by the caller.
///
/// A `dyn Fn` rather than a generic parameter so the public signatures stay readable: nearly every function
/// here needs it, and a generic would appear in each. The callers are the daemon (which passes
/// `jarvis_tools::ToolId::new`) and this crate's tests (which pass a function mirroring the same rule).
pub type ToolValidator<'a> = &'a dyn Fn(&str) -> bool;

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
            id, skill_id, workspace_id, version, description, steps, source_kind, source_locator, \
            source_trust, source_excerpt_hash, state, dropped_fields, promoted_by_actor_id, \
            promoted_at, supersedes_revision_id, superseded_by_revision_id, run_id, \
            created_by_actor_id, correlation_id, created_at, updated_at, version_counter\
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, \
            ?19, ?20, ?21, 1)",
    )
    .bind(revision.revision_id().to_string())
    .bind(revision.skill_id().to_string())
    .bind(revision.workspace_id().to_string())
    .bind(revision.version())
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
    let row = sqlx::query(
        "SELECT id, skill_id, workspace_id, version, description, steps, source_kind, \
                source_locator, source_trust, source_excerpt_hash, state, dropped_fields, \
                promoted_by_actor_id, promoted_at, supersedes_revision_id, superseded_by_revision_id, \
                run_id, created_by_actor_id, correlation_id, created_at, updated_at \
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
    decode_revision(&row, validate_tool)
}

/// Reads the revisions of one skill, newest first.
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
        "SELECT id, skill_id, workspace_id, version, description, steps, source_kind, \
                source_locator, source_trust, source_excerpt_hash, state, dropped_fields, \
                promoted_by_actor_id, promoted_at, supersedes_revision_id, superseded_by_revision_id, \
                run_id, created_by_actor_id, correlation_id, created_at, updated_at \
         FROM skill_revisions WHERE workspace_id = ?1 AND skill_id = ?2 ORDER BY created_at DESC",
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
/// # Errors
///
/// Returns [`DatabaseError::StoredSkillInvalid`] when any row cannot be decoded.
pub async fn read_usable_skill_revisions(
    database: &SqliteDatabase,
    workspace_id: &str,
    validate_tool: ToolValidator<'_>,
) -> Result<Vec<SkillRevision>, DatabaseError> {
    let rows = sqlx::query(
        "SELECT id, skill_id, workspace_id, version, description, steps, source_kind, \
                source_locator, source_trust, source_excerpt_hash, state, dropped_fields, \
                promoted_by_actor_id, promoted_at, supersedes_revision_id, superseded_by_revision_id, \
                run_id, created_by_actor_id, correlation_id, created_at, updated_at \
         FROM skill_revisions WHERE workspace_id = ?1 AND state = 'active' \
         ORDER BY created_at DESC",
    )
    .bind(workspace_id)
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
        "SELECT id, skill_id, workspace_id, version, description, steps, source_kind, \
                source_locator, source_trust, source_excerpt_hash, state, dropped_fields, \
                promoted_by_actor_id, promoted_at, supersedes_revision_id, superseded_by_revision_id, \
                run_id, created_by_actor_id, correlation_id, created_at, updated_at \
         FROM skill_revisions WHERE workspace_id = ?1 ORDER BY created_at DESC",
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
/// # Errors
///
/// Returns [`DatabaseError::SkillRevisionNotFound`] when no revision has that identifier,
/// [`DatabaseError::SkillPromotionRefused`] when the revision is not a proposal (already active, archived, or
/// promoted by a concurrent writer), and [`DatabaseError::StoredSkillInvalid`] when the approver is unusable.
pub async fn promote_skill_revision(
    database: &SqliteDatabase,
    revision_id: &str,
    approver_actor_id: &str,
    at: UtcTimestamp,
    validate_tool: ToolValidator<'_>,
) -> Result<SkillRevision, DatabaseError> {
    let current = find_skill_revision(database, revision_id, validate_tool).await?;
    let promoted =
        current
            .promote(approver_actor_id, at)
            .map_err(|error: InvalidSkill| match error {
                InvalidSkill::SelfReference => DatabaseError::SkillPromotionRefused {
                    reason: "the approver was empty",
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
         WHERE id = ?3 AND state = 'proposed'",
    )
    .bind(promoted.promoted_by_actor_id())
    .bind(at.to_string())
    .bind(revision_id)
    .execute(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "promote a skill revision",
        source,
    })?;

    // Zero rows means the guard refused it: the row is no longer a proposal, which is either a second
    // promotion or a concurrent writer that got there first. Both are reported rather than treated as
    // success, because the caller asked for a transition that did not happen.
    if updated.rows_affected() == 0 {
        return Err(DatabaseError::SkillPromotionRefused {
            reason: "the revision is not a proposal, so it was not promoted",
        });
    }
    Ok(promoted)
}

/// Sets a revision aside, retaining it for audit.
///
/// # Errors
///
/// Returns [`DatabaseError::SkillRevisionNotFound`], or [`DatabaseError::SkillTransitionRefused`] when the
/// revision is already archived — a redundant transition is refused rather than reported as success.
pub async fn archive_skill_revision(
    database: &SqliteDatabase,
    revision_id: &str,
    at: UtcTimestamp,
    validate_tool: ToolValidator<'_>,
) -> Result<SkillRevision, DatabaseError> {
    let current = find_skill_revision(database, revision_id, validate_tool).await?;
    let archived = current
        .archive(at)
        .map_err(|_| DatabaseError::SkillTransitionRefused {
            reason: "the revision is already archived",
        })?;
    write_state(database, revision_id, "archived", at).await?;
    Ok(archived)
}

/// Restores an archived revision to the state its promotion record implies.
///
/// # Errors
///
/// Returns [`DatabaseError::SkillRevisionNotFound`], or [`DatabaseError::SkillTransitionRefused`] when the
/// revision is not archived.
pub async fn restore_skill_revision(
    database: &SqliteDatabase,
    revision_id: &str,
    at: UtcTimestamp,
    validate_tool: ToolValidator<'_>,
) -> Result<SkillRevision, DatabaseError> {
    let current = find_skill_revision(database, revision_id, validate_tool).await?;
    let restored = current
        .restore(at)
        .map_err(|_| DatabaseError::SkillTransitionRefused {
            reason: "the revision is not archived",
        })?;
    write_state(database, revision_id, restored.state().as_str(), at).await?;
    Ok(restored)
}

/// Writes one revision's state and the counter, guarded by the state it must be leaving.
///
/// Shared by archive and restore, which differ only in the target state and the guard. The guard is passed
/// as the state the transition starts from, so the `WHERE` refuses the same races `promote_skill_revision`'s
/// does.
async fn write_state(
    database: &SqliteDatabase,
    revision_id: &str,
    state: &str,
    at: UtcTimestamp,
) -> Result<(), DatabaseError> {
    let updated = sqlx::query(
        "UPDATE skill_revisions SET state = ?1, updated_at = ?2, version_counter = version_counter + 1 \
         WHERE id = ?3",
    )
    .bind(state)
    .bind(at.to_string())
    .bind(revision_id)
    .execute(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "write a skill revision's state",
        source,
    })?;
    if updated.rows_affected() == 0 {
        return Err(DatabaseError::SkillRevisionNotFound);
    }
    Ok(())
}

/// Records that one revision replaced another, writing **both** legs of the declared pair.
///
/// `ADR-0117` §5 and `ADR-0045`: replacement is declared, never inferred. The successor already names what
/// it replaced through its `supersedes` at insert time; this writes the forward direction so a read of the
/// predecessor finds its replacement without scanning every later revision.
///
/// # Errors
///
/// Returns [`DatabaseError::SkillRevisionNotFound`] when either revision is missing — **both** are checked,
/// because a dangling `superseded_by` would make the chain unwalkable and the failure would only surface
/// when something tried to follow it.
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
