//! Tests for the memory-proposal tool.
//!
//! # What these are really about
//!
//! The adapter's contract is narrow on purpose — a model may propose **content**, a **type**, the **entities**,
//! and an **importance**, and nothing that decides what the claim means — and every omission is enforced by the
//! schema's `additionalProperties: false` plus the source kind being set in code. So the tests that matter most
//! are the ones that try to supply what is missing and assert it is refused, because a field silently ignored
//! reads as accepted.

use std::sync::Arc;

use jarvis_core::{CorrelationId, SystemClock, UtcTimestamp};
use jarvis_storage::{LOCAL_USER_ID, SqliteDatabase};
use jarvis_tools::{
    AdapterError, ApprovalPolicy, AuthorizationReceipt, AuthorizationReceiptParts, IdempotencyKey,
    Risk, ToolCallResult, ToolEffect, ToolExecutionRequest, ToolExecutionRequestParts,
    ToolExecutor, ToolId, ToolOutcome,
};
use serde_json::{Value, json};

use crate::memory_propose::{MemoryProposeTool, PROPOSE_SCOPE, PROPOSE_TOOL};

/// A temporary directory holding the fixture database, removed when the test ends.
///
/// Declared here rather than sharing `tool_pipeline_tests`' own helper, because reaching into another module's
/// test scope would mean making that module and its value `pub(crate)` — widening a production crate's
/// visibility for a test fixture. The type is nine lines; the visibility change would be permanent.
struct TempRoot(std::path::PathBuf);

impl TempRoot {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "jarvis-memory-propose-{}",
            jarvis_core::scratch_tag()
        ));
        std::fs::create_dir_all(&path).unwrap_or_else(|error| panic!("create root: {error}"));
        // Canonicalized because `temp_dir()` on Windows can return an 8.3 short form whose text differs from
        // the long form a directory handle reports.
        let path = path
            .canonicalize()
            .unwrap_or_else(|error| panic!("canonicalize: {error}"));
        Self(path)
    }

    fn join(&self, relative: &str) -> std::path::PathBuf {
        self.0.join(relative)
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

fn at(offset_seconds: i128) -> UtcTimestamp {
    let now = UtcTimestamp::now(&SystemClock);
    must(UtcTimestamp::from_unix_nanos(
        now.unix_nanos() + offset_seconds * 1_000_000_000,
    ))
}

/// A database with the seeded local identity, and the directory holding it.
async fn database() -> (TempRoot, Arc<SqliteDatabase>) {
    let directory = TempRoot::new();
    let database = Arc::new(must(
        SqliteDatabase::open(&directory.join("jarvis.sqlite3")).await,
    ));
    (directory, database)
}
/// Records one subject entity directly, because no surface creates entities yet.
///
/// Written through the repository rather than by hand so the fixture cannot hold a row the store would refuse.
/// The tool requires every named subject to exist, so a fixture that skipped this would only ever exercise the
/// unresolved-entity refusal.
async fn a_subject(database: &SqliteDatabase) -> String {
    let identity = must(jarvis_storage::load_local_identity(database).await);
    let id = jarvis_core::EntityId::new();
    must(
        jarvis_storage::record_entity(
            database,
            &jarvis_storage::NewEntity {
                id,
                workspace_id: must(identity.workspace_id().parse()),
                kind: jarvis_storage::EntityKind::Person,
                label: "Proposal fixture subject".to_owned(),
                attributes: None,
                confidence: jarvis_core::MemoryConfidence::Confirmed,
                created_at: UtcTimestamp::now(&SystemClock),
            },
        )
        .await,
    );
    id.to_string()
}

/// An allowing decision for the proposal tool, produced by the **real** policy engine.
///
/// Through `evaluate` rather than fabricated, because `AuthorizationReceipt::new` verifies the decision against
/// the arguments — so a fixture cannot invent authority, and a test that ran without a decision would be
/// testing a path the pipeline cannot produce.
fn allowing() -> jarvis_tools::PolicyDecision {
    use jarvis_tools::{
        ActorAuthority, PolicyRequest, Scope, ScopeSet, TargetAssessment, WorkspacePolicy, evaluate,
    };

    let definition = must(MemoryProposeTool::definition());
    let decision = evaluate(&PolicyRequest {
        definition: &definition,
        actor: ActorAuthority::active(ScopeSet::new([must(Scope::new(PROPOSE_SCOPE))])),
        workspace: &WorkspacePolicy::default(),
        available: true,
        target: TargetAssessment::none(),
    });
    assert!(
        decision.is_allowed(),
        "the fixture's authority must be real, got {decision:?}"
    );
    decision
}

fn request(tool: &str, arguments: Value, deadline: UtcTimestamp) -> ToolExecutionRequest {
    let tool_id = must(ToolId::new(tool));
    let intent_hash = must(jarvis_core::CanonicalIntentHash::compute(
        tool, "1.0.0", &arguments,
    ));
    let receipt = must(AuthorizationReceipt::new(AuthorizationReceiptParts {
        receipt_id: "0198f000-0000-7000-8000-0000000000e1".to_owned(),
        tool: tool_id.clone(),
        tool_version: "1.0.0".to_owned(),
        arguments: arguments.clone(),
        intent_hash,
        policy_version: "policy-1".to_owned(),
        // The decision names the proposal tool even when `tool` is not it, because the unknown-tool test needs
        // authority that is real and a tool the adapter cannot run — which is exactly the registry mistake it
        // reproduces.
        decision: allowing(),
        approval: None,
        correlation_id: CorrelationId::new(),
        issued_at: at(0),
    }));
    must(ToolExecutionRequest::new(ToolExecutionRequestParts {
        call_id: "0198f000-0000-7000-8000-0000000000e3".to_owned(),
        tool: tool_id,
        tool_version: "1.0.0".to_owned(),
        arguments,
        receipt,
        idempotency_key: must(IdempotencyKey::generate()),
        deadline,
        correlation_id: CorrelationId::new(),
    }))
}

/// A proposal body naming one subject.
fn body(subject: &str) -> Value {
    json!({
        "content": "Prefers dark roast coffee",
        "memory_type": "preference",
        "entity_ids": [subject],
    })
}

/// Returns the parsed output of a successful call.
fn output(result: &ToolCallResult) -> Value {
    let Some(text) = result.output().map(jarvis_tools::BoundedOutput::content) else {
        panic!("a proposal result must carry an output");
    };
    serde_json::from_str(text).unwrap_or_else(|error| panic!("decode {text}: {error}"))
}

/// **A model's proposal is stored as a proposal, and the tool reports the outcome.**
///
/// The deliverable: before this adapter no tool touched memory, so "the model may propose candidates" was an
/// intention rather than a capability. Asserted against a **subsequent read** of the store rather than only
/// against the output, because the output is what an adapter that stored nothing would still produce.
#[tokio::test]
async fn a_proposals_is_stored_as_a_proposal() {
    let (_directory, database) = database().await;
    let subject = a_subject(&database).await;
    let tool = MemoryProposeTool::new(Arc::clone(&database));

    let result = must(
        tool.execute(&request(PROPOSE_TOOL, body(&subject), at(30)))
            .await,
    );
    assert_eq!(result.record().outcome(), ToolOutcome::Confirmed);
    let output = output(&result);
    assert_eq!(output["outcome"], "proposed");
    assert_eq!(output["status"], "proposed");
    let memory_id = output["memory_id"]
        .as_str()
        .unwrap_or_else(|| panic!("the output must name the stored claim"));
    assert!(output["status"] != "active");

    // Read back through the same store the model wrote to.
    let stored = must(jarvis_storage::find_memory(&database, memory_id).await);
    assert_eq!(
        stored.record().status(),
        jarvis_core::MemoryStatus::Proposed,
        "a model inference must never be stored as current truth"
    );
    assert_eq!(
        stored.record().source().kind(),
        jarvis_core::MemorySourceKind::ModelInference,
        "the source is the model's own inference, set by the adapter"
    );
    assert_eq!(
        stored.record().confidence(),
        jarvis_core::MemoryConfidence::Unverified,
        "and it may carry no confidence, which is the boundary's other half"
    );
    // The locator is derived from the call, so the claim is traceable to what produced it.
    assert!(
        stored
            .record()
            .source()
            .locator()
            .starts_with("tool:jarvis.memory.propose/"),
        "the provenance must name the tool that submitted it, got {}",
        stored.record().source().locator()
    );
    assert_eq!(
        stored.record().created_by_actor_id(),
        LOCAL_USER_ID,
        "the author is the identity the daemon acts for, not a literal"
    );
}

/// **A proposal that names no subject is about the user, and is still only a proposal.**
///
/// The model has no way to know an entity identifier, so requiring one made the tool fail on its most ordinary use
/// ("remember that I like short answers", ADR-0140). Omitting the field now means the profile owner. What must not
/// change is the inference boundary, so the stored claim is asserted to still be a model inference, `Proposed`.
#[tokio::test]
async fn a_proposal_with_no_subject_is_about_the_owner_and_still_a_proposal() {
    let (_directory, database) = database().await;
    let tool = MemoryProposeTool::new(Arc::clone(&database));

    let result = must(
        tool.execute(&request(
            PROPOSE_TOOL,
            json!({ "content": "Prefers short answers", "memory_type": "preference" }),
            at(30),
        ))
        .await,
    );
    let output = output(&result);
    assert_eq!(output["outcome"], "proposed");
    let memory_id = output["memory_id"]
        .as_str()
        .unwrap_or_else(|| panic!("the output must name the stored claim"));
    let stored = must(jarvis_storage::find_memory(&database, memory_id).await);
    assert_eq!(
        stored.record().status(),
        jarvis_core::MemoryStatus::Proposed
    );
    assert_eq!(
        stored.record().source().kind(),
        jarvis_core::MemorySourceKind::ModelInference
    );
    let identity = must(jarvis_storage::load_local_identity(&database).await);
    let owners = must(
        jarvis_storage::read_entities_by_label(
            &database,
            must(identity.workspace_id().parse()),
            crate::memory_service::OWNER_ENTITY_LABEL,
            5,
        )
        .await,
    );
    assert_eq!(owners.len(), 1, "the claim is about the one owner entity");
}

/// **A model cannot declare its own claim a user statement, or set a confidence.**
///
/// The load-bearing refusals. Both fields exist on `RememberRequest`, so the tempting implementation is to
/// accept them here — and each would defeat the inference boundary through a *field* rather than a bug: a claim
/// labelled `user_statement` at `confirmed` is a model's own output recorded as something the person said.
///
/// The schema is what refuses them, via `additionalProperties: false`, which is why this test goes through the
/// **pipeline's** validation rather than through the adapter's parser — the adapter's parser never sees these
/// fields, so testing it alone would prove nothing.
#[tokio::test]
async fn a_model_cannot_choose_its_own_source_or_confidence() {
    let (_directory, database) = database().await;
    let subject = a_subject(&database).await;
    let tool = MemoryProposeTool::new(Arc::clone(&database));
    let definition = must(MemoryProposeTool::definition());

    for forbidden in [
        json!({
            "content": "Prefers tea",
            "memory_type": "preference",
            "entity_ids": [subject],
            "source_kind": "user_statement",
        }),
        json!({
            "content": "Prefers tea",
            "memory_type": "preference",
            "entity_ids": [subject],
            "confidence": "confirmed",
        }),
        json!({
            "content": "Prefers tea",
            "memory_type": "preference",
            "entity_ids": [subject],
            "supersedes": "0198f000-0000-7000-8000-0000000000ff",
        }),
    ] {
        let report = must(definition.input_schema().validate(&forbidden));
        assert!(
            !report.is_valid(),
            "the schema must refuse {forbidden}, and a silently ignored field reads as accepted"
        );
    }

    // The control: the same body **without** the forbidden field is valid, so the refusal above is about the
    // extra field and not about the fixture being unusable in general.
    let report = must(definition.input_schema().validate(&body(&subject)));
    assert!(report.is_valid(), "the plain body must be accepted");

    // And the tool still runs, because the schema refuses the *call* before the adapter is reached: the proof
    // is that the adapter's own view of the arguments cannot produce the forbidden values.
    let result = must(
        tool.execute(&request(PROPOSE_TOOL, body(&subject), at(30)))
            .await,
    );
    let output = output(&result);
    assert_eq!(output["status"], "proposed");
}

/// **A claim that is not a proposal is refused by the domain, and the refusal is an answer.**
///
/// `MemoryCandidate::admit` refuses a candidate whose entity did not resolve, and the adapter must report that
/// as a **completed call** rather than as a failure: the platform judged the input, which is the tool working.
/// A model told `Failed` would conclude the platform was broken and retry; told `refused` with the reason, it
/// can fix what it sent.
#[tokio::test]
async fn an_unresolved_entity_is_reported_as_a_judgement_not_a_failure() {
    let (_directory, database) = database().await;
    let tool = MemoryProposeTool::new(Arc::clone(&database));
    let unknown = jarvis_core::EntityId::new().to_string();

    let result = must(
        tool.execute(&request(PROPOSE_TOOL, body(&unknown), at(30)))
            .await,
    );
    assert_eq!(
        result.record().outcome(),
        ToolOutcome::Failed,
        "a refused candidate did not establish a claim"
    );
    let output = output(&result);
    assert_eq!(output["outcome"], "refused");
    assert!(
        output["detail"]
            .as_str()
            .is_some_and(|text| !text.is_empty()),
        "the refusal must say which rule fired, got {output}"
    );
}

/// **An unknown tool is refused, so a registry mistake is visible rather than silent.**
///
/// The adapter is registered against one identifier, and a call to another must be `NotImplemented` — the
/// distinction `Dispatch::verify_covers` exists to catch at startup and this catches at the boundary.
#[tokio::test]
async fn a_tool_this_adapter_does_not_provide_is_refused() {
    let (_directory, database) = database().await;
    let subject = a_subject(&database).await;
    let tool = MemoryProposeTool::new(database);

    let refused = tool
        .execute(&request("jarvis.files.read", body(&subject), at(30)))
        .await;
    assert!(
        matches!(refused, Err(AdapterError::NotImplemented { .. })),
        "an unclaimed tool must be refused, got {refused:?}"
    );
}

/// **A malformed argument is refused before anything is stored.**
///
/// Each case is a value the schema should have caught, so reaching the adapter means a caller bypassed
/// validation or the two read the schema differently. Both are faults, and coercing either would file a claim
/// nobody described — a defaulted type or a claim attached to fewer subjects than the model named.
#[tokio::test]
async fn a_malformed_argument_is_refused_before_any_write() {
    let (_directory, database) = database().await;
    let subject = a_subject(&database).await;
    let tool = MemoryProposeTool::new(Arc::clone(&database));

    for (label, arguments) in [
        (
            "a blank content",
            json!({
                "content": "   ",
                "memory_type": "preference",
                "entity_ids": [subject],
            }),
        ),
        (
            "an unknown type",
            json!({
                "content": "Prefers tea",
                "memory_type": "opinion",
                "entity_ids": [subject],
            }),
        ),
        (
            "no subjects",
            json!({
                "content": "Prefers tea",
                "memory_type": "preference",
                "entity_ids": [],
            }),
        ),
        (
            "an out-of-range importance",
            json!({
                "content": "Prefers tea",
                "memory_type": "preference",
                "entity_ids": [subject],
                "importance": 900,
            }),
        ),
    ] {
        let refused = tool
            .execute(&request(PROPOSE_TOOL, arguments, at(30)))
            .await;
        assert!(
            matches!(refused, Err(AdapterError::RefusedBeforeReaching { .. })),
            "{label} must be refused before reaching the store, got {refused:?}"
        );
    }

    // Nothing was written by any of them, which is what "before any write" means.
    let listing = must(
        jarvis_storage::read_workspace_memories(
            &database,
            must(
                must(jarvis_storage::load_local_identity(&database).await)
                    .workspace_id()
                    .parse(),
            ),
            10,
        )
        .await,
    );
    assert!(
        listing.is_empty(),
        "a refused argument must not store a claim, got {} rows",
        listing.len()
    );
}

/// **The definition is consistent with itself, and the risk floor is the one the effect carries.**
///
/// A `write` effect floors risk at 1, and `ToolDefinition::new` refuses a declaration that claims less. Asserted
/// here rather than assumed, because the tempting value for "a claim nobody may act on" is risk 0 — which the
/// constructor would refuse, and a reader overriding it would be rounding a durable write down to a read.
#[test]
fn the_proposal_tool_declares_a_write_at_its_risk_floor() {
    let definition = must(MemoryProposeTool::definition());
    assert_eq!(definition.id().to_string(), PROPOSE_TOOL);
    assert!(
        definition.effects().contains(ToolEffect::Write),
        "a proposal is stored, so it is a write"
    );
    assert_eq!(
        definition.risk(),
        Risk::Low,
        "the floor a write carries is risk 1, which is `Low`"
    );
    assert!(
        definition
            .required_scopes()
            .contains(&must(jarvis_tools::Scope::new(PROPOSE_SCOPE))),
        "and it needs its own scope, so it can be withheld independently of a read"
    );
    // `Auto`: the claim is stored `proposed`, the retrieval path excludes a model inference at any status, and
    // the confidence is capped — so an unreviewed proposal changes nothing a user reads, and the review IS the
    // approval step. Requiring approval would ask a person to authorize a row whose only effect is to appear in
    // the list they review.
    assert_eq!(definition.approval(), ApprovalPolicy::Auto);
}

/// **The proposal tool reaches the registry the executor offers a model.**
///
/// `P4-014` says the model "may submit memory candidates", and that is true only if the tool is in the table
/// `execute_run_with_tools` reads its `with_tools` list from. An adapter that is correct and never composed is
/// the defect shape this repository keeps finding: right in isolation, unreachable because nothing calls it.
/// `Dispatch::verify_covers` catches the *opposite* mistake — a registered tool with no adapter — and nothing
/// catches an adapter with no registration except a test.
///
/// Composed through `with_adapters` rather than asserted on the definition alone, because the definition being
/// valid is a different claim from the registry holding it.
#[tokio::test]
async fn the_proposal_tool_is_registered_for_a_model_to_call() {
    let (_directory, database) = database().await;
    let pipeline = must(crate::tool_pipeline::ToolPipeline::with_adapters(
        Arc::clone(&database),
        None,
        jarvis_tools::WorkspacePolicy::default(),
        vec![(
            vec![must(MemoryProposeTool::definition())],
            Arc::new(MemoryProposeTool::new(Arc::clone(&database)))
                as Arc<dyn jarvis_tools::ToolExecutor>,
        )],
    ));

    let definitions = must(pipeline.definitions());
    assert!(
        definitions
            .iter()
            .any(|definition| definition.id().to_string() == PROPOSE_TOOL),
        "the tool must be in the registry a model is offered: {:?}",
        definitions
            .iter()
            .map(|definition| definition.id().to_string())
            .collect::<Vec<_>>()
    );
}
