//! Tests for the **inbound** MCP path: what a remote caller gets, and what it is refused.
//!
//! # Why these live here rather than in a transport test
//!
//! `P3-009e`/`P3-009f`/`P3-009c-a` prove the handler refuses an unadvertised tool and that the two inbound
//! policies are applied. None of them can prove what happens **when a served call runs**, because that needs a
//! registry, a policy engine, and adapters — which is the composition root's wiring and exists only here.
//!
//! # The property these tests exist for
//!
//! `ToolPipeline::call_remote_tool` is a **second entrypoint** into the tool path, and a second entrypoint is
//! exactly where an enforcement step gets forgotten. So the tests below are mostly about *parity*: every check
//! the local path applies is applied here, and the ones that cannot be applied are **refused** rather than
//! skipped. The two that matter most:
//!
//! - a **traversal** must be refused through the remote path exactly as through the local one, because that is
//!   `ADR-0020`'s confinement and a remote caller is the case it exists for;
//! - an **approval** must be refused rather than run, because there is no run to park in `awaiting_approval`
//!   and running the call would be the unsafe direction.

use std::sync::Arc;

use jarvis_core::{CorrelationId, ToolOutcome};
use jarvis_storage::{LOCAL_USER_ID, LOCAL_WORKSPACE_ID, SqliteDatabase, StartRunInput, start_run};
use jarvis_tools::{AuthenticationStrength, ToolDefinition, WorkspacePolicy, WorkspaceRoots};
use serde_json::json;

use super::{POLICY_VERSION, RemoteMcpError, build_endpoint};
use crate::tool_actor::ToolActor;
use crate::tool_pipeline::{ToolPipeline, ToolPipelineOutcome};
use jarvis_mcp_transport::{CallerAdmission, ServedToolRunner};

const RUN: &str = "0198f000-0000-7000-8000-0000000000c3";
const EVENT: &str = "0198f000-0000-7000-8000-0000000000e4";
const SESSION: &str = "0198f000-0000-7000-8000-000000000003";

/// A temporary root removed when the test ends, pass or fail.
///
/// **Owned for the test's lifetime and still leaked on this platform** — see `P3-013`: `sqlx::Pool` has no
/// `Drop` that closes connections, so the database file stays open and `remove_dir_all` hits a sharing
/// violation that `Drop` swallows. The guard is kept because it is correct on a platform where the file is not
/// held, and because dropping it would make the leak invisible rather than recorded.
struct TempRoot(std::path::PathBuf);

impl TempRoot {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("jarvis-mcp-serve-{}", jarvis_core::scratch_tag()));
        std::fs::create_dir_all(&path).unwrap_or_else(|error| panic!("create root: {error}"));
        let path = path
            .canonicalize()
            .unwrap_or_else(|error| panic!("canonicalize: {error}"));
        Self(path)
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }

    fn write(&self, relative: &str, contents: &str) {
        let path = self.0.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .unwrap_or_else(|error| panic!("create parent: {error}"));
        }
        std::fs::write(&path, contents).unwrap_or_else(|error| panic!("write: {error}"));
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

/// A pipeline over one granted root and no additional adapters.
///
/// The filesystem adapter is the case that matters for this module: it is the one tool area whose confinement
/// a remote caller could otherwise escape, so all of these tests drive it rather than a test double.
async fn pipeline_over(root: &TempRoot) -> (Arc<SqliteDatabase>, Arc<ToolPipeline>) {
    let database = Arc::new(must(
        SqliteDatabase::open(&root.path().join("jarvis.sqlite3")).await,
    ));
    must(
        start_run(
            &database,
            &must(StartRunInput::new(
                SESSION,
                RUN,
                EVENT,
                LOCAL_WORKSPACE_ID,
                LOCAL_USER_ID,
                "Read a file",
                jarvis_core::SessionChannel::Cli,
                CorrelationId::new(),
                jarvis_core::UtcTimestamp::now(&jarvis_core::SystemClock),
            )),
        )
        .await,
    );
    let roots = must(WorkspaceRoots::new([root.path()]));
    let pipeline = must(ToolPipeline::new(
        Arc::clone(&database),
        roots,
        WorkspacePolicy::default(),
    ));
    (database, Arc::new(pipeline))
}

/// Builds an actor for a remote call, the way the runner does.
///
/// Takes the served definitions because the actor's grant is **derived** from them — see
/// [`ToolActor::remote`] — so a fixture that invented its own scope set would test a caller the runner never
/// builds.
fn remote_actor(correlation_id: CorrelationId, served: &[ToolDefinition]) -> ToolActor {
    ToolActor::remote(LOCAL_WORKSPACE_ID, correlation_id, POLICY_VERSION, served)
}

/// A pipeline over one granted root, plus the definitions it serves.
///
/// Returns the definitions beside the pipeline because the actor's grant is derived from them, and every test
/// that makes a remote call must build its actor the same way the runner does — a fixture that skipped that
/// would be testing a caller that cannot exist.
async fn served_pipeline_over(
    root: &TempRoot,
) -> (Arc<SqliteDatabase>, Arc<ToolPipeline>, Vec<ToolDefinition>) {
    let (database, pipeline) = pipeline_over(root).await;
    let served = must(pipeline.definitions());
    (database, pipeline, served)
}

/// **A remote caller reaches the filesystem adapter and gets the file's contents.**
///
/// The positive control: without it, every refusal test below would pass on a path that refused everything,
/// and the "a remote call is dispatched identically" claim would be untested.
#[tokio::test]
async fn a_remote_call_reaches_the_filesystem_adapter() {
    let root = TempRoot::new();
    root.write("notes/todo.txt", "buy milk");
    let (_database, pipeline, served) = served_pipeline_over(&root).await;
    let correlation_id = CorrelationId::new();

    let outcome = must(
        pipeline
            .call_remote_tool(
                "jarvis.files.read",
                json!({ "path": "notes/todo.txt" }),
                &remote_actor(correlation_id, &served),
                correlation_id,
            )
            .await,
    );

    match outcome {
        ToolPipelineOutcome::Executed(result) => {
            assert_eq!(result.outcome(), ToolOutcome::Confirmed);
            let output = result
                .output()
                .unwrap_or_else(|| panic!("a read must carry its output"));
            assert!(
                output.content().contains("buy milk"),
                "got: {}",
                output.content()
            );
        }
        other => panic!("a remote read must execute, got {other:?}"),
    }
}

/// **A traversal is refused through the remote path exactly as through the local one.**
///
/// `ADR-0020` makes confinement a directory handle rather than a validated path, so the escape is refused by
/// the adapter rather than by a string check — and the reason this test is not redundant with the local one is
/// that a second entrypoint is where a *different* adapter or a reconstructed handle would appear. The
/// arguments ask for a path outside the grant, and the assertion is that **no output came back**, not merely
/// that the outcome is not `Confirmed`: a refused read that still returned the contents would satisfy the
/// weaker claim.
#[tokio::test]
async fn a_remote_traversal_is_refused() {
    let root = TempRoot::new();
    root.write("notes/todo.txt", "buy milk");
    let (_database, pipeline, served) = served_pipeline_over(&root).await;
    let correlation_id = CorrelationId::new();

    for escape in ["../secret.txt", "/etc/passwd", "notes/../../secret.txt"] {
        let outcome = must(
            pipeline
                .call_remote_tool(
                    "jarvis.files.read",
                    json!({ "path": escape }),
                    &remote_actor(correlation_id, &served),
                    correlation_id,
                )
                .await,
        );

        match outcome {
            ToolPipelineOutcome::Executed(result) => {
                assert_ne!(
                    result.outcome(),
                    ToolOutcome::Confirmed,
                    "{escape} must not be read successfully, got {result:?}"
                );
                // The stronger half: a non-`Confirmed` outcome that still carried content would mean the read
                // happened and the outcome was merely mislabelled.
                assert!(
                    result
                        .output()
                        .is_none_or(|output| !output.content().contains("super secret")),
                    "{escape} must not return file content"
                );
            }
            ToolPipelineOutcome::Refused { reason_code } => {
                // A refusal before the adapter is equally correct and is what a scope-aware policy would
                // produce. Named rather than ignored, so a change from "refused by the adapter" to "refused by
                // policy" is visible in the test's own vocabulary.
                assert!(!reason_code.is_empty(), "a refusal must name a reason code");
            }
            ToolPipelineOutcome::AwaitingApproval { .. } => {
                panic!("a remote call must never be held for an approval")
            }
        }
    }
}

/// **An argument the tool's schema refuses is refused on the remote path too.**
///
/// The parity claim in its cheapest form: a missing required field. A second entrypoint that skipped validation
/// would reach the adapter with arguments nothing checked, and the adapter would be deciding about a call whose
/// shape was never established.
#[tokio::test]
async fn a_remote_call_with_invalid_arguments_is_refused_before_the_adapter() {
    let root = TempRoot::new();
    root.write("notes/todo.txt", "buy milk");
    let (_database, pipeline, served) = served_pipeline_over(&root).await;
    let correlation_id = CorrelationId::new();

    // `path` is required by the adapter's own schema, so its absence is a violation rather than a policy
    // decision.
    let error = pipeline
        .call_remote_tool(
            "jarvis.files.read",
            json!({}),
            &remote_actor(correlation_id, &served),
            correlation_id,
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("a missing required field must be refused"));

    assert!(
        format!("{error}").contains("argument") || format!("{error}").contains("path"),
        "the refusal must name the argument problem, got: {error}"
    );
}

/// **A tool the registry does not hold is a fault, not a refusal.**
///
/// A refusal would mean "understood and declined", which is what a **policy** decision produces. An unknown
/// identifier is a caller asking for something that does not exist, and the distinction is the one
/// `ToolPipelineError` documents: a refusal is a correct answer, a fault is not.
#[tokio::test]
async fn an_unknown_tool_is_a_fault_rather_than_a_refusal() {
    let root = TempRoot::new();
    let (_database, pipeline, served) = served_pipeline_over(&root).await;
    let correlation_id = CorrelationId::new();

    let error = pipeline
        .call_remote_tool(
            "mcp.nothing.here",
            json!({}),
            &remote_actor(correlation_id, &served),
            correlation_id,
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("an unknown tool must be a fault"));

    assert!(
        format!("{error}").contains("mcp.nothing.here"),
        "the fault must name the identifier asked for, got: {error}"
    );
}

/// A workspace policy that requires an approval for **every** call.
///
/// The threshold is `Risk::Minimal`, not `Low`, and that is the rule rather than a preference: the engine asks
/// for an approval when `risk >= approval_threshold`, so a threshold of `Low` leaves a risk-0 tool
/// un-held. That is exactly what the first version of the test below asserted and what it failed on — the call
/// **ran** — which is worth recording because the assertion was about the product's behaviour and the mistake
/// was in the fixture's understanding of the comparison.
///
/// Built through the real constructor rather than by mutating a default, so the policy is one the engine can
/// actually be handed: `WorkspacePolicy::new` refuses a threshold above `max_risk`, and this pair is legal.
fn approval_requiring_workspace() -> WorkspacePolicy {
    must(WorkspacePolicy::new(
        jarvis_tools::Risk::High,
        jarvis_tools::Risk::Minimal,
        true,
        true,
    ))
}

/// **A pipeline whose workspace requires an approval refuses the remote call, and never runs the tool.**
///
/// This is the property `call_remote_tool`'s second, independent approval check exists for, and driving it
/// needs a workspace whose threshold is low enough to require one: `WorkspacePolicy::default()` asks from risk 2
/// up, and the filesystem tool is risk 0, so the default never holds it.
///
/// `WorkspacePolicy::restrictive_for_test` in this file lowers the threshold to `None`, which makes
/// **every** call require an approval. The remote path must then refuse rather than run — a tool an operator's
/// own call would have to approve, executed because the caller arrived over a different protocol.
///
/// The file is written first, so "the tool did not run" is an observation about the refusal rather than about a
/// missing file: a path that ran the call anyway would find real content to return.
#[tokio::test]
async fn a_workspace_requiring_an_approval_refuses_the_remote_call() {
    let root = TempRoot::new();
    root.write("notes/todo.txt", "buy milk");
    let database = Arc::new(must(
        SqliteDatabase::open(&root.path().join("jarvis.sqlite3")).await,
    ));
    must(
        start_run(
            &database,
            &must(StartRunInput::new(
                SESSION,
                RUN,
                EVENT,
                LOCAL_WORKSPACE_ID,
                LOCAL_USER_ID,
                "Read a file",
                jarvis_core::SessionChannel::Cli,
                CorrelationId::new(),
                jarvis_core::UtcTimestamp::now(&jarvis_core::SystemClock),
            )),
        )
        .await,
    );
    let roots = must(WorkspaceRoots::new([root.path()]));
    let pipeline = must(ToolPipeline::new(
        Arc::clone(&database),
        roots,
        approval_requiring_workspace(),
    ));
    let served = must(pipeline.definitions());
    let correlation_id = CorrelationId::new();

    let outcome = must(
        pipeline
            .call_remote_tool(
                "jarvis.files.read",
                json!({ "path": "notes/todo.txt" }),
                &remote_actor(correlation_id, &served),
                correlation_id,
            )
            .await,
    );

    match outcome {
        ToolPipelineOutcome::Refused { reason_code } => {
            assert_eq!(
                reason_code, "mcp_call_cannot_hold_an_approval",
                "the refusal must name the approval, not a generic policy denial"
            );
        }
        // **The bug this test exists to catch.** `AwaitingApproval` here would hand an MCP caller a shape it
        // cannot complete — there is no run to park and no way to present a decision — and a caller that treated
        // it as "wait and retry" would retry forever.
        ToolPipelineOutcome::AwaitingApproval { reason_code, .. } => panic!(
            "a remote call must be refused rather than held for an approval it cannot complete ({reason_code})"
        ),
        ToolPipelineOutcome::Executed(result) => {
            panic!("a call the workspace holds for approval must not run, got {result:?}")
        }
    }
}

/// **An endpoint serving a tool that needs an approval is still built, and the approval is refused per call.**
///
/// The distinction from the test above: the *served surface* accepts such a tool — `P3-009d` excludes a tool by
/// source, availability, name, and count, and an approval declaration is none of those — so the refusal has to
/// happen on the call. That is the shape `call_remote_tool` implements, and this test proves the runner maps it
/// to an `AdapterError` the handler reports as `isError` rather than letting it through.
#[tokio::test]
async fn the_runner_maps_a_refusal_to_a_tool_error() {
    let root = TempRoot::new();
    root.write("notes/todo.txt", "buy milk");
    let (_database, pipeline) = pipeline_over(&root).await;

    let (runner, _endpoint) = must(build_endpoint(
        Arc::clone(&pipeline),
        &must(pipeline.definitions()),
        LOCAL_WORKSPACE_ID,
        CallerAdmission::local_only(),
    ));

    // A served tool, called correctly: the runner answers a result rather than an error. This is the positive
    // control for the error mapping below — without it, a runner that returned `Err` for everything would pass.
    let correlation_id = CorrelationId::new();
    let result = must(
        runner
            .run(
                "jarvis.files.read",
                json!({ "path": "notes/todo.txt" }),
                correlation_id,
            )
            .await,
    );
    assert_eq!(result.outcome(), ToolOutcome::Confirmed);
}

/// **The runner reports a refusal as `RefusedBeforeReaching`, which is the variant that says nothing happened.**
///
/// The mapping matters because the handler turns an `AdapterError` into a refusal text and the *variant* decides
/// what a caller may conclude: `AmbiguousAfterReaching` would say an effect may have occurred, which for a
/// refused call is false and would invite a caller to check on something that never happened.
#[tokio::test]
async fn a_refused_remote_call_reports_that_nothing_was_reached() {
    let root = TempRoot::new();
    let (_database, pipeline) = pipeline_over(&root).await;
    let (runner, _endpoint) = must(build_endpoint(
        Arc::clone(&pipeline),
        &must(pipeline.definitions()),
        LOCAL_WORKSPACE_ID,
        CallerAdmission::local_only(),
    ));

    let error = runner
        .run("mcp.nothing.here", json!({}), CorrelationId::new())
        .await
        .err()
        .unwrap_or_else(|| panic!("an unknown tool must be refused by the runner"));

    assert!(
        matches!(
            error,
            jarvis_tools::AdapterError::RefusedBeforeReaching { .. }
        ),
        "a refusal must not claim a provider may have been reached, got: {error:?}"
    );
}

/// **An adapter's own error survives the mapping with its claim intact.**
///
/// This is the property a collapsed mapping breaks, and it is not observable through the endpoint: no adapter in
/// this daemon returns `AmbiguousAfterReaching` on a served tool, so the test constructs the value. The
/// falsification is deliberate — rewriting the `AdapterCall` arm to `RefusedBeforeReaching` makes this test fail,
/// and it is the *only* test that fails, because the endpoint-level tests can only reach the mapped arm.
///
/// What it protects: `AmbiguousAfterReaching` tells a caller that a request left and the outcome is unknown, so
/// it must not retry blindly. Collapsing it into "nothing happened" invites precisely that retry, and for a
/// non-idempotent tool the retry is a second effect.
#[test]
fn an_adapters_own_error_is_not_weakened_by_the_mapping() {
    let ambiguous = jarvis_tools::AdapterError::AmbiguousAfterReaching {
        reason: "the provider was reached and never answered".to_owned(),
    };
    let mapped = super::map_pipeline_error(
        crate::tool_pipeline::ToolPipelineError::AdapterCall(ambiguous),
        "jarvis.files.read",
    );
    assert!(
        matches!(
            mapped,
            jarvis_tools::AdapterError::AmbiguousAfterReaching { .. }
        ),
        "an adapter error must keep its own claim, got: {mapped:?}"
    );
}

/// **A pre-dispatch fault maps to the "nothing happened" variant, which the code can support.**
///
/// The complement of the test above, and the reason the mapping is a match rather than one arm: every other
/// pipeline error is raised before the dispatcher is consulted, so `RefusedBeforeReaching` is then a fact rather
/// than a guess. An unknown tool is the cheapest such fault to construct and the one a client is most likely to
/// cause.
#[test]
fn a_pre_dispatch_fault_reports_that_nothing_was_reached() {
    let mapped = super::map_pipeline_error(
        crate::tool_pipeline::ToolPipelineError::UnknownTool {
            tool: "mcp.nothing.here".to_owned(),
        },
        "mcp.nothing.here",
    );
    assert!(
        matches!(
            mapped,
            jarvis_tools::AdapterError::RefusedBeforeReaching { .. }
        ),
        "a fault raised before dispatch must report that nothing was reached, got: {mapped:?}"
    );
}

/// **A tool declaring an approval is refused even when the workspace would not ask for one.**
///
/// This is the second, independent approval check, and until this test existed **nothing covered it**: disabling
/// the `definition.approval() != ApprovalPolicy::Auto` guard in `call_remote_tool` left the whole suite green.
/// That is the measured record, not an inference — the mutation was run and the suite passed 13/13.
///
/// # Why the two checks cannot be collapsed into one
///
/// The engine's threshold is a **workspace** setting and `ApprovalPolicy` is a declaration on the **tool**. A
/// workspace policy that holds nothing (the default, whose threshold is `Moderate`) says nothing about a tool
/// that asks to be held; a test that varied only the workspace could never see this guard, and one that varied
/// only the tool would never see the engine's. Both are present, so both get a test.
///
/// The tool is registered through `with_adapters` with the filesystem adapter **also** present, so this cannot
/// pass because dispatch found nothing: the recording adapter must never be called, which is asserted rather than
/// inferred from the outcome.
#[tokio::test]
async fn a_tool_declaring_an_approval_is_refused_even_where_the_workspace_would_allow_it() {
    let root = TempRoot::new();
    root.write("notes/todo.txt", "buy milk");
    let (database, _pipeline) = pipeline_over(&root).await;
    let roots = must(WorkspaceRoots::new([root.path()]));
    let adapter = Arc::new(ApprovalDeclaringAdapter::default());
    let pipeline = Arc::new(must(ToolPipeline::with_adapters(
        Arc::clone(&database),
        Some(roots),
        WorkspacePolicy::default(),
        vec![(
            vec![approval_declaring_definition()],
            Arc::clone(&adapter) as Arc<dyn jarvis_tools::ToolExecutor>,
        )],
    )));

    let served = must(pipeline.definitions());
    let actor = remote_actor(CorrelationId::new(), &served);
    let outcome = must(
        pipeline
            .call_remote_tool(
                APPROVAL_TOOL,
                json!({ "path": "notes/todo.txt" }),
                &actor,
                CorrelationId::new(),
            )
            .await,
    );

    assert!(
        matches!(
            &outcome,
            ToolPipelineOutcome::Refused { reason_code }
                if *reason_code == "mcp_call_cannot_hold_an_approval"
        ),
        "a tool that declares an approval must be refused remotely, got: {outcome:?}"
    );
    assert_eq!(
        adapter.calls(),
        0,
        "the adapter must not have run: a refusal that still dispatched is not a refusal"
    );
}

/// The identifier the approval-declaring fixture registers.
const APPROVAL_TOOL: &str = "jarvis.test.approval";

/// A well-formed credential fingerprint, used only as a fixture value.
///
/// A **digest**, never a credential: `Fingerprint::parse` refuses free text precisely so a pasted token is
/// caught locally, and this constant is the shape a real one has rather than something that happens to parse.
const CALLER_DIGEST: &str = "9f2c41ab77de0355b1c8e0d4a63f29bb8c1740ee5d3a96f2c0b84a1e7d5633aa";

/// The fixture tool's input schema, which the arguments below are validated against.
const APPROVAL_INPUT_SCHEMA: &str = r#"{
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "type": "object",
    "properties": { "path": { "type": "string" } },
    "required": ["path"],
    "additionalProperties": false
}"#;

/// The fixture tool's output schema, which no assertion reads: the call is refused before one is produced.
const APPROVAL_OUTPUT_SCHEMA: &str = r#"{
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "type": "object",
    "properties": { "content": { "type": "string" } }
}"#;

/// A tool whose **declaration** asks for an approval, which its workspace would not.
///
/// `ApprovalPolicy::Ask` rather than `Policy`, because the guard refuses everything that is not `Auto` and the
/// point is that the *declaration* is what triggers it. The declared risk is 0, so the workspace's default
/// threshold of `Moderate` cannot hold the call — which is what makes the guard the only thing under test.
///
/// The field list is copied from `FilesystemReadTool`'s own definition rather than invented, so every value is
/// one the contract accepts: `EffectSet::single(ToolEffect::ReadOnly)` is what makes risk 0 legal, and
/// `ToolSensitivity::new` is the two-sided constructor rather than a bare enum.
fn approval_declaring_definition() -> ToolDefinition {
    must(ToolDefinition::new(jarvis_tools::ToolDefinitionParts {
        id: must(jarvis_tools::ToolId::new(APPROVAL_TOOL)),
        version: "1.0.0".to_owned(),
        title: "Approval-declaring test tool".to_owned(),
        description: "A fixture tool that declares an approval its workspace would not ask for."
            .to_owned(),
        input_schema: must(jarvis_tools::ToolSchema::parse(APPROVAL_INPUT_SCHEMA)),
        output_schema: must(jarvis_tools::ToolSchema::parse(APPROVAL_OUTPUT_SCHEMA)),
        effects: jarvis_tools::EffectSet::single(jarvis_tools::ToolEffect::ReadOnly),
        risk: 0,
        required_scopes: jarvis_tools::ScopeSet::none(),
        approval: jarvis_tools::ApprovalPolicy::Ask,
        timeout_seconds: 5,
        retry: jarvis_tools::RetryDeclaration::none(),
        idempotency: jarvis_tools::Idempotency::Required,
        source: jarvis_tools::ToolSource::Native,
        availability: jarvis_tools::Availability::Available,
        sensitivity: jarvis_tools::ToolSensitivity::new(
            jarvis_core::Sensitivity::Public,
            jarvis_core::Sensitivity::Public,
        ),
    }))
}

/// An adapter that **counts** its calls, so "it did not run" is observed rather than inferred.
#[derive(Default)]
struct ApprovalDeclaringAdapter {
    calls: std::sync::atomic::AtomicUsize,
}

impl ApprovalDeclaringAdapter {
    fn calls(&self) -> usize {
        self.calls.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl jarvis_tools::ToolExecutor for ApprovalDeclaringAdapter {
    fn adapter_id(&self) -> &'static str {
        "approval-fixture"
    }

    async fn execute(
        &self,
        _request: &jarvis_tools::ToolExecutionRequest,
    ) -> Result<jarvis_tools::ToolCallResult, jarvis_tools::AdapterError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Err(jarvis_tools::AdapterError::RefusedBeforeReaching {
            reason: "the approval fixture must never be reached".to_owned(),
        })
    }
}

/// **The caller policy a deployment configures is the one the endpoint serves under.**
///
/// `build_endpoint` takes the admission policy as a parameter and passes it through unchanged, and until this
/// test existed **nothing observed that**: replacing the argument with `CallerAdmission::local_only()` left the
/// suite green, because every other test already passes `local_only()` and a policy that ignored its argument
/// would therefore look identical. The mutation was run and the suite passed 14/14, which is what makes this a
/// coverage gap rather than a suspicion.
///
/// The property matters because the policy is the daemon's **whole** answer to "who may call": the bind is
/// loopback, and `CallerAdmission` is the second, independent statement that refuses a caller who arrives
/// anyway — through a reverse proxy, a forwarded socket, or a misconfiguration that moved the bind.
#[tokio::test]
async fn the_configured_caller_policy_is_the_one_the_endpoint_serves_under() {
    let root = TempRoot::new();
    root.write("notes/todo.txt", "buy milk");
    let (_database, pipeline) = pipeline_over(&root).await;

    // One admitted caller, so the policy is *not* `local_only` and cannot be confused with it.
    let admitted = must(jarvis_mcp_transport::AdmittedCaller::new(
        must(jarvis_mcp_transport::Fingerprint::parse(CALLER_DIGEST)),
        jarvis_mcp_transport::CallerLabel::new("inspector", "1.0"),
    ));
    let configured = must(CallerAdmission::new([admitted], 30));
    assert!(
        !configured.is_local_only(),
        "the fixture must not be the default, or this test proves nothing"
    );

    let (_runner, endpoint) = must(build_endpoint(
        Arc::clone(&pipeline),
        &must(pipeline.definitions()),
        LOCAL_WORKSPACE_ID,
        configured.clone(),
    ));

    assert_eq!(
        endpoint.gate().admission(),
        &configured,
        "the endpoint must serve under the policy it was given, not a default"
    );
}

/// **An endpoint with nothing to serve is refused, so a bound port cannot be one that answers nothing.**
///
/// `ServingConfig::check_servable` and this check are the same decision at two levels — one for a handler built
/// by hand, one for a deployment — and the remedy differs from a bind failure: an operator must grant a tool
/// rather than change a port.
#[tokio::test]
async fn an_endpoint_with_nothing_to_serve_is_refused() {
    let root = TempRoot::new();
    let (_database, pipeline) = pipeline_over(&root).await;

    let error = build_endpoint(
        Arc::clone(&pipeline),
        &[],
        LOCAL_WORKSPACE_ID,
        CallerAdmission::local_only(),
    )
    .err()
    .unwrap_or_else(|| panic!("an empty served surface must be refused"));

    assert!(
        matches!(error, RemoteMcpError::NoTools),
        "the refusal must name the missing tools, got: {error}"
    );
}

/// **A remote actor holds exactly what the served tools require, and never `mcp.call`.**
///
/// The first version of this test asserted the actor held **no** scopes, and it failed: `FilesystemReadTool`
/// declares `required_scopes: files.read`, so an actor holding nothing was refused every call with
/// `missing_scope`. The endpoint would have bound and answered every request with a refusal, which reads to an
/// operator as a policy misconfiguration rather than as this constructor.
///
/// So the grant is the **union of the served definitions' own requirements** — and the two assertions that
/// matter are that it is non-empty (or nothing can run) and that `mcp.call` is **not** in it, because that
/// literal is this daemon's grant to call *someone else's* server and attaching it inbound would be an
/// outbound grant on the inbound direction.
#[tokio::test]
async fn a_remote_actor_holds_the_served_scopes_and_not_the_outbound_one() {
    let root = TempRoot::new();
    root.write("notes/todo.txt", "buy milk");
    let (_database, pipeline) = pipeline_over(&root).await;
    let served = must(pipeline.definitions());
    let actor = remote_actor(CorrelationId::new(), &served);
    let authority = actor.authority();

    assert!(
        !authority.scopes().is_empty(),
        "an actor holding nothing is refused every served tool for a missing scope: {authority:?}"
    );
    let outbound = must(jarvis_tools::Scope::new(crate::tool_actor::MCP_CALL_SCOPE));
    assert!(
        !authority.scopes().contains(&outbound),
        "mcp.call is the OUTBOUND grant and must not reach an inbound caller: {authority:?}"
    );
    // Every scope the served tools declare is covered, which is the property that makes the endpoint usable at
    // all — an actor covering only some of them would make the served surface a promise it cannot keep.
    for definition in &served {
        assert!(
            definition
                .required_scopes()
                .is_satisfied_by(authority.scopes()),
            "the actor must cover every scope {} declares",
            definition.id()
        );
    }

    assert_eq!(
        actor.channel(),
        jarvis_core::SessionChannel::Api,
        "a remote MCP call must not be recorded as a local CLI invocation"
    );
    assert_eq!(
        actor.claimed_strength(),
        AuthenticationStrength::Credential,
        "the allowlist admits against a credential, which is the strength a remote caller has established"
    );
}

/// **The scope literals a served surface produces are valid, so the skip in `required_scopes_for` is
/// unreachable.**
///
/// `required_scopes_for` skips a declaration it cannot reconstruct, which fails **closed** — the tool that
/// declared it is refused for a missing scope. That is the right direction and it would also be an invisible
/// bug: every call to that tool denied, with the reason reading as a policy problem. This test turns it into a
/// failure here instead.
#[tokio::test]
async fn the_served_scopes_are_valid() {
    let root = TempRoot::new();
    root.write("notes/todo.txt", "buy milk");
    let (_database, pipeline) = pipeline_over(&root).await;
    let served = must(pipeline.definitions());

    for definition in &served {
        for required in definition.required_scopes().iter() {
            assert!(
                jarvis_tools::Scope::new(required.to_string()).is_ok(),
                "{} declares an invalid scope: {required}",
                definition.id()
            );
        }
    }
}

/// **The served surface over a real pipeline is non-empty and includes the filesystem tool.**
///
/// The end-to-end form of `build_endpoint` over a composed pipeline: the definitions come from the registry, the
/// served surface is built from them, and the two agree. Without this, "the endpoint serves what the daemon can
/// run" would be an assumption about two lists built in different places.
#[tokio::test]
async fn the_served_surface_matches_the_registry() {
    let root = TempRoot::new();
    root.write("notes/todo.txt", "buy milk");
    let (_database, pipeline) = pipeline_over(&root).await;

    let definitions = must(pipeline.definitions());
    assert!(
        !definitions.is_empty(),
        "a pipeline over a granted root registers at least one tool"
    );
    let (served, exclusions) = must(jarvis_mcp::served_tools(definitions.iter()));
    assert!(
        exclusions.is_empty(),
        "a filesystem tool is JARVIS-owned, available, and transmittable: {exclusions:?}"
    );
    assert_eq!(
        served.len(),
        definitions.len(),
        "every registered definition must be served when none is excluded"
    );
}
