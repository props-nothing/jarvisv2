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
///
/// `content` is `Option` rather than a bare string because an assistant turn that only requested
/// tool calls carries an empty content, and some providers reject a `null` content field while
/// others require it to be present. Serializing `""` for the empty text and omitting nothing keeps
/// the request valid on the interoperable set of servers.
#[derive(Serialize)]
pub(super) struct WireMessage<'a> {
    pub(super) role: &'a str,
    pub(super) content: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) tool_call_id: Option<&'a str>,
    /// The tool calls an assistant turn requested, omitted when there are none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) tool_calls: Vec<WireOutgoingToolCall<'a>>,
}

/// One tool call an assistant turn requested, in the shape a provider reads.
#[derive(Serialize)]
pub(super) struct WireOutgoingToolCall<'a> {
    pub(super) id: &'a str,
    #[serde(rename = "type")]
    pub(super) kind: &'static str,
    pub(super) function: WireOutgoingFunction<'a>,
}

/// The name and argument text of an outgoing tool call.
#[derive(Serialize)]
pub(super) struct WireOutgoingFunction<'a> {
    pub(super) name: &'a str,
    pub(super) arguments: &'a str,
}

/// One tool offered to the model, in the provider's shape.
#[derive(Serialize)]
pub(super) struct WireToolDefinition<'a> {
    #[serde(rename = "type")]
    pub(super) kind: &'static str,
    pub(super) function: WireToolFunction<'a>,
}

/// The function a tool definition declares.
#[derive(Serialize)]
pub(super) struct WireToolFunction<'a> {
    pub(super) name: &'a str,
    pub(super) description: &'a str,
    pub(super) parameters: &'a serde_json::Value,
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
    /// The tools offered to the model, absent when none are.
    ///
    /// `tool_choice` is deliberately **not** sent: the research record
    /// (`docs/research/integrations/openai-compatible-model-api.md`) excludes it from the shared
    /// contract because its spelling is not interoperable across compatible servers, and `auto` —
    /// the only value JARVIS would ever want — is the server's default when `tools` is present.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) tools: Vec<WireToolDefinition<'a>>,
}

/// A tool invocation as returned by the provider.
#[derive(Deserialize)]
pub(super) struct WireToolCall {
    /// The position of this call within the turn.
    ///
    /// A streaming turn delivers a call's fragments across several chunks, each tagged with the same
    /// index; the index is what lets the reassembler group fragments belonging to one invocation.
    /// Absent in a non-streaming response, which carries each call whole.
    #[serde(default)]
    pub(super) index: Option<u64>,
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
/// `tool_calls` fragments arrive across chunks and are reassembled by the adapter before any
/// invocation reaches the domain layer. The field is decoded here so the reassembler can see the
/// fragments, but a fragment is **never** surfaced as a complete call: only the reassembled result
/// (`WireToolAccumulator::finish`) becomes a `StreamEvent::ToolCall`, and only once, at the point the
/// provider finishes the turn.
#[derive(Deserialize)]
pub(super) struct WireDelta {
    #[serde(default)]
    pub(super) content: Option<String>,
    /// Reasoning tokens, under the name Ollama and several hosts use (`reasoning`) or the one others use
    /// (`reasoning_content`). Never shown or kept: only counted, so a long silent think is visible as progress.
    #[serde(default, alias = "reasoning_content")]
    pub(super) reasoning: Option<String>,
    #[serde(default)]
    pub(super) tool_calls: Vec<WireToolCall>,
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

/// Projects domain tool specifications into the provider's definition shape.
///
/// The parameters are borrowed rather than cloned, because a schema can be large and the wire
/// request only serializes them.
pub(super) fn tool_definitions(tools: &[crate::request::ToolSpec]) -> Vec<WireToolDefinition<'_>> {
    tools
        .iter()
        .map(|tool| WireToolDefinition {
            // The provider's discriminator for a function tool. Constant because only function tools
            // are sent; a provider-specific tool type is not something this adapter speaks.
            kind: "function",
            function: WireToolFunction {
                name: tool.name(),
                description: tool.description(),
                parameters: tool.parameters(),
            },
        })
        .collect()
}

/// Reassembles an assistant turn's tool calls from streaming fragments.
///
/// # Why this type exists rather than decoding each fragment into a `ToolCall`
///
/// A streaming turn delivers one invocation across many chunks: the first carries the identifier and
/// name, and subsequent chunks append argument text. A consumer that reported the first chunk would
/// ask JARVIS to authorize a call whose arguments are still being written, and a consumer that
/// reported each chunk would present one call as several. So fragments are **accumulated** here and
/// the whole turn is emitted once, when the provider signals the turn is finished.
///
/// # Ordering by index
///
/// The provider tags each fragment with the index of the call it belongs to, so fragments are grouped
/// by index rather than by arrival order. Grouping by arrival order would be correct only when a turn
/// interleaves nothing, which is not guaranteed once a model requests several tools at once.
#[derive(Default)]
pub(super) struct WireToolAccumulator {
    /// Per-index partial invocations, in first-seen order.
    slots: Vec<ToolSlot>,
}

/// A partial invocation under construction.
#[derive(Default)]
struct ToolSlot {
    index: u64,
    id: Option<String>,
    name: Option<String>,
    arguments: String,
}

impl WireToolAccumulator {
    /// Applies the fragments from one chunk.
    ///
    /// A fragment whose `index` is absent is treated as index `0`, which is what a provider that does
    /// not tag single-call turns produces. The identifier and name are kept from the first fragment
    /// that carries them rather than overwritten, because a later chunk normally sends neither and a
    /// provider that repeated one would not be changing it.
    pub(super) fn observe(&mut self, fragments: &[WireToolCall]) {
        for fragment in fragments {
            let index = fragment.index.unwrap_or_default();
            // Find the slot for this index, or append one. `if let` over the position rather than a
            // `match` on it, because the miss arm has a side effect and clippy reads a one-pattern
            // `match` as a destructure.
            let position =
                if let Some(position) = self.slots.iter().position(|slot| slot.index == index) {
                    position
                } else {
                    self.slots.push(ToolSlot {
                        index,
                        ..ToolSlot::default()
                    });
                    self.slots.len().saturating_sub(1)
                };

            let slot = &mut self.slots[position];
            if let Some(id) = fragment.id.as_ref() {
                slot.id.get_or_insert_with(|| id.clone());
            }
            if let Some(function) = fragment.function.as_ref() {
                if let Some(name) = function.name.as_ref() {
                    slot.name.get_or_insert_with(|| name.clone());
                }
                if let Some(arguments) = function.arguments.as_ref() {
                    slot.arguments.push_str(arguments);
                }
            }
        }
    }

    /// Produces the reassembled calls.
    ///
    /// An invocation with no identifier or no name is **dropped** rather than emitted, for the same
    /// reason `output_items` drops one: a call with no identifier cannot be answered and one with no
    /// name cannot be authorized, so emitting it would ask JARVIS to act on an unusable request. The
    /// slots are returned in index order so the output matches what a non-streaming response would
    /// have produced.
    pub(super) fn finish(mut self) -> Vec<ToolCall> {
        self.slots.sort_by_key(|slot| slot.index);
        self.slots
            .into_iter()
            .filter_map(|slot| {
                let id = slot.id?;
                let name = slot.name?;
                Some(ToolCall::new(id, name, slot.arguments))
            })
            .collect()
    }
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
                    index: Some(0),
                    id: None,
                    function: Some(WireFunctionCall {
                        name: Some("search".to_owned()),
                        arguments: None,
                    }),
                },
                WireToolCall {
                    index: Some(1),
                    id: Some("call_1".to_owned()),
                    function: None,
                },
                WireToolCall {
                    index: Some(2),
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
    fn streaming_fragments_reassemble_into_one_call_with_concatenated_arguments() {
        let mut accumulator = WireToolAccumulator::default();
        accumulator.observe(&[WireToolCall {
            index: Some(0),
            id: Some("call_1".to_owned()),
            function: Some(WireFunctionCall {
                name: Some("search".to_owned()),
                arguments: Some("{\"q\":".to_owned()),
            }),
        }]);
        // A later chunk carries only argument text, which is the shape a provider sends.
        accumulator.observe(&[WireToolCall {
            index: Some(0),
            id: None,
            function: Some(WireFunctionCall {
                name: None,
                arguments: Some("\"rust\"}".to_owned()),
            }),
        }]);

        let calls = accumulator.finish();
        assert_eq!(calls.len(), 1, "one invocation, however many fragments");
        assert_eq!(calls[0].id(), "call_1");
        assert_eq!(calls[0].name(), "search");
        assert_eq!(calls[0].arguments(), "{\"q\":\"rust\"}");
    }

    #[test]
    fn fragments_of_several_calls_are_grouped_by_index_not_arrival_order() {
        let mut accumulator = WireToolAccumulator::default();
        // Index 1 arrives first, exactly the interleaving that arrival-order grouping would scramble.
        accumulator.observe(&[WireToolCall {
            index: Some(1),
            id: Some("b".to_owned()),
            function: Some(WireFunctionCall {
                name: Some("second".to_owned()),
                arguments: Some("{}".to_owned()),
            }),
        }]);
        accumulator.observe(&[WireToolCall {
            index: Some(0),
            id: Some("a".to_owned()),
            function: Some(WireFunctionCall {
                name: Some("first".to_owned()),
                arguments: Some("{}".to_owned()),
            }),
        }]);

        let calls = accumulator.finish();
        let names: Vec<&str> = calls.iter().map(ToolCall::name).collect();
        assert_eq!(names, vec!["first", "second"]);
    }

    #[test]
    fn an_incomplete_invocation_is_dropped_rather_than_emitted() {
        let mut accumulator = WireToolAccumulator::default();
        // An id with no name: usable neither to answer nor to authorize.
        accumulator.observe(&[WireToolCall {
            index: Some(0),
            id: Some("call_1".to_owned()),
            function: Some(WireFunctionCall {
                name: None,
                arguments: Some("{}".to_owned()),
            }),
        }]);
        assert!(
            accumulator.finish().is_empty(),
            "a call with no name must not be surfaced"
        );
    }

    #[test]
    fn a_short_description_of_a_tool_is_projected_into_the_provider_shape() {
        let schema = serde_json::json!({"type": "object"});
        let tools = vec![crate::request::ToolSpec::new("files.read", "Read", schema)];
        let projected = tool_definitions(&tools);
        assert_eq!(projected.len(), 1);
        assert_eq!(projected[0].kind, "function");
        assert_eq!(projected[0].function.name, "files.read");
        let json = serde_json::to_string(&projected).unwrap_or_else(|_| String::new());
        assert!(json.contains("\"type\":\"function\""), "wire shape: {json}");
        assert!(json.contains("\"parameters\""), "wire shape: {json}");
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
