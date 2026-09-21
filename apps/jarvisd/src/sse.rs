//! Server-sent events for a run's activity and output stream.
//!
//! `docs/architecture/protocols.md` requires stream items to carry a sequence and an event
//! identifier and requires the server to "replay from durable records within retention or
//! return an explicit resync requirement". This module is the transport half of that
//! contract, reading the durable record `P2-007b` added.
//!
//! # Replay is a sequence cursor, not a session
//!
//! A reconnecting client sends `Last-Event-ID` naming the last sequence it received, and the
//! stream continues from the next one. There is no server-side subscriber registry: a cursor
//! is durable, survives a reconnect to a different daemon process, and cannot leak between
//! clients. Per-connection state would fail the restart case this record exists to serve.
//!
//! # `Last-Event-ID` is untrusted input
//!
//! The header arrives from the network, so a value that is not a positive integer is refused
//! with `400` rather than defaulted. Defaulting a corrupt cursor to the first sequence would
//! silently replay a stream the client already holds; defaulting it to the end would silently
//! skip events. Both failures look like a successful stream.
//!
//! # The stream is a poll over the durable log
//!
//! Each step reads a bounded page of stored events and yields them, then waits before reading
//! again. Polling rather than subscribing means the stream cannot emit an event the log does
//! not hold, so a reconnect is the same query with a different starting sequence instead of a
//! second, weaker delivery path.
//!
//! # The stream ends when the run settles
//!
//! A terminal event is the last event a settled run can have, because the append path refuses
//! to write after settlement. The stream therefore closes once it has emitted a terminal event,
//! rather than leaving a client waiting on a connection that can produce nothing.

use std::{collections::VecDeque, convert::Infallible, time::Duration};

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
};
use jarvis_core::{ErrorCode, ReplayRequest, RunEventKind, RunEventSequence};
use jarvis_protocol::{RunEventReply, rest_error, safe};
use jarvis_storage::{
    DatabaseError, StoredRunEvent, find_run, highest_run_event_sequence, read_run_events,
};

use crate::gateway::GatewayState;

/// Events returned per read while a run is still active.
///
/// Well below the protocol's maximum page size so a long backlog is delivered in several
/// bounded reads rather than one large allocation, and so a client receives its first event
/// promptly on a stream that already holds thousands.
pub const DEFAULT_PAGE: u32 = 200;

/// How long to wait between reads while a run is active.
const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// How often to send a comment-only keep-alive frame.
///
/// A comment frame rather than a `heartbeat` **event**: a heartbeat event would occupy a
/// sequence number and enter the durable log, so a later replay would replay it as though the
/// run had done something. A comment proves the connection is alive without altering the record.
const KEEP_ALIVE: Duration = Duration::from_secs(15);

/// `GET /api/v1/runs/{id}/stream`
///
/// # Errors
///
/// Returns `400` for a malformed `Last-Event-ID`, `404` when the run is unknown, and `409`
/// when the requested cursor is beyond what this daemon holds.
pub async fn stream_events(
    State(state): State<GatewayState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let cursor = match resume_cursor(&headers) {
        Ok(cursor) => cursor,
        Err(message) => return bad_request(message),
    };

    // The run is checked before a stream response is committed, so a stream for an unknown run is
    // a `404` rather than a stream that opens and immediately closes. This distinction is not
    // cosmetic: `highest_run_event_sequence` reports `None` both for a run that does not exist and
    // for a run that exists with no events, so resolving the run row is the only way to tell them
    // apart. An empty stream would otherwise be indistinguishable from a truncated one.
    let known = match find_run(state.database(), &id).await {
        Ok(_) => true,
        Err(DatabaseError::RunNotFound) => false,
        Err(error) => return storage_error(&error),
    };
    if !known {
        return storage_error(&DatabaseError::RunNotFound);
    }

    let highest = match highest_run_event_sequence(state.database(), &id).await {
        Ok(highest) => highest,
        Err(error) => return storage_error(&error),
    };
    let beyond_end = match highest {
        Some(highest) => cursor > highest,
        None => cursor > RunEventSequence::first(),
    };
    if beyond_end {
        // A cursor past the end cannot be resumed without silently skipping events, so it is
        // refused explicitly. `docs/architecture/protocols.md` requires this be visible rather
        // than resolved by guessing.
        return (
            StatusCode::CONFLICT,
            Json(rest_error(
                ErrorCode::Conflict,
                safe("the requested stream position is beyond what this daemon holds"),
            )),
        )
            .into_response();
    }

    Sse::new(event_stream(state, id, cursor))
        .keep_alive(KeepAlive::new().interval(KEEP_ALIVE))
        .into_response()
}

/// Extracts the resume cursor from `Last-Event-ID`.
///
/// # Errors
///
/// Returns a message describing the rejected value when the header is present but is not a
/// positive integer.
fn resume_cursor(headers: &HeaderMap) -> Result<RunEventSequence, &'static str> {
    let Some(value) = headers.get("last-event-id") else {
        // A fresh client gets the retained history rather than only future events. A client
        // that wants only new events sends a cursor naming the highest sequence it holds.
        return Ok(RunEventSequence::first());
    };
    let text = value
        .to_str()
        .map_err(|_| "the Last-Event-ID header is not valid text")?;
    let position: u32 = text
        .trim()
        .parse()
        .map_err(|_| "the Last-Event-ID header is not a positive integer")?;
    // Zero is refused along with non-numeric values: the domain makes zero unrepresentable
    // because a zero-based and a one-based stream look identical to a client that only appends,
    // and the difference surfaces as one dropped event on reconnect.
    RunEventSequence::new(position)
        .map_err(|_| "the Last-Event-ID header is not a positive integer")
}

fn bad_request(message: &'static str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(rest_error(ErrorCode::Validation, safe(message))),
    )
        .into_response()
}

/// Maps a storage failure onto the shared envelope without echoing source text.
fn storage_error(error: &DatabaseError) -> Response {
    let (status, code, message) = match error {
        DatabaseError::RunNotFound | DatabaseError::RunEventNotFound => (
            StatusCode::NOT_FOUND,
            ErrorCode::Validation,
            "no run exists for the requested identifier",
        ),
        DatabaseError::StoredRunInvalid { .. }
        | DatabaseError::StoredRunEventInvalid { .. }
        | DatabaseError::StoredSessionInvalid { .. } => (
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::Internal,
            "the stored run state is not internally consistent",
        ),
        _ => (
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::Internal,
            "the local database is not available",
        ),
    };
    (status, Json(rest_error(code, safe(message)))).into_response()
}

/// The state one stream advances through.
struct StreamState {
    gateway: GatewayState,
    run_id: String,
    next: RunEventSequence,
    buffered: VecDeque<StoredRunEvent>,
    finished: bool,
}

/// Streams a run's stored events from `cursor` until the run settles.
///
/// Built from `unfold` rather than a generator macro so this module adds no dependency, and so
/// each step is one bounded read whose progress is visible in the state it carries.
fn event_stream(
    gateway: GatewayState,
    run_id: String,
    cursor: RunEventSequence,
) -> impl futures_util::Stream<Item = Result<Event, Infallible>> + Send {
    let initial = StreamState {
        gateway,
        run_id,
        next: cursor,
        buffered: VecDeque::new(),
        finished: false,
    };

    futures_util::stream::unfold(initial, |mut current| async move {
        loop {
            if let Some(event) = current.buffered.pop_front() {
                return Some((Ok(encode(&event)), current));
            }
            if current.finished {
                return None;
            }

            let request = ReplayRequest::new(current.next, DEFAULT_PAGE).ok()?;
            let events =
                match read_run_events(current.gateway.database(), &current.run_id, request).await {
                    Ok(events) => events,
                    Err(error) => {
                        // The stream has already sent its status line, so a failure cannot become
                        // a status code. It is delivered as an `error` event carrying the shared
                        // envelope, so the client sees a typed failure rather than an unexplained
                        // close.
                        current.finished = true;
                        return Some((Ok(error_frame(&error)), current));
                    }
                };

            if events.is_empty() {
                if run_has_settled(&current.gateway, &current.run_id).await {
                    return None;
                }
                tokio::time::sleep(POLL_INTERVAL).await;
                continue;
            }

            // A terminal event is the run's last event, so the stream is marked finished once
            // the page holding it is drained. Marking it now rather than after the next empty
            // read avoids one unnecessary poll against a log that is already closed.
            let settled = events
                .last()
                .is_some_and(|event| event.kind().is_terminal());
            if let Some(last) = events.last() {
                current.next = last.sequence().next().unwrap_or(current.next);
            }
            current.buffered.extend(events);
            current.finished = settled;
        }
    })
}

/// Reports whether a run's highest stored event is terminal.
///
/// Reads the last event rather than a run column: a settlement is recorded in the run row, but
/// the log is what this stream delivers and the two are written by the same transaction. Using
/// the log makes the stopping condition derive from the record the client is reading.
async fn run_has_settled(gateway: &GatewayState, run_id: &str) -> bool {
    let Ok(Some(highest)) = highest_run_event_sequence(gateway.database(), run_id).await else {
        // A read failure here ends the stream. Retrying would loop against a database that is
        // already refusing reads, and a client that reconnects re-reads the same position.
        return true;
    };
    let Ok(request) = ReplayRequest::new(highest, 1) else {
        return true;
    };
    match read_run_events(gateway.database(), run_id, request).await {
        Ok(events) => events
            .last()
            .is_some_and(|event| event.kind().is_terminal()),
        Err(_) => true,
    }
}

/// Builds an SSE `error` frame for a mid-stream failure.
fn error_frame(error: &DatabaseError) -> Event {
    let (code, message) = match error {
        DatabaseError::RunNotFound | DatabaseError::RunEventNotFound => (
            ErrorCode::Validation,
            "no run exists for the requested identifier",
        ),
        DatabaseError::StoredRunInvalid { .. }
        | DatabaseError::StoredRunEventInvalid { .. }
        | DatabaseError::StoredSessionInvalid { .. } => (
            ErrorCode::Internal,
            "the stored run state is not internally consistent",
        ),
        _ => (ErrorCode::Internal, "the run stream could not be read"),
    };
    let body = serde_json::to_string(&rest_error(code, safe(message)))
        .unwrap_or_else(|_| String::from("{}"));
    Event::default().event("error").data(body)
}

/// Encodes one stored event as an SSE frame.
///
/// The `id:` field carries the sequence, not the event UUID, because the cursor a client must
/// send back is a position in the stream. An opaque identifier would make the client's resume
/// value unrelated to the reader's query, and the mapping would need state a restart cannot
/// reconstruct.
fn encode(stored: &StoredRunEvent) -> Event {
    let body = serde_json::to_string(&to_reply(stored)).unwrap_or_else(|_| String::from("{}"));
    Event::default()
        .id(stored.sequence().get().to_string())
        .event(event_name(stored.kind()))
        .data(body)
}

/// The SSE event name for a kind, which is its stored code.
///
/// Taken from [`RunEventKind::as_str`] so a client switches on the same names it sees in the
/// durable log and in the REST page. A second naming scheme here would make the streaming and
/// historical views of one event disagree.
fn event_name(kind: RunEventKind) -> &'static str {
    kind.as_str()
}

/// Converts stored events into their wire form.
#[must_use]
pub fn to_replies(events: &[StoredRunEvent]) -> Vec<RunEventReply> {
    events.iter().map(to_reply).collect()
}

/// Converts one stored event into its wire form.
#[must_use]
pub fn to_reply(stored: &StoredRunEvent) -> RunEventReply {
    RunEventReply {
        event_id: stored.id().to_owned(),
        sequence: stored.sequence(),
        kind: stored.kind(),
        summary: stored.summary().map(ToOwned::to_owned),
        // The payload is stored as validated JSON text, so it is parsed back into a value
        // rather than forwarded as a string. A double-encoded payload is exactly what makes a
        // stream hard to debug, and a client should not have to parse twice.
        payload: serde_json::from_str(stored.payload()).unwrap_or(serde_json::Value::Null),
        correlation_id: stored.correlation_id(),
        occurred_at: stored.occurred_at(),
    }
}
