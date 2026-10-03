//! Client-side contract for the `/api/v1` run surface: request paths and the SSE frame decoder.
//!
//! `docs/architecture/protocols.md` puts the HTTP surface behind the same versioned contract as
//! local IPC, and ADR-0011 keeps its DTOs in this crate so a domain change cannot silently change
//! a client-visible shape. Path building and stream decoding belong here for the same reason: they
//! are the **client half** of one contract, so a daemon that renames an event or moves a route and
//! a client that still expects the old one is a change to a shared file rather than two files that
//! can drift apart.
//!
//! # Why the SSE decoder is hand-written
//!
//! `docs/research/integrations/rust-http-client-and-sse.md` records this decision for the model
//! adapter and it applies unchanged here: `eventsource-stream` is a `nom`-based transformer, and
//! adopting it would add a parser dependency for the narrow subset the daemon emits — `event`,
//! `id`, and `data` lines separated by a blank line, plus `:` comment keep-alives. The same record
//! fixes the shape this module reuses: buffer **bytes** and split on separators, because a TCP
//! chunk boundary can fall inside a multi-byte character.
//!
//! # The declared event name must match the payload
//!
//! A frame's `event:` name is compared against the parsed payload's [`RunEventKind`]. The daemon
//! writes both from `RunEventKind::as_str`, so the check costs nothing, and it is what turns a
//! one-sided rename into a refusal instead of an event silently typed as something else.

use jarvis_core::RunEventKind;
use thiserror::Error;

use crate::{JSON_CONTENT_TYPE, RunEventReply, SSE_CONTENT_TYPE, WireError};

/// Base path of the versioned HTTP API.
pub const API_BASE_PATH: &str = "/api/v1";

/// Maximum characters accepted in a run identifier interpolated into a request path.
///
/// A run identifier is a `UUIDv7` (36 characters) minted by the daemon, so this bound is far above
/// any legitimate value and exists to refuse an interpolated segment that is not one.
pub const MAX_PATH_SEGMENT_CHARS: usize = 64;

/// The `Accept` value a streaming request must send.
pub const SSE_ACCEPT: &str = SSE_CONTENT_TYPE;

/// The `Content-Type` value a JSON request body must send.
pub const JSON_BODY_CONTENT_TYPE: &str = JSON_CONTENT_TYPE;

/// The `event:` name the daemon uses for a mid-stream failure.
///
/// Matches `apps/jarvisd/src/sse.rs`, which sends `Event::default().event("error")`. Named here
/// rather than inline so the client and the daemon name it once.
pub const ERROR_EVENT_NAME: &str = "error";

/// Maximum bytes of buffered, unframed stream data before the stream is rejected.
///
/// A daemon that never emits a frame separator would otherwise grow the buffer without bound. It
/// is generous relative to one event payload (`jarvis_core::MAX_EVENT_PAYLOAD_BYTES`).
pub const MAX_PENDING_BYTES: usize = 1024 * 1024;

/// Why a request path could not be built.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum RunPathError {
    /// The identifier is empty, oversized, or contains characters that are unsafe in a URL path.
    ///
    /// Deliberately neutral about **which** identifier, because this type is used for run, memory, skill, and
    /// tool paths. It said "the run identifier" while being returned from a tool preview, which sent a reader to
    /// inspect a run they had not mentioned — the defect class this repository keeps finding, where a message
    /// identifies something other than the thing that failed.
    #[error("the identifier is not a safe URL path segment")]
    UnsafeRunId,
}

/// Returns a validated path segment for `id`.
///
/// The identifier is validated rather than interpolated verbatim because it goes into a URL. A
/// value containing `/`, `?`, `#`, or a percent-escape would change the request's target — turning
/// a read of one run into a different route, or appending a query parameter. The value arrives from
/// the daemon's own reply, so a failure here means a broken or hostile response, and refusing is
/// the honest outcome rather than sending a mangled path.
///
/// # Errors
///
/// Returns [`RunPathError::UnsafeRunId`] when `id` is empty, longer than
/// [`MAX_PATH_SEGMENT_CHARS`], or contains anything other than ASCII alphanumerics and hyphens.
pub fn path_segment(id: &str) -> Result<&str, RunPathError> {
    validate_segment(id, |byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

/// Returns a validated path segment for a **dotted** identifier, such as a tool's `namespace.name`.
///
/// # Why this exists beside `path_segment` rather than replacing it
///
/// `path_segment` refuses `.` because a run, memory, or skill identifier is a UUID and cannot contain one. A
/// **tool** identifier is `namespace.name` by construction, so the same rule rejects every tool — which is what
/// it did: `jarvis tools preview jarvis.memory.propose` failed with "the run identifier is not a safe URL path
/// segment", and would have failed for every identifier the daemon can register. The feature was unusable and
/// the message pointed at a run.
///
/// Widening `path_segment` was rejected: it is the rule for four other path families, and a `.` there would
/// accept a value none of them can produce while loosening the check for all of them. A dot is
/// **path-safe** — RFC 3986's `pchar` includes it, and it is not a delimiter in a path segment — so the wider
/// rule is still a safety rule rather than a relaxation of one.
///
/// # Why a leading or trailing dot is still refused
///
/// `.` and `..` are not ordinary segment contents: RFC 3986 §5.2.4 **removes** them during reference
/// resolution, so a request whose path contains one reaches a different target than the text suggests — and an
/// intermediary that normalises the path could turn `a/../b` into `b`. Refusing the whole-segment cases is a
/// safety rule.
///
/// The leading/trailing refusal is broader than that and is a **grammar** rule: no `ToolId` half may begin or
/// end with a dot (`validate_segment` in `jarvis-tools` refuses it), so a value like `jarvis.` can never name a
/// tool. Accepting one would build a request for a tool that cannot exist, and the refusal is what makes that
/// reachable as an error rather than as a `404` at the daemon.
///
/// # Errors
///
/// Returns [`RunPathError::UnsafeRunId`] on the same conditions as [`path_segment`], with the set widened to the
/// characters a tool identifier may contain.
pub fn dotted_path_segment(id: &str) -> Result<&str, RunPathError> {
    // **The set is the tool identifier's own grammar, restated here.** `jarvis-protocol` cannot depend on
    // `jarvis-tools` (an adapter may depend on the protocol and not the reverse), so the set has to be written
    // out — and writing it out is where a guess goes wrong. Two attempts did:
    //
    // 1. Alphanumerics plus `.` and `-` refused `mcp.github.list_issues`, because a tool **name** may contain an
    //    underscore. The feature would have failed for the ordinary case while passing for `jarvis.memory.propose`,
    //    which is the fixture a test is most likely to use — so a hand-written set was both wrong and
    //    self-concealing.
    // 2. `is_ascii_alphanumeric` also admits **uppercase**, which no `ToolId` can contain (`validate_segment`
    //    requires lowercase), so it accepted values that can never name a tool.
    //
    // This is the grammar from `ToolId`'s `validate_segment`: lowercase letters, digits, and `_ - . :`. A test
    // pins the correspondence by asserting two real identifiers from different sources, so a divergence shows up
    // as a failing assertion rather than as a `404` an operator has to diagnose.
    validate_segment(id, |byte| {
        byte.is_ascii_lowercase()
            || byte.is_ascii_digit()
            || matches!(byte, b'_' | b'-' | b'.' | b':')
    })
    .and_then(|segment| {
        // The grammar and traversal cases. Checked after the character set so the refusal above is the one a
        // caller sees for a genuinely unsafe character, and these are reserved for the shapes a dot can create.
        // `.` and `..` are not ordinary segment contents: RFC 3986 §5.2.4 removes them during reference
        // resolution, so a request whose path contains one reaches a target other than the text suggests. No
        // `ToolId` half may begin or end with a dot either, so a whole-segment dot can never name a tool — the
        // refusal is what makes that reachable as an error rather than as a `404` at the daemon.
        if segment.starts_with('.') || segment.ends_with('.') {
            return Err(RunPathError::UnsafeRunId);
        }
        Ok(segment)
    })
}

/// Applies a shared identifier rule, so the two segment builders cannot drift apart.
///
/// Factored out rather than copied, because the length bound and the emptiness rule are the parts a second copy
/// would forget. Only the permitted set differs, and it is the parameter.
///
/// A function **pointer** rather than `&dyn Fn`: the two callers pass the two predicates below, neither
/// captures anything, and a pointer makes that visible — an elided lifetime on a borrowed closure would have to
/// be named, and naming it would suggest the parameter could outlive the call when it cannot.
fn validate_segment(id: &str, permitted: fn(u8) -> bool) -> Result<&str, RunPathError> {
    if id.is_empty() || id.chars().count() > MAX_PATH_SEGMENT_CHARS {
        return Err(RunPathError::UnsafeRunId);
    }
    if !id.bytes().all(permitted) {
        return Err(RunPathError::UnsafeRunId);
    }
    Ok(id)
}

/// Returns the path for `POST /api/v1/runs`.
#[must_use]
pub fn runs_path() -> String {
    format!("{API_BASE_PATH}/runs")
}

/// Returns the path for `GET /api/v1/runs/{id}`.
///
/// # Errors
///
/// Returns [`RunPathError::UnsafeRunId`] when the identifier is not a safe path segment.
pub fn run_path(id: &str) -> Result<String, RunPathError> {
    Ok(format!("{API_BASE_PATH}/runs/{}", path_segment(id)?))
}

/// Returns the path for `GET /api/v1/runs/{id}/stream`.
///
/// # Errors
///
/// Returns [`RunPathError::UnsafeRunId`] when the identifier is not a safe path segment.
pub fn run_stream_path(id: &str) -> Result<String, RunPathError> {
    Ok(format!("{API_BASE_PATH}/runs/{}/stream", path_segment(id)?))
}

/// One framed item from a run stream.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunStreamFrame {
    /// A run event. The frame's declared name matched the payload's kind.
    Event(Box<RunEventReply>),
    /// A failure the daemon reported mid-stream, carrying the shared error envelope.
    Error(Box<WireError>),
    /// A comment-only keep-alive frame, which carries no data.
    ///
    /// The daemon sends these as SSE **comments** rather than `heartbeat` events precisely so they
    /// never occupy a sequence number and never enter the durable log. Reporting one as an event
    /// would invite a caller to record it as run activity, so it is a distinct variant.
    KeepAlive,
}

/// Why a stream could not be decoded.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum RunStreamError {
    /// The buffer exceeded [`MAX_PENDING_BYTES`] without a frame separator.
    #[error("the run stream exceeded its framing buffer without a complete event")]
    Overflow,
    /// A frame was readable but was not a valid event or error.
    #[error("the run stream carried a frame that could not be interpreted")]
    Malformed,
}

/// An incremental decoder for the run stream.
///
/// Feed bytes with [`Self::feed`], then drain complete frames with [`Self::next_frame`]. Keeping
/// the two separate means one chunk carrying several frames yields all of them rather than leaving
/// the rest waiting for a read that may not come.
#[derive(Debug, Default)]
pub struct RunStreamDecoder {
    pending: Vec<u8>,
    overflowed: bool,
}

impl RunStreamDecoder {
    /// Creates an empty decoder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends received bytes to the framing buffer.
    ///
    /// Records overflow rather than truncating: a truncated buffer would drop the front of a frame
    /// and resynchronise mid-JSON, so the stream would look merely malformed at an arbitrary point
    /// instead of reporting that the peer violated the framing limit.
    pub fn feed(&mut self, chunk: &[u8]) {
        self.pending.extend_from_slice(chunk);
        if self.pending.len() > MAX_PENDING_BYTES {
            self.overflowed = true;
        }
    }

    /// Returns the next complete frame, if the buffer holds one.
    ///
    /// Returns `None` while more bytes are needed. A comment-only frame is consumed and reported as
    /// [`RunStreamFrame::KeepAlive`]. A block that carries no field at all is skipped, because it is
    /// not something a caller can act on and refusing the stream for it would fail on a daemon that
    /// added a field.
    ///
    /// # Errors
    ///
    /// Returns [`RunStreamError::Overflow`] once when the buffer bound was exceeded, and
    /// [`RunStreamError::Malformed`] for a frame that claims to be an event but cannot be read as
    /// one. A malformed frame is reported rather than skipped: skipping it would drop an event from
    /// a stream whose whole purpose is to account for every event.
    pub fn next_frame(&mut self) -> Option<Result<RunStreamFrame, RunStreamError>> {
        if self.overflowed {
            self.overflowed = false;
            return Some(Err(RunStreamError::Overflow));
        }
        loop {
            let (end, separator_len) = find_separator(&self.pending)?;
            let block: Vec<u8> = self.pending.drain(..end).collect();
            self.pending.drain(..separator_len);
            match interpret_block(&block) {
                Block::Ignore => {}
                Block::KeepAlive => return Some(Ok(RunStreamFrame::KeepAlive)),
                Block::Decode(name, data) => return Some(decode_frame(name.as_deref(), &data)),
            }
        }
    }
}

/// What one frame block meant.
enum Block {
    /// A block carrying nothing a caller can use.
    Ignore,
    /// A comment-only block, which is the daemon's keep-alive.
    KeepAlive,
    /// An optional event name and the data payload.
    Decode(Option<String>, String),
}

/// Finds the first blank-line separator and its byte length.
///
/// Scans bytes rather than decoding to text: a separator is ASCII and cannot occur inside a
/// multi-byte character, so this is safe on a chunk boundary that splits one.
fn find_separator(buffer: &[u8]) -> Option<(usize, usize)> {
    for index in 0..buffer.len() {
        if buffer[index..].starts_with(b"\r\n\r\n") {
            return Some((index, 4));
        }
        if buffer[index..].starts_with(b"\n\n") {
            return Some((index, 2));
        }
    }
    None
}

/// Interprets one complete event block.
fn interpret_block(block: &[u8]) -> Block {
    let mut name: Option<String> = None;
    let mut data: Option<String> = None;
    let mut saw_comment = false;

    for line in block.split(|byte| *byte == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.is_empty() {
            continue;
        }
        if line.starts_with(b":") {
            // A comment. `axum::response::sse::KeepAlive::DEFAULT_KEEP_ALIVE` is exactly `:`, so
            // comments are the daemon's keep-alive mechanism rather than a curiosity.
            saw_comment = true;
            continue;
        }
        let Some(separator) = line.iter().position(|byte| *byte == b':') else {
            // A line with no colon is a field name with an empty value in the SSE format. The
            // daemon writes no such field, so it carries nothing this decoder needs.
            continue;
        };
        let field = &line[..separator];
        let raw = &line[separator + 1..];
        // One optional leading space is part of the format rather than of the value.
        let value = raw.strip_prefix(b" ").unwrap_or(raw);
        match field {
            b"event" => name = Some(String::from_utf8_lossy(value).into_owned()),
            b"data" => {
                let text = String::from_utf8_lossy(value).into_owned();
                match &mut data {
                    // Repeated `data:` lines join with a newline, which is the format's rule and
                    // is what a re-wrapping intermediary produces.
                    Some(existing) => {
                        existing.push('\n');
                        existing.push_str(&text);
                    }
                    None => data = Some(text),
                }
            }
            // `id` carries the sequence, which the payload also carries, and unknown fields are
            // ignored so an added field stays a compatible change.
            _ => {}
        }
    }

    match (name, data) {
        (Some(name), Some(data)) => Block::Decode(Some(name), data),
        // Data with no name is the format's default `message` event. The daemon always names its
        // events, so this only arises from an intermediary, and decoding it is more useful than
        // discarding it.
        (None, Some(data)) => Block::Decode(None, data),
        // A comment-only block is the daemon's keep-alive.
        (None, None) if saw_comment => Block::KeepAlive,
        // Everything else carries nothing. In particular a name with no data claims to be an event
        // but has no payload, which is an empty block for the caller's purposes; refusing the whole
        // stream for it would be stricter than the format requires.
        (_, None) => Block::Ignore,
    }
}

/// Decodes a frame's payload into a typed stream item.
fn decode_frame(name: Option<&str>, data: &str) -> Result<RunStreamFrame, RunStreamError> {
    if name == Some(ERROR_EVENT_NAME) {
        let error: WireError = serde_json::from_str(data).map_err(|_| RunStreamError::Malformed)?;
        return Ok(RunStreamFrame::Error(Box::new(error)));
    }

    let reply: RunEventReply = serde_json::from_str(data).map_err(|_| RunStreamError::Malformed)?;
    if let Some(name) = name {
        // The daemon writes both from `RunEventKind::as_str`, so a disagreement means one side
        // renamed the kind. Refusing it is what stops an event being typed as something it is not.
        if name != reply.kind.as_str() {
            return Err(RunStreamError::Malformed);
        }
    }
    Ok(RunStreamFrame::Event(Box::new(reply)))
}

/// How a caller should present one event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StreamReading {
    /// The run settled successfully.
    Completed,
    /// The run failed.
    Failed,
    /// The run was cancelled.
    Cancelled,
    /// A state change, which carries no terminal meaning on its own.
    StateChanged,
    /// One **incremental** piece of answer text, to be rendered as it arrives.
    OutputDelta,
    /// The **whole** answer, sent once generation finished.
    ///
    /// Distinct from [`Self::OutputDelta`] and not interchangeable with it. A client that renders
    /// both prints the answer twice, because the completed event repeats every fragment it already
    /// received. That is not hypothetical: it is what the first end-to-end run of this stream did,
    /// and the two kinds carried the same classification until the duplication was seen on screen.
    OutputCompleted,
    /// The run is waiting for a person to decide a held action.
    ///
    /// Not terminal — the run is parked, not settled — but a client that keeps waiting for it to finish
    /// would wait for a human it never told. The event's payload names the approval.
    AwaitingApproval,
    /// Anything else worth showing as an operational line.
    Activity,
}

impl StreamReading {
    /// Classifies an event by its kind.
    #[must_use]
    pub const fn of(kind: RunEventKind) -> Self {
        match kind {
            RunEventKind::RunCompleted => Self::Completed,
            RunEventKind::RunFailed => Self::Failed,
            RunEventKind::RunCancelled => Self::Cancelled,
            RunEventKind::StateChanged => Self::StateChanged,
            RunEventKind::OutputDelta => Self::OutputDelta,
            RunEventKind::OutputCompleted => Self::OutputCompleted,
            RunEventKind::ApprovalRequested => Self::AwaitingApproval,
            _ => Self::Activity,
        }
    }

    /// Returns whether this reading ends the stream.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

/// An event's answer text, when it carries any.
///
/// Reads the `text` field the daemon's event payload uses, as a borrowed slice so a caller can
/// print it without copying. Keeping the field name here stops it spreading into callers as a
/// stringly `payload["text"]` lookup.
#[must_use]
pub fn output_text(payload: &serde_json::Value) -> Option<&str> {
    payload.get("text")?.as_str()
}

/// A run's state name from an event payload, when the payload carries one.
#[must_use]
pub fn state_name(payload: &serde_json::Value) -> Option<&str> {
    payload.get("state")?.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;
    use jarvis_core::{ErrorCode, RunEventSequence};

    /// A frame exactly as `apps/jarvisd/src/sse.rs` writes one: `id`, `event`, `data`, blank line.
    ///
    /// `axum` writes each field as `name: value\n` and `Event::finalize` appends the blank line, so
    /// this is the shape on the wire rather than an idealised one.
    fn frame(sequence: u32, kind: &str, payload: &str) -> String {
        format!("id: {sequence}\nevent: {kind}\ndata: {payload}\n\n")
    }

    /// A `RunEventReply` payload carrying one extra field inside `payload`.
    fn event_payload(kind: &str, extra: &str) -> String {
        format!(
            r#"{{"event_id":"018f0000-0000-7000-8000-000000000000","sequence":1,"kind":"{kind}","payload":{{{extra}}},"correlation_id":"018f0000-0000-7000-8000-000000000001","occurred_at":"2026-09-22T10:00:00Z"}}"#
        )
    }

    #[test]
    fn a_complete_frame_decodes_to_a_typed_event() {
        let mut decoder = RunStreamDecoder::new();
        decoder.feed(
            frame(
                1,
                "state_changed",
                &event_payload("state_changed", r#""state":"received""#),
            )
            .as_bytes(),
        );

        let Some(Ok(RunStreamFrame::Event(reply))) = decoder.next_frame() else {
            panic!("a state_changed frame must decode as an event");
        };
        assert_eq!(reply.kind, RunEventKind::StateChanged);
        assert_eq!(reply.sequence, RunEventSequence::first());
        assert_eq!(state_name(&reply.payload), Some("received"));
        assert!(decoder.next_frame().is_none(), "the buffer must be drained");
    }

    /// One chunk carrying several frames must yield all of them, not leave the rest waiting.
    #[test]
    fn one_chunk_yields_every_frame_it_contains() {
        let mut decoder = RunStreamDecoder::new();
        decoder.feed(
            format!(
                "{}{}",
                frame(
                    1,
                    "state_changed",
                    &event_payload("state_changed", r#""state":"received""#)
                ),
                frame(
                    2,
                    "output_delta",
                    &event_payload("output_delta", r#""text":"hi""#)
                ),
            )
            .as_bytes(),
        );

        let Some(Ok(RunStreamFrame::Event(first))) = decoder.next_frame() else {
            panic!("the first frame must decode");
        };
        let Some(Ok(RunStreamFrame::Event(second))) = decoder.next_frame() else {
            panic!("the second frame must decode");
        };
        assert!(decoder.next_frame().is_none());

        assert_eq!(first.kind, RunEventKind::StateChanged);
        assert_eq!(second.kind, RunEventKind::OutputDelta);
        assert_eq!(output_text(&second.payload), Some("hi"));
        assert_eq!(StreamReading::of(second.kind), StreamReading::OutputDelta);
    }

    /// A fragment and the completed answer are DIFFERENT readings, because rendering both prints the
    /// answer twice. Asserted as a difference rather than as two equal values, which is the whole
    /// point of the distinction.
    #[test]
    fn a_delta_and_a_completed_answer_are_different_readings() {
        assert_ne!(
            StreamReading::of(RunEventKind::OutputDelta),
            StreamReading::of(RunEventKind::OutputCompleted),
            "a client that treats these alike repeats the whole answer"
        );
        assert_eq!(
            StreamReading::of(RunEventKind::OutputCompleted),
            StreamReading::OutputCompleted
        );
    }

    /// A frame split across two reads must still decode, because a chunk boundary is arbitrary.
    #[test]
    fn a_frame_split_across_reads_decodes_once() {
        let text = frame(
            1,
            "state_changed",
            &event_payload("state_changed", r#""state":"received""#),
        );
        let (head, tail) = text.split_at(text.len() / 2);

        let mut decoder = RunStreamDecoder::new();
        decoder.feed(head.as_bytes());
        assert!(decoder.next_frame().is_none(), "a partial frame must wait");
        decoder.feed(tail.as_bytes());
        assert!(
            decoder.next_frame().is_some(),
            "the frame must decode once the remainder arrives"
        );
    }

    /// A multi-byte character split across reads must survive, which is why the buffer holds bytes.
    #[test]
    fn a_multibyte_character_split_across_reads_is_not_corrupted() {
        let text = frame(
            1,
            "output_delta",
            &event_payload("output_delta", r#""text":"é""#),
        );
        let bytes = text.as_bytes();
        let split = bytes
            .iter()
            .position(|byte| *byte >= 0x80)
            .unwrap_or_else(|| panic!("the fixture must contain a multibyte character"));

        let mut decoder = RunStreamDecoder::new();
        decoder.feed(&bytes[..split]);
        assert!(decoder.next_frame().is_none());
        decoder.feed(&bytes[split..]);

        let Some(Ok(RunStreamFrame::Event(reply))) = decoder.next_frame() else {
            panic!("the event must decode");
        };
        assert_eq!(output_text(&reply.payload), Some("é"));
    }

    /// The daemon's keep-alive is `axum`'s default comment frame. It must be reported as a
    /// keep-alive rather than as an empty event, or a caller would record it as run activity.
    #[test]
    fn a_comment_keep_alive_is_not_an_event() {
        let mut decoder = RunStreamDecoder::new();
        decoder.feed(b":\n\n");
        assert_eq!(
            decoder.next_frame(),
            Some(Ok(RunStreamFrame::KeepAlive)),
            "the default axum keep-alive is a comment frame"
        );
        assert!(decoder.next_frame().is_none());
    }

    /// A mid-stream failure arrives as an `error` event carrying the shared envelope.
    #[test]
    fn an_error_frame_decodes_to_the_shared_envelope() {
        let body = r#"{"code":"internal","message":"the run stream could not be read","retryable":false,"correlation_id":"018f0000-0000-7000-8000-000000000002"}"#;
        let mut decoder = RunStreamDecoder::new();
        decoder.feed(frame(3, ERROR_EVENT_NAME, body).as_bytes());

        let Some(Ok(RunStreamFrame::Error(error))) = decoder.next_frame() else {
            panic!("an error frame must decode to the error envelope");
        };
        assert_eq!(error.code, ErrorCode::Internal);
        assert_eq!(error.message.as_str(), "the run stream could not be read");
    }

    /// A frame whose declared name disagrees with its payload is refused. Accepting it would type
    /// the event as whatever the payload said while the daemon meant something else.
    #[test]
    fn a_mismatched_event_name_is_refused() {
        let mut decoder = RunStreamDecoder::new();
        decoder.feed(
            frame(
                1,
                "run_completed",
                &event_payload("output_delta", r#""text":"x""#),
            )
            .as_bytes(),
        );
        assert_eq!(
            decoder.next_frame(),
            Some(Err(RunStreamError::Malformed)),
            "a declared name that disagrees with the payload must be refused"
        );
    }

    /// A buffer that never yields a separator is bounded rather than growing without limit.
    #[test]
    fn an_unframed_stream_is_bounded() {
        let mut decoder = RunStreamDecoder::new();
        decoder.feed(b"data: partial");
        assert!(decoder.next_frame().is_none());
        decoder.feed(&vec![b'x'; MAX_PENDING_BYTES]);
        assert_eq!(
            decoder.next_frame(),
            Some(Err(RunStreamError::Overflow)),
            "an unbounded frame must be reported rather than buffered forever"
        );
    }

    /// Repeated `data:` lines are joined by a newline, which is the format's rule for multi-line
    /// data and what a re-wrapping intermediary produces.
    #[test]
    fn repeated_data_lines_are_joined() {
        let mut decoder = RunStreamDecoder::new();
        decoder.feed(
            b"event: error\ndata: {\"code\":\"internal\",\ndata: \"message\":\"split\",\"retryable\":false,\"correlation_id\":\"018f0000-0000-7000-8000-000000000003\"}\n\n",
        );
        assert!(
            matches!(decoder.next_frame(), Some(Ok(RunStreamFrame::Error(_)))),
            "data lines split by a re-wrapping intermediary must still decode"
        );
    }

    #[test]
    fn run_paths_are_versioned_and_use_a_validated_segment() {
        assert_eq!(runs_path(), "/api/v1/runs");
        let id = "018f0000-0000-7000-8000-000000000000";
        assert_eq!(
            run_path(id).unwrap_or_else(|error| panic!("a uuid is a safe segment: {error}")),
            format!("/api/v1/runs/{id}")
        );
        assert_eq!(
            run_stream_path(id).unwrap_or_else(|error| panic!("a uuid is a safe segment: {error}")),
            format!("/api/v1/runs/{id}/stream")
        );
    }

    /// An identifier that could change the request's target must be refused rather than
    /// interpolated. Every case below is a way a value could escape its path segment.
    #[test]
    fn an_unsafe_run_identifier_is_refused() {
        for unsafe_id in ["", "a/b", "a?from=0", "a#frag", "%2e%2e", "a b", "id\n"] {
            assert_eq!(
                run_path(unsafe_id),
                Err(RunPathError::UnsafeRunId),
                "{unsafe_id:?} must not be interpolated into a request path"
            );
        }
        assert_eq!(
            path_segment(&"a".repeat(MAX_PATH_SEGMENT_CHARS + 1)),
            Err(RunPathError::UnsafeRunId)
        );
    }

    /// **The UUID rule refuses every tool identifier, which is why a second rule exists.**
    ///
    /// This asserts the *defect* rather than the fix, because the two facts are separate and only one of them
    /// is obvious: `path_segment` is the right rule for a UUID and the wrong one for `namespace.name`. Stating
    /// it here is what stops a later reader from "simplifying" the two builders back into one — the
    /// simplification is exactly the bug, and it makes `jarvis tools preview` fail for every identifier the
    /// daemon can register.
    #[test]
    fn the_uuid_rule_refuses_a_dotted_tool_identifier() {
        assert_eq!(
            path_segment("jarvis.memory.propose"),
            Err(RunPathError::UnsafeRunId),
            "a tool identifier is dotted by construction, so the uuid rule rejects all of them"
        );
        // And the dotted rule accepts it, which is the whole reason it was added.
        assert_eq!(
            dotted_path_segment("jarvis.memory.propose"),
            Ok("jarvis.memory.propose")
        );
        // A dotted namespace, which is what an MCP server's tools carry.
        assert_eq!(
            dotted_path_segment("mcp.github.list_issues"),
            Ok("mcp.github.list_issues")
        );
    }

    /// **The dotted rule is still a safety rule: the character set moves and nothing else does.**
    ///
    /// The point of a separate builder is that the permitted set is the *only* difference, so this asserts that
    /// every escape the UUID rule refuses is still refused. A builder that had relaxed the check rather than
    /// widened the set would pass the test above and fail here.
    ///
    /// **The uppercase case is the one a hand-written set gets wrong.** A tool identifier is lowercase by
    /// construction, so accepting `Jarvis.memory` would build a request for a tool that cannot exist — and it
    /// would do so silently, because the daemon answers `404` and the caller learns nothing about the case.
    #[test]
    fn the_dotted_rule_refuses_every_path_escape() {
        for unsafe_id in [
            "",
            "a/b",
            "a?from=0",
            "a#frag",
            "%2e%2e",
            "a b",
            "id\n",
            "..",
            "./a",
            "a\\b",
            "a@b",
            "Jarvis.memory",
            "jarvis.Memory",
        ] {
            assert_eq!(
                dotted_path_segment(unsafe_id),
                Err(RunPathError::UnsafeRunId),
                "{unsafe_id:?} must not be interpolated into a request path"
            );
        }
        assert_eq!(
            dotted_path_segment(&"a".repeat(MAX_PATH_SEGMENT_CHARS + 1)),
            Err(RunPathError::UnsafeRunId),
            "the length bound is shared, not re-implemented"
        );
        // A leading or trailing dot has no meaning in the identifier grammar and is refused, so a value cannot
        // be padded into looking like a different tool.
        assert_eq!(
            dotted_path_segment(".jarvis"),
            Err(RunPathError::UnsafeRunId)
        );
        assert_eq!(
            dotted_path_segment("jarvis."),
            Err(RunPathError::UnsafeRunId)
        );
        // A colon is **permitted**, because the grammar allows it — an MCP server may namespace a tool with one.
        // Asserted so a later tightening does not silently refuse a legal identifier.
        assert_eq!(
            dotted_path_segment("mcp:github/list"),
            Err(RunPathError::UnsafeRunId)
        );
        assert_eq!(
            dotted_path_segment("mcp.github:list"),
            Ok("mcp.github:list")
        );
    }
}
