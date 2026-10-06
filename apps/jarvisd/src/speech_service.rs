//! The spoken voice: `GET /api/v1/speech` says whether one is configured, `POST /api/v1/speech` turns text into audio.
//!
//! The provider's key lives only in the daemon (`ADR-0138`). The page asks for speech and plays the audio it gets back,
//! so the key is in no browser, no page script and no URL. Both routes sit behind the same bearer credential as the
//! rest of the API.

use axum::{
    Json,
    extract::State,
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use jarvis_core::ErrorCode;
use jarvis_voice::SpeechError;
use serde::Deserialize;
use serde_json::json;

use crate::gateway::{GatewayState, error_response};

/// The body of `POST /api/v1/speech`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpeakRequest {
    text: String,
}

/// `GET /api/v1/speech`
pub async fn status(State(state): State<GatewayState>) -> Response {
    let body = state.speech().map_or_else(
        || json!({ "enabled": false }),
        |speech| {
            json!({
                "enabled": true,
                "provider": "elevenlabs",
                "voice": speech.voice_id(),
                "model": speech.model_id(),
                "max_text_chars": jarvis_voice::MAX_TEXT_CHARS,
            })
        },
    );
    (StatusCode::OK, Json(body)).into_response()
}

/// `POST /api/v1/speech`
pub async fn speak(
    State(state): State<GatewayState>,
    Json(request): Json<SpeakRequest>,
) -> Response {
    let Some(speech) = state.speech() else {
        return error_response(
            StatusCode::NOT_FOUND,
            ErrorCode::UnavailableCapability,
            "no speech provider is configured; set daemon.speech_api_key_ref",
        );
    };
    match speech.synthesize(&request.text).await {
        Ok(audio) => {
            let mut response = (StatusCode::OK, audio.bytes().clone()).into_response();
            let headers = response.headers_mut();
            headers.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static(audio.content_type()),
            );
            headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response
        }
        Err(error) => {
            // The error text names the failure and never the key or the provider's body (`jarvis-voice`).
            tracing::warn!(%error, "speech could not be produced");
            let (status, code) = match &error {
                SpeechError::EmptyText | SpeechError::TooLong => {
                    (StatusCode::UNPROCESSABLE_ENTITY, ErrorCode::Validation)
                }
                SpeechError::OverLimit => (StatusCode::TOO_MANY_REQUESTS, ErrorCode::RateLimited),
                SpeechError::KeyRejected | SpeechError::Settings(_) => {
                    (StatusCode::BAD_GATEWAY, ErrorCode::PermanentUpstream)
                }
                SpeechError::Unreachable | SpeechError::Provider(_) | SpeechError::BadAudio => {
                    (StatusCode::BAD_GATEWAY, ErrorCode::TransientUpstream)
                }
            };
            error_response(status, code, &error.to_string())
        }
    }
}
