//! The durable run-event record: event kinds, ordering, and replay bounds.
//!
//! `docs/architecture/protocols.md` requires every stream event to carry a sequence and
//! an event ID and requires the server to "replay from durable records within retention
//! or return an explicit resync requirement". This module owns the vocabulary and the
//! ordering rules that make that answerable; `jarvis_storage::run_event_repository` makes
//! them durable. The decision to have a per-run record at all, and why it is not the
//! Phase 6 event bus, is recorded in ADR-0011.
//!
//! # Ordering is a property of the run, not the process
//!
//! `sequence` is scoped to one run and starts at 1. A global counter would make one
//! run's stream depend on unrelated work and would leak activity volume between runs to
//! any client that can read two of them. [`RunEventSequence::first`] and
//! [`RunEventSequence::next`] exist so the "starts at 1, increments by exactly one"
//! rule is one implementation rather than a convention each writer re-derives.
//!
//! # Unknown kinds fail; unknown fields do not
//!
//! `docs/architecture/runtime-and-models.md` draws this line: "Unknown additive fields
//! are tolerated within a compatible version; unknown event kinds fail the adapter
//! session safely." A client switches on the kind, so a kind it does not recognize
//! cannot be rendered or safely ignored. [`RunEventKind`] is therefore a closed set and
//! [`RunEventKind::from_str`] rejects anything else, while [`RunEventPayload`] keeps
//! unmodelled JSON fields rather than dropping them.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use thiserror::Error;

/// Bounded maximum length of an event summary, matching the migration's `CHECK`.
pub const MAX_EVENT_SUMMARY_CHARS: usize = 1024;
/// Bounded maximum size of one stored event payload in bytes, matching the migration.
pub const MAX_EVENT_PAYLOAD_BYTES: usize = 65_536;
/// Bounded maximum number of events one replay request may return.
///
/// A replay is paginated, and this is the ceiling. It exists because "give me the whole
/// stream" on a long-lived run is an unbounded read that a client can trigger for free.
pub const MAX_REPLAY_EVENTS: u32 = 1000;

/// Explains why an event kind or summary was rejected.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum InvalidRunEvent {
    /// The text did not name a known event kind.
    #[error("unknown run event kind")]
    UnknownKind,
    /// The summary was empty or exceeded the stored bound.
    #[error("run event summary is empty or exceeds 1024 characters")]
    SummaryInvalid,
    /// The payload was empty, not JSON, or exceeded the stored bound.
    #[error("run event payload is not bounded JSON")]
    PayloadInvalid,
    /// A sequence number was zero.
    #[error("a run event sequence starts at 1, not 0")]
    SequenceIsZero,
    /// A requested replay size was zero or exceeded [`MAX_REPLAY_EVENTS`].
    #[error("the requested replay size must be between 1 and 1000 events")]
    ReplaySizeInvalid,
}

/// The normalized kind of one run event.
///
/// Names match the spellings `docs/architecture/runtime-and-models.md` lists for the
/// runtime event contract, and the migration `0004_run_events.sql` constrains the column
/// to the same set, so a kind the domain rejects cannot be stored through a direct write.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RunEventKind {
    /// The run moved between two states.
    StateChanged,
    /// A concise operational fact changed. Never hidden chain-of-thought.
    ActivityUpdated,
    /// A piece of the answer was produced.
    OutputDelta,
    /// The answer finished.
    OutputCompleted,
    /// The run wants a capability it does not own authority for.
    ToolRequested,
    /// The run is waiting on a human decision.
    ApprovalRequested,
    /// A durable artifact was produced.
    ArtifactCreated,
    /// Usage or cost was updated.
    UsageUpdated,
    /// The run completed successfully.
    RunCompleted,
    /// The run failed.
    RunFailed,
    /// The run was cancelled.
    RunCancelled,
    /// A protocol heartbeat. Not content, and never rendered as an answer.
    Heartbeat,
}

impl RunEventKind {
    /// Returns the stable snake-case wire/storage code.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::StateChanged => "state_changed",
            Self::ActivityUpdated => "activity_updated",
            Self::OutputDelta => "output_delta",
            Self::OutputCompleted => "output_completed",
            Self::ToolRequested => "tool_requested",
            Self::ApprovalRequested => "approval_requested",
            Self::ArtifactCreated => "artifact_created",
            Self::UsageUpdated => "usage_updated",
            Self::RunCompleted => "run_completed",
            Self::RunFailed => "run_failed",
            Self::RunCancelled => "run_cancelled",
            Self::Heartbeat => "heartbeat",
        }
    }

    /// Returns whether this kind ends the run's stream.
    ///
    /// A terminal kind is the last event a run emits. Sending anything after it would
    /// mean either the run settled twice or the stream is describing two runs, and a
    /// reader that stops at the first terminal event would never notice.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::RunCompleted | Self::RunFailed | Self::RunCancelled
        )
    }

    /// Returns whether this kind carries answer text a client renders.
    #[must_use]
    pub const fn carries_output(self) -> bool {
        matches!(self, Self::OutputDelta | Self::OutputCompleted)
    }

    /// Returns every kind, for exhaustive tests and documentation.
    #[must_use]
    pub const fn all() -> [Self; 12] {
        [
            Self::StateChanged,
            Self::ActivityUpdated,
            Self::OutputDelta,
            Self::OutputCompleted,
            Self::ToolRequested,
            Self::ApprovalRequested,
            Self::ArtifactCreated,
            Self::UsageUpdated,
            Self::RunCompleted,
            Self::RunFailed,
            Self::RunCancelled,
            Self::Heartbeat,
        ]
    }
}

impl fmt::Display for RunEventKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for RunEventKind {
    type Err = InvalidRunEvent;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "state_changed" => Ok(Self::StateChanged),
            "activity_updated" => Ok(Self::ActivityUpdated),
            "output_delta" => Ok(Self::OutputDelta),
            "output_completed" => Ok(Self::OutputCompleted),
            "tool_requested" => Ok(Self::ToolRequested),
            "approval_requested" => Ok(Self::ApprovalRequested),
            "artifact_created" => Ok(Self::ArtifactCreated),
            "usage_updated" => Ok(Self::UsageUpdated),
            "run_completed" => Ok(Self::RunCompleted),
            "run_failed" => Ok(Self::RunFailed),
            "run_cancelled" => Ok(Self::RunCancelled),
            "heartbeat" => Ok(Self::Heartbeat),
            _ => Err(InvalidRunEvent::UnknownKind),
        }
    }
}

impl Serialize for RunEventKind {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for RunEventKind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(de::Error::custom)
    }
}

/// A validated, one-based position of an event within one run's stream.
///
/// Zero is unrepresentable rather than merely discouraged. A stream numbered from zero
/// and one numbered from one look identical to a client that only appends, and the
/// difference surfaces as a silently dropped first event on reconnect — the case the
/// durable record exists to serve.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RunEventSequence(u32);

impl RunEventSequence {
    /// Returns the first sequence of any run.
    #[must_use]
    pub const fn first() -> Self {
        Self(1)
    }

    /// Validates a stored or requested sequence number.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidRunEvent::SequenceIsZero`] for zero.
    pub fn new(value: u32) -> Result<Self, InvalidRunEvent> {
        if value == 0 {
            return Err(InvalidRunEvent::SequenceIsZero);
        }
        Ok(Self(value))
    }

    /// Returns the next sequence, or `None` at the numeric ceiling.
    ///
    /// Saturating rather than wrapping: wrapping to 1 would restart the stream and make
    /// `UNIQUE (run_id, sequence)` reject a legitimate event, and silently reusing a
    /// sequence number would corrupt a replay. Exhaustion is reported as `None` so the
    /// caller must decide.
    #[must_use]
    pub const fn next(self) -> Option<Self> {
        match self.0.checked_add(1) {
            Some(next) => Some(Self(next)),
            None => None,
        }
    }

    /// Returns the underlying number.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

impl fmt::Display for RunEventSequence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

// Serialization is manual rather than derived, matching `RunState`. A derived
// newtype representation would put the value inside a one-field object, so the wire
// form would be `{"sequence":{"0":3}}` rather than `"sequence":3`. More importantly,
// deserialization must go through `RunEventSequence::new` so a wire value of 0 is
// rejected here rather than becoming a stored sequence the migration forbids. A derived
// impl would bypass that check entirely.
impl Serialize for RunEventSequence {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u32(self.0)
    }
}

impl<'de> Deserialize<'de> for RunEventSequence {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = u32::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

/// A bounded operational summary attached to an event.
///
/// Counted in **characters** because the migration bounds the column with `length()` on
/// a TEXT value, which counts characters. Counting bytes here would reject text the
/// schema accepts, presenting a unit mismatch as a caller error — the same trap
/// `agent_runs.objective` and `messages.content_bytes` already document.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct EventSummary(String);

impl EventSummary {
    /// Validates a summary.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidRunEvent::SummaryInvalid`] when the summary is empty or longer
    /// than [`MAX_EVENT_SUMMARY_CHARS`].
    pub fn new(value: impl Into<String>) -> Result<Self, InvalidRunEvent> {
        let value = value.into();
        let length = value.chars().count();
        if length == 0 || length > MAX_EVENT_SUMMARY_CHARS {
            return Err(InvalidRunEvent::SummaryInvalid);
        }
        Ok(Self(value))
    }

    /// Borrows the validated summary.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A validated event payload: bounded JSON.
///
/// Stored as text and bounded by **bytes**, because `docs/architecture/security.md`
/// requires bounded output and a byte is the unit a transport actually enforces. The
/// bound is asserted against the serialized form so a payload cannot be small as a
/// value and large on the wire.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunEventPayload(String);

impl RunEventPayload {
    /// Validates a serialized JSON payload.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidRunEvent::PayloadInvalid`] when the text is not a JSON object or
    /// array, or when its byte length exceeds [`MAX_EVENT_PAYLOAD_BYTES`].
    pub fn new(value: impl Into<String>) -> Result<Self, InvalidRunEvent> {
        let value = value.into();
        let bytes = value.len();
        if bytes == 0 || bytes > MAX_EVENT_PAYLOAD_BYTES {
            return Err(InvalidRunEvent::PayloadInvalid);
        }
        // Parse rather than pattern-match on the first character: `"{` is also the prefix
        // of malformed JSON, and storing a payload a reader cannot parse would turn a
        // writer bug into a reader crash much later.
        let parsed: serde_json::Value =
            serde_json::from_str(&value).map_err(|_| InvalidRunEvent::PayloadInvalid)?;
        if !parsed.is_object() && !parsed.is_array() {
            return Err(InvalidRunEvent::PayloadInvalid);
        }
        Ok(Self(value))
    }

    /// Borrows the validated JSON text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A request for part of one run's event stream.
///
/// Both bounds are explicit because "send everything" is an unbounded read a client can
/// trigger for free, and a replay that silently truncates is worse than one that refuses:
/// a truncated replay looks like a complete history to a client that cannot tell.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReplayRequest {
    from: RunEventSequence,
    limit: u32,
}

impl ReplayRequest {
    /// Creates a bounded replay request.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidRunEvent::ReplaySizeInvalid`] when `limit` is zero or greater
    /// than [`MAX_REPLAY_EVENTS`].
    pub fn new(from: RunEventSequence, limit: u32) -> Result<Self, InvalidRunEvent> {
        if limit == 0 || limit > MAX_REPLAY_EVENTS {
            return Err(InvalidRunEvent::ReplaySizeInvalid);
        }
        Ok(Self { from, limit })
    }

    /// Returns the first sequence to include.
    #[must_use]
    pub const fn from(self) -> RunEventSequence {
        self.from
    }

    /// Returns the maximum number of events to return.
    #[must_use]
    pub const fn limit(self) -> u32 {
        self.limit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The kind set must stay identical to the migration's `CHECK` list.
    #[test]
    fn every_kind_round_trips_through_its_stored_code() {
        for kind in RunEventKind::all() {
            let parsed = kind.as_str().parse::<RunEventKind>();
            assert_eq!(parsed, Ok(kind), "{kind} must round-trip through its code");
        }
    }

    /// A client switches on the kind, so an unknown one must be refused rather than
    /// carried and guessed at.
    #[test]
    fn an_unknown_kind_is_refused() {
        assert_eq!(
            "reasoning_trace".parse::<RunEventKind>(),
            Err(InvalidRunEvent::UnknownKind)
        );
        assert_eq!(
            "StateChanged".parse::<RunEventKind>(),
            Err(InvalidRunEvent::UnknownKind),
            "matching is case-sensitive so the stored spelling is unambiguous"
        );
    }

    /// Exactly three kinds end a stream, and each corresponds to a terminal run state.
    #[test]
    fn only_settlement_kinds_are_terminal() {
        for kind in RunEventKind::all() {
            let expected = matches!(
                kind,
                RunEventKind::RunCompleted | RunEventKind::RunFailed | RunEventKind::RunCancelled
            );
            assert_eq!(kind.is_terminal(), expected, "{kind} terminal flag");
        }
    }

    /// Only the two output kinds carry answer text. A heartbeat carrying text would be
    /// rendered as an answer by any client that switches on "has text".
    #[test]
    fn only_output_kinds_carry_text() {
        for kind in RunEventKind::all() {
            let expected = matches!(
                kind,
                RunEventKind::OutputDelta | RunEventKind::OutputCompleted
            );
            assert_eq!(
                kind.carries_output(),
                expected,
                "{kind} carries_output flag"
            );
        }
    }

    /// A stream starts at 1, so a zero stored or requested sequence is a defect.
    #[test]
    fn a_sequence_cannot_be_zero() {
        assert_eq!(
            RunEventSequence::new(0),
            Err(InvalidRunEvent::SequenceIsZero)
        );
        assert_eq!(RunEventSequence::first().get(), 1);
        assert_eq!(RunEventSequence::new(1), Ok(RunEventSequence::first()));
    }

    /// The last sequence must not wrap, because wrapping would collide with the unique
    /// constraint and silently corrupt a replay.
    #[test]
    fn the_last_sequence_reports_exhaustion_instead_of_wrapping() {
        let last = RunEventSequence::new(u32::MAX);
        assert_eq!(last.map(RunEventSequence::next), Ok(None));
        assert_eq!(
            RunEventSequence::first().next().map(RunEventSequence::get),
            Some(2)
        );
    }

    /// The bound is characters, matching the column's `length()` semantics, not bytes.
    #[test]
    fn a_summary_is_bounded_in_characters_not_bytes() {
        let ascii_limit = "a".repeat(MAX_EVENT_SUMMARY_CHARS);
        assert!(EventSummary::new(ascii_limit.clone()).is_ok());
        assert_eq!(
            EventSummary::new(format!("{ascii_limit}a")),
            Err(InvalidRunEvent::SummaryInvalid)
        );

        // "é" is 2 UTF-8 bytes. At the character limit this is 2x the byte count, so a
        // byte-based bound would reject a summary the schema accepts.
        let multibyte = "é".repeat(MAX_EVENT_SUMMARY_CHARS);
        assert_eq!(multibyte.len(), MAX_EVENT_SUMMARY_CHARS * 2);
        assert!(EventSummary::new(multibyte).is_ok());

        assert_eq!(EventSummary::new(""), Err(InvalidRunEvent::SummaryInvalid));
    }

    /// Malformed JSON must be refused at construction. Storing it would turn a writer bug
    /// into a reader failure much later, after the context to diagnose it is gone.
    #[test]
    fn a_payload_must_be_bounded_json() {
        assert!(RunEventPayload::new(r#"{"text":"hi"}"#).is_ok());
        assert!(RunEventPayload::new(r"[1,2,3]").is_ok());

        for rejected in [
            "",
            "{",
            r#"{"text":"hi""#,
            r#""just a string""#,
            "42",
            "null",
        ] {
            assert_eq!(
                RunEventPayload::new(rejected),
                Err(InvalidRunEvent::PayloadInvalid),
                "{rejected:?} must be refused"
            );
        }
    }

    /// A payload that is only valid as a non-object must still be refused, and the byte
    /// bound must be enforced on the serialized form.
    #[test]
    fn a_payload_is_bounded_in_bytes() {
        let oversized = format!(r#"{{"text":"{}"}}"#, "a".repeat(MAX_EVENT_PAYLOAD_BYTES));
        assert!(
            oversized.len() > MAX_EVENT_PAYLOAD_BYTES,
            "fixture must exceed the bound"
        );
        assert_eq!(
            RunEventPayload::new(oversized),
            Err(InvalidRunEvent::PayloadInvalid)
        );
    }

    /// An unbounded replay request is refused rather than served, because a replay that
    /// silently truncates is indistinguishable from a complete history.
    #[test]
    fn a_replay_request_is_bounded() {
        let from = RunEventSequence::first();
        assert_eq!(
            ReplayRequest::new(from, 0),
            Err(InvalidRunEvent::ReplaySizeInvalid)
        );
        assert_eq!(
            ReplayRequest::new(from, MAX_REPLAY_EVENTS + 1),
            Err(InvalidRunEvent::ReplaySizeInvalid)
        );
        let at_limit = ReplayRequest::new(from, MAX_REPLAY_EVENTS);
        assert_eq!(at_limit.map(ReplayRequest::limit), Ok(MAX_REPLAY_EVENTS));
        assert_eq!(at_limit.map(ReplayRequest::from), Ok(from));
    }

    /// The stored spellings are the wire spellings. A derive attribute could change one
    /// silently, and a client persisted against the old name would break.
    #[test]
    fn kinds_serialize_as_their_stored_codes() {
        let json = serde_json::to_string(&RunEventKind::OutputDelta)
            .ok()
            .unwrap_or_default();
        assert_eq!(json, "\"output_delta\"");

        let parsed: Option<RunEventKind> = serde_json::from_str("\"run_failed\"").ok();
        assert_eq!(parsed, Some(RunEventKind::RunFailed));
    }

    /// The sequence serializes as a bare number, not a wrapped object, and a wire value of
    /// zero is refused. This is why the impls are hand-written: a derived impl would use
    /// the newtype form and would accept 0, producing a value the storage constraint
    /// rejects later as an opaque failure.
    #[test]
    fn a_sequence_serializes_as_a_number_and_rejects_zero_on_the_wire() {
        let sequence = RunEventSequence::first();
        assert_eq!(
            serde_json::to_string(&sequence).ok().as_deref(),
            Some("1"),
            "a derived impl would emit {{\"0\":1}}"
        );

        let parsed: Option<RunEventSequence> = serde_json::from_str("7").ok();
        assert_eq!(parsed.map(RunEventSequence::get), Some(7));

        assert!(
            serde_json::from_str::<RunEventSequence>("0").is_err(),
            "a wire sequence of 0 must be refused, not stored"
        );
    }
}
