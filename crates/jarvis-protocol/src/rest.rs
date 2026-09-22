//! Versioned REST DTOs for the `/api/v1` HTTP surface.
//!
//! `docs/architecture/repository-layout.md` gives this crate the "REST/OpenAPI DTOs" role,
//! and ADR-0011 places the routing in `apps/jarvisd` while keeping the shapes here. The
//! split matters: these are **wire types**, not domain types, so a change to a domain
//! struct does not silently change a client-visible contract, and a client-visible
//! contract can be deprecrated without touching the state machine.
//!
//! # Errors are the SAME envelope as protocol v1
//!
//! Every failure reuses [`WireError`] and therefore the same [`jarvis_core::ErrorCode`]
//! vocabulary as the local IPC protocol. ADR-0011 requires this: one error shape across
//! transports, so a caller cannot infer a different trust model from a different body, and
//! so error handling is written once.
//!
//! # Unknown fields fail closed on request, are tolerated on response
//!
//! Requests deny unknown fields. A client that misspells `objective` must be told, rather
//! than having the run start with an empty objective that the model then interprets.
//! Responses tolerate unknown fields so a newer daemon's additive field does not break an
//! older client — the same rule `docs/architecture/runtime-and-models.md` states for
//! runtime events.

use jarvis_core::{ErrorCode, RunEventKind, RunEventSequence, RunState, SafeMessage, UtcTimestamp};
use serde::{Deserialize, Serialize};

use crate::WireError;

/// Stable JSON content type for every REST response.
pub const JSON_CONTENT_TYPE: &str = "application/json";

/// Stable server-sent-events content type, including the charset some clients require.
pub const SSE_CONTENT_TYPE: &str = "text/event-stream";

/// Bounded maximum number of events one stream page may return.
///
/// A limit above this is refused rather than clamped. Clamping would let a caller believe
/// it asked for a larger page than it received, which is the same defect as a silently
/// truncated replay.
pub const MAX_STREAM_PAGE: u32 = 1000;

/// Request body for `POST /api/v1/runs`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StartRunRequest {
    /// The objective the run must achieve. Bounded by the storage schema.
    pub objective: String,
    /// The conversation to continue, or absent to begin a new one.
    ///
    /// A multi-turn conversation is a sequence of runs that share one session: the second turn is a
    /// run that replays the first turn's transcript, not a mutation of the first run. That is why
    /// this identifies a session rather than a run.
    ///
    /// Absent rather than required, because single-turn use is the common case and requiring a
    /// session identifier would make every caller create one first. The daemon **verifies** any
    /// value supplied: a session identifier is guessable, so accepting one without checking its
    /// workspace would let a caller append to a conversation it was never granted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Optional client-supplied idempotency key.
    ///
    /// Absent by default. When present, a repeated request with the same key must not
    /// create a second run; that behaviour belongs to the run-start path, and the field
    /// exists here so the contract is not added later in a breaking way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
}

/// Response body for `POST /api/v1/runs` and `GET /api/v1/runs/{id}`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunReply {
    /// Run identifier.
    pub run_id: String,
    /// Owning session identifier.
    pub session_id: String,
    /// Owning workspace identifier.
    pub workspace_id: String,
    /// The objective that was accepted.
    pub objective: String,
    /// Current lifecycle state.
    pub state: RunState,
    /// Row version, which a client must supply to make a state-dependent request.
    pub version: i64,
    /// Terminal outcome, present only once the run settled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<jarvis_core::RunOutcome>,
    /// Bounded failure code, present only for a failed run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    /// When the run started.
    pub started_at: UtcTimestamp,
    /// When the run settled, if it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<UtcTimestamp>,
    /// When cancellation was requested, if it was.
    ///
    /// Distinct from `state`: a run whose cancellation is *requested* may still be
    /// running, because cancellation stops new work rather than the in-flight step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancellation_requested_at: Option<UtcTimestamp>,
}

/// Request body for `POST /api/v1/runs/{id}/cancel`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CancelRunRequest {
    /// The row version the caller believes is current.
    ///
    /// Required, not optional. Cancellation is a state-dependent request, so supplying the
    /// version is what stops a client from cancelling a run that already settled and
    /// silently getting `already cancelled` for a different run's terminal state.
    pub expected_version: i64,
}

/// One event stream item as a REST DTO.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunEventReply {
    /// Event identifier, also used as the SSE `id:` field.
    pub event_id: String,
    /// This event's one-based position in its run's stream.
    pub sequence: RunEventSequence,
    /// Normalized event kind.
    pub kind: RunEventKind,
    /// Bounded operational summary, absent when the kind carries none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// The event payload, passed through as JSON rather than re-encoded as a string.
    ///
    /// Nested JSON rather than a string: a client should not have to parse twice, and a
    /// double-encoded payload is what makes a stream hard to debug.
    pub payload: serde_json::Value,
    /// Correlation identifier shared with the originating request.
    pub correlation_id: jarvis_core::CorrelationId,
    /// When the run produced the event.
    pub occurred_at: UtcTimestamp,
}

/// Response body for `GET /api/v1/runs/{id}/events`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunEventPageReply {
    /// The requested page, in ascending sequence order.
    pub events: Vec<RunEventReply>,
    /// The highest sequence this daemon holds for the run, when it holds any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub highest_sequence: Option<RunEventSequence>,
    /// Whether the caller must resynchronise instead of trusting this page alone.
    ///
    /// Set when the requested start position is beyond what the daemon holds, which is the
    /// case `docs/architecture/protocols.md` requires be explicit. A client that receives
    /// `true` must discard its cached position; the alternative is a client that believes
    /// it has the whole history because it received an empty page.
    pub resync_required: bool,
}

/// The `Retry-After`-style hint a resync response carries, in seconds.
pub const RESYNC_HINT_SECONDS: u32 = 5;

/// Builds the shared error envelope for a REST failure.
///
/// Centralised so every route produces the same shape. A route that hand-built a JSON
/// error would drift from [`WireError`], and the drift would only be visible to a client
/// switching transports.
#[must_use]
pub fn rest_error(code: ErrorCode, message: SafeMessage) -> WireError {
    WireError::new(code, message, jarvis_core::CorrelationId::new())
}

/// Builds a bounded, valid safe message without a fallible call at the route.
///
/// Route code must never panic while constructing an error, so a message that fails
/// validation collapses to a stable placeholder rather than propagating a second failure.
#[must_use]
pub fn safe(message: &str) -> SafeMessage {
    SafeMessage::new(message).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixed instant for fixtures.
    ///
    /// `UtcTimestamp` deliberately has no `Default`, so a test cannot silently acquire
    /// "the epoch" as a value it never chose. The parse is the same checked path production
    /// code uses.
    fn at_epoch() -> UtcTimestamp {
        "2026-09-22T10:00:00Z"
            .parse::<UtcTimestamp>()
            .unwrap_or_else(|error| panic!("valid fixture timestamp: {error}"))
    }

    /// A request with a misspelled field must be refused, not accepted with a default.
    #[test]
    fn start_run_requests_deny_unknown_fields() {
        let good = r#"{"objective":"summarise the inbox"}"#;
        assert!(serde_json::from_str::<StartRunRequest>(good).is_ok());

        let misspelled = r#"{"objectiv":"summarise the inbox"}"#;
        assert!(
            serde_json::from_str::<StartRunRequest>(misspelled).is_err(),
            "a misspelled objective must not start a run with an empty one"
        );

        let extra = r#"{"objective":"x","admin":true}"#;
        assert!(
            serde_json::from_str::<StartRunRequest>(extra).is_err(),
            "an unknown field must be refused rather than ignored"
        );
    }

    /// An optional field is omitted from the encoding rather than sent as null, so a
    /// client does not have to distinguish "absent" from "explicitly null".
    #[test]
    fn optional_fields_are_omitted_when_absent() {
        let request = StartRunRequest {
            objective: "summarise".to_owned(),
            session_id: None,
            idempotency_key: None,
        };
        let encoded = serde_json::to_string(&request).unwrap_or_default();
        assert_eq!(encoded, r#"{"objective":"summarise"}"#);

        let reply = RunReply {
            run_id: "0198f000-0000-7000-8000-000000000001".to_owned(),
            session_id: "0198f000-0000-7000-8000-000000000002".to_owned(),
            workspace_id: "0198f000-0000-7000-8000-000000000003".to_owned(),
            objective: "summarise".to_owned(),
            state: RunState::Received,
            version: 1,
            outcome: None,
            error_code: None,
            started_at: at_epoch(),
            completed_at: None,
            cancellation_requested_at: None,
        };
        let encoded = serde_json::to_string(&reply).unwrap_or_default();
        assert!(!encoded.contains("outcome"));
        assert!(!encoded.contains("completed_at"));
        assert!(!encoded.contains("error_code"));
    }

    /// A cancel request without a version must be refused: the version is what makes the
    /// operation safe against a run that already settled.
    #[test]
    fn a_cancel_request_requires_a_version() {
        assert!(serde_json::from_str::<CancelRunRequest>(r#"{"expected_version":1}"#).is_ok());
        assert!(
            serde_json::from_str::<CancelRunRequest>("{}").is_err(),
            "an absent expected_version must not default to a value that always matches"
        );
    }

    /// A response tolerates an additive field, so a newer daemon does not break an older
    /// client within the same major version.
    #[test]
    fn responses_tolerate_an_unknown_additive_field() {
        let page = r#"{"events":[],"resync_required":false,"future_field":1}"#;
        let decoded: Result<RunEventPageReply, _> = serde_json::from_str(page);
        assert!(
            decoded.is_ok(),
            "an additive response field must not break a client"
        );
    }

    /// The payload is nested JSON, not a string containing JSON. A client that had to
    /// parse twice would be the symptom of double encoding.
    #[test]
    fn an_event_payload_is_nested_json() {
        let event = RunEventReply {
            event_id: "0198f000-0000-7000-8000-000000000004".to_owned(),
            sequence: RunEventSequence::first(),
            kind: RunEventKind::OutputDelta,
            summary: None,
            payload: serde_json::json!({ "text": "hello" }),
            correlation_id: jarvis_core::CorrelationId::new(),
            occurred_at: at_epoch(),
        };
        let encoded = serde_json::to_string(&event).unwrap_or_default();
        assert!(
            encoded.contains(r#""payload":{"text":"hello"}"#),
            "payload must be an object, not an escaped string: {encoded}"
        );
    }

    /// An empty page with `resync_required` false is a complete history, and with it true
    /// is a resync. The two must be distinguishable, which is the whole reason the field
    /// is not optional.
    #[test]
    fn an_empty_page_can_still_require_a_resync() {
        let complete = RunEventPageReply {
            events: Vec::new(),
            highest_sequence: None,
            resync_required: false,
        };
        let stale = RunEventPageReply {
            events: Vec::new(),
            highest_sequence: None,
            resync_required: true,
        };
        assert_ne!(complete, stale);
        assert!(!complete.resync_required);
        assert!(stale.resync_required);
    }
}
