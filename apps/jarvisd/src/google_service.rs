//! The routes behind Settings, Google: status, sign in, sign out, and the public callback Google sends the browser to.
//!
//! The three `/api/v1/google` routes need the bearer credential like every other. The callback cannot carry one (it is a browser
//! redirect from Google), so it is the one public route besides the console's own assets, and it is safe to leave open for the same
//! reason a login callback is anywhere: it does nothing unless the `state` it carries matches a sign-in this daemon started a moment ago
//! with the credential (`GoogleAccount::complete` fails closed otherwise, and consumes the transaction on the first answer).

use std::sync::Arc;

use axum::{
    Json,
    extract::State,
    http::{HeaderValue, Method, StatusCode, Uri, header},
    response::{IntoResponse, Response},
};
use jarvis_core::ErrorCode;
use serde_json::json;

use crate::gateway::{GatewayState, error_response};
use crate::google_account::{CALLBACK_PATH, GoogleAccount};

/// The three authenticated routes, mounted under /api/v1.
pub fn routes() -> axum::Router<GatewayState> {
    axum::Router::new()
        .route("/google", axum::routing::get(status))
        .route("/google/connect", axum::routing::post(connect))
        .route("/google/disconnect", axum::routing::post(disconnect))
}

/// Whether a request is the browser's return from Google, which carries no bearer credential.
#[must_use]
pub fn is_oauth_callback(method: &Method, path: &str) -> bool {
    method == Method::GET && path == CALLBACK_PATH
}

// The error is the response itself, as every handler here returns it; boxing it would only add an unwrap at each use.
#[allow(clippy::result_large_err)]
fn account(state: &GatewayState) -> Result<&Arc<GoogleAccount>, Response> {
    state.google().ok_or_else(|| {
        error_response(
            StatusCode::NOT_FOUND,
            ErrorCode::UnavailableCapability,
            "Google sign-in is not available on this daemon",
        )
    })
}

/// `GET /api/v1/google`: whether Google is configured and who is signed in.
pub async fn status(State(state): State<GatewayState>) -> Response {
    match account(&state) {
        Ok(account) => (StatusCode::OK, Json(account.status())).into_response(),
        Err(response) => response,
    }
}

/// `POST /api/v1/google/connect`: starts a sign-in and returns the Google address to open.
pub async fn connect(State(state): State<GatewayState>) -> Response {
    let account = match account(&state) {
        Ok(account) => account,
        Err(response) => return response,
    };
    let Some(context) = state.settings() else {
        return error_response(
            StatusCode::NOT_FOUND,
            ErrorCode::UnavailableCapability,
            "settings are not available on this daemon",
        );
    };
    let port = jarvis_storage::ConfigStore::from_paths(context.paths())
        .load()
        .map_or(8765, |loaded| loaded.config().daemon().http_port());
    match account.begin(port) {
        Ok(url) => (StatusCode::OK, Json(json!({ "auth_url": url }))).into_response(),
        Err(error) => error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::Validation,
            &error.to_string(),
        ),
    }
}

/// `POST /api/v1/google/disconnect`: revokes the sign-in and deletes it.
pub async fn disconnect(State(state): State<GatewayState>) -> Response {
    let account = match account(&state) {
        Ok(account) => account,
        Err(response) => return response,
    };
    match account.disconnect().await {
        Ok(()) => (StatusCode::OK, Json(json!({ "connected": false }))).into_response(),
        Err(error) => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::Internal,
            &error.to_string(),
        ),
    }
}

/// `GET /oauth/google/callback`: the browser's return from Google. Public; see the module note.
pub async fn callback(State(state): State<GatewayState>, uri: Uri) -> Response {
    let target = uri
        .path_and_query()
        .map_or_else(|| uri.path().to_owned(), ToString::to_string);
    let (status, heading, detail) = match state.google() {
        Some(account) => match account.complete(&target).await {
            Ok(_) => (
                StatusCode::OK,
                "Google is connected",
                "You can close this tab and go back to JARVIS.".to_owned(),
            ),
            Err(error) => (
                StatusCode::BAD_REQUEST,
                "Google was not connected",
                error.to_string(),
            ),
        },
        None => (
            StatusCode::NOT_FOUND,
            "Google was not connected",
            "Google sign-in is not available on this daemon.".to_owned(),
        ),
    };
    // Fixed words only: nothing from the request is echoed into the page.
    let page = format!(
        "<!doctype html><meta charset=\"utf-8\"><title>JARVIS</title><h1>{heading}</h1><p>{detail}</p><p><a href=\"/\">Back to JARVIS</a></p>"
    );
    let mut response = (status, page).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
        ),
    );
    response
}
