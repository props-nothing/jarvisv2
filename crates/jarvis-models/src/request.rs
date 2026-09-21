use jarvis_core::CorrelationId;
use serde::{Deserialize, Serialize};

use crate::identity::ModelId;

/// Maximum number of messages in a single request.
///
/// Bounded so a runaway context builder cannot produce an unbounded request body.
pub const MAX_MESSAGES: usize = 512;

/// Maximum byte length of a single message's text.
pub const MAX_MESSAGE_BYTES: usize = 1_048_576;

/// The authority a message carries, from highest to lowest.
///
/// This mirrors the documented chain-of-command model, in which application
/// instructions outrank end-user input. The wire spelling differs between
/// compatible servers — some accept `system` where others accept `developer` — so
/// the adapter chooses the spelling and this enum records the authority.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Instructions supplied by the application, prioritized ahead of user input.
    System,
    /// Input supplied by an end user.
    User,
    /// A previous model output being replayed as conversation history.
    Assistant,
    /// The result of a tool execution, correlated to a previous tool call.
    Tool,
}

impl Role {
    /// Returns the stable snake-case wire code.
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

/// One part of a multi-part message.
///
/// Only the text part is implemented. Image references are named in the
/// architecture but require a documented data-URL contract and a size bound, so
/// they are added when a capability actually requests them rather than as a
/// placeholder that would silently accept an unbounded payload.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    /// Plain text content.
    Text {
        /// The literal text.
        text: String,
    },
}

impl ContentPart {
    /// Creates a text part.
    #[must_use]
    pub fn text(value: impl Into<String>) -> Self {
        Self::Text { text: value.into() }
    }

    /// Returns the part's text.
    #[must_use]
    pub fn as_text(&self) -> &str {
        match self {
            Self::Text { text } => text,
        }
    }
}

/// The content of a single message.
///
/// A message may be plain text or an ordered sequence of parts. Both forms are
/// represented because compatible servers accept both, and silently coercing a
/// string to a one-element array would change the request the provider receives.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub enum MessageContent {
    /// A single run of text.
    Text(String),
    /// An ordered sequence of content parts.
    Parts(Vec<ContentPart>),
}

impl MessageContent {
    /// Returns the concatenated text of every text part.
    ///
    /// Used for budgeting and for adapters whose target server accepts only a
    /// string. Concatenation is lossless for text-only content and drops nothing
    /// silently, because only text parts exist.
    #[must_use]
    pub fn text(&self) -> String {
        match self {
            Self::Text(value) => value.clone(),
            Self::Parts(parts) => parts
                .iter()
                .map(ContentPart::as_text)
                .collect::<Vec<_>>()
                .join(""),
        }
    }

    /// Returns the total byte length of the content.
    #[must_use]
    pub fn byte_len(&self) -> usize {
        match self {
            Self::Text(value) => value.len(),
            Self::Parts(parts) => parts.iter().map(|part| part.as_text().len()).sum(),
        }
    }
}

/// One conversation message.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ChatMessage {
    role: Role,
    content: MessageContent,
    tool_call_id: Option<String>,
}

impl ChatMessage {
    /// Creates a message with the given role and content.
    #[must_use]
    pub fn new(role: Role, content: MessageContent) -> Self {
        Self {
            role,
            content,
            tool_call_id: None,
        }
    }

    /// Creates a message carrying application instructions.
    #[must_use]
    pub fn system(value: impl Into<String>) -> Self {
        Self::new(Role::System, MessageContent::Text(value.into()))
    }

    /// Creates a message carrying end-user input.
    #[must_use]
    pub fn user(value: impl Into<String>) -> Self {
        Self::new(Role::User, MessageContent::Text(value.into()))
    }

    /// Creates a message replaying a previous model output.
    #[must_use]
    pub fn assistant(value: impl Into<String>) -> Self {
        Self::new(Role::Assistant, MessageContent::Text(value.into()))
    }

    /// Creates a message carrying a tool result.
    #[must_use]
    pub fn tool(tool_call_id: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: MessageContent::Text(value.into()),
            tool_call_id: Some(tool_call_id.into()),
        }
    }

    /// Returns the message role.
    #[must_use]
    pub const fn role(&self) -> Role {
        self.role
    }

    /// Returns the message content.
    #[must_use]
    pub const fn content(&self) -> &MessageContent {
        &self.content
    }

    /// Returns the tool call this message answers, when it is a tool result.
    #[must_use]
    pub fn tool_call_id(&self) -> Option<&str> {
        self.tool_call_id.as_deref()
    }

    /// Returns whether the message is internally consistent.
    ///
    /// A tool result without a correlation identifier cannot be matched to the call
    /// it answers, and a non-tool message carrying one would misrepresent history.
    #[must_use]
    pub fn is_consistent(&self) -> bool {
        match self.role {
            Role::Tool => self.tool_call_id.is_some(),
            _ => self.tool_call_id.is_none(),
        }
    }
}

/// A provider-neutral request for one model completion.
///
/// The request carries a [`CorrelationId`] rather than a run identifier so this
/// crate does not depend on run persistence. The adapter forwards it to the
/// provider as a client request identifier, which keeps a correlation identity
/// recoverable when no response header arrives at all.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ChatRequest {
    model: ModelId,
    messages: Vec<ChatMessage>,
    correlation_id: CorrelationId,
    max_output_tokens: Option<u32>,
    temperature_milli: Option<u16>,
    stop: Vec<String>,
    /// When true, the adapter must request token usage from the provider.
    ///
    /// Usage is requested explicitly so a streaming run persists a cost record
    /// instead of silently omitting one.
    include_usage: bool,
}

impl ChatRequest {
    /// Creates a request that asks the provider for usage accounting.
    #[must_use]
    pub fn new(model: ModelId, messages: Vec<ChatMessage>, correlation_id: CorrelationId) -> Self {
        Self {
            model,
            messages,
            correlation_id,
            max_output_tokens: None,
            temperature_milli: None,
            stop: Vec::new(),
            include_usage: true,
        }
    }

    /// Sets the output token ceiling.
    #[must_use]
    pub const fn with_max_output_tokens(mut self, value: Option<u32>) -> Self {
        self.max_output_tokens = value;
        self
    }

    /// Sets the sampling temperature in thousandths.
    ///
    /// Integer thousandths avoid a floating-point value whose decimal text varies
    /// by platform, which would make a recorded request fixture unstable.
    #[must_use]
    pub const fn with_temperature_milli(mut self, value: Option<u16>) -> Self {
        self.temperature_milli = value;
        self
    }

    /// Sets the stop sequences.
    #[must_use]
    pub fn with_stop(mut self, value: Vec<String>) -> Self {
        self.stop = value;
        self
    }

    /// Sets whether the provider must report token usage.
    #[must_use]
    pub const fn with_include_usage(mut self, value: bool) -> Self {
        self.include_usage = value;
        self
    }

    /// Returns the selected model.
    #[must_use]
    pub const fn model(&self) -> &ModelId {
        &self.model
    }

    /// Returns the conversation messages.
    #[must_use]
    pub fn messages(&self) -> &[ChatMessage] {
        &self.messages
    }

    /// Returns the correlation identity to forward as a client request identifier.
    #[must_use]
    pub const fn correlation_id(&self) -> CorrelationId {
        self.correlation_id
    }

    /// Returns the output token ceiling.
    #[must_use]
    pub const fn max_output_tokens(&self) -> Option<u32> {
        self.max_output_tokens
    }

    /// Returns the sampling temperature in thousandths.
    #[must_use]
    pub const fn temperature_milli(&self) -> Option<u16> {
        self.temperature_milli
    }

    /// Returns the stop sequences.
    #[must_use]
    pub fn stop(&self) -> &[String] {
        &self.stop
    }

    /// Returns whether usage accounting was requested.
    #[must_use]
    pub const fn include_usage(&self) -> bool {
        self.include_usage
    }

    /// Returns the total content byte length of all messages.
    #[must_use]
    pub fn content_byte_len(&self) -> usize {
        self.messages
            .iter()
            .map(|message| message.content().byte_len())
            .sum()
    }

    /// Returns whether the request is within every documented bound.
    ///
    /// Adapters call this before transmitting, so an oversized request fails as a
    /// local validation error instead of as a provider rejection that costs a
    /// round trip and may cost money.
    #[must_use]
    pub fn is_within_bounds(&self) -> bool {
        !self.messages.is_empty()
            && self.messages.len() <= MAX_MESSAGES
            && self
                .messages
                .iter()
                .all(|message| message.content().byte_len() <= MAX_MESSAGE_BYTES)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model() -> ModelId {
        ModelId::new("gpt-oss:20b").unwrap_or_else(|error| panic!("valid fixture model: {error}"))
    }

    #[test]
    fn text_and_part_content_agree_on_their_text() {
        let plain = MessageContent::Text("hello world".to_owned());
        let parts = MessageContent::Parts(vec![
            ContentPart::text("hello "),
            ContentPart::text("world"),
        ]);
        assert_eq!(plain.text(), parts.text());
        assert_eq!(plain.byte_len(), parts.byte_len());
    }

    #[test]
    fn tool_messages_require_a_correlation_identifier() {
        let correlated = ChatMessage::tool("call_1", "result");
        assert!(correlated.is_consistent());

        let orphan = ChatMessage::new(Role::Tool, MessageContent::Text("result".to_owned()));
        assert!(
            !orphan.is_consistent(),
            "a tool result with no call id cannot be matched to its request"
        );
    }

    #[test]
    fn a_non_tool_message_must_not_carry_a_tool_call_identifier() {
        let mut message = ChatMessage::user("hello");
        message.tool_call_id = Some("call_1".to_owned());
        assert!(!message.is_consistent());
    }

    #[test]
    fn bounds_reject_an_empty_or_oversized_request() {
        let empty = ChatRequest::new(model(), Vec::new(), CorrelationId::new());
        assert!(!empty.is_within_bounds(), "an empty request has no prompt");

        let oversized = ChatRequest::new(
            model(),
            vec![ChatMessage::user("a".repeat(MAX_MESSAGE_BYTES + 1))],
            CorrelationId::new(),
        );
        assert!(!oversized.is_within_bounds());

        let acceptable = ChatRequest::new(
            model(),
            vec![ChatMessage::user("hello")],
            CorrelationId::new(),
        );
        assert!(acceptable.is_within_bounds());
    }

    #[test]
    fn content_that_is_not_ascii_is_measured_in_bytes_not_characters() {
        // A bound expressed in characters would let a multi-byte payload through
        // four times larger than intended.
        let message = ChatMessage::user("héllo wörld");
        assert!(message.content().byte_len() > "héllo wörld".chars().count());
    }

    #[test]
    fn usage_is_requested_by_default() {
        let request =
            ChatRequest::new(model(), vec![ChatMessage::user("hi")], CorrelationId::new());
        assert!(
            request.include_usage(),
            "usage must be requested by default or cost records are silently absent"
        );
    }
}
