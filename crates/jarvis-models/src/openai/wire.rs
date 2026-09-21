//! Private provider wire shapes and their mapping into domain types.
//!
//! Nothing here is exported. A provider field name, a provider JSON value, or a
//! provider status code must not reach a JARVIS domain type, so the mapping
//! functions are the only bridge and the wire structs stay private.
//!
//! Every field is optional. `OpenAI` documents that adding properties to response
//! objects is a backwards-compatible change, and self-hosted servers omit fields
//! freely, so a missing field is normal rather than an error.

use serde::{Deserialize, Serialize};

use crate::error::ModelErrorKind;
use crate::request::Role;
use crate::response::{FinishReason, OutputContent, ToolCall};
use crate::usage::TokenUsage;

/// One outgoing message.
#[derive(Serialize)]
pub(super) struct WireMessage<'a> {
    pub(super) role: &'a str,
    pub(super) content: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) tool_call_id: Option<&'a str>,
}

/// Returns the wire role name for a domain role.
///
/// The higher-authority instruction role is sent as `system`, not `developer`.
/// `OpenAI` accepts both, but several compatible servers accept only `system`, and
/// `system` is the interoperable spelling. Capability discovery records which the
/// server accepts; this default keeps the request valid everywhere.
pub(super) const fn role_wire_name(role: Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    }
}

/// Streaming options.
#[derive(Serialize)]
pub(super) struct WireStreamOptions {
    pub(super) include_usage: bool,
}

/// An outgoing chat completion request.
#[derive(Serialize)]
pub(super) struct WireChatRequest<'a> {
    pub(super) model: &'a str,
    pub(super) messages: Vec<WireMessage<'a>>,
    pub(super) stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) stream_options: Option<WireStreamOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) temperature: Option<f64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) stop: Vec<&'a str>,
}

/// A tool invocation as returned by the provider.
#[derive(Deserialize)]
pub(super) struct WireToolCall {
    #[serde(default)]
    pub(super) id: Option<String>,
    #[serde(default)]
    pub(super) function: Option<WireFunctionCall>,
}

/// The function name and argument text of a tool invocation.
#[derive(Deserialize)]
pub(super) struct WireFunctionCall {
    #[serde(default)]
    pub(super) name: Option<String>,
    #[serde(default)]
    pub(super) arguments: Option<String>,
}

/// A complete message in a non-streaming response.
#[derive(Deserialize)]
pub(super) struct WireResponseMessage {
    #[serde(default)]
    pub(super) content: Option<String>,
    #[serde(default)]
    pub(super) tool_calls: Option<Vec<WireToolCall>>,
}

/// One choice in a non-streaming response.
#[derive(Deserialize)]
pub(super) struct WireChoice {
    #[serde(default)]
    pub(super) message: Option<WireResponseMessage>,
    #[serde(default)]
    pub(super) finish_reason: Option<String>,
}

/// Cached-token detail on a usage object.
#[derive(Deserialize)]
pub(super) struct WirePromptDetails {
    #[serde(default)]
    pub(super) cached_tokens: Option<u64>,
}

/// A usage object.
#[derive(Deserialize)]
pub(super) struct WireUsage {
    #[serde(default)]
    pub(super) prompt_tokens: Option<u64>,
    #[serde(default)]
    pub(super) completion_tokens: Option<u64>,
    #[serde(default)]
    pub(super) prompt_tokens_details: Option<WirePromptDetails>,
}

impl WireUsage {
    /// Maps provider usage onto the domain type.
    pub(super) fn to_usage(&self) -> TokenUsage {
        let cached = self
            .prompt_tokens_details
            .as_ref()
            .and_then(|details| details.cached_tokens)
            .unwrap_or_default();
        TokenUsage::new(
            self.prompt_tokens.unwrap_or_default(),
            self.completion_tokens.unwrap_or_default(),
        )
        .with_cached_input_tokens(cached)
    }
}

/// A non-streaming chat completion response.
#[derive(Deserialize)]
pub(super) struct WireChatResponse {
    #[serde(default)]
    pub(super) model: Option<String>,
    #[serde(default)]
    pub(super) choices: Vec<WireChoice>,
    #[serde(default)]
    pub(super) usage: Option<WireUsage>,
}

/// The delta of a streaming choice.
///
/// `tool_calls` is deliberately absent. Streaming tool calls arrive as fragments that
/// must be reassembled before they mean anything, and a partially assembled
/// invocation must never be reported as a complete one. Because the field is not
/// decoded, a fragment is structurally incapable of reaching the domain layer; it is
/// added together with the reassembly logic rather than as an ignored placeholder.
#[derive(Deserialize)]
pub(super) struct WireDelta {
    #[serde(default)]
    pub(super) content: Option<String>,
}

/// One choice in a streaming chunk.
#[derive(Deserialize)]
pub(super) struct WireStreamChoice {
    #[serde(default)]
    pub(super) delta: Option<WireDelta>,
    #[serde(default)]
    pub(super) finish_reason: Option<String>,
}

/// One streaming chunk.
#[derive(Deserialize)]
pub(super) struct WireStreamChunk {
    #[serde(default)]
    pub(super) choices: Vec<WireStreamChoice>,
    #[serde(default)]
    pub(super) usage: Option<WireUsage>,
}

/// A provider error body.
#[derive(Deserialize)]
pub(super) struct WireError {
    #[serde(default, rename = "type")]
    pub(super) kind: Option<String>,
    /// String in documented examples, but some compatible servers send a number.
    ///
    /// Kept as an untyped value so a numeric code is read rather than rejecting the
    /// whole error body and losing the classification.
    #[serde(default)]
    pub(super) code: Option<serde_json::Value>,
}

/// The envelope a provider wraps an error in.
#[derive(Deserialize)]
pub(super) struct WireErrorEnvelope {
    #[serde(default)]
    pub(super) error: Option<WireError>,
}

impl WireError {
    /// Returns the error code as text, whichever JSON type carried it.
    pub(super) fn code_text(&self) -> Option<String> {
        match self.code.as_ref()? {
            serde_json::Value::String(text) => Some(text.clone()),
            serde_json::Value::Number(number) => Some(number.to_string()),
            _ => None,
        }
    }
}

/// Error codes and types documented as billing, spend, or quota limits.
///
/// These must never be retried: official documentation states that retrying does
/// not restore access and that the relevant limit must be changed first.
const QUOTA_CODE_MARKERS: &[&str] = &[
    "insufficient_quota",
    "credit_balance_exhausted",
    "organization_spend_limit_exceeded",
    "project_spend_limit_exceeded",
    "organization_usage_limit_exceeded",
    "billing_hard_limit_reached",
    "insufficient_credits",
];

/// Error codes that indicate the prompt exceeded the model's context window.
const CONTEXT_CODE_MARKERS: &[&str] = &[
    "context_length_exceeded",
    "context_overflow",
    "string_above_max_length",
];

/// Classifies a provider error from its status and optional parsed body.
///
/// Status alone is insufficient: `429` covers both "slow down" (retryable) and
/// "out of credit" (permanent), and only the body distinguishes them. Classifying
/// by status alone would either retry a dead account or refuse a transient limit.
pub(super) fn classify_error(status: u16, error: Option<&WireError>) -> ModelErrorKind {
    let code = error.and_then(WireError::code_text);
    let kind_text = error.and_then(|value| value.kind.clone());
    let haystack = format!(
        "{} {}",
        code.as_deref().unwrap_or_default(),
        kind_text.as_deref().unwrap_or_default()
    )
    .to_ascii_lowercase();

    let matches_marker = |markers: &[&str]| {
        markers
            .iter()
            .any(|marker| haystack.contains(&marker.to_ascii_lowercase()))
    };

    if matches_marker(QUOTA_CODE_MARKERS) {
        return ModelErrorKind::QuotaExhausted;
    }
    if matches_marker(CONTEXT_CODE_MARKERS) {
        return ModelErrorKind::ContextOverflow;
    }

    match status {
        400 | 422 => ModelErrorKind::InvalidRequest,
        401 => ModelErrorKind::Authentication,
        403 => ModelErrorKind::Authorization,
        404 => ModelErrorKind::ModelNotFound,
        408 => ModelErrorKind::Timeout,
        429 => ModelErrorKind::RateLimited,
        503 => ModelErrorKind::Overloaded,
        500 | 502 | 504 => ModelErrorKind::Transient,
        other if (500..600).contains(&other) => ModelErrorKind::Transient,
        other if (400..500).contains(&other) => ModelErrorKind::InvalidRequest,
        _ => ModelErrorKind::MalformedResponse,
    }
}

/// Maps a provider finish-reason label, treating an absent label as incomplete.
///
/// A response with no finish reason is not a completed answer. Reporting it as
/// success would let a truncated or filtered reply pass as complete.
pub(super) fn finish_reason(label: Option<&str>) -> FinishReason {
    match label {
        Some(value) => FinishReason::from_provider_label(value),
        None => FinishReason::Incomplete,
    }
}

/// Builds the ordered output of a non-streaming response.
pub(super) fn output_items(message: Option<&WireResponseMessage>) -> Vec<OutputContent> {
    let Some(message) = message else {
        return Vec::new();
    };

    let mut items = Vec::new();
    if let Some(text) = message.content.as_ref().filter(|text| !text.is_empty()) {
        items.push(OutputContent::Text { text: text.clone() });
    }
    for call in message.tool_calls.iter().flatten() {
        // A tool call with no id cannot be answered, and one with no name cannot be
        // authorized, so an incomplete call is dropped rather than invented.
        let (Some(id), Some(function)) = (call.id.as_ref(), call.function.as_ref()) else {
            continue;
        };
        let Some(name) = function.name.as_ref() else {
            continue;
        };
        items.push(OutputContent::ToolCall {
            call: ToolCall::new(
                id.clone(),
                name.clone(),
                function.arguments.clone().unwrap_or_default(),
            ),
        });
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    fn error(kind: Option<&str>, code: Option<serde_json::Value>) -> WireError {
        WireError {
            kind: kind.map(str::to_owned),
            code,
        }
    }

    #[test]
    fn a_quota_429_is_distinguished_from_a_rate_limit_429_by_its_body() {
        // The status is identical; only the body separates a transient limit from a
        // dead account. If this collapses, one of the two is handled wrongly.
        let quota = error(
            Some("insufficient_quota"),
            Some(serde_json::Value::String(
                "credit_balance_exhausted".to_owned(),
            )),
        );
        assert_eq!(
            classify_error(429, Some(&quota)),
            ModelErrorKind::QuotaExhausted
        );
        assert!(
            !ModelErrorKind::QuotaExhausted.is_retryable(),
            "quota must not be retryable"
        );

        let rate = error(
            Some("rate_limit_error"),
            Some(serde_json::Value::String("slow_down".to_owned())),
        );
        assert_eq!(
            classify_error(429, Some(&rate)),
            ModelErrorKind::RateLimited
        );
        assert!(ModelErrorKind::RateLimited.is_retryable());
    }

    #[test]
    fn documented_quota_codes_are_all_recognized() {
        for code in QUOTA_CODE_MARKERS {
            let body = error(None, Some(serde_json::Value::String((*code).to_owned())));
            assert_eq!(
                classify_error(429, Some(&body)),
                ModelErrorKind::QuotaExhausted,
                "{code} must be classified as quota so it is not retried"
            );
        }
    }

    #[test]
    fn a_numeric_error_code_is_still_classified() {
        // Some compatible servers send a number where OpenAI sends a string.
        let body = error(None, Some(serde_json::Value::from(400)));
        assert_eq!(body.code_text().as_deref(), Some("400"));
        assert_eq!(
            classify_error(400, Some(&body)),
            ModelErrorKind::InvalidRequest
        );
    }

    #[test]
    fn context_overflow_is_separated_from_a_generic_bad_request() {
        // A context overflow is a budgeting problem, not a malformed request, and the
        // remedy is different.
        let body = error(
            None,
            Some(serde_json::Value::String(
                "context_length_exceeded".to_owned(),
            )),
        );
        assert_eq!(
            classify_error(400, Some(&body)),
            ModelErrorKind::ContextOverflow
        );
        assert_eq!(classify_error(400, None), ModelErrorKind::InvalidRequest);
    }

    #[test]
    fn an_absent_finish_reason_is_incomplete_not_stopped() {
        assert_eq!(finish_reason(None), FinishReason::Incomplete);
        assert_eq!(finish_reason(Some("stop")), FinishReason::Stop);
    }

    #[test]
    fn a_tool_call_missing_its_id_or_name_is_dropped_not_invented() {
        let message = WireResponseMessage {
            content: Some("text".to_owned()),
            tool_calls: Some(vec![
                WireToolCall {
                    id: None,
                    function: Some(WireFunctionCall {
                        name: Some("search".to_owned()),
                        arguments: None,
                    }),
                },
                WireToolCall {
                    id: Some("call_1".to_owned()),
                    function: None,
                },
                WireToolCall {
                    id: Some("call_2".to_owned()),
                    function: Some(WireFunctionCall {
                        name: Some("read".to_owned()),
                        arguments: Some("{}".to_owned()),
                    }),
                },
            ]),
        };

        let items = output_items(Some(&message));
        assert_eq!(items.len(), 2, "only the complete call survives");
        assert!(matches!(items[0], OutputContent::Text { .. }));
        assert!(matches!(items[1], OutputContent::ToolCall { .. }));
    }

    #[test]
    fn cached_tokens_survive_the_mapping() {
        let usage = WireUsage {
            prompt_tokens: Some(100),
            completion_tokens: Some(20),
            prompt_tokens_details: Some(WirePromptDetails {
                cached_tokens: Some(900),
            }),
        };
        let mapped = usage.to_usage();
        assert_eq!(mapped.input_tokens(), 100);
        assert_eq!(mapped.cached_input_tokens(), 900);
        assert_eq!(mapped.total_tokens(), Some(1020));
    }

    #[test]
    fn the_instruction_role_is_sent_as_the_interoperable_spelling() {
        assert_eq!(role_wire_name(Role::System), "system");
    }
}
