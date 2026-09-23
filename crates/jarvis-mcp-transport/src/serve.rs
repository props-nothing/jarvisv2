//! JARVIS as an MCP **server**: the handler a remote client's requests arrive at.
//!
//! # The limit this closes
//!
//! `P3-009a` decided which browser origins may reach us and `P3-009d` decided which of our tools may be
//! advertised. Both are **values**, and a value decides nothing until something consults it. This module
//! is the thing that consults them, and it is the first place JARVIS is the one being called.
//!
//! # Filtering a list is not authorization
//!
//! The most important property here is not that `tools/list` is filtered. It is that **`tools/call`
//! re-derives the decision rather than trusting the name it is given.** An MCP client is free to send
//! `tools/call` for a name the server never advertised: nothing in the protocol couples the two, and a
//! client that was shown a filtered list yesterday can call anything today. A server that only filtered
//! `tools/list` would have a **catalogue**, not a control.
//!
//! So [`JarvisMcpServer`] holds the served set itself, and [`JarvisMcpServer::invoke`] refuses a name that
//! is not in it **before the runner is consulted at all**. The test that pins this uses a runner which
//! **fails the test if it is called**, because "the call was refused" and "the call was refused before
//! anything ran" are different claims and only the second is the security property.
//!
//! # Why the runner is a port rather than the pipeline
//!
//! A remote call has no JARVIS run, no session, and no actor of its own, and the daemon alone knows what
//! one should be attributed to. Keeping that out of this crate means the handler holds no identity, no
//! database, and no receipt — so the refusal rule can be tested with no daemon, which is the same reason
//! `jarvis-mcp` is pure and `jarvis-mcp-transport` keeps the SDK behind a port.
//!
//! # Two SDK defaults are overridden here, deliberately
//!
//! Read from `rmcp-3.4.0`'s source rather than assumed:
//!
//! - `ServerConfig::default()`/`InitializeResult::new` sets `protocol_version` to
//!   `ProtocolVersion::default()`, which is `LATEST` — and `LATEST` is **`V_2025_11_25`, the legacy
//!   handshake era**. A server inheriting it would advertise a revision it does not implement.
//! - `supported_protocol_versions` defaults to `ProtocolVersion::KNOWN_VERSIONS` — **every** version the
//!   SDK knows, including the legacy ones. A dual-era server would then accept an `initialize` handshake
//!   this build does not drive.
//!
//! Both are narrowed to one version, because the honest statement is "this server speaks
//! `2026-07-28`" rather than "this server speaks whatever the SDK happens to list".

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use jarvis_core::CorrelationId;
use jarvis_mcp::ServedTool;
use jarvis_tools::{AdapterError, ToolCallResult, ToolOutcome};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    ListToolsResult, ProtocolVersion, ServerCapabilities, ServerConfig, TextContent, Tool,
};
use rmcp::service::{MaybeSendFuture, RoleServer};
use rmcp::{ErrorData as McpError, ServerHandler};
use serde_json::Value;

/// The one protocol revision this server speaks.
///
/// Named explicitly rather than taken from the SDK, because `ProtocolVersion::LATEST` is `V_2025_11_25` —
/// the legacy handshake era — so a default would advertise the wrong revision. This is the same fact
/// `P3-008e` recorded for the client side, now with a second consequence.
///
/// **Private**, because `ProtocolVersion` is an SDK type and a `pub const` of that type would put it in this
/// crate's public surface. [`served_protocol_version`] is the public door, and it returns the wire string,
/// which is JARVIS's own vocabulary for the same fact.
const SERVED_PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion::V_2026_07_28;

/// Returns the protocol revision this server speaks, as its wire string.
///
/// The string rather than the SDK's type, so this crate's public surface names no SDK type. `"2026-07-28"` is
/// what a client sends and what this server compares against, so it is the form a caller outside this crate
/// needs — the SDK's newtype adds nothing but a dependency at the boundary.
#[must_use]
pub fn served_protocol_version() -> &'static str {
    SERVED_PROTOCOL_VERSION.as_str()
}

/// The server name as a remote client sees it.
///
/// A **product** name rather than an identity: MCP's own guidance is that a server's self-reported name is
/// not for disambiguation, which is why `jarvis-mcp` refuses to use one as an identifier. This is the
/// value an operator sees in a client's connection list.
pub const SERVER_NAME: &str = "jarvis";

/// The version this server reports as its implementation version.
///
/// Deliberately the crate version, so a client's "server changed" observation tracks a build rather than a
/// hand-maintained string that drifts. MCP version negotiation is **not** affected by this field — it is
/// `protocol_version` that is negotiated — so this is a diagnostic, not a protocol statement.
pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Runs one call the handler has already established is served.
///
/// The implementation is the daemon's, because attributing a remote call — to a workspace, an actor, a
/// policy version, an idempotency key, and a durable call row — needs the composition root's wiring. The
/// handler deliberately does not know any of that, so this port is the whole of what it needs.
///
/// # Why the handler checks the name first, and this does not
///
/// The name check lives in [`JarvisMcpServer::invoke`] so that it happens **before** this is reached, and so
/// that it is provable with a runner that fails when called. An implementation may therefore assume it is
/// asked only for a served tool — but it must still handle the case honestly if that assumption breaks,
/// which is why the return type is a full `Result` rather than something infallible.
#[async_trait]
pub trait ServedToolRunner: Send + Sync {
    /// Runs one served tool.
    ///
    /// Returns [`ToolCallResult`] for any outcome the pipeline could establish, and [`AdapterError`] when
    /// it could not run the tool at all. The distinction is carried through unchanged to the caller.
    ///
    /// # Errors
    ///
    /// Returns [`AdapterError`] when the call could not be run: an unimplemented tool, a refusal before
    /// reaching anything, or an ambiguous result after reaching a provider.
    async fn run(
        &self,
        name: &str,
        arguments: Value,
        correlation_id: CorrelationId,
    ) -> Result<ToolCallResult, AdapterError>;
}

/// The MCP server handler: it advertises the served set and refuses anything outside it.
///
/// `Clone` because the SDK's Streamable HTTP service factory constructs one handler **per request**, so a
/// shared handler must be clonable. Both fields are an `Arc` or a `Vec` of owned values, so a clone shares the
/// runner and copies the served list — the expensive parts are not duplicated.
#[derive(Clone)]
pub struct JarvisMcpServer {
    served: Vec<ServedTool>,
    runner: Arc<dyn ServedToolRunner>,
}

impl fmt::Debug for JarvisMcpServer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Prints the served **names** rather than the runner: an implementation holds the daemon's
        // database and actor wiring, none of which belongs in a formatted value.
        let names: Vec<&str> = self.served.iter().map(ServedTool::name).collect();
        formatter
            .debug_struct("JarvisMcpServer")
            .field("served", &names)
            .finish_non_exhaustive()
    }
}

impl JarvisMcpServer {
    /// Builds a handler over an already-filtered served set.
    ///
    /// Takes [`ServedTool`] values rather than definitions, so **the filtering has already happened** and
    /// this type cannot advertise something the exposure rules excluded. That is a type-level statement
    /// rather than a convention: `served_tools` is the only thing that produces a `ServedTool`.
    #[must_use]
    pub fn new(served: Vec<ServedTool>, runner: Arc<dyn ServedToolRunner>) -> Self {
        Self { served, runner }
    }

    /// Returns the names this server advertises, in the order it advertises them.
    #[must_use]
    pub fn served_names(&self) -> Vec<&str> {
        self.served.iter().map(ServedTool::name).collect()
    }

    /// Returns whether a name is servable.
    ///
    /// The single predicate `tools/list` and `tools/call` both consult, so the two cannot disagree. A
    /// second test — for instance a prefix check at call time and an exact match at list time — is how a
    /// server comes to advertise one set and accept another.
    #[must_use]
    pub fn serves(&self, name: &str) -> bool {
        self.served.iter().any(|served| served.name() == name)
    }

    /// Returns whether this server advertises **nothing**.
    ///
    /// Offered so a layer deciding whether to offer the endpoint asks the handler rather than scanning
    /// `served_names`, and so the emptiness check has one home. An endpoint that advertises nothing is a
    /// server a client cannot use.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.served.is_empty()
    }

    /// Builds the tool list a remote client sees.
    ///
    /// **`pub(crate)` because `Tool` is an SDK type and this is not a surface a caller outside this crate
    /// needs**: the trait impl below is what answers `tools/list`, and the daemon reaches this through the SDK
    /// service rather than by calling it. A public `Vec<Tool>` would put the SDK's tool type in this crate's
    /// contract, which is the invariant `boundary_tests.rs` enforces.
    #[must_use]
    pub(crate) fn tool_list(&self) -> Vec<Tool> {
        self.served
            .iter()
            .map(|served| {
                // The schema is converted from a `Value` to the SDK's object type. A non-object schema
                // cannot be produced by `ToolSchema`, which validates 2020-12 and requires an object root,
                // so the fallback below is unreachable — but it is a *schema* rather than a panic, because
                // a panic while answering `tools/list` would take the whole server down for one tool.
                let schema = match served.input_schema() {
                    Value::Object(object) => object.clone(),
                    _ => serde_json::Map::new(),
                };
                // Built through the SDK's constructor rather than a struct literal: `Tool` is
                // `#[non_exhaustive]`, deliberately, so a field added by a protocol revision cannot be
                // forgotten silently at a literal. `new_with_raw` carries a description and no title, and
                // the title is set explicitly below.
                let mut tool = Tool::new_with_raw(
                    served.name().to_owned(),
                    Some(served.description().to_owned().into()),
                    Arc::new(schema),
                );
                tool.title = Some(served.title().to_owned());
                // No annotations. The protocol's `readOnlyHint`/`destructiveHint`/`idempotentHint` are
                // exactly the facts a policy engine needs, and the specification warns clients to treat
                // them as untrusted — so a JARVIS tool's declared effects stay in the JARVIS contract
                // (ADR-0025) rather than being restated in a field the protocol says not to trust.
                tool
            })
            .collect()
    }

    /// Handles one `tools/call`, enforcing the served set **before** anything runs.
    ///
    /// This is the method the security property lives in, and it is public so it can be tested without
    /// constructing an SDK request context — the trait impl below is a thin delegation.
    ///
    /// **`pub(crate)`**, because its return type is the SDK's `CallToolResult` and this crate's public surface
    /// may not name an SDK type. The trait impl below is what a client reaches; [`Self::serves`] is the public
    /// statement of the same rule, so a caller outside the crate can ask the question without the wire answer.
    ///
    /// A name that is not served, or absent, is refused with the same answer so a caller cannot use the
    /// difference to enumerate what exists. An unserved name is indistinguishable from a tool that does
    /// not exist, which is the honest answer: this server does not have it.
    pub(crate) async fn invoke(
        &self,
        name: Option<&str>,
        arguments: Option<serde_json::Map<String, Value>>,
    ) -> CallToolResult {
        let Some(name) = name else {
            return refusal("a tool call must name a tool");
        };
        if !self.serves(name) {
            // **The refusal happens here, before the runner exists in the control flow.** A caller that
            // sends a name the server never advertised learns only that it is not available.
            return refusal(&format!("{name} is not available on this server"));
        }

        // The correlation identity is minted rather than taken from the request: MCP's `_meta`
        // trace-context keys are optional OpenTelemetry conventions, and a client-supplied value is a
        // client-supplied identifier. The daemon shares this with the call row it writes, so a served call
        // is traceable even though the client cannot name it.
        let correlation_id = CorrelationId::new();
        let arguments = Value::Object(arguments.unwrap_or_default());

        match self.runner.run(name, arguments, correlation_id).await {
            Ok(result) => call_result(&result),
            // An `AdapterError` means nothing was established: either the call was refused before reaching
            // anything, or it reached a provider without learning the outcome. Both are refused to the
            // caller, and the message is the adapter's own bounded text.
            Err(error) => refusal(&error.to_string()),
        }
    }
}

/// Builds a refusal the caller can read.
///
/// `CallToolResult::error` rather than a JSON-RPC error, deliberately, and the SDK's own documentation is
/// the reason: a protocol error is rendered opaquely by clients, so a caller "will not see your message".
/// A refused tool call is a **result** about that call, not an unroutable request, so it travels as one.
fn refusal(message: &str) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::Text(TextContent::new(message))])
}

/// Converts an established JARVIS outcome into the protocol's answer.
///
/// **`pub(crate)`**: the return type is the SDK's `CallToolResult`, so this is an internal conversion rather
/// than a boundary this crate offers. The outcome table below is the reasoning, and a caller outside the crate
/// reasons about `ToolOutcome` itself rather than about the wire form.
///
/// # The table, and why each row is the way it is
///
/// | Outcome | Reported as | Why |
/// | --- | --- | --- |
/// | `Confirmed` | success with the bounded output | The provider gave evidence the effect completed. |
/// | `Failed` | error with the reason | Evidence says no effect occurred, so the caller may act on it. |
/// | `Unknown` | error with the reason | The result **cannot establish** whether an effect occurred. Reporting success would tell a caller an effect happened that may not have; reporting a plain failure would invite a repeat of an effect that may have already happened. The reason therefore says the outcome is unknown rather than that it failed. |
/// | anything else | error naming the outcome | `Requested`/`Authorized`/`Submitted`/`Cancelled` are not results an adapter reports here, so reaching one means the pipeline returned a non-terminal state. Naming it is better than mapping it onto success or failure, either of which would be a claim nothing made. |
///
/// **The `Unknown` row is the one that matters**, and it is the same distinction
/// `AdapterError::AmbiguousAfterReaching` draws one level down: the protocol offers a caller only
/// success-or-error, so an unprovable outcome has to travel in the text rather than in a third state that
/// does not exist. A caller that reads the message knows not to assume either way.
#[must_use]
pub(crate) fn call_result(result: &ToolCallResult) -> CallToolResult {
    let outcome = result.outcome();
    let detail = |fallback: &str| -> String {
        result
            .record()
            .reason()
            .map_or_else(|| fallback.to_owned(), str::to_owned)
    };

    match outcome {
        ToolOutcome::Confirmed => {
            let content = result.output().map_or_else(String::new, |output| {
                // A truncation is stated rather than hidden: a caller that received a clipped payload
                // without being told would treat it as the whole answer. `BoundedOutput` already refuses
                // silent truncation for exactly this reason, and the marker is how that survives the wire.
                if output.is_truncated() {
                    format!(
                        "{}\n\n[truncated: the full output was {} bytes]",
                        output.content(),
                        output.byte_len()
                    )
                } else {
                    output.content().to_owned()
                }
            });
            CallToolResult::success(vec![ContentBlock::Text(TextContent::new(content))])
        }
        ToolOutcome::Failed => refusal(&detail("the tool reported a failure")),
        ToolOutcome::Unknown => refusal(&format!(
            "the call's outcome is unknown, so no effect can be assumed either way: {}",
            detail("no reason was recorded")
        )),
        other => refusal(&format!(
            "the call did not reach a result ({})",
            other.as_str()
        )),
    }
}

impl ServerHandler for JarvisMcpServer {
    fn get_info(&self) -> ServerConfig {
        // `protocol_version` is set to the revision this server speaks rather than left at the SDK's
        // default, which is `LATEST` = `V_2025_11_25` — the legacy handshake era. Advertising it would
        // claim a revision this build does not drive.
        let mut config = ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(SERVER_NAME, SERVER_VERSION));
        config.protocol_version = SERVED_PROTOCOL_VERSION;
        config
    }

    fn supported_protocol_versions(&self) -> std::borrow::Cow<'static, [ProtocolVersion]> {
        // **Narrowed rather than inherited.** The SDK's default is every version it knows, including the
        // legacy ones, so a server inheriting it would accept an `initialize` handshake whose semantics
        // this build does not implement — and the client would have no way to tell.
        std::borrow::Cow::Borrowed(&[SERVED_PROTOCOL_VERSION])
    }

    /// Answers `tools/list` with the served set.
    ///
    /// Written as an `impl Future` returning `ready(..)` rather than an `async fn`, because there is nothing
    /// to await — and an `async fn` with no `.await` is the thing clippy's `unused_async` exists to catch.
    fn list_tools(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _context: rmcp::service::RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListToolsResult, McpError>> + MaybeSendFuture + '_
    {
        // No pagination: the served set is bounded by `jarvis_mcp::MAX_EXPOSED_TOOLS`, and a cursor over a
        // list that always fits in one response would be a mechanism a client must implement for no gain.
        std::future::ready(Ok(ListToolsResult::with_all_items(self.tool_list())))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: rmcp::service::RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        // Delegates so the refusal rule has one home. The trait returns a response union because the
        // protocol allows `input_required` and `task`; JARVIS answers `complete` for every call, because
        // an approval-shaped interaction belongs to the JARVIS approval path rather than being answered
        // by a remote client, which is the same rule ADR-0025 records for MRTR.
        let result = self
            .invoke(Some(request.name.as_ref()), request.arguments)
            .await;
        Ok(CallToolResponse::Complete(result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Mutex;

    use jarvis_core::{SystemClock, UtcTimestamp};
    use jarvis_tools::{
        BoundedOutput, ProviderEvidence, ToolOutcome, ToolOutcomeRecord, ToolSensitivity,
    };

    /// A runner that **fails the test if it is called**.
    ///
    /// This is the falsification instrument rather than a convenience. A runner that merely returned an
    /// error would let "the call was refused" pass even if the refusal happened *after* the runner ran —
    /// and refusing after running is not a control, it is a report. Recording the invocation and panicking
    /// makes "the refusal happened first" the thing under test.
    struct NeverCalledRunner {
        calls: Mutex<Vec<String>>,
    }

    impl NeverCalledRunner {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                calls: Mutex::new(Vec::new()),
            })
        }
    }

    #[async_trait]
    impl ServedToolRunner for NeverCalledRunner {
        async fn run(
            &self,
            name: &str,
            _arguments: Value,
            _correlation_id: CorrelationId,
        ) -> Result<ToolCallResult, AdapterError> {
            self.calls
                .lock()
                .unwrap_or_else(|error| panic!("the calls lock is usable: {error}"))
                .push(name.to_owned());
            panic!("the runner must not be reached for {name}");
        }
    }

    /// A runner that records what it was asked for and answers a scripted result.
    struct ScriptedRunner {
        calls: Mutex<Vec<(String, Value)>>,
        result: ToolCallResult,
    }

    impl ScriptedRunner {
        fn new(result: ToolCallResult) -> Arc<Self> {
            Arc::new(Self {
                calls: Mutex::new(Vec::new()),
                result,
            })
        }

        fn calls(&self) -> Vec<(String, Value)> {
            self.calls
                .lock()
                .unwrap_or_else(|error| panic!("the calls lock is usable: {error}"))
                .clone()
        }
    }

    #[async_trait]
    impl ServedToolRunner for ScriptedRunner {
        async fn run(
            &self,
            name: &str,
            arguments: Value,
            _correlation_id: CorrelationId,
        ) -> Result<ToolCallResult, AdapterError> {
            self.calls
                .lock()
                .unwrap_or_else(|error| panic!("the calls lock is usable: {error}"))
                .push((name.to_owned(), arguments));
            Ok(self.result.clone())
        }
    }

    /// Builds a record for an outcome, going through the constructors that enforce the honesty rules.
    ///
    /// `Confirmed` needs evidence and `Failed` needs a reason, so those two go through their own
    /// constructors; every other state carries neither and is built directly. That is why this is not a
    /// single call: the two states that **make a claim** cannot be recorded without a basis, which is the
    /// rule `ToolOutcomeRecord` exists to hold.
    fn outcome(outcome: ToolOutcome, reason: Option<&str>, output: Option<&str>) -> ToolCallResult {
        let record = match (outcome, reason) {
            (ToolOutcome::Confirmed, _) => ToolOutcomeRecord::confirmed("jarvis:test")
                .unwrap_or_else(|error| panic!("{error}")),
            (ToolOutcome::Failed, Some(reason)) => ToolOutcomeRecord::failed(reason)
                .unwrap_or_else(|error| panic!("{reason}: {error}")),
            (other, _) => {
                ToolOutcomeRecord::new(other).unwrap_or_else(|error| panic!("{other:?}: {error}"))
            }
        };
        ToolCallResult::new(
            record,
            Some(ProviderEvidence::new("jarvis:test").unwrap_or_else(|error| panic!("{error}"))),
            output.map(|text| BoundedOutput::truncating(text.to_owned())),
            UtcTimestamp::now(&SystemClock),
        )
    }

    /// A served tool with a minimal but real schema.
    fn served(name: &str) -> ServedTool {
        let definition_schema = serde_json::json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "properties": { "path": { "type": "string" } },
            "required": ["path"],
            "additionalProperties": false
        });
        // Built through the same constructor the daemon uses, so a test cannot construct a `ServedTool`
        // the exposure rules would not have produced.
        let definition = jarvis_tools::ToolDefinition::new(jarvis_tools::ToolDefinitionParts {
            id: jarvis_tools::ToolId::new(name).unwrap_or_else(|error| panic!("{name}: {error}")),
            version: "1.0.0".to_owned(),
            title: format!("title for {name}"),
            description: format!("description for {name}"),
            input_schema: jarvis_tools::ToolSchema::parse(&definition_schema.to_string())
                .unwrap_or_else(|error| panic!("{error}")),
            output_schema: jarvis_tools::ToolSchema::parse(
                &serde_json::json!({
                    "$schema": "https://json-schema.org/draft/2020-12/schema",
                    "type": "object",
                    "properties": { "contents": { "type": "string" } },
                    "required": ["contents"],
                    "additionalProperties": false
                })
                .to_string(),
            )
            .unwrap_or_else(|error| panic!("{error}")),
            effects: jarvis_tools::EffectSet::single(jarvis_tools::ToolEffect::ReadOnly),
            risk: 0,
            required_scopes: jarvis_tools::ScopeSet::none(),
            approval: jarvis_tools::ApprovalPolicy::Auto,
            timeout_seconds: 10,
            retry: jarvis_tools::RetryDeclaration::none(),
            idempotency: jarvis_tools::Idempotency::Unsupported,
            // Derived from the identifier rather than declared, because `ToolDefinition::new` **refuses** a
            // source that disagrees with the namespace — which is exactly what caught this fixture when it
            // declared `Native` for `gmail.messages.send`. A test cannot construct a definition the
            // production path would reject.
            source: jarvis_tools::ToolSource::from_namespace(
                name.rsplit_once('.')
                    .map_or(name, |(namespace, _)| namespace),
            ),
            availability: jarvis_tools::Availability::Available,
            sensitivity: ToolSensitivity::new(
                jarvis_core::Sensitivity::Internal,
                jarvis_core::Sensitivity::Internal,
            ),
        })
        .unwrap_or_else(|error| panic!("{name}: {error}"));

        let (served, exclusions) = jarvis_mcp::served_tools([&definition])
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        assert!(
            exclusions.is_empty(),
            "{name} must be servable: {exclusions:?}"
        );
        served
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("{name} produced no served tool"))
    }

    /// **The security property: an unadvertised tool is refused before anything runs.**
    ///
    /// The runner panics if it is reached, so this test fails loudly rather than quietly if the refusal
    /// moves after the call — which is the difference between a control and a report. Falsified by removing
    /// the `serves` check: this test then fails inside the runner with
    /// `the runner must not be reached for mcp.github.search`.
    #[tokio::test]
    async fn an_unadvertised_tool_is_refused_before_the_runner_is_reached() {
        let runner = NeverCalledRunner::new();
        let server = JarvisMcpServer::new(vec![served("jarvis.files.read")], runner.clone());

        // A tool this project owns but does not serve, and one that does not exist at all: both refused.
        for name in ["mcp.github.search", "jarvis.files.write", ""] {
            let result = server.invoke(Some(name), None).await;
            assert_eq!(
                result.is_error,
                Some(true),
                "{name} must be refused rather than run"
            );
        }
        // And no name at all, which is a malformed call rather than an unserved one.
        assert_eq!(server.invoke(None, None).await.is_error, Some(true));

        assert!(
            runner
                .calls
                .lock()
                .unwrap_or_else(|error| panic!("{error}"))
                .is_empty(),
            "no refused name may reach the runner"
        );
    }

    /// A served name **does** reach the runner, so the refusal test is not passing because nothing works.
    #[tokio::test]
    async fn a_served_tool_reaches_the_runner_and_its_arguments_are_passed_through() {
        let runner = ScriptedRunner::new(outcome(
            ToolOutcome::Confirmed,
            None,
            Some("the file contents"),
        ));
        let server = JarvisMcpServer::new(vec![served("jarvis.files.read")], runner.clone());

        let mut arguments = serde_json::Map::new();
        arguments.insert(
            "path".to_owned(),
            Value::String("notes/todo.txt".to_owned()),
        );
        let result = server
            .invoke(Some("jarvis.files.read"), Some(arguments))
            .await;

        assert_eq!(result.is_error, Some(false));
        let calls = runner.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "jarvis.files.read");
        assert_eq!(calls[0].1["path"], "notes/todo.txt");
    }

    /// A call with no arguments sends an **empty object**, not `null`: the tool's schema is an object, so
    /// passing `null` would fail validation for a tool whose parameters are all optional.
    #[tokio::test]
    async fn absent_arguments_become_an_empty_object() {
        let runner = ScriptedRunner::new(outcome(ToolOutcome::Confirmed, None, Some("ok")));
        let server = JarvisMcpServer::new(vec![served("jarvis.files.read")], runner.clone());
        server.invoke(Some("jarvis.files.read"), None).await;

        let calls = runner.calls();
        assert_eq!(calls[0].1, serde_json::json!({}));
    }

    /// The advertised list and the accepted set are the **same set**, derived from one predicate.
    ///
    /// Falsified by giving `serves` a different rule from `tool_list` — a prefix test, say: the advertised
    /// name passes but a longer name does too, so the server accepts what it never advertised.
    #[tokio::test]
    async fn the_advertised_set_and_the_accepted_set_are_identical() {
        let server = JarvisMcpServer::new(
            vec![served("jarvis.files.read"), served("gmail.messages.send")],
            NeverCalledRunner::new(),
        );

        let advertised: Vec<String> = server
            .tool_list()
            .iter()
            .map(|tool| tool.name.to_string())
            .collect();
        assert_eq!(advertised, vec!["jarvis.files.read", "gmail.messages.send"]);

        for name in &advertised {
            assert!(
                server.serves(name),
                "{name} is advertised so it must be served"
            );
        }
        // And the set is closed: nothing else is served, including near-misses.
        for name in [
            "jarvis.files.rea",
            "jarvis.files.read.more",
            "gmail.messages",
        ] {
            assert!(!server.serves(name), "{name} must not be served");
        }
    }

    /// **The `Unknown` row of the outcome table, and the reason it is not a plain failure.**
    ///
    /// The message must say the outcome is unknown rather than that the call failed, because a caller that
    /// read "failed" would reasonably retry, and a retry of a non-idempotent effect is a second effect.
    ///
    /// Falsified by mapping `Unknown` onto the same text as `Failed`: this test fails, and the distinction
    /// that decides whether a repeat is safe is gone.
    #[tokio::test]
    async fn an_unknown_outcome_says_so_rather_than_reporting_a_failure() {
        let result = outcome(
            ToolOutcome::Unknown,
            Some("the connection closed after the request was sent"),
            None,
        );
        let converted = call_result(&result);
        assert_eq!(converted.is_error, Some(true));
        let text = text_of(&converted);
        assert!(
            text.contains("unknown"),
            "the message must say the outcome is unknown, got: {text}"
        );
        assert!(
            text.contains("no effect can be assumed either way"),
            "the message must tell a caller not to assume either way, got: {text}"
        );
        // And it is not the failure text, which is the falsifiable part.
        assert_ne!(
            text,
            text_of(&call_result(&outcome(
                ToolOutcome::Failed,
                Some("it failed"),
                None
            )))
        );
    }

    /// A `Confirmed` outcome carries the output as text, which is what a caller acts on.
    #[tokio::test]
    async fn a_confirmed_outcome_returns_the_output_as_content() {
        let converted = call_result(&outcome(ToolOutcome::Confirmed, None, Some("buy milk")));
        assert_eq!(converted.is_error, Some(false));
        assert!(
            text_of(&converted).contains("buy milk"),
            "the output must reach the caller"
        );
    }

    /// The protocol **SHOULD**-constrains one implementation name and version, and the version is the
    /// build's rather than a hand-maintained string.
    #[test]
    fn the_server_reports_itself_with_the_build_version() {
        let server =
            JarvisMcpServer::new(vec![served("jarvis.files.read")], NeverCalledRunner::new());
        let info = server.get_info();
        assert_eq!(info.server_info.name, SERVER_NAME);
        assert_eq!(info.server_info.version, SERVER_VERSION);
    }

    /// **The protocol version is the revision this server speaks, not the SDK's default.**
    ///
    /// Falsified by letting `ServerConfig::new`'s default stand: it is `ProtocolVersion::default()` =
    /// `LATEST` = `V_2025_11_25`, the legacy handshake era, so the server would advertise a revision it
    /// does not implement.
    #[test]
    fn the_advertised_protocol_version_is_the_modern_revision() {
        let server =
            JarvisMcpServer::new(vec![served("jarvis.files.read")], NeverCalledRunner::new());
        assert_eq!(
            server.get_info().protocol_version,
            ProtocolVersion::V_2026_07_28
        );
        assert_eq!(server.get_info().protocol_version, SERVED_PROTOCOL_VERSION);
        assert_ne!(
            server.get_info().protocol_version,
            ProtocolVersion::LATEST,
            "the SDK's LATEST is a legacy revision and must not be advertised"
        );
    }

    /// **The supported list is narrowed to one version, not inherited from the SDK's full list.**
    ///
    /// Falsified by removing the override: the default is `ProtocolVersion::KNOWN_VERSIONS`, which contains
    /// versions that use the `initialize` handshake this build does not drive — so the server would accept a
    /// handshake whose semantics it does not implement, and the client could not tell.
    #[test]
    fn only_the_modern_revision_is_supported() {
        let server =
            JarvisMcpServer::new(vec![served("jarvis.files.read")], NeverCalledRunner::new());
        let supported = server.supported_protocol_versions();
        assert_eq!(supported.as_ref(), &[ProtocolVersion::V_2026_07_28]);
        assert!(
            supported.len() < ProtocolVersion::KNOWN_VERSIONS.len(),
            "the supported set must be narrower than every version the SDK knows"
        );
        for version in ProtocolVersion::KNOWN_VERSIONS {
            if *version != ProtocolVersion::V_2026_07_28 {
                assert!(
                    !supported.contains(version),
                    "{version} must not be advertised as supported"
                );
            }
        }
    }

    /// A served tool's schema travels as declared, so a caller sees the contract the daemon enforces.
    #[test]
    fn the_advertised_schema_is_the_declared_schema() {
        let server =
            JarvisMcpServer::new(vec![served("jarvis.files.read")], NeverCalledRunner::new());
        let tools = server.tool_list();
        assert_eq!(tools.len(), 1);
        assert_eq!(
            tools[0].input_schema.get("required"),
            Some(&serde_json::json!(["path"])),
            "the schema's constraints must reach the client"
        );
        // And no annotations, which the protocol declares untrusted — a tool's effects stay in the JARVIS
        // contract rather than being restated where a client is told not to believe them.
        assert!(tools[0].annotations.is_none());
    }

    /// An empty served set advertises nothing and accepts nothing, so it cannot be a half-open server.
    #[tokio::test]
    async fn an_empty_served_set_serves_nothing() {
        let runner = NeverCalledRunner::new();
        let server = JarvisMcpServer::new(Vec::new(), runner.clone());
        assert!(server.tool_list().is_empty());
        assert!(server.served_names().is_empty());
        assert_eq!(
            server
                .invoke(Some("jarvis.files.read"), None)
                .await
                .is_error,
            Some(true)
        );
        assert!(
            runner
                .calls
                .lock()
                .unwrap_or_else(|error| panic!("{error}"))
                .is_empty()
        );
    }

    /// Pulls the text out of a result, for asserting on the message a caller would read.
    fn text_of(result: &CallToolResult) -> String {
        result
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text(text) => Some(text.text.clone()),
                _ => None,
            })
            .collect::<Vec<String>>()
            .join("\n")
    }
}
