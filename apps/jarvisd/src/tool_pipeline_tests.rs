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
        let _ = std::fs::remove_dir_all(&self.0);
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
/// The tuple carries the directory so the fixture cannot leak it, and the directory is **still leaked on a
/// platform where the database file is open** — see `P3-013`. An earlier version of this comment claimed the
/// field order fixed that; it does not, and the claim was withdrawn: `sqlx::Pool` has **no `Drop`** that closes
/// connections, so no ordering can release the file. Reversing the pair changed the directory count by zero,
/// which is how the wrong explanation was found.
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
    let pipeline = must(ToolPipeline::new(Arc::clone(&database), roots, workspace));
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
    let pipeline = must(ToolPipeline::with_adapters(
        Arc::clone(&database),
        Some(roots),
        WorkspacePolicy::default(),
        vec![(definitions, adapter)],
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
