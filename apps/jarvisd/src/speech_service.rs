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
use futures_util::StreamExt;
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
    /// The sentence spoken just before this one, so the voice carries on in the same tone. Optional.
    #[serde(default)]
    previous_text: Option<String>,
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
    // The provider is asked to stream, and the first chunk is awaited here, so a refusal is still an honest status code and
    // a `200` always means audio is on its way. The rest is passed on as it arrives rather than collected, so the browser can
    // start playing while the sentence is still being made.
    let opened = match speech
        .open(&request.text, request.previous_text.as_deref())
        .await
    {
        Ok(mut stream) => match stream.next_chunk().await {
            Ok(Some(first)) => Ok((stream, first)),
            Ok(None) => Err(SpeechError::BadAudio),
            Err(error) => Err(error),
        },
        Err(error) => Err(error),
    };
    match opened {
        Ok((stream, first)) => {
            let content_type = stream.content_type();
            let rest = futures_util::stream::unfold(stream, |mut stream| async move {
                match stream.next_chunk().await {
                    Ok(Some(chunk)) => Some((Ok::<_, std::io::Error>(chunk), stream)),
                    Ok(None) => None,
                    // Ends the response abnormally, which the browser sees as a failed read and falls back from.
                    Err(error) => Some((Err(std::io::Error::other(error.to_string())), stream)),
                }
            });
            let body = axum::body::Body::from_stream(
                futures_util::stream::once(async move { Ok::<_, std::io::Error>(first) })
                    .chain(rest),
            );
            let mut response = (StatusCode::OK, body).into_response();
            let headers = response.headers_mut();
            headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
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
