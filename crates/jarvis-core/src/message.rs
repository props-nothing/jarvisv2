//! Conversation message vocabulary.
//!
//! `P2-004` created the `messages` table and `P2-009` is the first slice that must actually write it.
//! Until now only test fixtures inserted rows, so nothing in production recorded an accepted user
//! message — which is what `docs/quality/acceptance-tests.md` A03 requires ("never loses an accepted
//! user message").
//!
//! # Why this is here and not in `jarvis-models`
//!
//! `jarvis-models` owns the provider-neutral *request* vocabulary, and `jarvis-core` owns durable
//! state. A stored message is durable state: it outlives the request that carried it, it carries a
//! sensitivity classification, and it is what a later turn replays. `jarvis-core` cannot depend on
//! `jarvis-models`, so the stored role is defined here and the executor maps between the two at the
//! adapter boundary. Duplicating a four-value enum is the cheap side of that dependency rule.

use std::fmt;
use std::str::FromStr;

use thiserror::Error;

/// Maximum bytes of one stored message's content.
///
/// The schema derives `content_bytes` from the content itself rather than trusting a writer, so it
/// bounds nothing on its own. `docs/architecture/security.md` requires bounded payloads, and an
/// unbounded message is a way to make one row arbitrarily large, so the bound is enforced here.
pub const MAX_MESSAGE_CONTENT_BYTES: usize = 256 * 1024;

/// Who authored one stored message.
///
/// The value set is identical to [`MessageSource`] by coincidence of the schema's two columns: one
/// records *who spoke* and the other records *where the content came from*, and a system message
/// authored by JARVIS is both `system` and `system`. They are kept as separate types because they
/// answer different questions and collapsing them would make a change to one silently change the
/// other's meaning.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum MessageRole {
    /// JARVIS's own instructions.
    System,
    /// The end user.
    User,
    /// The model.
    Assistant,
    /// A tool result.
    Tool,
}

impl MessageRole {
    /// Returns the stable snake-case wire/storage code.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Tool => "tool",
        }
    }
}

impl fmt::Display for MessageRole {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for MessageRole {
    type Err = InvalidMessage;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "system" => Ok(Self::System),
            "user" => Ok(Self::User),
            "assistant" => Ok(Self::Assistant),
            "tool" => Ok(Self::Tool),
            _ => Err(InvalidMessage::UnknownRole),
        }
    }
}

/// Where one stored message's content originated.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum MessageSource {
    /// The end user typed it.
    User,
    /// A model produced it.
    Model,
    /// A tool produced it.
    Tool,
    /// JARVIS itself produced it.
    System,
}

impl MessageSource {
    /// Returns the stable snake-case wire/storage code.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Model => "model",
            Self::Tool => "tool",
            Self::System => "system",
        }
    }
}

impl fmt::Display for MessageSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for MessageSource {
    type Err = InvalidMessage;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "user" => Ok(Self::User),
            "model" => Ok(Self::Model),
            "tool" => Ok(Self::Tool),
            "system" => Ok(Self::System),
            _ => Err(InvalidMessage::UnknownSource),
        }
    }
}

/// Why a message could not be accepted.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum InvalidMessage {
    /// The content was empty.
    #[error("the message content is empty")]
    Empty,
    /// The content exceeded [`MAX_MESSAGE_CONTENT_BYTES`].
    #[error("the message content exceeds its byte bound")]
    TooLong,
    /// A stored role was not one of the four the schema allows.
    #[error("the stored message role is unknown")]
    UnknownRole,
    /// A stored source was not one the schema allows.
    #[error("the stored message source is unknown")]
    UnknownSource,
    /// A tool result carried no identifier for the call it answers.
    #[error("a tool message must name the call it answers")]
    ToolWithoutCall,
    /// A non-tool message carried a call identifier, which would misrepresent history.
    #[error("only a tool message may name a tool call")]
    NonToolWithCall,
}

/// One validated message to store.
///
/// `content_bytes` is deliberately absent: it is derived from `content` here and checked by the
/// migration, so a caller cannot store a size that disagrees with the text it describes. That is
/// the same rule `RunEventPayload` follows for its own bound.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewMessage {
    content: String,
    role: MessageRole,
    source: MessageSource,
    sensitivity: crate::Sensitivity,
    tool_call_id: Option<String>,
}

impl NewMessage {
    /// Validates a message.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidMessage`] for empty or oversized content, or for an inconsistent relation
    /// between the role and the tool-call identifier.
    pub fn new(
        content: impl Into<String>,
        role: MessageRole,
        source: MessageSource,
        sensitivity: crate::Sensitivity,
        tool_call_id: Option<String>,
    ) -> Result<Self, InvalidMessage> {
        let content = content.into();
        if content.is_empty() {
            return Err(InvalidMessage::Empty);
        }
        if content.len() > MAX_MESSAGE_CONTENT_BYTES {
            return Err(InvalidMessage::TooLong);
        }
        // A tool result must name the call it answers, and only a tool message may name one. The two
        // cases are written as one arm each rather than four, because the rule is about the
        // *combination* rather than about either value alone.
        match (role, tool_call_id.is_some()) {
            (MessageRole::Tool, false) => return Err(InvalidMessage::ToolWithoutCall),
            (MessageRole::Tool, true) | (_, false) => {}
            (_, true) => return Err(InvalidMessage::NonToolWithCall),
        }
        Ok(Self {
            content,
            role,
            source,
            sensitivity,
            tool_call_id,
        })
    }

    /// Builds a message the user sent.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidMessage`] for empty or oversized content.
    pub fn user(content: impl Into<String>) -> Result<Self, InvalidMessage> {
        Self::new(
            content,
            MessageRole::User,
            MessageSource::User,
            crate::Sensitivity::Internal,
            None,
        )
    }

    /// Builds a message a model produced.
    ///
    /// The sensitivity is the request's destination ceiling, so a stored answer carries the
    /// classification it was produced under rather than a default that could understate it.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidMessage`] for empty or oversized content.
    pub fn assistant(
        content: impl Into<String>,
        sensitivity: crate::Sensitivity,
    ) -> Result<Self, InvalidMessage> {
        Self::new(
            content,
            MessageRole::Assistant,
            MessageSource::Model,
            sensitivity,
            None,
        )
    }

    /// Returns the content.
    #[must_use]
    pub fn content(&self) -> &str {
        &self.content
    }

    /// Returns the derived byte length, which is what the migration checks.
    #[must_use]
    pub fn content_bytes(&self) -> i64 {
        // A `usize` cannot exceed `i64` for any value the length bound above admits, so the
        // conversion is lossless in practice; saturating rather than wrapping keeps a hypothetical
        // overflow from storing a small number beside a large row.
        i64::try_from(self.content.len()).unwrap_or(i64::MAX)
    }

    /// Returns the author role.
    #[must_use]
    pub const fn role(&self) -> MessageRole {
        self.role
    }

    /// Returns the content's origin.
    #[must_use]
    pub const fn source(&self) -> MessageSource {
        self.source
    }

    /// Returns the sensitivity classification.
    #[must_use]
    pub const fn sensitivity(&self) -> crate::Sensitivity {
        self.sensitivity
    }

    /// Returns the tool call this message answers, when it is a tool result.
    #[must_use]
    pub fn tool_call_id(&self) -> Option<&str> {
        self.tool_call_id.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_role_and_source_round_trips_through_its_stored_code() {
        for role in [
            MessageRole::System,
            MessageRole::User,
            MessageRole::Assistant,
            MessageRole::Tool,
        ] {
            assert_eq!(role.as_str().parse::<MessageRole>(), Ok(role));
        }
        for source in [
            MessageSource::User,
            MessageSource::Model,
            MessageSource::Tool,
            MessageSource::System,
        ] {
            assert_eq!(source.as_str().parse::<MessageSource>(), Ok(source));
        }
        assert_eq!(
            "robot".parse::<MessageRole>(),
            Err(InvalidMessage::UnknownRole)
        );
    }

    /// The byte length is derived here so it cannot disagree with the content the migration checks.
    #[test]
    fn the_byte_length_counts_bytes_not_characters() {
        let message = NewMessage::user("héllo").unwrap_or_else(|error| panic!("valid: {error}"));
        assert_eq!(message.content_bytes(), 6, "é is two bytes");
        assert_eq!(message.content().chars().count(), 5);
    }

    #[test]
    fn empty_and_oversized_content_is_refused() {
        assert_eq!(NewMessage::user(""), Err(InvalidMessage::Empty));
        let oversized = "a".repeat(MAX_MESSAGE_CONTENT_BYTES + 1);
        assert_eq!(
            NewMessage::user(oversized),
            Err(InvalidMessage::TooLong),
            "an unbounded message would let one row be arbitrarily large"
        );
    }

    /// The role and the call identifier must agree, because a tool result with no call cannot be
    /// matched to what it answers and a user message naming a call misrepresents history.
    #[test]
    fn the_role_and_the_tool_call_identifier_must_agree() {
        assert_eq!(
            NewMessage::new(
                "result",
                MessageRole::Tool,
                MessageSource::Tool,
                crate::Sensitivity::Internal,
                None,
            ),
            Err(InvalidMessage::ToolWithoutCall)
        );
        assert_eq!(
            NewMessage::new(
                "hello",
                MessageRole::User,
                MessageSource::User,
                crate::Sensitivity::Internal,
                Some("call-1".to_owned()),
            ),
            Err(InvalidMessage::NonToolWithCall)
        );
        assert!(
            NewMessage::new(
                "result",
                MessageRole::Tool,
                MessageSource::Tool,
                crate::Sensitivity::Internal,
                Some("call-1".to_owned()),
            )
            .is_ok()
        );
    }

    #[test]
    fn a_user_message_is_internal_sensitivity() {
        let message = NewMessage::user("hello").unwrap_or_else(|error| panic!("valid: {error}"));
        assert_eq!(message.sensitivity(), crate::Sensitivity::Internal);
        assert_eq!(message.role(), MessageRole::User);
        assert_eq!(message.source(), MessageSource::User);
    }
}
