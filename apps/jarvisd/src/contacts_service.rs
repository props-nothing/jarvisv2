//! The contact list, for its owner (`ADR-0155`): list, save, remove.
//!
//! What a model may do to it is narrower and lives in `contacts_tool`. Here the caller is the owner, so a save may also lift a
//! do-not-contact.

use axum::{
    Json,
    extract::{Path, RawQuery, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use jarvis_core::{ErrorCode, SystemClock, UtcTimestamp};
use jarvis_protocol::{ContactCount, ContactListReply, ContactReply, SaveContactRequest};
use jarvis_storage::{ContactInput, ContactStatus, DatabaseError, StoredContact};

use crate::gateway::{GatewayState, error_response};

/// The most one page holds.
const MAX_PAGE: u32 = 1000;

/// The authenticated routes, mounted under /api/v1.
pub fn routes() -> axum::Router<GatewayState> {
    use axum::routing::{delete, get};
    axum::Router::new()
        .route("/contacts", get(list).post(save))
        .route("/contacts/{id}", delete(remove))
}

fn reply(contact: &StoredContact) -> ContactReply {
    ContactReply {
        contact_id: contact.id.clone(),
        company: contact.company.clone(),
        person: contact.person.clone(),
        role: contact.role.clone(),
        email: contact.email.clone(),
        phone: contact.phone.clone(),
        status: contact.status.as_str().to_owned(),
        notes: contact.notes.clone(),
        source: contact.source.clone(),
        updated_at: contact.updated_at.clone(),
    }
}

fn unavailable() -> Response {
    error_response(
        StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::Internal,
        "the local database is not available",
    )
}

fn invalid(message: &str) -> Response {
    error_response(
        StatusCode::UNPROCESSABLE_ENTITY,
        ErrorCode::Validation,
        message,
    )
}

// The error is the response itself, as every handler here returns it; boxing it would only add an unwrap at each use.
#[allow(clippy::result_large_err)]
async fn workspace(state: &GatewayState) -> Result<String, Response> {
    jarvis_storage::load_local_identity(state.database())
        .await
        .map(|identity| identity.workspace_id().to_owned())
        .map_err(|_| unavailable())
}

/// Reads `status`, `limit` and `offset` from a query string. Only plain words and digits are understood, so nothing needs decoding.
fn parse_query(raw: Option<&str>) -> Result<(Option<ContactStatus>, u32, u32), &'static str> {
    let (mut status, mut limit, mut offset) = (None, 100_u32, 0_u32);
    for pair in raw
        .unwrap_or_default()
        .split('&')
        .filter(|pair| !pair.is_empty())
    {
        let (key, value) = pair
            .split_once('=')
            .ok_or("a query value needs a name=value form")?;
        match key {
            "status" => {
                status = Some(
                    ContactStatus::parse(value)
                        .ok_or("that status is not one of the known ones")?,
                );
            }
            "limit" => limit = value.parse().map_err(|_| "the limit must be a number")?,
            "offset" => offset = value.parse().map_err(|_| "the offset must be a number")?,
            _ => return Err("only status, limit and offset are understood"),
        }
    }
    if !(1..=MAX_PAGE).contains(&limit) {
        return Err("the limit must be between 1 and 1000");
    }
    Ok((status, limit, offset))
}

async fn list(State(state): State<GatewayState>, RawQuery(raw): RawQuery) -> Response {
    let (status, limit, offset) = match parse_query(raw.as_deref()) {
        Ok(parsed) => parsed,
        Err(message) => return invalid(message),
    };
    let workspace = match workspace(&state).await {
        Ok(id) => id,
        Err(response) => return response,
    };
    let page =
        jarvis_storage::list_contacts(state.database(), &workspace, status, limit, offset).await;
    let counts = jarvis_storage::contact_stats(state.database(), &workspace).await;
    match (page, counts) {
        (Ok(page), Ok(counts)) => Json(ContactListReply {
            contacts: page.iter().map(reply).collect(),
            counts: counts
                .into_iter()
                .map(|(status, count)| ContactCount {
                    status: status.as_str().to_owned(),
                    count,
                })
                .collect(),
        })
        .into_response(),
        _ => unavailable(),
    }
}

async fn save(
    State(state): State<GatewayState>,
    Json(request): Json<SaveContactRequest>,
) -> Response {
    let status = match request.status.as_deref() {
        Some(name) => match ContactStatus::parse(name) {
            Some(status) => Some(status),
            None => return invalid("that status is not one of the known ones"),
        },
        None => None,
    };
    let workspace = match workspace(&state).await {
        Ok(id) => id,
        Err(response) => return response,
    };
    let input = ContactInput {
        company: request.company,
        person: request.person,
        role: request.role,
        email: request.email,
        phone: request.phone,
        status,
        notes: request.notes,
        source: request.source,
    };
    let now = UtcTimestamp::now(&SystemClock);
    match jarvis_storage::save_contact(state.database(), &workspace, None, &input, true, now).await
    {
        Ok((contact, created)) => {
            let code = if created {
                StatusCode::CREATED
            } else {
                StatusCode::OK
            };
            (code, Json(reply(&contact))).into_response()
        }
        Err(DatabaseError::InvalidContact { field }) => {
            invalid(&format!("the contact's {field} is not acceptable"))
        }
        Err(DatabaseError::ContactLimit) => error_response(
            StatusCode::CONFLICT,
            ErrorCode::Conflict,
            "the contact list is full",
        ),
        Err(_) => unavailable(),
    }
}

async fn remove(State(state): State<GatewayState>, Path(id): Path<String>) -> Response {
    let workspace = match workspace(&state).await {
        Ok(id) => id,
        Err(response) => return response,
    };
    match jarvis_storage::delete_contact(state.database(), &workspace, &id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => error_response(
            StatusCode::NOT_FOUND,
            ErrorCode::Validation,
            "no contact has that identifier",
        ),
        Err(_) => unavailable(),
    }
}
