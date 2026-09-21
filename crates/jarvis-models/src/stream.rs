use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::response::{FinishReason, OutputContent};
use crate::usage::TokenUsage;

/// Explains why an event sequence was rejected.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum StreamEventError {
    /// The sequence number repeated or moved backwards.
    #[error("stream sequence did not advance")]
    OutOfOrder,
    /// The sequence number skipped ahead, so events were lost.
    #[error("stream sequence gap")]
    Gap,
    /// The stream produced no terminal event before ending.
    #[error("stream ended without a terminal event")]
    NotFinished,
    /// A terminal event was followed by more events.
    #[error("stream produced events after completion")]
    AlreadyFinished,
}

/// One normalized event from a streaming model call.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum StreamEvent {
    /// Generation began; emitted at most once, before any output.
    Started,
    /// A fragment of generated text.
    TextDelta {
        /// The fragment, which may be empty.
        text: String,
    },
    /// A requested tool invocation, emitted once the provider has finished it.
    ToolCall {
        /// The requested invocation.
        call: crate::response::ToolCall,
    },
    /// Updated token usage, which may arrive more than once.
    Usage {
        /// The usage reported so far.
        usage: TokenUsage,
    },
    /// Generation ended with the provider's stated reason.
    Finished {
        /// Why generation stopped.
        reason: FinishReason,
    },
}

impl StreamEvent {
    /// Returns the stable snake-case event name.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::TextDelta { .. } => "text_delta",
            Self::ToolCall { .. } => "tool_call",
            Self::Usage { .. } => "usage",
            Self::Finished { .. } => "finished",
        }
    }

    /// Returns whether this event terminates the stream.
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        matches!(self, Self::Finished { .. })
    }

    /// Applies the event to an accumulator.
    pub fn apply(self, summary: &mut StreamSummary) {
        match self {
            Self::Started => summary.started = true,
            Self::TextDelta { text } => summary.text.push_str(&text),
            Self::ToolCall { call } => summary.tool_calls.push(call),
            Self::Usage { usage } => summary.usage = Some(usage),
            Self::Finished { reason } => summary.finish_reason = Some(reason),
        }
    }
}

/// A sequence-numbered event envelope.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StreamEnvelope {
    sequence: u64,
    event: StreamEvent,
}

impl StreamEnvelope {
    /// Wraps an event with its sequence number.
    #[must_use]
    pub const fn new(sequence: u64, event: StreamEvent) -> Self {
        Self { sequence, event }
    }

    /// Returns the contiguous sequence number.
    #[must_use]
    pub const fn sequence(&self) -> u64 {
        self.sequence
    }

    /// Returns the carried event.
    #[must_use]
    pub const fn event(&self) -> &StreamEvent {
        &self.event
    }
}

/// The accumulated result of a streamed response.
///
/// Output is only usable when [`Self::finish_reason`] is present, so a consumer
/// cannot accidentally treat a truncated stream as a complete answer.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct StreamSummary {
    output: Vec<OutputContent>,
    #[serde(skip)]
    text: String,
    tool_calls: Vec<crate::response::ToolCall>,
    usage: Option<TokenUsage>,
    finish_reason: Option<FinishReason>,
    #[serde(skip)]
    started: bool,
}

impl StreamSummary {
    /// Returns whether a start event was seen.
    #[must_use]
    pub const fn has_started(&self) -> bool {
        self.started
    }

    /// Returns the concatenated text produced so far.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Returns the requested tool invocations.
    #[must_use]
    pub fn tool_calls(&self) -> &[crate::response::ToolCall] {
        &self.tool_calls
    }

    /// Returns the last reported usage.
    #[must_use]
    pub const fn usage(&self) -> Option<TokenUsage> {
        self.usage
    }

    /// Returns why generation stopped, when it did.
    #[must_use]
    pub const fn finish_reason(&self) -> Option<FinishReason> {
        self.finish_reason
    }

    /// Returns the output items in presentation order.
    ///
    /// Text produced before the first tool call is emitted first, followed by the
    /// tool calls and then any remaining text, so the ordered output matches what a
    /// non-streaming response would have produced.
    #[must_use]
    pub fn output(&self) -> Vec<OutputContent> {
        let mut output = Vec::new();
        if !self.text.is_empty() {
            output.push(OutputContent::Text {
                text: self.text.clone(),
            });
        }
        for call in &self.tool_calls {
            output.push(OutputContent::ToolCall { call: call.clone() });
        }
        output
    }
}

/// Enforces the ordering and completion rules of a normalized event stream.
///
/// The validator is the mechanism that turns "the socket closed" into a decision
/// about whether an answer exists. Without it, a dropped connection and a finished
/// response look identical to a consumer that only concatenates deltas.
#[derive(Clone, Debug, Default)]
pub struct StreamValidator {
    next_sequence: u64,
    finished: bool,
    summary: StreamSummary,
}

impl StreamValidator {
    /// Creates a validator positioned before the first event.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Validates and applies the next event.
    ///
    /// # Errors
    ///
    /// Returns [`StreamEventError`] when the event repeats or skips a sequence
    /// number, or when it arrives after a terminal event.
    pub fn accept(&mut self, envelope: StreamEnvelope) -> Result<(), StreamEventError> {
        if self.finished {
            return Err(StreamEventError::AlreadyFinished);
        }
        if envelope.sequence < self.next_sequence {
            return Err(StreamEventError::OutOfOrder);
        }
        if envelope.sequence > self.next_sequence {
            return Err(StreamEventError::Gap);
        }

        self.next_sequence += 1;
        self.finished = envelope.event.is_terminal();
        envelope.event.apply(&mut self.summary);
        Ok(())
    }

    /// Finalizes the stream, rejecting a sequence that ended without a terminal event.
    ///
    /// # Errors
    ///
    /// Returns [`StreamEventError::NotFinished`] when no terminal event was seen.
    pub fn finish(self) -> Result<StreamSummary, StreamEventError> {
        if !self.finished {
            return Err(StreamEventError::NotFinished);
        }
        Ok(self.summary)
    }

    /// Returns whether a terminal event has been accepted.
    #[must_use]
    pub const fn is_finished(&self) -> bool {
        self.finished
    }

    /// Returns the next expected sequence number.
    #[must_use]
    pub const fn next_sequence(&self) -> u64 {
        self.next_sequence
    }

    /// Returns the partial result accumulated so far.
    #[must_use]
    pub const fn summary(&self) -> &StreamSummary {
        &self.summary
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_truncated_stream_does_not_yield_a_complete_answer() {
        let mut validator = StreamValidator::new();
        validator
            .accept(StreamEnvelope::new(0, StreamEvent::Started))
            .unwrap_or_else(|error| panic!("valid first event: {error}"));
        validator
            .accept(StreamEnvelope::new(
                1,
                StreamEvent::TextDelta {
                    text: "half an answ".to_owned(),
                },
            ))
            .unwrap_or_else(|error| panic!("valid delta: {error}"));

        assert_eq!(
            validator.finish(),
            Err(StreamEventError::NotFinished),
            "a dropped connection must not be reported as a finished answer"
        );
    }

    #[test]
    fn a_skipped_sequence_number_is_detected_as_a_gap() {
        let mut validator = StreamValidator::new();
        validator
            .accept(StreamEnvelope::new(0, StreamEvent::Started))
            .unwrap_or_else(|error| panic!("valid first event: {error}"));

        assert_eq!(
            validator.accept(StreamEnvelope::new(
                2,
                StreamEvent::TextDelta {
                    text: "lost".to_owned(),
                }
            )),
            Err(StreamEventError::Gap)
        );
    }

    #[test]
    fn events_after_a_terminal_event_are_rejected() {
        let mut validator = StreamValidator::new();
        validator
            .accept(StreamEnvelope::new(
                0,
                StreamEvent::Finished {
                    reason: FinishReason::Stop,
                },
            ))
            .unwrap_or_else(|error| panic!("valid terminal event: {error}"));

        assert_eq!(
            validator.accept(StreamEnvelope::new(1, StreamEvent::Started)),
            Err(StreamEventError::AlreadyFinished)
        );
    }

    #[test]
    fn a_repeated_sequence_number_is_rejected() {
        let mut validator = StreamValidator::new();
        validator
            .accept(StreamEnvelope::new(0, StreamEvent::Started))
            .unwrap_or_else(|error| panic!("valid first event: {error}"));

        assert_eq!(
            validator.accept(StreamEnvelope::new(0, StreamEvent::Started)),
            Err(StreamEventError::OutOfOrder)
        );
    }

    #[test]
    fn a_finished_stream_accumulates_text_usage_and_reason() {
        let mut validator = StreamValidator::new();
        for envelope in [
            StreamEnvelope::new(0, StreamEvent::Started),
            StreamEnvelope::new(
                1,
                StreamEvent::TextDelta {
                    text: "hello ".to_owned(),
                },
            ),
            StreamEnvelope::new(
                2,
                StreamEvent::TextDelta {
                    text: "world".to_owned(),
                },
            ),
            StreamEnvelope::new(
                3,
                StreamEvent::Usage {
                    usage: TokenUsage::new(12, 2),
                },
            ),
            StreamEnvelope::new(
                4,
                StreamEvent::Finished {
                    reason: FinishReason::Stop,
                },
            ),
        ] {
            validator
                .accept(envelope)
                .unwrap_or_else(|error| panic!("valid stream: {error}"));
        }

        let summary = validator
            .finish()
            .unwrap_or_else(|error| panic!("finished stream: {error}"));
        assert_eq!(summary.text(), "hello world");
        assert_eq!(summary.usage(), Some(TokenUsage::new(12, 2)));
        assert_eq!(summary.finish_reason(), Some(FinishReason::Stop));
        assert_eq!(summary.output().len(), 1);
    }

    #[test]
    fn an_unknown_provider_finish_reason_still_terminates_the_stream() {
        // A new provider label must end the stream without failing it, because
        // adding a label is a backwards-compatible provider change.
        let mut validator = StreamValidator::new();
        validator
            .accept(StreamEnvelope::new(
                0,
                StreamEvent::Finished {
                    reason: FinishReason::from_provider_label("brand_new_label"),
                },
            ))
            .unwrap_or_else(|error| panic!("unknown label must still validate: {error}"));

        let summary = validator
            .finish()
            .unwrap_or_else(|error| panic!("finished stream: {error}"));
        assert_eq!(summary.finish_reason(), Some(FinishReason::Other));
        assert!(
            !summary
                .finish_reason()
                .unwrap_or(FinishReason::Other)
                .is_complete()
        );
    }
}
