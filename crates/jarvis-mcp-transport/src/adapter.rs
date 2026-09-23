//! Running a canonical MCP tool: the adapter that turns a policy-authorized call into `tools/call`.
//!
//! # What this closes
//!
//! `P3-008f` and `P3-008g` built the whole remote path — negotiate, list, translate, aggregate, and
//! call — and recorded the same limit each time: **"nothing drives it from a run"**. `tools/call`
//! worked and nothing in the product asked for one. This is the adapter that a policy-authorized call
//! reaches, and it is the piece that makes an MCP tool a *tool* rather than a capability.
//!
//! # The conversion is where the honesty rules live
//!
//! [`McpCallResult`] is what the server sent. [`ToolCallResult`] is what the adapter **established**,
//! with an outcome, evidence, and bounded output. The conversion between them is this module's real
//! subject, and every decision in it is one the project has already argued about elsewhere:
//!
//! | Server said | Outcome | Why |
//! | --- | --- | --- |
//! | a result, `isError` absent/false | `Confirmed` | The provider answered with a result. The evidence is the *call*, not the content. |
//! | a result, `isError` true | `Failed` | The tool ran and refused. Nothing happened, and the reason is known — which is exactly what `Failed` means. |
//! | a JSON-RPC error | `Failed` | The request was refused. The peer said so, so this is evidence, not an absence of it. |
//! | transport failure after the request went out | `AdapterError::AmbiguousAfterReaching` | **The case that matters.** The request was transmitted and no answer came back, so the effect may have happened. |
//! | `input_required` / `task` refused by the transport | `Failed` | The server wants a conversation JARVIS does not hold. Nothing was done and nothing was sent onward. |
//! | unusable transport | `AdapterError::RefusedBeforeReaching` | Nothing left this process. |
//!
//! **The `Confirmed` row deserves its own note.** A server answering `isError: false` is not proof that
//! an effect completed — the protocol has no notion of a receipt, and the tool might have done nothing
//! at all. But it *is* the strongest available evidence, and `docs/architecture/tools-and-connectors.md`
//! forbids only turning "a success-sounding string" into proof. So the evidence recorded is structural
//! (`mcp:<server>/<tool>` plus the call's own identifier) and the **output is kept separately** from it,
//! which is the same split the filesystem adapter makes: evidence is a locator, output is content, and a
//! reader can always tell which is which.
//!
//! # What this deliberately does not do
//!
//! - **It does not look the routing up during a call.** The mapping from a canonical identifier to the
//!   server's own tool name is given to it at construction from the catalog entries for one server. A
//!   lookup at call time would let a catalog refresh between authorization and execution change which
//!   tool runs — the argument the receipt binds is the identifier, so the *name sent* must be the one
//!   that identifier meant when the call was authorized.
//! - **It does not retry.** Retry is a declaration on the tool contract and a decision of the pipeline;
//!   an adapter that retried internally would repeat a non-idempotent effect invisibly.
//! - **It does not interpret the receipt.** Policy and approval already happened, and the pipeline
//!   revalidates before calling.
//!
//! # Why the routing is not an argument
//!
//! The obvious way to tell the adapter which server-side name to send is to put it in the request's
//! `arguments`. **That is wrong twice over, and the first attempt here did it.** The arguments are
//! validated against the tool's input schema, which declares the properties the *server* accepts — so an
//! injected key is a schema violation. And they are covered by the authorization digest, which
//! [`ToolExecutionRequest::new`] **recomputes and compares**; injecting a key after the receipt was
//! built would make every request fail its own binding check. Both are properties this project put
//! there deliberately, so the routing had to live somewhere else: in the adapter, supplied by the host
//! that holds the catalog.

use std::collections::BTreeMap;
use std::sync::Arc;

use jarvis_core::{SystemClock, ToolOutcome, ToolOutcomeRecord, UtcTimestamp};
use jarvis_mcp::CatalogEntry;
use jarvis_tools::{
    AdapterError, BoundedOutput, ProviderEvidence, ToolCallResult, ToolExecutionRequest,
    ToolExecutor,
};
use serde_json::Value;

use crate::client::{McpCallResult, McpConnection};
use crate::error::CallError;

/// Runs the tools of one MCP server.
///
/// Holds a live connection rather than a way to open one, because opening is the host's decision and a
/// server is a child process or a network endpoint whose lifecycle the host owns. One adapter therefore
/// serves one server's tools, which is also what makes the evidence locator unambiguous.
pub struct McpToolAdapter {
    server: String,
    connection: Arc<McpConnection>,
    routes: BTreeMap<String, String>,
}

impl std::fmt::Debug for McpToolAdapter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("McpToolAdapter")
            .field("server", &self.server)
            .field("tools", &self.routes.len())
            .finish_non_exhaustive()
    }
}

impl McpToolAdapter {
    /// Builds an adapter over one server's connection and that server's routing.
    ///
    /// The routing is derived from the catalog entries **for this server**, so an identifier belonging
    /// to another server cannot be routed here — a call for server A's tool would otherwise be sent to
    /// server B under a name B happens to recognize, which is a call running with the wrong server's
    /// posture. Entries whose server differs are skipped rather than refused, because the caller
    /// naturally holds the whole catalog and filtering is the convenient correct behaviour.
    #[must_use]
    pub fn new(
        server: &jarvis_mcp::ServerName,
        connection: Arc<McpConnection>,
        entries: &[CatalogEntry],
    ) -> Self {
        let routes = entries
            .iter()
            .filter(|entry| entry.server == *server)
            .map(|entry| (entry.definition.id().to_string(), entry.remote.clone()))
            .collect();
        Self {
            server: server.as_str().to_owned(),
            connection,
            routes,
        }
    }

    /// Returns the operator's name for the server this adapter runs against.
    #[must_use]
    pub fn server(&self) -> &str {
        &self.server
    }

    /// Returns how many tools this adapter can route.
    #[must_use]
    pub fn routed_tools(&self) -> usize {
        self.routes.len()
    }

    /// Resolves the server's own name for a canonical identifier.
    ///
    /// # Errors
    ///
    /// Returns [`AdapterError::NotImplemented`] when the identifier was never routed to this adapter,
    /// which is a registry mistake rather than a runtime one — the same vocabulary the filesystem
    /// adapter uses for an unrecognized tool, so a mistake has one shape wherever it appears.
    fn remote_for(&self, id: &str) -> Result<&str, AdapterError> {
        match self.routes.get(id) {
            Some(remote) => Ok(remote.as_str()),
            None => Err(AdapterError::NotImplemented {
                tool: id.to_owned(),
            }),
        }
    }

    /// Runs one call against the server.
    async fn call(&self, remote: &str, arguments: &Value) -> Result<ToolCallResult, AdapterError> {
        // The arguments were validated against the tool's input schema by the pipeline, and the schema
        // is `type: object`, so a non-object value cannot be a valid call. It is refused before the
        // transport rather than coerced, because coercing would send the server something other than
        // what was authorized — and the authorization digest covers the arguments as they are.
        let Some(object) = arguments.as_object() else {
            return Err(AdapterError::RefusedBeforeReaching {
                reason: "the validated arguments are not a JSON object".to_owned(),
            });
        };

        // The evidence is built **before** the call, so a value this module cannot record is discovered
        // as a refusal rather than after a tool has already run. Both inputs are the operator's own
        // configuration — a validated server name and a name the server listed — so this cannot fail in
        // practice, and building it here is what removes the need for an "outcome exists but cannot be
        // described" fallback after an effect has happened.
        let evidence =
            ProviderEvidence::new(Self::locator(&self.server, remote)).map_err(|_| {
                AdapterError::RefusedBeforeReaching {
                    reason: "the server and tool names cannot form a usable evidence locator"
                        .to_owned(),
                }
            })?;

        match self.connection.call_tool(remote, object).await {
            Ok(result) => Ok(Self::convert(&result, remote, evidence)),
            Err(error) => Err(Self::classify(error, remote)),
        }
    }

    /// Converts a wire result into an established outcome.
    ///
    /// The two branches are the protocol's own two kinds of bad news, and they map to different
    /// outcomes for the reason the module doc gives: a tool that ran and refused produced evidence that
    /// nothing happened, while a request that was refused never ran anything.
    fn convert(result: &McpCallResult, remote: &str, evidence: ProviderEvidence) -> ToolCallResult {
        let reported_at = UtcTimestamp::now(&SystemClock);
        if result.is_error {
            // The tool ran. It said it could not do the thing, which is `Failed` with a reason. The
            // reason is built by `bounded_reason`, which is bounded to fit the stored outcome detail by
            // construction, so `failed` cannot refuse it.
            let reason = bounded_reason(&result.text, remote, "the tool reported a failure");
            let Ok(record) = ToolOutcomeRecord::failed(reason) else {
                return undescribable(reported_at);
            };
            return ToolCallResult::new(record, None, None, reported_at);
        }

        // A success. The evidence is structural and the output is kept separately from it, which is the
        // split `tools-and-connectors.md` requires: "preserve provider IDs separately from user-facing
        // text". A server that returned no text and no structured content still gets `Confirmed`,
        // because the *call* is the evidence — an empty result from a tool that ran is a successful
        // call, not a missing one.
        let output = output_for(result);
        let Ok(record) = ToolOutcomeRecord::confirmed(evidence.as_str()) else {
            return undescribable(reported_at);
        };
        ToolCallResult::new(record, Some(evidence), output, reported_at)
    }

    /// Builds the evidence locator for a call.
    ///
    /// Names the operator's server and the server's own tool — the two facts that make the effect
    /// findable again — and deliberately **not** the call identifier, because the call row already
    /// carries that and repeating it here would make the evidence a second copy of a stored value.
    fn locator(server: &str, remote: &str) -> String {
        format!("mcp:{server}/{remote}")
    }

    /// Classifies a transport failure into the adapter's vocabulary.
    ///
    /// **The classification is the point of this function.** `jarvis_tools` separates "refused before
    /// reaching a provider" from "the provider was reached but the outcome is unknown" because that
    /// distinction is what maps onto `Failed` and `Unknown`, and `Unknown` is what stops a retry that
    /// would duplicate a non-idempotent effect. The transport knows *what* failed; only this adapter
    /// knows whether anything was transmitted, so the mapping belongs here.
    ///
    /// `Unavailable` is treated as ambiguous rather than as a refusal, and that direction is deliberate:
    /// a request that failed partway may have been received. Calling it "nothing happened" would invite
    /// a retry of a tool that may have sent a message, which is the exact defect
    /// [`AdapterError::AmbiguousAfterReaching`] exists to prevent — and the cost of the safe direction is
    /// only that a call which truly did nothing is not retried automatically.
    fn classify(error: CallError, remote: &str) -> AdapterError {
        match error {
            // The peer answered. Its answer is a fact about the request, not an absence of one, so
            // nothing happened and the reason is known.
            CallError::PeerError(reason) => AdapterError::ProviderRefused {
                reason: bounded_reason(&reason, remote, "the server refused the call"),
            },
            // The server's result could not be decoded. It answered, so it ran something, and an
            // outcome cannot be established — which is exactly what "unknown" is for.
            CallError::Undecodable(reason) => AdapterError::AmbiguousAfterReaching {
                reason: bounded_reason(&reason, remote, "the server's answer was undecodable"),
            },
            // MRTR and Tasks are modes JARVIS declared no capability for. The server reached its own
            // boundary and nothing was done, so the request was refused at the protocol level.
            CallError::InputRequired => AdapterError::ProviderRefused {
                reason: bounded_reason(
                    "the server requires an input round JARVIS cannot hold",
                    remote,
                    "the server requires client input",
                ),
            },
            CallError::Task => AdapterError::ProviderRefused {
                reason: bounded_reason(
                    "the server answered with a task JARVIS does not poll",
                    remote,
                    "the server answered with a task",
                ),
            },
            // The request may or may not have been received. See this function's doc for why this is
            // the safe direction.
            CallError::Unavailable(reason) => AdapterError::AmbiguousAfterReaching {
                reason: bounded_reason(&reason, remote, "the call did not complete"),
            },
        }
    }
}

#[async_trait::async_trait]
impl ToolExecutor for McpToolAdapter {
    fn adapter_id(&self) -> &'static str {
        "mcp-tool"
    }

    async fn execute(
        &self,
        request: &ToolExecutionRequest,
    ) -> Result<ToolCallResult, AdapterError> {
        // The deadline is checked before the call, matching the filesystem adapter: a call already past
        // its deadline must not start work, and reporting that as a refusal ("nothing happened") rather
        // than as a failure is the honest choice.
        if request.is_past_deadline(UtcTimestamp::now(&SystemClock)) {
            return Err(AdapterError::RefusedBeforeReaching {
                reason: "the call was already past its deadline".to_owned(),
            });
        }

        let remote = self.remote_for(&request.tool().to_string())?;
        self.call(remote, request.arguments()).await
    }
}

/// Bounds a server-supplied reason and gives it a fallback.
///
/// A server's text is unbounded input that is about to be stored, so it is truncated rather than trusted;
/// and an empty reason is replaced by the caller's own sentence, because "why did this fail" with no
/// answer is exactly the opaque diagnostic this project keeps removing.
///
/// **The result is bounded to fit [`jarvis_core::MAX_OUTCOME_DETAIL_CHARS`] by construction**, including
/// the `mcp:<tool>: ` prefix, so [`ToolOutcomeRecord::failed`] cannot refuse a reason this function
/// produced. That is what lets the caller treat the record's construction as infallible rather than
/// checking for a refusal that would leave it with no honest outcome to report.
fn bounded_reason(text: &str, remote: &str, fallback: &str) -> String {
    let prefix = format!("mcp:{remote}: ");
    if prefix.chars().count() >= jarvis_core::MAX_OUTCOME_DETAIL_CHARS {
        // A tool name so long that the prefix alone fills the bound. Truncating the prefix would hide
        // which tool the reason is about, so the last resort is a reason with no tool in it — still
        // actionable, and still within the bound.
        return "mcp: the refusal reason exceeded the recordable length".to_owned();
    }
    let budget = jarvis_core::MAX_OUTCOME_DETAIL_CHARS - prefix.chars().count();
    let trimmed = text.trim();
    let body: String = if trimmed.is_empty() {
        fallback.to_owned()
    } else {
        trimmed.chars().take(budget).collect()
    };
    format!("{prefix}{}", body.chars().take(budget).collect::<String>())
}

/// Reports a call that completed but whose outcome could not be described.
///
/// `Unknown` is the only honest answer here — the call happened and the record of what it did could not
/// be built — and it is reachable only if a value this module constructs is rejected by its own
/// validator, which would be an authoring error.
///
/// A `panic` rather than an `expect`: the workspace denies `expect_used`, `Unknown` requires neither
/// evidence nor a reason **by definition**, and a fallback here would have to fabricate an outcome —
/// the one thing this module must not do.
fn undescribable(reported_at: UtcTimestamp) -> ToolCallResult {
    let Ok(record) = ToolOutcomeRecord::new(ToolOutcome::Unknown) else {
        panic!(
            "the tool outcome vocabulary rejected a state needing neither evidence nor a reason"
        );
    };
    ToolCallResult::new(record, None, None, reported_at)
}

/// Builds the bounded output for a successful call, or `None` when the server returned nothing.
///
/// `structured_content` is preferred over the text blocks when both exist, because it is the protocol's
/// own machine-readable form and re-serializing the text would produce JSON encoded as a string — a
/// shape a model would then have to un-escape. When only text is present, the text is the output, which
/// is what a model-facing tool result usually is.
///
/// An associated function rather than a method because nothing about the decision depends on the
/// connection: it is a pure conversion from what the server sent.
fn output_for(result: &McpCallResult) -> Option<BoundedOutput> {
    if let Some(structured) = &result.structured {
        let rendered = serde_json::to_string(structured).unwrap_or_else(|_| "null".to_owned());
        return Some(BoundedOutput::truncating(rendered));
    }
    if result.text.is_empty() {
        // Nothing at all came back. `None` rather than an empty output: an empty string is a *value* the
        // tool might have returned, and recording it would make "the tool returned nothing"
        // indistinguishable from "the tool returned an empty string".
        return None;
    }
    Some(BoundedOutput::truncating(result.text.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reason_is_bounded_and_prefixed() {
        let long = "x".repeat(jarvis_core::MAX_OUTCOME_DETAIL_CHARS + 200);
        let reason = bounded_reason(&long, "search", "fell back");
        // The prefix names the tool, so a stored reason identifies what it was about.
        assert!(reason.starts_with("mcp:search: "), "{reason}");
        assert!(!reason.contains("fell back"), "a non-empty reason wins");
        assert!(
            reason.chars().count() <= jarvis_core::MAX_OUTCOME_DETAIL_CHARS,
            "{} chars exceeds the bound",
            reason.chars().count()
        );

        // An empty reason falls back rather than producing "mcp:search: ".
        let empty = bounded_reason("   ", "search", "the tool reported a failure");
        assert!(empty.ends_with("the tool reported a failure"), "{empty}");
    }

    /// **A reason this module builds must be one the recorder accepts.** This is the property that lets
    /// `convert` treat `ToolOutcomeRecord::failed` as infallible: if the bound were computed against the
    /// wrong limit, every refusal would silently become `Unknown` instead of the `Failed` it is.
    #[test]
    fn a_maximal_reason_fits_the_outcome_bound_and_is_accepted() {
        let maximal = bounded_reason(
            &"x".repeat(jarvis_core::MAX_OUTCOME_DETAIL_CHARS),
            // A maximal tool name, since the prefix is part of the total.
            &"s".repeat(64),
            "fallback",
        );
        assert!(
            maximal.chars().count() <= jarvis_core::MAX_OUTCOME_DETAIL_CHARS,
            "{} chars exceeds the bound",
            maximal.chars().count()
        );
        // The assertion that matters: it is *recordable*, which is what `convert` relies on.
        assert!(
            ToolOutcomeRecord::failed(maximal).is_ok(),
            "a bounded reason must be accepted by the recorder"
        );
    }

    /// A tool name long enough that the prefix alone would fill the bound gets a reason with no tool in
    /// it rather than a truncated prefix — truncating the prefix would hide which tool the reason is
    /// about, which is the part a reader needs.
    #[test]
    fn an_absurdly_long_tool_name_still_yields_a_recordable_reason() {
        let reason = bounded_reason("something went wrong", &"s".repeat(2000), "fallback");
        assert!(
            reason.chars().count() <= jarvis_core::MAX_OUTCOME_DETAIL_CHARS,
            "{} chars exceeds the bound",
            reason.chars().count()
        );
        assert!(ToolOutcomeRecord::failed(reason).is_ok());
    }
}
