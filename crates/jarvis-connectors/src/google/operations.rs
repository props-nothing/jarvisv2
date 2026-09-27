//! The read operations: what a transport result means for the connector's tools.
//!
//! # The boundary this module is built around
//!
//! `ToolExecutor`'s contract draws a sharp line, and this module's whole job is to decide which side of it a
//! transport result falls on:
//!
//! - A [`TransportResponse`] means the **provider answered**. The adapter has something to report, so it returns
//!   a [`ToolCallResult`] — including for a refusal, which is a result rather than an error.
//! - A [`TransportFailure`] means no answer arrived. Whether that is an error or a result depends on whether the
//!   request **may have reached the provider**, which is [`TransportFailure::may_have_reached_the_provider`].
//!
//! That predicate is the most consequential decision in this file, and the two directions are not symmetric:
//!
//! | failure | may have reached? | adapter returns | consequence |
//! | --- | --- | --- | --- |
//! | `Connect` | no | `RefusedBeforeReaching` | a safe retry is permitted |
//! | `Refused` | no | `RefusedBeforeReaching` | a safe retry is permitted |
//! | `Send` | **maybe** | `AmbiguousAfterReaching` | **no automatic retry** |
//! | `Timeout` | **maybe** | `AmbiguousAfterReaching` | **no automatic retry** |
//! | `Body` | **yes** | `AmbiguousAfterReaching` | **no automatic retry** |
//!
//! Reporting an ambiguous failure as `RefusedBeforeReaching` would invite a retry, and for a non-idempotent
//! effect that retry is a second effect. Reporting a certain one as ambiguous is merely inconvenient — the
//! asymmetry `P3-005` records for outcomes in general.
//!
//! # Why a refusal produces a result and not an error
//!
//! A `403` is an **answer**: the request was received and refused. So the shape is a `ToolCallResult` whose
//! outcome is `Failed` with a bounded reason, and the reason is the provider's **machine-readable reason code**
//! rather than its prose — a success-shaped string is not proof, and `P3-008c` forbids deriving a decision from
//! message text.
//!
//! # What is not verified
//!
//! **No request has been sent and no response has been parsed from a provider.** Every fixture in the tests is
//! built from `docs/research/integrations/google.md`, so they prove this module implements the *record*.

use jarvis_core::{ToolOutcome, ToolOutcomeRecord, UtcTimestamp};
use jarvis_tools::{AdapterError, BoundedOutput, ProviderEvidence, ToolCallResult};

use crate::google::client::{self, GmailErrorReason};
use crate::google::credential::AccessToken;
use crate::google::request::{self, MessageFormat, RequestError};
use crate::google::transport::{GoogleTransport, HttpMethod, TransportFailure, TransportResponse};

/// The longest failure reason this module builds.
///
/// `ToolOutcomeRecord::failed` bounds a reason itself and would refuse an oversized one — which would turn a
/// provider refusal into an adapter defect. So the reason is truncated here to a value that is **this crate's
/// own text** rather than the provider's.
pub const MAX_FAILURE_REASON_CHARS: usize = 200;

/// Why an operation could not be attempted at all.
///
/// Distinct from [`AdapterError`] because both are **argument** faults, which never reach the network. The
/// distinction an adapter preserves is between "the arguments are unusable" and "the provider was reached and
/// the answer is unknown", and this type is the first of those.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum OperationError {
    /// The arguments could not be turned into a request.
    #[error(transparent)]
    Request(#[from] RequestError),
    /// The tool is not one this connector implements.
    #[error("this connector does not implement {tool}")]
    UnknownTool {
        /// The tool that was asked for.
        tool: String,
    },
}

/// Turns an argument object into a request for one of the read tools.
///
/// # Errors
///
/// Returns [`OperationError::UnknownTool`] for a name this connector does not provide, and
/// [`OperationError::Request`] when the arguments are unusable. An unknown name is an error rather than a
/// default, because a fallback would make a mistyped tool name silently read a mailbox.
pub fn request_for(
    tool: &str,
    arguments: &serde_json::Value,
) -> Result<request::HttpRequest, OperationError> {
    // The `match` is on the **name segment** rather than the qualified identifier: the pipeline resolves the
    // adapter from the identifier and hands the tool along, so comparing the qualified form here would be a
    // second place the namespace is stated.
    match segment_of(tool) {
        "gmail_messages_list" => Ok(request::gmail_messages_list(
            text(arguments, "query")?,
            page_size(arguments)?,
            text(arguments, "page_token")?,
        )?),
        "gmail_messages_read" => Ok(request::gmail_messages_get(
            required(arguments, "message_id")?,
            format_of(arguments)?,
        )?),
        "calendar_events_read" => Ok(request::calendar_events_list(
            required(arguments, "calendar_id")?,
            text(arguments, "time_min")?,
            text(arguments, "time_max")?,
            page_size(arguments)?,
            text(arguments, "sync_token")?,
        )?),
        other => Err(OperationError::UnknownTool {
            tool: other.to_owned(),
        }),
    }
}

/// Returns the name segment of a canonical tool identifier.
fn segment_of(tool: &str) -> &str {
    tool.rsplit('.').next().unwrap_or(tool)
}

/// Returns an optional string argument.
///
/// # Errors
///
/// Returns [`RequestError::Argument`] when the value is present but is not a string. Dropping it would
/// silently ignore an argument the caller supplied, which is the defect `P5-004` records for a placeholder
/// filling an unused schema field.
fn text<'a>(
    arguments: &'a serde_json::Value,
    name: &'static str,
) -> Result<Option<&'a str>, RequestError> {
    match arguments.get(name) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(value)) => Ok(Some(value.as_str())),
        Some(_) => Err(RequestError::Argument {
            field: name,
            reason: "an argument that is supplied must be a string",
        }),
    }
}

/// Returns a required string argument.
///
/// # Errors
///
/// Returns [`RequestError::Argument`] when it is absent or not a string.
fn required<'a>(
    arguments: &'a serde_json::Value,
    name: &'static str,
) -> Result<&'a str, RequestError> {
    text(arguments, name)?.ok_or(RequestError::Argument {
        field: name,
        reason: "this read names a specific resource, so the identifier is required",
    })
}

/// Returns the optional `max_results` page size.
///
/// # Errors
///
/// Returns [`RequestError::Argument`] for a value that is not a whole number, for the reason [`text`] records.
/// The per-API bound is applied by the request builder rather than here, so one bound is enforced in one place.
fn page_size(arguments: &serde_json::Value) -> Result<Option<u32>, RequestError> {
    match arguments.get("max_results") {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::Number(number)) => number
            .as_u64()
            .and_then(|value| u32::try_from(value).ok())
            .map(Some)
            .ok_or(RequestError::Argument {
                field: "max_results",
                reason: "a page size must be a whole number, at most 4294967295",
            }),
        Some(_) => Err(RequestError::Argument {
            field: "max_results",
            reason: "a page size must be a number",
        }),
    }
}

/// Returns the message format argument.
///
/// # Errors
///
/// Returns [`RequestError::Argument`] for an unrecognised value. **An unknown format is refused rather than
/// defaulted**, because a model that asked for something this connector does not offer should not silently
/// receive the full body. `request::MessageFormat` has no `raw` variant, so the unparsed MIME message is not
/// representable at all.
fn format_of(arguments: &serde_json::Value) -> Result<MessageFormat, RequestError> {
    match text(arguments, "format")? {
        Some("minimal") => Ok(MessageFormat::Minimal),
        Some("metadata") => Ok(MessageFormat::Metadata),
        // `full` and an absent format are the same request: the schema documents `full` as the default.
        Some("full") | None => Ok(MessageFormat::Full),
        Some(_) => Err(RequestError::Argument {
            field: "format",
            reason: "the format must be `minimal`, `metadata`, or `full`",
        }),
    }
}

/// Turns a transport result into what the adapter must report.
///
/// # Errors
///
/// Returns [`AdapterError`] only where the adapter cannot establish an outcome: a failure that produced no
/// answer. It returns `Ok` for **every** provider answer, including a refusal, because a refusal is a result.
pub fn interpret(
    tool: &str,
    result: Result<TransportResponse, TransportFailure>,
    now: UtcTimestamp,
) -> Result<ToolCallResult, AdapterError> {
    match result {
        Ok(response) => interpret_response(tool, &response, now),
        Err(failure) => Err(failure_to_adapter_error(failure)),
    }
}

/// Maps a transport failure onto the adapter's vocabulary, preserving the ambiguity.
///
/// # Why `AdapterError::ProviderRefused` is not used here
///
/// That variant means "the provider answered a refusal". A transport failure is by definition **no answer**,
/// so using it would state that a provider decided something when nothing was heard from it.
fn failure_to_adapter_error(failure: TransportFailure) -> AdapterError {
    if failure.may_have_reached_the_provider() {
        // The request may have arrived. A caller that saw a plain error would retry, and for a non-idempotent
        // effect that retry is a second effect — which is what this variant exists to prevent.
        return AdapterError::AmbiguousAfterReaching {
            reason: failure.to_string(),
        };
    }
    AdapterError::RefusedBeforeReaching {
        reason: failure.to_string(),
    }
}

/// Maps a provider's answer onto an outcome.
fn interpret_response(
    tool: &str,
    response: &TransportResponse,
    now: UtcTimestamp,
) -> Result<ToolCallResult, AdapterError> {
    let segment = segment_of(tool);
    if !matches!(
        segment,
        "gmail_messages_list" | "gmail_messages_read" | "calendar_events_read"
    ) {
        return Err(AdapterError::NotImplemented {
            tool: tool.to_owned(),
        });
    }
    if !response.is_success() {
        return refusal(response, now);
    }
    // A 200 whose body cannot be read is **not** a success. The status says the request was answered and the
    // body says nothing about what it produced, so `Unknown` is the honest reading — and it is reachable only
    // through a returned result, never an error, because the provider did answer.
    let Some(output) = read_output(segment, response) else {
        return Ok(ToolCallResult::new(
            ToolOutcomeRecord::new(ToolOutcome::Unknown).unwrap_or_else(unreachable_state),
            None,
            None,
            now,
        ));
    };
    // A parsed body IS the evidence for a read: the answer is the data, so the locator is the rendering itself.
    // A JSON rendering is never empty, so `confirmed` cannot refuse it in practice.
    let record = ToolOutcomeRecord::confirmed(&output).unwrap_or_else(unreachable_state);
    Ok(ToolCallResult::new(
        record,
        None,
        Some(BoundedOutput::truncating(output)),
        now,
    ))
}

/// Reads the tool's declared output shape from a successful body.
///
/// Returns `None` when the body cannot be read, which the caller turns into `Unknown`. The rendering matches
/// the **tool's own declared schema** rather than the provider's resource, because a parser returning Gmail's
/// `Message` would make the declared schema a fiction (`ADR-0059`).
fn read_output(segment: &str, response: &TransportResponse) -> Option<String> {
    match segment {
        "gmail_messages_list" => request::parse_id_page(response.status, &response.body)
            .ok()
            .map(|page| {
                let mut value = serde_json::json!({ "message_ids": page.ids });
                if let Some(token) = page.next_page_token {
                    value["next_page_token"] = serde_json::Value::String(token);
                }
                value.to_string()
            }),
        "gmail_messages_read" => request::parse_single_id(response.status, &response.body)
            .ok()
            .map(|id| serde_json::json!({ "message_id": id }).to_string()),
        "calendar_events_read" => {
            request::parse_calendar_page(response.status, &response.body)
                .ok()
                .map(|page| {
                    let mut value = serde_json::json!({ "event_ids": page.ids });
                    if let Some(token) = page.next_page_token {
                        value["next_page_token"] = serde_json::Value::String(token);
                    }
                    // The sync token is a **different** token from the page token and only the last page
                    // carries one, so it is rendered separately rather than merged into one field.
                    if let Some(token) = page.next_sync_token {
                        value["next_sync_token"] = serde_json::Value::String(token);
                    }
                    value.to_string()
                })
        }
        _ => None,
    }
}

/// Builds a `Failed` result for a provider refusal.
///
/// # Errors
///
/// Returns [`AdapterError::AmbiguousAfterReaching`] if the outcome cannot be recorded, which is a defect in
/// this module rather than a provider condition.
fn refusal(
    response: &TransportResponse,
    now: UtcTimestamp,
) -> Result<ToolCallResult, AdapterError> {
    // The reason code, never the message. A body that cannot be parsed still yields a reason, because the
    // status is a fact even when the body is not readable.
    let reason = serde_json::from_str::<client::GmailErrorBody>(&response.body)
        .ok()
        .as_ref()
        .and_then(client::GmailErrorBody::reason)
        .map_or(GmailErrorReason::Unrecognised, GmailErrorReason::parse);
    let detail = reason.as_str().map_or_else(
        || format!("the provider answered {}", response.status),
        |code| format!("the provider refused the call: {code}"),
    );
    let record =
        ToolOutcomeRecord::failed(truncate(&detail, MAX_FAILURE_REASON_CHARS)).map_err(|_| {
            AdapterError::AmbiguousAfterReaching {
                reason: "a failed outcome could not be recorded".to_owned(),
            }
        })?;
    // No evidence: provider evidence is the locator for an **effect**, and a refusal produced none. The status
    // and reason live in the outcome's own reason, which is where a refusal belongs.
    Ok(ToolCallResult::new(record, None, None, now))
}

/// Truncates a reason to the module's bound, on a character boundary.
fn truncate(text: &str, maximum: usize) -> String {
    if text.chars().count() <= maximum {
        return text.to_owned();
    }
    text.chars().take(maximum).collect()
}

/// A state `ToolOutcomeRecord::new` accepts without evidence or reason.
///
/// `Unknown` is exactly that, so this cannot be reached in practice. It exists because `expect` is denied in
/// this crate: a fabricated fallback that is genuinely unreachable is better than an unwrap that would abort a
/// worker if the assumption were ever wrong.
///
/// Takes the refusal it replaces as an argument rather than ignoring it, so it satisfies `unwrap_or_else` for
/// both `confirmed` and `new` without a closure at each call site.
fn unreachable_state<E>(_refusal: E) -> ToolOutcomeRecord {
    // `Unknown` takes neither evidence nor a reason, which is what `new` accepts.
    ToolOutcomeRecord::new(ToolOutcome::Unknown).unwrap_or_else(|_| {
        unreachable!(
            "`ToolOutcomeRecord::new` refuses every reason-free state, which is a contract change"
        )
    })
}

/// Builds provider evidence from a locator the provider supplied.
///
/// # Why this returns `Option` rather than a `Result`
///
/// Evidence is **optional for a read**: a missing or malformed locator does not make a successful read fail.
/// Failing the call would let a provider's malformed header turn a good read into an error, which is worse than
/// carrying no locator.
#[must_use]
pub fn evidence_from(locator: Option<&str>) -> Option<ProviderEvidence> {
    locator.and_then(|value| {
        ProviderEvidence::new(value)
            .ok()
            .filter(|evidence| !evidence.as_str().trim().is_empty())
    })
}

/// Returns the method a request declares, so a caller does not restate it.
///
/// # Errors
///
/// Returns [`TransportFailure`] for a method the port cannot express, which is a defect in this crate rather
/// than a caller's mistake — `HttpRequest` builds only `GET`s today, and `P5-009` must extend both together.
pub fn method_for(request: &request::HttpRequest) -> Result<HttpMethod, TransportFailure> {
    crate::google::transport::method_of(request)
}

/// A read operation bound to a transport — what makes this module's pieces reachable.
///
/// Holds a borrowed transport so a caller can share one client across every tool, and an `AccessToken` so the
/// adapter is a **`ToolExecutor`** rather than a helper a caller has to drive with a credential it obtained
/// itself. That is the difference between this being reachable from a run and reachable only from a test.
///
/// # Why the token is borrowed from a caller rather than minted here
///
/// Minting or refreshing belongs to whoever holds the stored grant, and this crate has no store. A token that
/// expired mid-run would need a refresh exchange, and a refresh is a durable write with its own failure modes —
/// so this type takes the token it was given and reports a provider refusal honestly when that token is stale.
/// An adapter that silently refreshed would make a credential's rotation invisible to the audit record.
pub struct GoogleReadTool<'a> {
    transport: &'a dyn GoogleTransport,
    token: &'a AccessToken,
    adapter_id: &'static str,
}

impl<'a> GoogleReadTool<'a> {
    /// Binds a transport, a credential, and the adapter identifier to report.
    ///
    /// The identifier is a parameter rather than a constant here because it names the **adapter** in a
    /// diagnostic, and the adapter's identity is the pipeline's — the same reason `ToolExecutor::adapter_id`
    /// returns `&'static str` rather than deriving one.
    #[must_use]
    pub const fn new(
        transport: &'a dyn GoogleTransport,
        token: &'a AccessToken,
        adapter_id: &'static str,
    ) -> Self {
        Self {
            transport,
            token,
            adapter_id,
        }
    }

    /// Performs one read operation for a tool name and an argument object.
    ///
    /// This is the whole of the logic; [`ToolExecutor::execute`] is a thin unwrapping of the pipeline's request
    /// onto it, so the behaviour is asserted once rather than twice. `now` is a parameter rather than read from
    /// a clock here, so the pure layer stays deterministic and a test can assert an exact `reported_at`.
    ///
    /// # Errors
    ///
    /// Returns [`AdapterError`] for an argument fault, an unimplemented tool, or an ambiguous transport
    /// failure — as [`interpret`] and [`request_for`] do.
    pub async fn run(
        &self,
        tool: &str,
        arguments: &serde_json::Value,
        now: UtcTimestamp,
    ) -> Result<ToolCallResult, AdapterError> {
        let request = match request_for(tool, arguments) {
            Ok(request) => request,
            Err(OperationError::UnknownTool { tool }) => {
                return Err(AdapterError::NotImplemented { tool });
            }
            // A malformed argument never reaches the network, so "nothing happened at all" is the accurate
            // statement — the reading `jarvis-tools`' filesystem adapter records for a bad path.
            Err(OperationError::Request(error)) => {
                return Err(AdapterError::RefusedBeforeReaching {
                    reason: error.to_string(),
                });
            }
        };
        // The method comes from the request rather than being restated, so a request kind added later cannot be
        // sent with the wrong verb. A method the port cannot express is a crate defect rather than a caller's
        // mistake, so it is classified as a failure before writing — nothing was sent, so nothing happened.
        let method = match method_for(&request) {
            Ok(method) => method,
            Err(failure) => return Err(failure_to_adapter_error(failure)),
        };
        let response = self.transport.send(method, &request, self.token).await;
        interpret(tool, response, now)
    }
}

#[async_trait::async_trait]
impl jarvis_tools::ToolExecutor for GoogleReadTool<'_> {
    fn adapter_id(&self) -> &'static str {
        self.adapter_id
    }

    /// Runs one call the pipeline has already authorized.
    ///
    /// # The deadline is checked before anything is sent
    ///
    /// A call already past its deadline must not start a network write, and "nothing happened" is the honest
    /// reading — the same choice `jarvis-tools`' filesystem adapter makes before its first read. Reporting it as
    /// a *failure* would suggest something may have happened, which would then need investigating.
    ///
    /// # Errors
    ///
    /// As [`Self::run`], plus [`AdapterError::RefusedBeforeReaching`] for a lapsed deadline.
    async fn execute(
        &self,
        request: &jarvis_tools::ToolExecutionRequest,
    ) -> Result<ToolCallResult, AdapterError> {
        let now = UtcTimestamp::now(&jarvis_core::SystemClock);
        if request.is_past_deadline(now) {
            return Err(AdapterError::RefusedBeforeReaching {
                reason: "the call was already past its deadline".to_owned(),
            });
        }
        // The tool is taken from the request's own canonical identifier and matched by its name segment, so a
        // request naming a tool this adapter does not implement is refused rather than defaulting to a read.
        self.run(&request.tool().to_string(), request.arguments(), now)
            .await
    }
}

#[cfg(test)]
#[path = "operations_tests.rs"]
mod tests;
