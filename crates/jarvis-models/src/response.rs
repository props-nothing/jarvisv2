use serde::{Deserialize, Serialize};

use crate::identity::ModelId;
use crate::request::{MessageContent, Role};
use crate::usage::TokenUsage;

/// Why the provider stopped generating.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    /// The model produced a complete answer.
    Stop,
    /// The model stopped because it emitted a tool call.
    ToolCalls,
    /// The output token ceiling was reached.
    Length,
    /// The provider filtered the output on safety grounds.
    ContentFilter,
    /// The provider stopped for a reason JARVIS does not model.
    ///
    /// A finish reason JARVIS cannot classify is never treated as a clean
    /// completion, because "unknown" would otherwise be indistinguishable from
    /// success and would hide a provider-side content filter.
    Other,
    /// The stream ended without a terminal event.
    ///
    /// Synthesized by the adapter when a stream closes early; it must never be
    /// reported as [`Self::Stop`].
    Incomplete,
}

impl FinishReason {
    /// Returns the stable snake-case wire code.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stop => "stop",
            Self::ToolCalls => "tool_calls",
            Self::Length => "length",
            Self::ContentFilter => "content_filter",
            Self::Other => "other",
            Self::Incomplete => "incomplete",
        }
    }

    /// Classifies a provider's own finish-reason label.
    ///
    /// Compatible servers use several spellings for the same condition, so the
    /// labels are matched explicitly and anything unrecognized becomes
    /// [`Self::Other`] rather than being assumed benign. Adding a new provider label
    /// is a backwards-compatible change, so unknown labels must not fail the
    /// response.
    #[must_use]
    pub fn from_provider_label(label: &str) -> Self {
        match label {
            "stop" | "end_turn" | "eos" => Self::Stop,
            "tool_calls" | "tool_use" | "function_call" => Self::ToolCalls,
            "length" | "max_tokens" | "max_output_tokens" => Self::Length,
            "content_filter" | "safety" | "refusal" => Self::ContentFilter,
            _ => Self::Other,
        }
    }

    /// Returns whether the reason represents a fully generated answer.
    #[must_use]
    pub const fn is_complete(self) -> bool {
        matches!(self, Self::Stop | Self::ToolCalls | Self::Length)
    }

    /// Returns whether the provider blocked the content.
    #[must_use]
    pub const fn is_refusal(self) -> bool {
        matches!(self, Self::ContentFilter)
    }
}

/// A complete or partial tool invocation requested by the model.
///
/// A tool call is a **request for an effect**. JARVIS policy decides whether the
/// effect may happen; this type only records what the model asked for.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolCall {
    id: String,
    name: String,
    arguments: String,
}

impl ToolCall {
    /// Creates a tool call.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            arguments: arguments.into(),
        }
    }

    /// Returns the provider-assigned call identifier used to send the result back.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the requested tool name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the raw, unvalidated argument text.
    ///
    /// The text is provider output, not a parsed request. It must pass schema
    /// validation and authorization before any effect occurs, and it is never
    /// interpreted as a JARVIS command.
    #[must_use]
    pub fn arguments(&self) -> &str {
        &self.arguments
    }
}

/// One item of model output.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OutputContent {
    /// Generated text.
    Text {
        /// The generated text.
        text: String,
    },
    /// A requested tool invocation.
    ToolCall {
        /// The requested invocation.
        call: ToolCall,
    },
}

/// A completed model response.
///
/// The output is an ordered list rather than a single string, because a response
/// may interleave text and tool calls and the model's text is not guaranteed to be
/// the first item.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ChatResponse {
    provider: crate::identity::ProviderId,
    model: ModelId,
    output: Vec<OutputContent>,
    finish_reason: FinishReason,
    usage: Option<TokenUsage>,
    provider_request_id: Option<crate::error::ProviderRequestId>,
}

impl ChatResponse {
    /// Creates a response.
    #[must_use]
    pub fn new(
        provider: crate::identity::ProviderId,
        model: ModelId,
        output: Vec<OutputContent>,
        finish_reason: FinishReason,
    ) -> Self {
        Self {
            provider,
            model,
            output,
            finish_reason,
            usage: None,
            provider_request_id: None,
        }
    }

    /// Attaches token usage.
    #[must_use]
    pub const fn with_usage(mut self, usage: Option<TokenUsage>) -> Self {
        self.usage = usage;
        self
    }

    /// Attaches the provider-assigned request identifier.
    #[must_use]
    pub fn with_provider_request_id(mut self, id: Option<crate::error::ProviderRequestId>) -> Self {
        self.provider_request_id = id;
        self
    }

    /// Returns the provider that served the request.
    #[must_use]
    pub const fn provider(&self) -> &crate::identity::ProviderId {
        &self.provider
    }

    /// Returns the model that served the request.
    ///
    /// Recorded separately from the model that was *requested*, because a provider
    /// may serve an alias and only the response reveals what actually ran.
    #[must_use]
    pub const fn model(&self) -> &ModelId {
        &self.model
    }

    /// Returns the ordered output items.
    #[must_use]
    pub fn output(&self) -> &[OutputContent] {
        &self.output
    }

    /// Returns why generation stopped.
    #[must_use]
    pub const fn finish_reason(&self) -> FinishReason {
        self.finish_reason
    }

    /// Returns token usage when the provider reported it.
    #[must_use]
    pub const fn usage(&self) -> Option<&TokenUsage> {
        self.usage.as_ref()
    }

    /// Returns the provider-assigned request identifier.
    #[must_use]
    pub const fn provider_request_id(&self) -> Option<&crate::error::ProviderRequestId> {
        self.provider_request_id.as_ref()
    }

    /// Concatenates every text output item.
    #[must_use]
    pub fn text(&self) -> String {
        self.output
            .iter()
            .filter_map(|item| match item {
                OutputContent::Text { text } => Some(text.as_str()),
                OutputContent::ToolCall { .. } => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }

    /// Returns every requested tool invocation.
    #[must_use]
    pub fn tool_calls(&self) -> Vec<&ToolCall> {
        self.output
            .iter()
            .filter_map(|item| match item {
                OutputContent::Text { .. } => None,
                OutputContent::ToolCall { call } => Some(call),
            })
            .collect()
    }

    /// Replays the response as an assistant message for the next turn.
    #[must_use]
    pub fn as_assistant_message(&self) -> crate::request::ChatMessage {
        crate::request::ChatMessage::new(Role::Assistant, MessageContent::Text(self.text()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::ProviderId;

    #[test]
    fn an_unrecognized_finish_reason_is_not_treated_as_success() {
        // A provider that adds or renames a reason must not make a filtered or
        // truncated answer look complete.
        let unknown = FinishReason::from_provider_label("some_new_reason");
        assert_eq!(unknown, FinishReason::Other);
        assert!(!unknown.is_complete());
        assert!(
            !unknown.is_refusal(),
            "unknown must not be claimed as a refusal either"
        );
    }

    #[test]
    fn documented_provider_labels_map_to_canonical_reasons() {
        assert_eq!(
            FinishReason::from_provider_label("stop"),
            FinishReason::Stop
        );
        assert_eq!(
            FinishReason::from_provider_label("tool_calls"),
            FinishReason::ToolCalls
        );
        assert_eq!(
            FinishReason::from_provider_label("length"),
            FinishReason::Length
        );
        assert_eq!(
            FinishReason::from_provider_label("content_filter"),
            FinishReason::ContentFilter
        );
    }

    #[test]
    fn text_is_collected_from_the_whole_output_not_only_the_first_item() {
        // Official documentation warns that the output array often has more than one
        // item and that text is not guaranteed to be at position zero.
        let provider = ProviderId::new("openai-compatible")
            .unwrap_or_else(|error| panic!("valid fixture provider: {error}"));
        let model = ModelId::new("gpt-oss:20b")
            .unwrap_or_else(|error| panic!("valid fixture model: {error}"));
        let response = ChatResponse::new(
            provider,
            model,
            vec![
                OutputContent::ToolCall {
                    call: ToolCall::new("call_1", "search", "{}"),
                },
                OutputContent::Text {
                    text: "answer".to_owned(),
                },
            ],
            FinishReason::Stop,
        );

        assert_eq!(response.text(), "answer");
        assert_eq!(response.tool_calls().len(), 1);
    }

    #[test]
    fn a_synthesized_incomplete_stream_is_never_reported_as_stopped() {
        assert!(!FinishReason::Incomplete.is_complete());
        assert_ne!(FinishReason::Incomplete, FinishReason::Stop);
    }
}
