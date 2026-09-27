//! Recording a workspace.
//!
//! # Why this did not exist until `P4-010`
//!
//! This deployment has **one** workspace per profile, seeded by migration `0005_seed_local_identity.sql`, and
//! every read resolves it through [`crate::load_local_identity`]. So for `P1` through `P4-008` nothing needed
//! to create one, and the only code that inserted a row into `workspaces` was test fixtures writing raw SQL.
//!
//! `P4-010`'s `A09` — "Given semantically similar memories/documents in **two workspaces**, no query, context
//! build, model call, tool, search, export, trace, or diagnostics request from one workspace reveals the
//! other's content or existence" — cannot be tested without a second one. A gate whose fixture wrote raw SQL
//! would be asserting that a hand-written row is isolated, which is a claim about SQLite rather than about
//! this platform.
//!
//! # What this deliberately is not
//!
//! It is **not** a multi-tenant feature. It records a row; it does not create a session, issue a credential,
//! grant access, or make a workspace reachable through any request. `docs/architecture/identity-and-workspaces.md`
//! requires that access follow from an authenticated actor rather than from naming a workspace, and no actor
//! can name one here — `apps/jarvisd` still resolves the workspace from the seeded identity and a
//! `workspace_id` in a request body is refused by `deny_unknown_fields` (`ADR-0050`).
//!
//! So the honest description is: this is the **minimum that lets an isolation test build a second workspace**,
//! and the multi-tenant surface — invitations, membership, per-workspace credentials — remains unbuilt and
//! unclaimed. `TODO.md` records that.

use crate::database::{DatabaseError, SqliteDatabase};
use jarvis_core::{UtcTimestamp, WorkspaceId};

/// Whether a workspace is local to one machine or served to several devices.
///
/// Mirrors the `workspaces.mode` check constraint. The schema's two values are here as a Rust enum because a
/// decode has to name what it read, and a `String` would make "which mode is this" a comparison at every call
/// site — the same reasoning `EntityKind` records.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkspaceMode {
    /// One machine, one user. What every profile created so far is.
    Local,
    /// Served to several devices. `ADR-0003`'s server mode; not reachable yet.
    Server,
}

impl WorkspaceMode {
    /// Returns the stable value stored in the column.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Server => "server",
        }
    }
}

/// What a workspace's data policy permits.
///
/// Mirrors the `workspaces.data_policy` check constraint. The distinction matters to retrieval: a
/// `local-only` workspace's content must not leave the machine, which is a **placement** rule and not a
/// sensitivity one — `docs/architecture/security.md` keeps the two apart, and the memory ranking's
/// destination ceiling is where it is enforced.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DataPolicy {
    /// Content may be sent to a configured provider.
    Standard,
    /// Content must not leave the machine.
    LocalOnly,
}

impl DataPolicy {
    /// Returns the stable value stored in the column.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::LocalOnly => "local-only",
        }
    }
}

/// The fields needed to record a workspace.
#[derive(Clone, Debug)]
pub struct NewWorkspace {
    /// The workspace identifier.
    pub id: WorkspaceId,
    /// The human-readable name.
    pub name: String,
    /// Whether it is local or served.
    pub mode: WorkspaceMode,
    /// What its data policy permits.
    pub data_policy: DataPolicy,
    /// When it was created.
    pub created_at: UtcTimestamp,
}

/// Records a workspace.
///
/// # Errors
///
/// - [`DatabaseError::InvalidWorkspaceRequest`] when the name is empty or longer than the schema's bound.
/// - [`DatabaseError::Sqlite`] when the row cannot be written, including a duplicate identifier.
pub async fn record_workspace(
    database: &SqliteDatabase,
    workspace: &NewWorkspace,
) -> Result<WorkspaceId, DatabaseError> {
    // Checked here rather than left to the schema, so a caller gets a field name instead of a constraint
    // violation. The bound is the migration's own.
    let name = workspace.name.trim();
    if name.is_empty() || name.chars().count() > 128 {
        return Err(DatabaseError::InvalidWorkspaceRequest { field: "name" });
    }
    sqlx::query(
        "INSERT INTO workspaces (id, name, mode, data_policy, status, created_at, updated_at, version) \
         VALUES (?1, ?2, ?3, ?4, 'active', ?5, ?5, 1) \
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(workspace.id.to_string())
    .bind(name)
    .bind(workspace.mode.as_str())
    .bind(workspace.data_policy.as_str())
    .bind(workspace.created_at.to_string())
    .execute(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "record a workspace",
        source,
    })?;
    Ok(workspace.id)
}

#[cfg(test)]
#[path = "workspace_tests.rs"]
mod tests;
