//! Projects: the routes that manage them (`ADR-0151`).
//!
//! A project is what JARVIS is told about long-running work: a goal, standing guidance, a working folder and a journal. These routes
//! read and write that, and nothing else; no field here grants a tool, a scope or an approval.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use jarvis_core::{ErrorCode, SystemClock, UtcTimestamp};
use jarvis_protocol::{
    AddProjectNoteRequest, CreateProjectRequest, ProjectDetailReply, ProjectListReply,
    ProjectNoteReply, ProjectReply, UpdateProjectRequest,
};
use jarvis_storage::{
    DatabaseError, NewProject, NoteKind, ProjectChanges, ProjectStatus, StoredProject,
    StoredProjectNote,
};

use crate::gateway::{GatewayState, error_response};

/// The authenticated routes, mounted under /api/v1.
pub fn routes() -> axum::Router<GatewayState> {
    use axum::routing::{get, post};
    axum::Router::new()
        .route("/projects", post(create).get(list))
        .route("/projects/{id}", get(read).patch(update).delete(remove))
        .route("/projects/{id}/notes", post(add_note))
}

/// How many journal entries the detail view carries.
const DETAIL_NOTES: u32 = 50;

pub(crate) fn project_reply(project: &StoredProject) -> ProjectReply {
    ProjectReply {
        project_id: project.id.clone(),
        name: project.name.clone(),
        goal: project.goal.clone(),
        guidance: project.guidance.clone(),
        folder: project.folder.clone(),
        status: project.status.as_str().to_owned(),
        created_at: project.created_at.clone(),
        updated_at: project.updated_at.clone(),
    }
}

pub(crate) fn note_reply(note: &StoredProjectNote) -> ProjectNoteReply {
    ProjectNoteReply {
        note_id: note.id.clone(),
        kind: note.kind.as_str().to_owned(),
        text: note.body.clone(),
        run_id: note.run_id.clone(),
        created_at: note.created_at.clone(),
    }
}

fn storage_response(error: &DatabaseError) -> Response {
    match error {
        DatabaseError::ProjectNotFound => error_response(
            StatusCode::NOT_FOUND,
            ErrorCode::Validation,
            "no project has that name or identifier",
        ),
        DatabaseError::ProjectNameTaken => error_response(
            StatusCode::CONFLICT,
            ErrorCode::Conflict,
            "a project with that name already exists",
        ),
        DatabaseError::InvalidProject { field } => error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::Validation,
            &format!("the project's {field} is not acceptable"),
        ),
        other => {
            tracing::warn!(error = %other, "a project request could not be served");
            error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                ErrorCode::Internal,
                "the local database is not available",
            )
        }
    }
}

// The error is the response itself, as every handler here returns it; boxing it would only add an unwrap at each use.
#[allow(clippy::result_large_err)]
async fn workspace(state: &GatewayState) -> Result<String, Response> {
    jarvis_storage::load_local_identity(state.database())
        .await
        .map(|identity| identity.workspace_id().to_owned())
        .map_err(|_| {
            error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                ErrorCode::Internal,
                "the local database is not available",
            )
        })
}

/// `POST /api/v1/projects`
pub async fn create(
    State(state): State<GatewayState>,
    Json(request): Json<CreateProjectRequest>,
) -> Response {
    let workspace_id = match workspace(&state).await {
        Ok(id) => id,
        Err(response) => return response,
    };
    let new = NewProject {
        name: request.name,
        goal: request.goal,
        guidance: request.guidance,
        folder: request.folder,
    };
    match jarvis_storage::create_project(
        state.database(),
        &workspace_id,
        &new,
        UtcTimestamp::now(&SystemClock),
    )
    .await
    {
        Ok(project) => (StatusCode::CREATED, Json(project_reply(&project))).into_response(),
        Err(error) => storage_response(&error),
    }
}

/// `GET /api/v1/projects`
pub async fn list(State(state): State<GatewayState>) -> Response {
    let workspace_id = match workspace(&state).await {
        Ok(id) => id,
        Err(response) => return response,
    };
    match jarvis_storage::list_projects(state.database(), &workspace_id).await {
        Ok(projects) => {
            let reply = ProjectListReply {
                total: projects.len(),
                projects: projects.iter().map(project_reply).collect(),
            };
            (StatusCode::OK, Json(reply)).into_response()
        }
        Err(error) => storage_response(&error),
    }
}

/// `GET /api/v1/projects/{id}`; the identifier may be the project's name.
pub async fn read(State(state): State<GatewayState>, Path(id): Path<String>) -> Response {
    let workspace_id = match workspace(&state).await {
        Ok(id) => id,
        Err(response) => return response,
    };
    let project = match jarvis_storage::find_project(state.database(), &workspace_id, &id).await {
        Ok(project) => project,
        Err(error) => return storage_response(&error),
    };
    match jarvis_storage::recent_project_notes(state.database(), &project.id, DETAIL_NOTES).await {
        Ok(notes) => {
            let reply = ProjectDetailReply {
                project: project_reply(&project),
                notes: notes.iter().map(note_reply).collect(),
            };
            (StatusCode::OK, Json(reply)).into_response()
        }
        Err(error) => storage_response(&error),
    }
}

/// `PATCH /api/v1/projects/{id}`
pub async fn update(
    State(state): State<GatewayState>,
    Path(id): Path<String>,
    Json(request): Json<UpdateProjectRequest>,
) -> Response {
    let workspace_id = match workspace(&state).await {
        Ok(id) => id,
        Err(response) => return response,
    };
    let status = match request.status.as_deref().map(ProjectStatus::parse) {
        None => None,
        Some(Some(status)) => Some(status),
        Some(None) => {
            return error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                ErrorCode::Validation,
                "the status must be active, paused or done",
            );
        }
    };
    let changes = ProjectChanges {
        name: request.name,
        goal: request.goal,
        guidance: request.guidance,
        folder: request.folder,
        status,
    };
    match jarvis_storage::update_project(
        state.database(),
        &workspace_id,
        &id,
        &changes,
        UtcTimestamp::now(&SystemClock),
    )
    .await
    {
        Ok(project) => (StatusCode::OK, Json(project_reply(&project))).into_response(),
        Err(error) => storage_response(&error),
    }
}

/// `DELETE /api/v1/projects/{id}`
pub async fn remove(State(state): State<GatewayState>, Path(id): Path<String>) -> Response {
    let workspace_id = match workspace(&state).await {
        Ok(id) => id,
        Err(response) => return response,
    };
    match jarvis_storage::delete_project(state.database(), &workspace_id, &id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => storage_response(&error),
    }
}

/// `POST /api/v1/projects/{id}/notes`
pub async fn add_note(
    State(state): State<GatewayState>,
    Path(id): Path<String>,
    Json(request): Json<AddProjectNoteRequest>,
) -> Response {
    let workspace_id = match workspace(&state).await {
        Ok(id) => id,
        Err(response) => return response,
    };
    let kind = match request.kind.as_deref() {
        None => NoteKind::Owner,
        Some(text) => match NoteKind::parse(text) {
            Some(kind) => kind,
            None => {
                return error_response(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    ErrorCode::Validation,
                    "the kind must be progress, decision, blocker, next, result or owner",
                );
            }
        },
    };
    let project = match jarvis_storage::find_project(state.database(), &workspace_id, &id).await {
        Ok(project) => project,
        Err(error) => return storage_response(&error),
    };
    match jarvis_storage::add_project_note(
        state.database(),
        &project.id,
        kind,
        &request.text,
        None,
        UtcTimestamp::now(&SystemClock),
    )
    .await
    {
        Ok(note) => (StatusCode::CREATED, Json(note_reply(&note))).into_response(),
        Err(error) => storage_response(&error),
    }
}

#[cfg(test)]
#[path = "project_service_tests.rs"]
mod tests;
