//! Projects: the goal, standing guidance, folder, status and journal of long-running work (`ADR-0151`).
//!
//! A project changes what a run is **told**, never what it may do. Everything here is workspace-scoped: a project of another workspace is
//! indistinguishable from one that does not exist.

use jarvis_core::{ProjectId, ProjectNoteId, UtcTimestamp};
use sqlx::Row;

use crate::database::{DatabaseError, SqliteDatabase};

/// How far along a project is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectStatus {
    /// Being worked on; its schedules fire.
    Active,
    /// Set aside; its schedules do not fire, and the owner can still chat in it.
    Paused,
    /// Finished.
    Done,
}

impl ProjectStatus {
    /// The stable stored and wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Done => "done",
        }
    }

    /// Parses a stored or supplied name.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "active" => Some(Self::Active),
            "paused" => Some(Self::Paused),
            "done" => Some(Self::Done),
            _ => None,
        }
    }
}

/// What kind of journal entry a note is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NoteKind {
    /// Something was done.
    Progress,
    /// A choice was made, and why.
    Decision,
    /// Something stops the work and needs the owner or time.
    Blocker,
    /// What should happen next.
    Next,
    /// A finished deliverable or finding.
    Result,
    /// Written by the owner.
    Owner,
}

impl NoteKind {
    /// The stable stored and wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Progress => "progress",
            Self::Decision => "decision",
            Self::Blocker => "blocker",
            Self::Next => "next",
            Self::Result => "result",
            Self::Owner => "owner",
        }
    }

    /// Parses a stored or supplied name.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "progress" => Some(Self::Progress),
            "decision" => Some(Self::Decision),
            "blocker" => Some(Self::Blocker),
            "next" => Some(Self::Next),
            "result" => Some(Self::Result),
            "owner" => Some(Self::Owner),
            _ => None,
        }
    }
}

/// What a session or schedule can belong to a project as.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LinkKind {
    /// A conversation.
    Session,
    /// A scheduled task.
    Schedule,
}

impl LinkKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Schedule => "schedule",
        }
    }
}

/// One stored project.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredProject {
    /// The identifier.
    pub id: String,
    /// The owning workspace.
    pub workspace_id: String,
    /// The project's name, unique in the workspace.
    pub name: String,
    /// What done looks like.
    pub goal: String,
    /// Standing instructions for every run of the project.
    pub guidance: String,
    /// The working folder, relative to a granted root (empty means the root).
    pub folder: String,
    /// How far along it is.
    pub status: ProjectStatus,
    /// When it was made.
    pub created_at: String,
    /// When it was last changed.
    pub updated_at: String,
}

/// One journal entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredProjectNote {
    /// The identifier.
    pub id: String,
    /// The project it belongs to.
    pub project_id: String,
    /// What kind of entry it is.
    pub kind: NoteKind,
    /// The text.
    pub body: String,
    /// The run that wrote it, when a run did.
    pub run_id: Option<String>,
    /// When it was written.
    pub created_at: String,
}

/// The fields of a new project.
#[derive(Clone, Debug, Default)]
pub struct NewProject {
    /// The name.
    pub name: String,
    /// What done looks like.
    pub goal: String,
    /// Standing instructions.
    pub guidance: String,
    /// The working folder, relative to a granted root.
    pub folder: String,
}

/// The fields of a project that may change; `None` leaves a field as it is.
#[derive(Clone, Debug, Default)]
pub struct ProjectChanges {
    /// A new name.
    pub name: Option<String>,
    /// A new goal.
    pub goal: Option<String>,
    /// New standing instructions.
    pub guidance: Option<String>,
    /// A new working folder.
    pub folder: Option<String>,
    /// A new status.
    pub status: Option<ProjectStatus>,
}

const MAX_NAME_CHARS: usize = 80;
const MAX_GOAL_CHARS: usize = 4000;
const MAX_GUIDANCE_CHARS: usize = 12_000;
const MAX_FOLDER_CHARS: usize = 512;
/// The longest journal entry.
pub const MAX_NOTE_CHARS: usize = 4000;

fn invalid(field: &'static str) -> DatabaseError {
    DatabaseError::InvalidProject { field }
}

fn check_name(name: &str) -> Result<String, DatabaseError> {
    let name = name.trim();
    if name.is_empty()
        || name.chars().count() > MAX_NAME_CHARS
        || name.chars().any(char::is_control)
    {
        return Err(invalid("name"));
    }
    Ok(name.to_owned())
}

fn check_text(text: &str, limit: usize, field: &'static str) -> Result<String, DatabaseError> {
    let text = text.trim();
    if text.chars().count() > limit || text.contains('\0') {
        return Err(invalid(field));
    }
    Ok(text.to_owned())
}

/// A folder is relative and stays beneath the granted root: no drive, no leading separator, no `..`.
fn check_folder(folder: &str) -> Result<String, DatabaseError> {
    let folder = folder.trim().trim_end_matches(['/', '\\']);
    let bad = folder.chars().count() > MAX_FOLDER_CHARS
        || folder.chars().any(char::is_control)
        || folder.starts_with(['/', '\\'])
        || folder.contains(':')
        || folder.split(['/', '\\']).any(|part| part == "..");
    if bad {
        return Err(invalid("folder"));
    }
    Ok(folder.replace('\\', "/"))
}

fn decode(row: &sqlx::sqlite::SqliteRow) -> Result<StoredProject, DatabaseError> {
    let text = |name: &'static str| -> Result<String, DatabaseError> {
        row.try_get::<String, _>(name)
            .map_err(|source| DatabaseError::Sqlite {
                operation: "decode a project",
                source,
            })
    };
    Ok(StoredProject {
        id: text("id")?,
        workspace_id: text("workspace_id")?,
        name: text("name")?,
        goal: text("goal")?,
        guidance: text("guidance")?,
        folder: text("folder")?,
        status: ProjectStatus::parse(&text("status")?).ok_or(
            DatabaseError::StoredScheduleInvalid {
                field: "project status",
            },
        )?,
        created_at: text("created_at")?,
        updated_at: text("updated_at")?,
    })
}

macro_rules! columns {
    () => {
        "id, workspace_id, name, goal, guidance, folder, status, created_at, updated_at"
    };
}

/// Creates a project.
///
/// # Errors
///
/// Returns [`DatabaseError::InvalidProject`] for a field that is not acceptable and [`DatabaseError::ProjectNameTaken`] when the
/// workspace already has a project of that name.
pub async fn create_project(
    database: &SqliteDatabase,
    workspace_id: &str,
    new: &NewProject,
    now: UtcTimestamp,
) -> Result<StoredProject, DatabaseError> {
    let name = check_name(&new.name)?;
    let goal = check_text(&new.goal, MAX_GOAL_CHARS, "goal")?;
    let guidance = check_text(&new.guidance, MAX_GUIDANCE_CHARS, "guidance")?;
    let folder = check_folder(&new.folder)?;
    let id = ProjectId::new().to_string();
    let result = sqlx::query(
        "INSERT INTO projects (id, workspace_id, name, goal, guidance, folder, status, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'active', ?7, ?7)",
    )
    .bind(&id)
    .bind(workspace_id)
    .bind(&name)
    .bind(&goal)
    .bind(&guidance)
    .bind(&folder)
    .bind(now.to_string())
    .execute(database.pool())
    .await;
    match result {
        Ok(_) => find_project(database, workspace_id, &id).await,
        Err(sqlx::Error::Database(error)) if error.is_unique_violation() => {
            Err(DatabaseError::ProjectNameTaken)
        }
        Err(source) => Err(DatabaseError::Sqlite {
            operation: "create a project",
            source,
        }),
    }
}

/// Reads one project by identifier or, failing that, by name (ignoring case).
///
/// # Errors
///
/// Returns [`DatabaseError::ProjectNotFound`] when no project of this workspace matches.
pub async fn find_project(
    database: &SqliteDatabase,
    workspace_id: &str,
    id_or_name: &str,
) -> Result<StoredProject, DatabaseError> {
    let row = sqlx::query(concat!(
        "SELECT ",
        columns!(),
        " FROM projects WHERE workspace_id = ?1 AND (id = ?2 OR name = ?2 COLLATE NOCASE) ",
        "ORDER BY (id = ?2) DESC LIMIT 1"
    ))
    .bind(workspace_id)
    .bind(id_or_name.trim())
    .fetch_optional(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "read a project",
        source,
    })?;
    row.as_ref()
        .map_or(Err(DatabaseError::ProjectNotFound), decode)
}

/// Lists a workspace's projects, active first, then by name.
///
/// # Errors
///
/// Returns [`DatabaseError`] when the read fails.
pub async fn list_projects(
    database: &SqliteDatabase,
    workspace_id: &str,
) -> Result<Vec<StoredProject>, DatabaseError> {
    let rows = sqlx::query(concat!(
        "SELECT ",
        columns!(),
        " FROM projects WHERE workspace_id = ?1 ",
        "ORDER BY CASE status WHEN 'active' THEN 0 WHEN 'paused' THEN 1 ELSE 2 END, name COLLATE NOCASE"
    ))
    .bind(workspace_id)
    .fetch_all(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "list projects",
        source,
    })?;
    rows.iter().map(decode).collect()
}

/// Changes the given fields of a project.
///
/// # Errors
///
/// As [`create_project`], and [`DatabaseError::ProjectNotFound`].
pub async fn update_project(
    database: &SqliteDatabase,
    workspace_id: &str,
    id_or_name: &str,
    changes: &ProjectChanges,
    now: UtcTimestamp,
) -> Result<StoredProject, DatabaseError> {
    let current = find_project(database, workspace_id, id_or_name).await?;
    let name = changes
        .name
        .as_deref()
        .map_or_else(|| Ok(current.name.clone()), check_name)?;
    let goal = changes.goal.as_deref().map_or_else(
        || Ok(current.goal.clone()),
        |text| check_text(text, MAX_GOAL_CHARS, "goal"),
    )?;
    let guidance = changes.guidance.as_deref().map_or_else(
        || Ok(current.guidance.clone()),
        |text| check_text(text, MAX_GUIDANCE_CHARS, "guidance"),
    )?;
    let folder = changes
        .folder
        .as_deref()
        .map_or_else(|| Ok(current.folder.clone()), check_folder)?;
    let status = changes.status.unwrap_or(current.status);
    let result = sqlx::query(
        "UPDATE projects SET name = ?1, goal = ?2, guidance = ?3, folder = ?4, status = ?5, updated_at = ?6 \
         WHERE id = ?7 AND workspace_id = ?8",
    )
    .bind(&name)
    .bind(&goal)
    .bind(&guidance)
    .bind(&folder)
    .bind(status.as_str())
    .bind(now.to_string())
    .bind(&current.id)
    .bind(workspace_id)
    .execute(database.pool())
    .await;
    match result {
        Ok(_) => find_project(database, workspace_id, &current.id).await,
        Err(sqlx::Error::Database(error)) if error.is_unique_violation() => {
            Err(DatabaseError::ProjectNameTaken)
        }
        Err(source) => Err(DatabaseError::Sqlite {
            operation: "update a project",
            source,
        }),
    }
}

/// Deletes a project with its journal and links. Conversations and runs stay; they simply no longer belong to it.
///
/// # Errors
///
/// Returns [`DatabaseError::ProjectNotFound`] when there is no such project.
pub async fn delete_project(
    database: &SqliteDatabase,
    workspace_id: &str,
    id_or_name: &str,
) -> Result<(), DatabaseError> {
    let project = find_project(database, workspace_id, id_or_name).await?;
    sqlx::query("DELETE FROM projects WHERE id = ?1 AND workspace_id = ?2")
        .bind(&project.id)
        .bind(workspace_id)
        .execute(database.pool())
        .await
        .map_err(|source| DatabaseError::Sqlite {
            operation: "delete a project",
            source,
        })?;
    Ok(())
}

/// Appends a journal entry.
///
/// # Errors
///
/// Returns [`DatabaseError::InvalidProject`] for an empty or over-long entry.
pub async fn add_project_note(
    database: &SqliteDatabase,
    project_id: &str,
    kind: NoteKind,
    body: &str,
    run_id: Option<&str>,
    now: UtcTimestamp,
) -> Result<StoredProjectNote, DatabaseError> {
    let body = body.trim();
    if body.is_empty() || body.chars().count() > MAX_NOTE_CHARS || body.contains('\0') {
        return Err(invalid("note"));
    }
    let id = ProjectNoteId::new().to_string();
    sqlx::query("INSERT INTO project_notes (id, project_id, kind, body, run_id, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)")
        .bind(&id)
        .bind(project_id)
        .bind(kind.as_str())
        .bind(body)
        .bind(run_id)
        .bind(now.to_string())
        .execute(database.pool())
        .await
        .map_err(|source| DatabaseError::Sqlite {
            operation: "add a project note",
            source,
        })?;
    sqlx::query("UPDATE projects SET updated_at = ?1 WHERE id = ?2")
        .bind(now.to_string())
        .bind(project_id)
        .execute(database.pool())
        .await
        .map_err(|source| DatabaseError::Sqlite {
            operation: "touch a project",
            source,
        })?;
    Ok(StoredProjectNote {
        id,
        project_id: project_id.to_owned(),
        kind,
        body: body.to_owned(),
        run_id: run_id.map(str::to_owned),
        created_at: now.to_string(),
    })
}

/// The latest `limit` journal entries, oldest first (so they read as a story).
///
/// # Errors
///
/// Returns [`DatabaseError`] when the read fails.
pub async fn recent_project_notes(
    database: &SqliteDatabase,
    project_id: &str,
    limit: u32,
) -> Result<Vec<StoredProjectNote>, DatabaseError> {
    let rows = sqlx::query(
        "SELECT id, project_id, kind, body, run_id, created_at FROM project_notes WHERE project_id = ?1 \
         ORDER BY id DESC LIMIT ?2",
    )
    .bind(project_id)
    .bind(i64::from(limit))
    .fetch_all(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "read project notes",
        source,
    })?;
    let mut notes = Vec::with_capacity(rows.len());
    for row in &rows {
        let text = |name: &'static str| -> Result<String, DatabaseError> {
            row.try_get::<String, _>(name)
                .map_err(|source| DatabaseError::Sqlite {
                    operation: "decode a project note",
                    source,
                })
        };
        notes.push(StoredProjectNote {
            id: text("id")?,
            project_id: text("project_id")?,
            kind: NoteKind::parse(&text("kind")?)
                .ok_or(DatabaseError::StoredScheduleInvalid { field: "note kind" })?,
            body: text("body")?,
            run_id: row
                .try_get::<Option<String>, _>("run_id")
                .map_err(|source| DatabaseError::Sqlite {
                    operation: "decode a project note",
                    source,
                })?,
            created_at: text("created_at")?,
        });
    }
    notes.reverse();
    Ok(notes)
}

/// Makes a session or schedule belong to a project. Repeating the same link is fine; a different project is refused.
///
/// # Errors
///
/// Returns [`DatabaseError::ProjectConflict`] when it already belongs to another project.
pub async fn link_project(
    database: &SqliteDatabase,
    kind: LinkKind,
    ref_id: &str,
    project_id: &str,
) -> Result<(), DatabaseError> {
    let existing: Option<String> =
        sqlx::query_scalar("SELECT project_id FROM project_links WHERE kind = ?1 AND ref_id = ?2")
            .bind(kind.as_str())
            .bind(ref_id)
            .fetch_optional(database.pool())
            .await
            .map_err(|source| DatabaseError::Sqlite {
                operation: "read a project link",
                source,
            })?;
    match existing {
        Some(current) if current == project_id => return Ok(()),
        Some(_) => return Err(DatabaseError::ProjectConflict),
        None => {}
    }
    sqlx::query("INSERT INTO project_links (kind, ref_id, project_id) VALUES (?1, ?2, ?3)")
        .bind(kind.as_str())
        .bind(ref_id)
        .bind(project_id)
        .execute(database.pool())
        .await
        .map_err(|source| DatabaseError::Sqlite {
            operation: "link a project",
            source,
        })?;
    Ok(())
}

/// Puts a session or schedule in a project, replaces its project, or (with `None`) takes it out of any. Unlike [`link_project`] it
/// moves: it is for the owner re-filing a schedule, never for a conversation that already belongs somewhere.
///
/// # Errors
///
/// Returns [`DatabaseError`] when the write fails; nothing changes then.
pub async fn set_project_link(
    database: &SqliteDatabase,
    kind: LinkKind,
    ref_id: &str,
    project_id: Option<&str>,
) -> Result<(), DatabaseError> {
    let fail = |source| DatabaseError::Sqlite {
        operation: "move a project link",
        source,
    };
    let mut transaction = database.pool().begin().await.map_err(fail)?;
    sqlx::query("DELETE FROM project_links WHERE kind = ?1 AND ref_id = ?2")
        .bind(kind.as_str())
        .bind(ref_id)
        .execute(&mut *transaction)
        .await
        .map_err(fail)?;
    if let Some(project_id) = project_id {
        sqlx::query("INSERT INTO project_links (kind, ref_id, project_id) VALUES (?1, ?2, ?3)")
            .bind(kind.as_str())
            .bind(ref_id)
            .bind(project_id)
            .execute(&mut *transaction)
            .await
            .map_err(fail)?;
    }
    transaction.commit().await.map_err(fail)
}

/// The project a session or schedule belongs to, if any.
///
/// # Errors
///
/// Returns [`DatabaseError`] when the read fails.
pub async fn project_for(
    database: &SqliteDatabase,
    kind: LinkKind,
    ref_id: &str,
) -> Result<Option<StoredProject>, DatabaseError> {
    let row = sqlx::query(concat!(
        "SELECT ",
        columns!(),
        " FROM projects WHERE id = (SELECT project_id FROM project_links WHERE kind = ?1 AND ref_id = ?2)"
    ))
    .bind(kind.as_str())
    .bind(ref_id)
    .fetch_optional(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "read a project link",
        source,
    })?;
    row.as_ref().map(decode).transpose()
}

#[cfg(test)]
#[path = "project_tests.rs"]
mod tests;
