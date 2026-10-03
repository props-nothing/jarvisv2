//! In-crate tests for the composed tool pipeline.
//!
//! These are the tests `P3-001` through `P3-006` could not have. Every one of those slices was correct
//! in isolation, and **two real security defects lived in the space between them** (`P3-006a` and
//! `P3-006b`). A slice's own tests cannot see that space, so these tests exercise the **seams** — the
//! points where one slice's output becomes another's input — and they exist because composition is what
//! found those defects.
//!
//! | Property | Why it is a seam rather than a unit |
//! | --- | --- |
//! | a read reaches the filesystem and is recorded | six slices had no production caller at all |
//! | arguments are validated before policy reads them | policy decides about declared fields, so unvalidated input is a decision about a different call |
//! | a denial never reaches the adapter | reaching an adapter means the denial was ignored |
//! | a traversal is refused through every layer | confinement is only real if the composed path honours it |
//! | the outcome is durable and not replaceable | `P3-005`'s states only mean something once something writes them |

use std::path::PathBuf;
use std::sync::Arc;

use jarvis_core::{CorrelationId, SessionChannel, SystemClock, ToolOutcome, UtcTimestamp};
use jarvis_storage::{
    DatabaseError, LOCAL_USER_ID, LOCAL_WORKSPACE_ID, SqliteDatabase, StartRunInput, start_run,
};
use jarvis_tools::{
    AuthenticationStrength, LIST_TOOL, READ_TOOL, ToolId, ToolOutcomeRecord, WorkspacePolicy,
    WorkspaceRoots,
};
use serde_json::json;

use super::{ToolPipeline, ToolPipelineError, ToolPipelineOutcome};
use crate::tool_actor::ToolActor;

const SESSION: &str = "0198f000-0000-7000-8000-000000000003";
const RUN: &str = "0198f000-0000-7000-8000-0000000000c3";
const EVENT: &str = "0198f000-0000-7000-8000-0000000000e4";

/// A temporary root removed when the test ends, pass or fail.
struct TempRoot(PathBuf);

impl TempRoot {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "jarvis-tool-pipeline-{}",
            jarvis_core::scratch_tag()
        ));
        std::fs::create_dir_all(&path).unwrap_or_else(|error| panic!("create root: {error}"));
        // Canonicalized because `temp_dir()` on Windows can return an 8.3 short form whose text
        // differs from the long form a directory handle reports.
        let path = path
            .canonicalize()
            .unwrap_or_else(|error| panic!("canonicalize: {error}"));
        Self(path)
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }

    fn join(&self, relative: &str) -> PathBuf {
        self.0.join(relative)
    }

    fn write(&self, relative: &str, contents: &str) {
        let path = self.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .unwrap_or_else(|error| panic!("create parent: {error}"));
        }
        std::fs::write(&path, contents)
            .unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        jarvis_core::remove_scratch_dir(&self.0);
    }
}

fn must<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("expected success, got {error:?}"),
    }
}

fn must_err<T, E>(result: Result<T, E>) -> E {
    match result {
        Ok(_) => panic!("expected an error"),
        Err(error) => error,
    }
}

fn at(offset_seconds: i128) -> UtcTimestamp {
    // Anchored to the clock rather than to a fixed instant: the adapter's deadline check reads the real
    // clock, so a fixture based on a hardcoded date silently becomes "already lapsed" once that date
    // passes. That bug was found in `P3-006` and this fixture avoids repeating it.
    let now = UtcTimestamp::now(&SystemClock);
    UtcTimestamp::from_unix_nanos(now.unix_nanos() + offset_seconds * 1_000_000_000)
        .unwrap_or_else(|error| panic!("{error}"))
}

/// A database with the seeded local identity and one live run, plus the directory holding it.
///
/// The tuple carries the directory so the fixture cannot leak it. **An earlier version of this comment explained
/// the leak by tuple drop order and claimed that reversing the pair fixed it — both the claim and the explanation
/// were wrong, and the correction is worth keeping.** Reversing the pair changed the directory count by **zero**
/// (19 before, 19 after), because the file is not held by the directory guard at all: `sqlx` releases it **late and
/// off-thread**, so `Drop` order cannot matter. The real mechanism is in `P3-013` and
/// `docs/development/testing.md`; `TempRoot`'s removal now goes through `jarvis_core::remove_scratch_dir`, which
/// retries until the handle is released. *A fix that does not move the measured number is not the fix.*
async fn database_with_run() -> (TempRoot, Arc<SqliteDatabase>) {
    let directory = TempRoot::new();
    let database = Arc::new(must(
        SqliteDatabase::open(&directory.join("jarvis.sqlite3")).await,
    ));

    // `start_run` creates the session and the run in one transaction, which is the same path a client
    // takes, so this fixture cannot invent a state the daemon cannot produce.
    let input = must(StartRunInput::new(
        SESSION,
        RUN,
        EVENT,
        LOCAL_WORKSPACE_ID,
        LOCAL_USER_ID,
        "Read a file",
        SessionChannel::Cli,
        CorrelationId::new(),
        at(0),
    ));
    must(start_run(&database, &input).await);
    (directory, database)
}

/// A pipeline confined to `roots`, over a database with one live run.
async fn pipeline_with(
    roots: WorkspaceRoots,
    workspace: WorkspacePolicy,
) -> (TempRoot, Arc<SqliteDatabase>, ToolPipeline) {
    let (directory, database) = database_with_run().await;
    // The secret store lives **inside this test's own root**, never in the shared temp directory: a fixed
    // path would let two tests' nonce files collide, and a nonce file is exactly the thing that must belong
    // to one approval.
    let secrets = jarvis_storage::SecretStore::in_state(&directory.join("state"));
    let pipeline = must(ToolPipeline::new(
        Arc::clone(&database),
        roots,
        workspace,
        secrets,
    ));
    (directory, database, pipeline)
}

/// A pipeline over granted filesystem roots **and** an additional adapter.
///
/// # Why the filesystem adapter must be present for this to prove routing
///
/// The first version of the routing test built a pipeline with the MCP adapter as the **only** one, and the
/// falsification run is what showed the test proved nothing: replacing the dispatch lookup with "return any
/// adapter" left it green, because a table with one entry finds the right adapter however it is looked up. A
/// routing test needs **at least two** candidates, or it cannot tell a correct lookup from an arbitrary one.
///
/// The roots are a real granted directory, so the filesystem adapter is registered and its two tools are in
/// the table alongside the MCP tool. A call to the MCP identifier that reached the filesystem adapter would be
/// an `AdapterError::NotImplemented` — or, worse, a read of whatever path the arguments named.
async fn pipeline_with_extra(
    roots: WorkspaceRoots,
    definitions: Vec<jarvis_tools::ToolDefinition>,
    adapter: Arc<dyn jarvis_tools::ToolExecutor>,
) -> (TempRoot, Arc<SqliteDatabase>, ToolPipeline) {
    let (directory, database) = database_with_run().await;
    let secrets = jarvis_storage::SecretStore::in_state(&directory.join("state"));
    let pipeline = must(ToolPipeline::with_adapters(
        Arc::clone(&database),
        Some(roots),
        WorkspacePolicy::default(),
        vec![(definitions, adapter)],
        secrets,
    ));
    (directory, database, pipeline)
}

/// An adapter that records what it was asked to run and answers with a fixed confirmation.
///
/// A **recording** adapter rather than a scripted one, because the property under test is *routing*: which
/// adapter ran, and with which arguments. An adapter that returned a fixed outcome either way would satisfy
/// every result-shaped assertion, which is the same reasoning `ScriptedModel::seen_messages` follows for the
/// conversation tests.
#[derive(Default)]
struct RecordingAdapter {
    seen: std::sync::Mutex<Vec<(String, serde_json::Value)>>,
}

#[async_trait::async_trait]
impl jarvis_tools::ToolExecutor for RecordingAdapter {
    fn adapter_id(&self) -> &'static str {
        "mcp-test"
    }

    async fn execute(
        &self,
        request: &jarvis_tools::ToolExecutionRequest,
    ) -> Result<jarvis_tools::ToolCallResult, jarvis_tools::AdapterError> {
        if let Ok(mut guard) = self.seen.lock() {
            guard.push((request.tool().to_string(), request.arguments().clone()));
        }
        let record = must(ToolOutcomeRecord::confirmed("mcp:test/search"));
        Ok(jarvis_tools::ToolCallResult::new(
            record,
            jarvis_tools::ProviderEvidence::new("mcp:test/search").ok(),
            Some(jarvis_tools::BoundedOutput::truncating("found it")),
            UtcTimestamp::now(&SystemClock),
        ))
    }
}

/// **The seam this slice adds: a call reaches the adapter that owns its definition, not another one.**
///
/// The pipeline held exactly one adapter before this slice, so dispatch was not a question it could get
/// wrong. With two, a call to an MCP tool that reached the filesystem adapter would be an
/// `AdapterError::NotImplemented` — or, worse, a read of whatever path the arguments happened to name.
///
/// **The falsification run is what made this test meaningful.** The first version registered the MCP adapter
/// as the *only* one, and replacing the dispatch lookup with "return any adapter" left the whole suite green:
/// a one-entry table finds the right adapter however it is looked up. Two candidates are therefore required,
/// and the test asserts the *other* adapter did not run.
#[tokio::test]
async fn a_call_reaches_the_adapter_that_owns_its_definition() {
    // A real granted root, so the filesystem adapter is registered and its two tools share the table with the
    // MCP one. A file is written as well, so a misroute that happened to parse its arguments would find
    // something to read rather than failing for a missing file — making "it did not run" an observation about
    // routing rather than about the fixture.
    let directory = TempRoot::new();
    directory.write("notes/todo.txt", "buy milk");
    let roots = must(WorkspaceRoots::new([directory.path()]));

    let definition = mcp_definition("mcp.test.search", "search");
    let adapter = Arc::new(RecordingAdapter::default());
    let (_directory, _database, pipeline) = pipeline_with_extra(
        roots,
        vec![definition.clone()],
        Arc::clone(&adapter) as Arc<dyn jarvis_tools::ToolExecutor>,
    )
    .await;

    // Three tools are dispatchable: the filesystem adapter's two and the MCP one. Asserted so a table that
    // registered only one cannot make the routing assertion below vacuous — which is exactly how the first
    // version of this test passed a falsification it should have failed.
    assert_eq!(
        pipeline.dispatchable_tools(),
        3,
        "the filesystem adapter's two tools and the additional one must all be dispatchable"
    );

    let arguments = json!({ "q": "pumps" });
    // The correlation id is bound to a `let` because **the call id IS the correlation id** (`P3-006d`), so
    // reading the stored row back needs the same value. The first version of this test passed
    // `CorrelationId::new()` inline and then looked the row up under a hardcoded identifier that never
    // existed, which failed with `ToolCallNotFound` — a *test* mistake that the storage layer correctly
    // reported rather than resolving to something plausible.
    let correlation_id = CorrelationId::new();
    let outcome = must(
        pipeline
            .call_tool(
                "mcp.test.search",
                arguments.clone(),
                &mcp_actor(),
                correlation_id,
            )
            .await,
    );

    // The recording adapter ran, and it saw the canonical identifier and the exact arguments.
    let seen = adapter
        .seen
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_default();
    assert_eq!(
        seen.len(),
        1,
        "the additional adapter must run: {outcome:?}"
    );
    assert_eq!(seen[0].0, "mcp.test.search");
    assert_eq!(seen[0].1, arguments);

    // **And the outcome is the MCP adapter's confirmation, which a filesystem call cannot produce.** Its
    // arguments would be `{"q": "pumps"}`, which the filesystem adapter refuses as a missing `path`, so a
    // misroute appears as a refusal with a filesystem reason rather than as this confirmation.
    let ToolPipelineOutcome::Executed(result) = outcome else {
        panic!("a call to the additional adapter must execute, got {outcome:?}");
    };
    assert_eq!(result.outcome(), ToolOutcome::Confirmed);
    assert_eq!(
        result
            .evidence()
            .map(jarvis_tools::ProviderEvidence::as_str),
        Some("mcp:test/search"),
        "the recorded evidence must be the additional adapter's, not the filesystem adapter's"
    );

    // The durable row agrees with the returned result, so the two cannot disagree across an await. Asserted by
    // **reading the row back**, not by trusting the return value: `P3-005`'s ledger is what makes an outcome
    // mean something, and a routing change must not stop it being written.
    let stored = must(pipeline.call(&correlation_id.to_string()).await);
    assert_eq!(stored.outcome(), ToolOutcome::Confirmed);
    assert_eq!(
        stored.record().evidence(),
        Some("mcp:test/search"),
        "the stored row must carry the additional adapter's evidence"
    );
}

/// An actor holding the scope an MCP tool requires.
///
/// `mcp.call` as well as `files.read`, which is what the gateway now grants — and the reason is a **real
/// defect the first version of this test caught**: it used the filesystem-only actor, and the call was
/// refused with `missing_scope`. That is a *correct* policy decision, so the test was failing for a legitimate
/// reason rather than passing for the wrong one — which is exactly what the assertion's `{outcome:?}` was
/// there to show.
fn mcp_actor() -> ToolActor {
    ToolActor::workspace_and_mcp(
        LOCAL_WORKSPACE_ID,
        RUN,
        SessionChannel::Cli,
        AuthenticationStrength::Credential,
        "policy-1",
    )
    .unwrap_or_else(|| panic!("both fixed scope literals must be accepted"))
}

/// A read-only MCP-namespaced definition, built the way the catalog builds one.
///
/// Built through the transport's `translate_tool` with a read-only policy rather than assembled by hand, so
/// the definition carries the same identifier, version, scope, and risk the catalog would produce — a
/// hand-built one could declare a posture the catalog would never emit, and the routing test would then be
/// exercising a tool that does not exist in production.
///
/// The MCP types come from `jarvis-mcp` **as a dev-dependency**, not as a normal one: the daemon composes
/// adapters and never names an MCP type itself, so declaring the pure translation crate for production would
/// be a dependency with no production consumer. A test that builds an MCP definition genuinely needs it.
fn mcp_definition(id: &str, remote: &str) -> jarvis_tools::ToolDefinition {
    let policy = must(jarvis_mcp::ToolEffectPolicy::read_only());
    let schema = json!({
        "type": "object",
        "properties": { "q": { "type": "string" } },
        "required": ["q"]
    });
    let listing = jarvis_mcp::McpToolListing {
        name: remote,
        title: None,
        description: None,
        input_schema: &schema,
        output_schema: None,
    };
    let server = must(jarvis_mcp::ServerName::new("test"));
    let translated = must(jarvis_mcp::translate_tool(
        &server,
        &listing,
        jarvis_mcp::NamingStrategy::Prefixed,
        &policy,
    ));
    assert_eq!(
        translated.definition.id().to_string(),
        id,
        "the fixture's identifier must be the one the translation produces"
    );
    translated.definition
}

fn actor() -> ToolActor {
    ToolActor::workspace_reader(
        LOCAL_WORKSPACE_ID,
        RUN,
        SessionChannel::Cli,
        AuthenticationStrength::Credential,
        "policy-1",
    )
}

/// **The seam that did not exist: a read reaches the filesystem and is recorded durably.**
///
/// Before this slice no caller admitted a tool call at all — six slices of contract, registry, policy,
/// approval, lifecycle, and adapter had no production caller. This asserts the whole path runs, that the
/// result carries evidence separately from content, and that the row agrees with it.
#[tokio::test]
async fn a_read_through_the_pipeline_returns_content_and_is_recorded() {
    let directory = TempRoot::new();
    directory.write("notes/todo.txt", "buy milk");
    let roots = must(WorkspaceRoots::new([directory.path()]));
    let (_root, _database, pipeline) = pipeline_with(roots, WorkspacePolicy::default()).await;

    // The correlation identity is the handle on the durable row: the pipeline uses it as the call
    // identifier, so keeping it is what lets this assert the stored state as well as the result.
    let correlation = CorrelationId::new();
    let outcome = must(
        pipeline
            .call_tool(
                READ_TOOL,
                json!({"path": "notes/todo.txt"}),
                &actor(),
                correlation,
            )
            .await,
    );

    let ToolPipelineOutcome::Executed(result) = outcome else {
        panic!("a risk-0 scoped read must execute, got {outcome:?}");
    };
    assert_eq!(result.outcome(), ToolOutcome::Confirmed);

    // Evidence is separate from output, which is what makes a confirmation checkable: a
    // success-sounding content string is not proof, and the locator is.
    let evidence = result
        .evidence()
        .unwrap_or_else(|| panic!("a confirmation must carry evidence"));
    assert!(evidence.as_str().starts_with("file:"));
    let output = result
        .output()
        .unwrap_or_else(|| panic!("a read must return output"));
    assert_eq!(output.content(), "buy milk");

    // The durable row is what an audit reads, so the row and the returned result must agree. This is
    // the assertion that the pipeline *recorded* rather than only returned.
    let stored = must(pipeline.call(&correlation.to_string()).await);
    assert_eq!(stored.outcome(), ToolOutcome::Confirmed);
    assert_eq!(
        stored.output(),
        Some("buy milk"),
        "the stored row must carry the adapter's output"
    );
    assert_eq!(
        stored.record().evidence().map(str::to_owned),
        Some(evidence.as_str().to_owned()),
        "the stored row must carry the same evidence the result did"
    );
    assert!(
        !stored.must_not_repeat(),
        "a confirmed read is not in flight and had a proven effect, so it is not unrepeatable"
    );
}

/// **Invalid arguments are refused before policy sees them.**
///
/// Validation precedes policy deliberately: policy decides about a tool's **declared** fields, so a
/// decision about arguments nothing checked would be a decision about a different call. The violation
/// must name the schema's own keyword, which proves the compiled validator was consulted rather than a
/// hand-written check.
#[tokio::test]
async fn arguments_the_schema_rejects_are_refused_before_policy() {
    let directory = TempRoot::new();
    directory.write("notes.txt", "buy milk");
    let roots = must(WorkspaceRoots::new([directory.path()]));
    let (_root, _database, pipeline) = pipeline_with(roots, WorkspacePolicy::default()).await;

    let error = must_err(
        pipeline
            .call_tool(READ_TOOL, json!({}), &actor(), CorrelationId::new())
            .await,
    );
    match error {
        ToolPipelineError::InvalidArguments { tool, violations } => {
            assert_eq!(tool, READ_TOOL);
            assert!(
                violations.contains("required") || violations.contains("path"),
                "the violation must name the keyword or the field, got {violations}"
            );
        }
        other => panic!("expected an argument refusal, got {other:?}"),
    }
}

/// **A denied call never reaches the adapter, and the reason code is policy's own.**
///
/// The seam this protects: `P3-003` denies, and if the pipeline continued it would hand an adapter a
/// request the workspace refused. The assertion is policy's reason code rather than a generic error, so
/// a caller learns *why* — and the file is genuinely readable, so the refusal cannot be "it was absent".
#[tokio::test]
async fn a_denied_call_is_refused_by_reason_code_and_not_executed() {
    let directory = TempRoot::new();
    directory.write("secret.txt", "buy milk");
    let roots = must(WorkspaceRoots::new([directory.path()]));

    let denied = must(ToolId::new(READ_TOOL));
    let policy = WorkspacePolicy::default().denying(denied);
    let (_root, _database, pipeline) = pipeline_with(roots, policy).await;

    let outcome = must(
        pipeline
            .call_tool(
                READ_TOOL,
                json!({"path": "secret.txt"}),
                &actor(),
                CorrelationId::new(),
            )
            .await,
    );

    let ToolPipelineOutcome::Refused { reason_code } = outcome else {
        panic!("a workspace-denied tool must be refused, got {outcome:?}");
    };
    assert_eq!(
        reason_code, "tool_denied_by_workspace",
        "the refusal must name policy's reason rather than a generic one"
    );
}

/// **An unknown tool is an error rather than a default.**
///
/// The registry is the only source of tools, so a name it does not hold cannot be resolved — and a
/// fallback would make a mistyped name read a file.
#[tokio::test]
async fn an_unknown_tool_is_not_resolved() {
    let directory = TempRoot::new();
    directory.write("notes.txt", "buy milk");
    let roots = must(WorkspaceRoots::new([directory.path()]));
    let (_root, _database, pipeline) = pipeline_with(roots, WorkspacePolicy::default()).await;

    let error = must_err(
        pipeline
            .call_tool(
                "jarvis.files.delete",
                json!({"path": "notes.txt"}),
                &actor(),
                CorrelationId::new(),
            )
            .await,
    );
    assert!(
        matches!(error, ToolPipelineError::UnknownTool { .. }),
        "got {error:?}"
    );
}

/// **A traversal attempt is refused through every layer, and a real file still reads.**
///
/// The end-to-end form of the confinement guarantee: the same escape `P3-006` refuses at the boundary,
/// driven through policy, admission, the receipt, and the request. The answer is a `Failed` **outcome**
/// rather than an adapter error, because the filesystem *was* reached and said no.
#[tokio::test]
async fn a_traversal_attempt_fails_without_reading_anything() {
    let directory = TempRoot::new();
    directory.write("outside.txt", "outside the grant");
    directory.write("root/inside.txt", "inside the grant");
    let roots = must(WorkspaceRoots::new([directory.join("root")]));
    let (_root, _database, pipeline) = pipeline_with(roots, WorkspacePolicy::default()).await;

    let outcome = must(
        pipeline
            .call_tool(
                READ_TOOL,
                json!({"path": "../outside.txt"}),
                &actor(),
                CorrelationId::new(),
            )
            .await,
    );

    let ToolPipelineOutcome::Executed(result) = outcome else {
        panic!("the adapter answers a refusal as a result, got {outcome:?}");
    };
    assert_eq!(result.outcome(), ToolOutcome::Failed);
    assert!(
        result.output().is_none(),
        "a refused read must return no content at all"
    );

    // The control: a real file in the root still reads, so the failure is about the escape rather than
    // about the composed path being broken.
    let inside = must(
        pipeline
            .call_tool(
                READ_TOOL,
                json!({"path": "inside.txt"}),
                &actor(),
                CorrelationId::new(),
            )
            .await,
    );
    assert!(matches!(inside, ToolPipelineOutcome::Executed(_)));
}

/// **A second tool goes through the same path, so it is not a second code path.**
#[tokio::test]
async fn a_listing_reads_the_granted_directory() {
    let directory = TempRoot::new();
    directory.write("root/alpha.txt", "a");
    directory.write("root/bravo.txt", "b");
    let roots = must(WorkspaceRoots::new([directory.join("root")]));
    let (_root, _database, pipeline) = pipeline_with(roots, WorkspacePolicy::default()).await;

    let outcome = must(
        pipeline
            .call_tool(
                LIST_TOOL,
                json!({"path": "."}),
                &actor(),
                CorrelationId::new(),
            )
            .await,
    );
    let ToolPipelineOutcome::Executed(result) = outcome else {
        panic!("a listing must execute, got {outcome:?}");
    };
    let output = result
        .output()
        .unwrap_or_else(|| panic!("a listing must return output"));
    let parsed: serde_json::Value = must(serde_json::from_str(output.content()));
    let Some(entries) = parsed.get("entries").and_then(serde_json::Value::as_array) else {
        panic!("entries must be an array: {parsed}");
    };
    let names: Vec<&str> = entries
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect();
    assert_eq!(names, vec!["alpha.txt", "bravo.txt"]);
}

/// **The recorded outcome is the adapter's, and a different terminal outcome cannot replace it.**
///
/// This is the seam that makes `P3-005`'s honesty rules meaningful: the pipeline records what the
/// adapter established, and the ledger refuses a later writer that claims something else. Without this,
/// a re-drive could downgrade a `Confirmed` to a `Failed` and report an effect that happened as one that
/// did not.
#[tokio::test]
async fn the_recorded_outcome_is_the_adapters_and_is_not_replaceable() {
    let directory = TempRoot::new();
    directory.write("notes.txt", "buy milk");
    let roots = must(WorkspaceRoots::new([directory.path()]));
    let (_root, _database, pipeline) = pipeline_with(roots, WorkspacePolicy::default()).await;

    let correlation = CorrelationId::new();
    let outcome = must(
        pipeline
            .call_tool(
                READ_TOOL,
                json!({"path": "notes.txt"}),
                &actor(),
                correlation,
            )
            .await,
    );
    let ToolPipelineOutcome::Executed(result) = outcome else {
        panic!("expected an execution, got {outcome:?}");
    };
    assert_eq!(result.outcome(), ToolOutcome::Confirmed);

    // The call id is the correlation identity the pipeline was given, which is the only handle a caller
    // has on the durable row. Recording a different terminal outcome must be refused.
    let call_id = correlation.to_string();
    let replacement = must(ToolOutcomeRecord::failed("a later writer"));
    let refused = jarvis_storage::record_tool_outcome(
        pipeline.database(),
        &call_id,
        &replacement,
        None,
        UtcTimestamp::now(&SystemClock),
    )
    .await;
    assert!(
        matches!(refused, Err(DatabaseError::ToolCallAlreadyResolved { .. })),
        "a terminal outcome must not be replaceable, got {refused:?}"
    );
}

/// An adapter that declares the risk a hold needs, and panics if it is ever reached.
///
/// A **panicking** adapter rather than a returning one, because the property under test is that a held
/// call reaches no adapter at all. An adapter that answered would satisfy an outcome-shaped assertion
/// whether or not the hold worked, which is the same reasoning the MCP refusal test uses.
struct UnreachableAdapter;

#[async_trait::async_trait]
impl jarvis_tools::ToolExecutor for UnreachableAdapter {
    fn adapter_id(&self) -> &'static str {
        "unreachable"
    }

    async fn execute(
        &self,
        _request: &jarvis_tools::ToolExecutionRequest,
    ) -> Result<jarvis_tools::ToolCallResult, jarvis_tools::AdapterError> {
        panic!("an adapter must not be reached for a call held for approval");
    }
}

/// A definition that policy holds: a write-shaped tool at risk 2.
///
/// Built by hand rather than through the MCP translation, because the property under test is about the
/// **policy threshold** rather than about any one server's posture: `WorkspacePolicy::default()` requires
/// approval from risk 2 up, so a risk-2 tool in the default workspace is held whatever namespace it lives
/// in. Its effects declare a write, which is what makes the risk honest — `P3-001` refuses a risk below an
/// effect's floor at construction, so this definition could not claim to be risk-2 otherwise.
///
/// `source` is derived from the identifier rather than stated, because `ToolDefinition::new` refuses a
/// definition whose declared source disagrees with its namespace — the same rule that stops a server from
/// claiming a native prefix.
fn held_definition() -> jarvis_tools::ToolDefinition {
    let schema = json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "properties": { "path": { "type": "string" } },
        "required": ["path"],
        "additionalProperties": false
    });
    let effects = jarvis_tools::EffectSet::new([jarvis_tools::ToolEffect::Write])
        .unwrap_or_else(|| panic!("Write is a non-empty effect set"));
    let definition = jarvis_tools::ToolDefinition::new(jarvis_tools::ToolDefinitionParts {
        id: must(ToolId::new("mcp.test.write")),
        version: "schema-00000000".to_owned(),
        title: "Write a file".to_owned(),
        description: "A write-shaped tool that policy holds for approval.".to_owned(),
        input_schema: must(jarvis_tools::ToolSchema::from_value(schema)),
        // The dialect keyword is **required** rather than defaulted: `jarvis-tools` refuses to infer
        // 2020-12 from an omission, because `exclusiveMinimum` is a boolean in earlier drafts and a
        // number here, so defaulting would silently reinterpret an older document (`P3-008b`).
        output_schema: must(jarvis_tools::ToolSchema::from_value(json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object"
        }))),
        effects,
        // The number, because that is the manifest shape: `ToolDefinition::new` validates it against
        // the effects and turns it into a `Risk`.
        risk: 2,
        required_scopes: jarvis_tools::ScopeSet::none(),
        approval: jarvis_tools::ApprovalPolicy::Auto,
        idempotency: jarvis_tools::Idempotency::Unsupported,
        retry: jarvis_tools::RetryDeclaration::none(),
        source: jarvis_tools::ToolSource::from_namespace("mcp.test"),
        availability: jarvis_tools::Availability::Available,
        sensitivity: jarvis_tools::ToolSensitivity::default(),
        timeout_seconds: 30,
    });
    must(definition)
}

/// **The seam this slice adds: a held call writes the durable approval it is waiting on.**
///
/// Before this, `AwaitingApproval` returned a `call_id` and a strength and persisted **nothing** — the
/// long-recorded limit every tool slice restated. The call row stayed truthfully `requested` forever, and
/// the documented resume path (`security.md`'s "a trusted desktop/mobile/CLI approval may resume a
/// voice-originated run") had no record to resume against.
///
/// Three properties, and each is a way the write could be wrong rather than a way it could pass:
/// no adapter ran; the row's intent is the **call's** canonical intent, so a decision binds to the action
/// rather than to the request; and the requester is the **run**, which is what makes the domain's
/// self-approval refusal do work in a single-owner profile.
#[tokio::test]
async fn a_held_call_writes_the_approval_it_is_waiting_on() {
    let directory = TempRoot::new();
    let roots = must(WorkspaceRoots::new([directory.path()]));
    let definition = held_definition();
    let (_root, database, pipeline) = pipeline_with_extra(
        roots,
        vec![definition.clone()],
        Arc::new(UnreachableAdapter) as Arc<dyn jarvis_tools::ToolExecutor>,
    )
    .await;

    let arguments = json!({ "path": "notes/todo.txt" });
    let correlation = CorrelationId::new();
    let outcome = must(
        pipeline
            .call_tool(
                "mcp.test.write",
                arguments.clone(),
                &mcp_actor(),
                correlation,
            )
            .await,
    );

    let ToolPipelineOutcome::AwaitingApproval {
        call_id,
        approval_id,
        required_strength,
        reason_code,
    } = outcome
    else {
        panic!("a risk-2 write must be held, got {outcome:?}");
    };

    // The reason code is the engine's own, asserted against the literal the engine emits rather than
    // against what the decision "should" be called: `P3-003` owns the code, and a hand-written
    // expectation would drift from it silently.
    assert_eq!(reason_code, "approval_required");
    // Risk 2 asks for `Credential`, not `Present`: `P3-003`'s table maps `Moderate` to a verified
    // credential and reserves user presence for `High`. Asserting the value the engine computed also
    // pins the **risk** the definition produced, which is what this fixture exists to hold.
    assert_eq!(required_strength, AuthenticationStrength::Credential);

    // **The row exists, and a decision can name it.** Read back with the `approval_id` the outcome
    // carried, so the assertion is about the value a caller receives rather than about a lookup that
    // could find any row.
    let approval = must(jarvis_storage::find_approval(&database, &approval_id).await);
    assert_eq!(approval.id().to_string(), approval_id);
    assert_eq!(approval.tool(), "mcp.test.write");
    assert_eq!(approval.tool_version(), "schema-00000000");
    assert_eq!(
        approval.preview().as_str(),
        "mcp.test.write schema-00000000",
        "the preview names the tool and version, and carries no argument text"
    );

    // **The intent is the call's own canonical intent**, recomputed here from the same three inputs the
    // pipeline hashed. A decision therefore binds to the action, and the check is not vacuous: the
    // control below proves a different argument set hashes differently.
    let expected = must(jarvis_core::CanonicalIntentHash::compute(
        "mcp.test.write",
        "schema-00000000",
        &arguments,
    ));
    assert_eq!(approval.intent().to_hex(), expected.to_hex());
    let other = must(jarvis_core::CanonicalIntentHash::compute(
        "mcp.test.write",
        "schema-00000000",
        &json!({ "path": "elsewhere.txt" }),
    ));
    assert_ne!(
        expected.to_hex(),
        other.to_hex(),
        "the control: different arguments must not produce the same intent"
    );

    // **The requester is the run, so the human is eligible to approve and the agent is not.** This is the
    // identity decision the slice turns on, asserted on the stored row rather than on the value that was
    // passed in, so the pipeline cannot record one identity while claiming another.
    assert_eq!(
        approval.actor_id(),
        RUN,
        "the requester must be the run that asked for the action, not the person"
    );
    assert_ne!(
        approval.actor_id(),
        LOCAL_USER_ID,
        "the requester must not be the only identity eligible to approve it"
    );

    // The call row is `requested` and holds no receipt, which is the honest state of a call that has not
    // been authorized: a receipt is what an adapter treats as permission (`P3-006a`). The binding records
    // the call's **own** identifier for both fields — a call that is never authorized cannot cite a
    // receipt, and inventing one would be the fabricated authority `P3-006a` removed.
    let stored = must(pipeline.call(&call_id).await);
    assert_eq!(stored.outcome(), ToolOutcome::Requested);
    assert_eq!(stored.receipt(), call_id);

    // **The call now names the approval holding it**, which is what `P3-012c` adds. Without it the
    // decision route can move the approval to `approved` and nothing can tell which call was waiting:
    // the two rows would share no column, so there would be no join from a decision back to its subject.
    assert_eq!(
        stored.approval_id(),
        Some(approval.id().to_string().as_str()),
        "a held call must link to the approval that holds it"
    );
}

/// **A held call is recorded in the run's stream, in the order it happened.**
///
/// `P3-012c` links tool calls to the event log, and the property that matters is not "an event exists" but
/// **which** events and **in what order**. A client replaying the stream sees `tool_requested` and then
/// `approval_requested`, which is the sequence that tells it the run asked for something and is now blocked
/// on a human. Emitting only the approval event would make a held call appear with no request behind it;
/// emitting them out of order would show the run asking for a tool it had already been blocked on.
///
/// The sequence numbers are asserted as well, because they are allocated by the writer rather than supplied:
/// a test that only counted events would pass against a writer that appended the same kind twice.
///
/// **The assertion that the refusal path writes nothing** is the other half. A `Deny` produces no event at
/// all, deliberately — a stream entry per denied attempt would let a caller fill the log by asking for
/// tools it may not use, so a refusal must not be a write primitive. That is asserted separately below so a
/// future change that emits on refusal fails here rather than being discovered as log growth.
#[tokio::test]
async fn a_held_call_writes_a_request_then_a_hold_to_the_run_stream() {
    let directory = TempRoot::new();
    let roots = must(WorkspaceRoots::new([directory.path()]));
    let definition = held_definition();
    let (root, database, pipeline) = pipeline_with_extra(
        roots,
        vec![definition],
        Arc::new(UnreachableAdapter) as Arc<dyn jarvis_tools::ToolExecutor>,
    )
    .await;

    let outcome = must(
        pipeline
            .call_tool(
                "mcp.test.write",
                json!({ "path": "notes/todo.txt" }),
                &mcp_actor(),
                CorrelationId::new(),
            )
            .await,
    );
    let ToolPipelineOutcome::AwaitingApproval { approval_id, .. } = outcome else {
        panic!("expected a hold, got {outcome:?}");
    };
    let _ = root;

    let events = must(
        jarvis_storage::read_run_events(
            &database,
            RUN,
            must(jarvis_core::ReplayRequest::new(
                must(jarvis_core::RunEventSequence::new(1)),
                16,
            )),
        )
        .await,
    );
    // Only the tool-path kinds are compared, because `start_run` legitimately emits the run's own
    // `state_changed` before any tool is asked for — a stream for a live run is never empty, so an
    // assertion over all events would be asserting the fixture rather than this slice.
    let kinds: Vec<&str> = events
        .iter()
        .map(|event| event.kind().as_str())
        .filter(|kind| kind.starts_with("tool_") || kind.starts_with("approval_"))
        .collect();
    assert_eq!(
        kinds,
        vec!["tool_requested", "approval_requested"],
        "a hold must appear in the stream after the request that produced it"
    );

    // The identifiers are what makes the event useful rather than decorative: a client reading the stream
    // has to be able to name the approval without reading the approvals table.
    let hold = events
        .last()
        .unwrap_or_else(|| panic!("the hold event must be the last one"));
    assert_eq!(hold.kind().as_str(), "approval_requested");
    assert!(
        hold.payload().contains(&approval_id),
        "the hold event must carry its approval identifier: {}",
        hold.payload()
    );
    let request = &events[events.len() - 2];
    assert_eq!(
        request.sequence().get() + 1,
        hold.sequence().get(),
        "the writer allocates consecutive sequences"
    );
}

/// A refused call writes no event, so a refusal cannot be used to fill the run's log.
#[tokio::test]
async fn a_refused_call_writes_nothing_to_the_run_stream() {
    let directory = TempRoot::new();
    let roots = must(WorkspaceRoots::new([directory.path()]));
    let (root, database) = database_with_run().await;
    let secrets = jarvis_storage::SecretStore::in_state(&root.join("state"));

    // A workspace that denies the tool outright, so the refusal happens at step 2 and before admission.
    let denied = must(ToolId::new("mcp.test.write"));
    let policy = WorkspacePolicy::default().denying(denied);
    let pipeline = must(ToolPipeline::with_adapters(
        Arc::clone(&database),
        Some(roots),
        policy,
        vec![(
            vec![held_definition()],
            Arc::new(UnreachableAdapter) as Arc<dyn jarvis_tools::ToolExecutor>,
        )],
        secrets,
    ));

    let outcome = must(
        pipeline
            .call_tool(
                "mcp.test.write",
                json!({ "path": "notes/todo.txt" }),
                &mcp_actor(),
                CorrelationId::new(),
            )
            .await,
    );
    assert!(
        matches!(outcome, ToolPipelineOutcome::Refused { .. }),
        "the fixture needs a refusal, got {outcome:?}"
    );
    let _ = directory;

    let events = must(
        jarvis_storage::read_run_events(
            &database,
            RUN,
            must(jarvis_core::ReplayRequest::new(
                must(jarvis_core::RunEventSequence::new(1)),
                16,
            )),
        )
        .await,
    );
    let tool_kinds: Vec<&str> = events
        .iter()
        .map(|event| event.kind().as_str())
        .filter(|kind| kind.starts_with("tool_") || kind.starts_with("approval_"))
        .collect();
    assert!(
        tool_kinds.is_empty(),
        "a refusal must not be a write primitive, but the stream holds {tool_kinds:?}"
    );
}

/// **A decided approval resumes the held call it was decided about, exactly once.**
///
/// This is where the whole slice closes. `P3-012a` wrote the approval, `P3-012b` made it decidable,
/// `P3-016` linked the call to it, and `P3-017` made the decision carry its approver. None of them ran
/// anything, so an approved action sat at `requested` with an authority nothing acted on. This asserts the
/// effect happens — and, more importantly, that asking **twice** does not produce it twice.
///
/// # The assertions, and each is a way the resume could be wrong
///
/// - the adapter **ran**, and ran with the arguments the approval was decided against;
/// - the stored outcome is terminal, so the call is not left ambiguous;
/// - a **second** resume is refused, because a decision that is delivered twice must not become a second
///   effect. This is the property the whole `P3-012` slice exists for, and it is the one a naive "call
///   again if approved" implementation gets wrong;
/// - the resumed receipt **cites the approver**, which is what `P3-017` unblocked — without the approver
///   surviving decoding, this call could not be built at all.
#[tokio::test]
async fn an_approved_call_resumes_once_and_not_twice() {
    let directory = TempRoot::new();
    let roots = must(WorkspaceRoots::new([directory.path()]));
    let (root, database, pipeline) = pipeline_with_extra(
        roots,
        vec![held_definition()],
        Arc::new(RecordingAdapter::default()) as Arc<dyn jarvis_tools::ToolExecutor>,
    )
    .await;

    let arguments = json!({ "path": "notes/todo.txt" });
    let outcome = must(
        pipeline
            .call_tool(
                "mcp.test.write",
                arguments.clone(),
                &mcp_actor(),
                CorrelationId::new(),
            )
            .await,
    );
    let ToolPipelineOutcome::AwaitingApproval {
        call_id,
        approval_id,
        required_strength,
        ..
    } = outcome
    else {
        panic!("expected a hold, got {outcome:?}");
    };
    assert_eq!(
        required_strength,
        AuthenticationStrength::Credential,
        "a risk-2 write needs a credential, and the resume records what the hold demanded"
    );

    // The operator reads the delivered nonce and answers, exactly as the route does.
    let secrets = jarvis_storage::SecretStore::in_state(&root.join("state"));
    let nonce = must(secrets.take(&approval_id));
    let now = UtcTimestamp::now(&SystemClock);
    must(
        jarvis_storage::record_decision(
            &database,
            &approval_id,
            nonce.expose(),
            &must(jarvis_core::ApprovalDecision::new(
                jarvis_core::ApprovalDecisionOutcome::Approve,
                jarvis_core::ApprovalChannel::Cli,
                jarvis_core::AuthenticationStrength::Present,
                now,
                LOCAL_USER_ID,
            )),
        )
        .await,
    );

    // The resume runs the call.
    let resumed = must(
        pipeline
            .resume(
                &call_id,
                arguments.clone(),
                &mcp_actor(),
                CorrelationId::new(),
            )
            .await,
    );
    assert!(
        matches!(resumed, ToolPipelineOutcome::Executed(_)),
        "an approved call must run, got {resumed:?}"
    );

    let stored = must(pipeline.call(&call_id).await);
    assert!(
        stored.outcome().is_terminal(),
        "the call must reach a terminal outcome, got {:?}",
        stored.outcome()
    );
    assert!(
        !stored.must_not_repeat(),
        "a resumed call that completed must not be marked unrepeatable"
    );

    // **The duplicate-delivery refusal.** The same decision delivered again must not run the call a second
    // time: the effect is the thing being protected, and a resume that reached the adapter twice would be
    // exactly the second effect this slice exists to prevent.
    let again = pipeline
        .resume(&call_id, arguments, &mcp_actor(), CorrelationId::new())
        .await;
    assert!(
        matches!(again, Err(ToolPipelineError::ResumeRefused { .. })),
        "a second resume must be refused, got {again:?}"
    );
}

/// **A denied — or lapsed — approval cannot be resumed, however the caller asks.**
///
/// The refusal reads `authorizes_at`, which is false for a denial, a cancellation, an expiry, and an
/// undecided approval. So this is one gate covering four ways an action must not run, and the assertion is
/// that the adapter was **not** reached rather than only that an error came back: a `Refused` outcome with
/// the effect already made would satisfy an error-shaped check while doing exactly the wrong thing.
#[tokio::test]
async fn an_undecided_approval_cannot_be_resumed() {
    let directory = TempRoot::new();
    let roots = must(WorkspaceRoots::new([directory.path()]));
    let (root, database, pipeline) = pipeline_with_extra(
        roots,
        vec![held_definition()],
        Arc::new(RecordingAdapter::default()) as Arc<dyn jarvis_tools::ToolExecutor>,
    )
    .await;

    let arguments = json!({ "path": "notes/todo.txt" });
    let outcome = must(
        pipeline
            .call_tool(
                "mcp.test.write",
                arguments.clone(),
                &mcp_actor(),
                CorrelationId::new(),
            )
            .await,
    );
    let ToolPipelineOutcome::AwaitingApproval {
        call_id,
        approval_id,
        ..
    } = outcome
    else {
        panic!("expected a hold, got {outcome:?}");
    };
    let _ = (root, database);

    // Nothing has answered yet, so the approval is pending and does not authorize.
    let refused = pipeline
        .resume(&call_id, arguments, &mcp_actor(), CorrelationId::new())
        .await;
    assert!(
        matches!(refused, Err(ToolPipelineError::ResumeRefused { .. })),
        "a pending approval must not resume its call, got {refused:?}"
    );
    let _ = approval_id;

    // The call is untouched: still `requested`, not run, and still repeatable-by-a-decision.
    let stored = must(pipeline.call(&call_id).await);
    assert_eq!(
        stored.outcome(),
        ToolOutcome::Requested,
        "a refused resume must not advance the call"
    );
}

/// **A resume with different arguments is refused**, because the intent digest must match the one the
/// approval was decided against.
///
/// This is what makes the absent arguments column safe. `tool_calls` stores no payload (`0007`), so a
/// resume receives the arguments from its caller — and the digest comparison is what stops that from being a
/// hole: the caller may supply them, but only the ones the human actually approved.
#[tokio::test]
async fn a_resume_with_different_arguments_is_refused() {
    let directory = TempRoot::new();
    let roots = must(WorkspaceRoots::new([directory.path()]));
    let (root, database, pipeline) = pipeline_with_extra(
        roots,
        vec![held_definition()],
        Arc::new(RecordingAdapter::default()) as Arc<dyn jarvis_tools::ToolExecutor>,
    )
    .await;

    let arguments = json!({ "path": "notes/todo.txt" });
    let outcome = must(
        pipeline
            .call_tool(
                "mcp.test.write",
                arguments.clone(),
                &mcp_actor(),
                CorrelationId::new(),
            )
            .await,
    );
    let ToolPipelineOutcome::AwaitingApproval {
        call_id,
        approval_id,
        ..
    } = outcome
    else {
        panic!("expected a hold, got {outcome:?}");
    };

    let secrets = jarvis_storage::SecretStore::in_state(&root.join("state"));
    let nonce = must(secrets.take(&approval_id));
    must(
        jarvis_storage::record_decision(
            &database,
            &approval_id,
            nonce.expose(),
            &must(jarvis_core::ApprovalDecision::new(
                jarvis_core::ApprovalDecisionOutcome::Approve,
                jarvis_core::ApprovalChannel::Cli,
                jarvis_core::AuthenticationStrength::Present,
                UtcTimestamp::now(&SystemClock),
                LOCAL_USER_ID,
            )),
        )
        .await,
    );

    // A different path: the same tool, the same shape, and **not** the action that was approved.
    let substituted = json!({ "path": "notes/other.txt" });
    let refused = pipeline
        .resume(&call_id, substituted, &mcp_actor(), CorrelationId::new())
        .await;
    assert!(
        matches!(refused, Err(ToolPipelineError::ResumeRefused { .. })),
        "an approved action must not be substituted, got {refused:?}"
    );
    assert_eq!(
        must(pipeline.call(&call_id).await).outcome(),
        ToolOutcome::Requested,
        "a refused resume must leave the call exactly where it was"
    );
}

/// A call belonging to another run cannot be resumed, even with the right approval.
///
/// The identifier a caller names is not authority: the check is against the **stored** row's run, so knowing
/// a call identifier does not let a caller run somebody else's call under its own approval.
#[tokio::test]
async fn a_call_for_another_run_cannot_be_resumed() {
    let directory = TempRoot::new();
    let roots = must(WorkspaceRoots::new([directory.path()]));
    let (root, database, pipeline) = pipeline_with_extra(
        roots,
        vec![held_definition()],
        Arc::new(RecordingAdapter::default()) as Arc<dyn jarvis_tools::ToolExecutor>,
    )
    .await;

    let arguments = json!({ "path": "notes/todo.txt" });
    let outcome = must(
        pipeline
            .call_tool(
                "mcp.test.write",
                arguments.clone(),
                &mcp_actor(),
                CorrelationId::new(),
            )
            .await,
    );
    let ToolPipelineOutcome::AwaitingApproval {
        call_id,
        approval_id,
        ..
    } = outcome
    else {
        panic!("expected a hold, got {outcome:?}");
    };

    let secrets = jarvis_storage::SecretStore::in_state(&root.join("state"));
    let nonce = must(secrets.take(&approval_id));
    must(
        jarvis_storage::record_decision(
            &database,
            &approval_id,
            nonce.expose(),
            &must(jarvis_core::ApprovalDecision::new(
                jarvis_core::ApprovalDecisionOutcome::Approve,
                jarvis_core::ApprovalChannel::Cli,
                jarvis_core::AuthenticationStrength::Present,
                UtcTimestamp::now(&SystemClock),
                LOCAL_USER_ID,
            )),
        )
        .await,
    );

    // An actor whose run is a different identifier. The scopes are the same, so the run is the only
    // difference under test.
    let other = ToolActor::workspace_and_mcp(
        LOCAL_WORKSPACE_ID,
        "0198f000-0000-7000-8000-0000000000c9",
        SessionChannel::Cli,
        AuthenticationStrength::Credential,
        "policy-1",
    )
    .unwrap_or_else(|| panic!("the fixed scope literals must be valid"));

    let refused = pipeline
        .resume(&call_id, arguments, &other, CorrelationId::new())
        .await;
    assert!(
        matches!(refused, Err(ToolPipelineError::ResumeRefused { .. })),
        "a call must not be resumable by a run that does not own it, got {refused:?}"
    );
    assert_eq!(
        must(pipeline.call(&call_id).await).outcome(),
        ToolOutcome::Requested
    );
}

/// **A held call can actually be decided, which is what `P3-012a` could not deliver on its own.**
///
/// `P3-012a` wrote the durable approval but the plaintext nonce was generated, digested, and dropped, so
/// `record_decision` — which compares a presented value against the stored digest — had nothing to accept.
/// This is the end-to-end proof that the delivery exists: hold a call, take the nonce the way the
/// operator's client would, decide the approval, and assert the row moved to `approved`.
///
/// Four properties, each a way the delivery could be wrong rather than a way it could pass:
///
/// - the nonce is **not** in the durable row (it is a digest there, `ADR-0018`);
/// - the nonce **is** presentable once and the decision is accepted;
/// - the **approver must not equal the requester** — deciding as the run is refused, which is the guard
///   that makes the identity decision in `P3-012a` load-bearing rather than decorative;
/// - a decision on a **different intent** cannot be produced from this approval, because the intent is
///   what a decision binds to and `record_decision` re-reads it from the stored row.
#[tokio::test]
async fn a_pending_nonce_is_delivered_and_the_approval_can_be_decided() {
    let directory = TempRoot::new();
    let roots = must(WorkspaceRoots::new([directory.path()]));
    let definition = held_definition();
    let (root, _database, pipeline) = pipeline_with_extra(
        roots,
        vec![definition],
        Arc::new(UnreachableAdapter) as Arc<dyn jarvis_tools::ToolExecutor>,
    )
    .await;

    let outcome = must(
        pipeline
            .call_tool(
                "mcp.test.write",
                json!({ "path": "notes/todo.txt" }),
                &mcp_actor(),
                CorrelationId::new(),
            )
            .await,
    );
    let ToolPipelineOutcome::AwaitingApproval { approval_id, .. } = outcome else {
        panic!("expected a hold, got {outcome:?}");
    };

    // The store is the profile's state directory, which for a fixture is inside its own root.
    let secrets = jarvis_storage::SecretStore::in_state(&root.join("state"));

    // **The nonce is presentable exactly once**, which is the one-time property `ADR-0018` requires and
    // the only way a decision can be verified at all.
    let nonce = must(secrets.take(&approval_id));
    let again = secrets.take(&approval_id);
    assert!(
        matches!(again, Err(jarvis_storage::SecretStoreError::Absent { .. })),
        "a nonce must not be presentable twice, got {again:?}"
    );

    // **Deciding as the run is refused.** The requester *is* the run (`P3-012a`), so an approver equal to
    // it must be rejected — this is the self-approval guard doing real work rather than being
    // unreachable, which is the whole reason the requester was chosen to be the agent.
    let now = UtcTimestamp::now(&SystemClock);
    let as_the_agent = jarvis_storage::record_decision(
        pipeline.database(),
        &approval_id,
        nonce.expose(),
        &must(jarvis_core::ApprovalDecision::new(
            jarvis_core::ApprovalDecisionOutcome::Approve,
            jarvis_core::ApprovalChannel::Cli,
            jarvis_core::AuthenticationStrength::Present,
            now,
            RUN,
        )),
    )
    .await;
    assert!(
        matches!(
            as_the_agent,
            Err(DatabaseError::InvalidApprovalRequest {
                field: "decided_by"
            })
        ),
        "the agent that asked must not be able to approve, got {as_the_agent:?}"
    );

    // Deciding as the human is accepted. The nonce was **not** consumed by the refused attempt: the
    // refusal happens in the domain after the digest check, and `record_decision`'s guarded UPDATE only
    // rotates the digest when the write lands. Asserted rather than assumed, because a refused decision
    // that silently burned the nonce would make an approval undecidable after one honest mistake.
    let decided = must(
        jarvis_storage::record_decision(
            pipeline.database(),
            &approval_id,
            nonce.expose(),
            &must(jarvis_core::ApprovalDecision::new(
                jarvis_core::ApprovalDecisionOutcome::Approve,
                jarvis_core::ApprovalChannel::Cli,
                jarvis_core::AuthenticationStrength::Present,
                now,
                LOCAL_USER_ID,
            )),
        )
        .await,
    );

    assert_eq!(decided.tool(), "mcp.test.write");
    assert_eq!(
        decided.state_at(now),
        jarvis_core::ApprovalState::Approved,
        "the approval must be approved at the instant it was decided"
    );

    // **The durable row holds a digest, not the nonce.** This is the property `ADR-0018` exists for, and
    // asserting it here rather than only in the storage crate proves the *pipeline* did not accidentally
    // write the plaintext anywhere a reader could reach.
    let stored = must(jarvis_storage::find_approval(pipeline.database(), &approval_id).await);
    assert_eq!(
        stored.actor_id(),
        RUN,
        "the requester must still be the run after a decision"
    );

    // The intent the approval binds to is the call's own, so a decision authorizes the action and not a
    // description of it. Recomputed from the same three inputs, and the control proves it is not a
    // constant that would match anything.
    let expected = must(jarvis_core::CanonicalIntentHash::compute(
        "mcp.test.write",
        "schema-00000000",
        &json!({ "path": "notes/todo.txt" }),
    ));
    assert_eq!(stored.intent().to_hex(), expected.to_hex());
}

/// A pipeline whose only adapter is the real web fetch tool, and the actor the daemon derives for it.
async fn web_pipeline(
    workspace: WorkspacePolicy,
) -> (TempRoot, Arc<SqliteDatabase>, ToolPipeline, ToolActor) {
    let (directory, database) = database_with_run().await;
    let definition = must(jarvis_web::WebFetchTool::definition());
    let pipeline = must(ToolPipeline::with_adapters(
        Arc::clone(&database),
        None,
        workspace,
        vec![(
            vec![definition.clone()],
            Arc::new(jarvis_web::WebFetchTool::new()) as Arc<dyn jarvis_tools::ToolExecutor>,
        )],
        jarvis_storage::SecretStore::in_state(&directory.join("state")),
    ));
    // Derived from the composed definitions exactly as the daemon derives it, so a scope the tool declares is
    // granted without a list that could omit it.
    let actor = ToolActor::for_composed_tools(
        LOCAL_WORKSPACE_ID,
        RUN,
        SessionChannel::Cli,
        AuthenticationStrength::Credential,
        "policy-1",
        &[definition],
    );
    (directory, database, pipeline, actor)
}

/// **A model-chosen URL is held for a person under the default workspace policy.**
///
/// This is the decision `ADR-0129` rests on, asserted through the real pipeline and the real adapter rather
/// than on the definition alone: if the declared risk and the default threshold ever stopped agreeing, the
/// fetch would run unattended and nothing about the tool's own tests would notice.
#[tokio::test]
async fn a_web_fetch_is_held_for_approval_by_default() {
    let (_root, database, pipeline, actor) = web_pipeline(WorkspacePolicy::default()).await;
    let outcome = must(
        pipeline
            .call_tool(
                jarvis_web::FETCH_TOOL,
                json!({ "url": "https://example.com/" }),
                &actor,
                CorrelationId::new(),
            )
            .await,
    );
    let ToolPipelineOutcome::AwaitingApproval {
        reason_code,
        approval_id,
        ..
    } = outcome
    else {
        panic!("a web fetch must be held under the default policy, got {outcome:?}");
    };
    assert_eq!(reason_code, "approval_required");

    // The held approval carries the URL, so the person deciding can see what they are approving (`ADR-0130`).
    let held = must(jarvis_storage::read_approval_arguments(&database, &approval_id).await);
    let held: serde_json::Value = serde_json::from_str(&held.unwrap_or_default())
        .unwrap_or_else(|error| panic!("the held arguments must be JSON: {error}"));
    assert_eq!(held, json!({ "url": "https://example.com/" }));
}

/// **With the operator's opt-in the call reaches the adapter, and the adapter's guard still refuses.**
///
/// The workspace threshold is raised so the policy allows the call. The URL is a loopback literal on the
/// standard port, so the only thing that can stop it is the address rule — and what the pipeline records is the
/// adapter's own refusal, not a policy denial. That shows the scope was granted, the call was authorized, the
/// adapter ran, and the guard behind the policy is a second, independent layer.
#[tokio::test]
async fn an_unattended_web_fetch_is_still_refused_by_the_address_guard() {
    let workspace = must(WorkspacePolicy::new(
        jarvis_tools::Risk::High,
        jarvis_tools::Risk::High,
        true,
        true,
    ));
    let (_root, _database, pipeline, actor) = web_pipeline(workspace).await;
    let correlation = CorrelationId::new();
    let outcome = must(
        pipeline
            .call_tool(
                jarvis_web::FETCH_TOOL,
                json!({ "url": "http://127.0.0.1/admin" }),
                &actor,
                correlation,
            )
            .await,
    );
    let ToolPipelineOutcome::Executed(result) = outcome else {
        panic!("an allowed call must execute, got {outcome:?}");
    };
    assert_eq!(result.outcome(), ToolOutcome::Failed);
    let stored = must(pipeline.call(&correlation.to_string()).await);
    assert_eq!(stored.outcome(), ToolOutcome::Failed);
    assert_eq!(stored.record().reason(), Some("egress_refused"));
    let output = result
        .output()
        .unwrap_or_else(|| panic!("a refusal explains itself to the model"))
        .content()
        .to_owned();
    assert!(output.contains("not on the public internet"), "{output}");
}
/// **No workspace setting makes model-authored code run unattended.**
///
/// The code tool declares `Ask` at risk 3, and a workspace can only tighten an approval (`ADR-0122`). So even
/// with the ceiling and the approval threshold both raised to `High` — the most permissive document an operator
/// can write — the call is held, and the approval carries the code so the person sees what they would run.
#[tokio::test]
async fn code_is_held_for_a_person_under_the_most_permissive_policy() {
    let (directory, database) = database_with_run().await;
    let interpreter = vec!["sh".to_owned(), "-c".to_owned()];
    let definition = must(crate::code_run::CodeRunTool::definition(&interpreter));
    let tool = must(crate::code_run::CodeRunTool::new(
        jarvis_sandbox::ContainerBackend::probe(),
        "alpine:3".to_owned(),
        interpreter,
    ));
    let workspace = must(WorkspacePolicy::new(
        jarvis_tools::Risk::High,
        jarvis_tools::Risk::High,
        false,
        false,
    ));
    let pipeline = must(ToolPipeline::with_adapters(
        Arc::clone(&database),
        None,
        workspace,
        vec![(
            vec![definition.clone()],
            Arc::new(tool) as Arc<dyn jarvis_tools::ToolExecutor>,
        )],
        jarvis_storage::SecretStore::in_state(&directory.join("state")),
    ));
    let actor = ToolActor::for_composed_tools(
        LOCAL_WORKSPACE_ID,
        RUN,
        SessionChannel::Cli,
        AuthenticationStrength::Credential,
        "policy-1",
        &[definition],
    );
    let outcome = must(
        pipeline
            .call_tool(
                crate::code_run::RUN_TOOL,
                json!({ "code": "echo hi" }),
                &actor,
                CorrelationId::new(),
            )
            .await,
    );
    let ToolPipelineOutcome::AwaitingApproval { approval_id, .. } = outcome else {
        panic!("code must be held whatever the workspace allows, got {outcome:?}");
    };
    let held = must(jarvis_storage::read_approval_arguments(&database, &approval_id).await);
    assert_eq!(
        held.as_deref(),
        Some(r#"{"code":"echo hi"}"#),
        "the person deciding must be able to see the code"
    );
}
