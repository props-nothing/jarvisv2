//! End-to-end tests for delegation: a parent run starts a sub-agent through the real pipeline, store and executor.

use std::sync::Arc;

use jarvis_core::RunOutcome;
use jarvis_models::{Role, Turn, scripted};
use jarvis_protocol::StartRunRequest;
use jarvis_storage::SqliteDatabase;

use super::*;
use crate::executor::{Executor, SCRIPTED_MODEL_NAME, execute_run_with_tools};

struct Profile(std::path::PathBuf);

impl Drop for Profile {
    fn drop(&mut self) {
        jarvis_core::remove_scratch_dir(&self.0);
    }
}

struct Fixture {
    _profile: Profile,
    database: Arc<SqliteDatabase>,
    runs: RunService,
    pipeline: Arc<ToolPipeline>,
    executor: Arc<Executor>,
}

async fn fixture() -> Fixture {
    let path = std::env::temp_dir().join(format!(
        "jarvis-delegate-{}-{}",
        std::process::id(),
        jarvis_core::RunId::new()
    ));
    std::fs::create_dir_all(&path).unwrap_or_else(|error| panic!("create profile: {error}"));
    let profile = Profile(path.clone());
    let database = Arc::new(
        SqliteDatabase::open(&path.join(jarvis_storage::DEFAULT_DATABASE_FILENAME))
            .await
            .unwrap_or_else(|error| panic!("open database: {error}")),
    );
    let secrets = SecretStore::in_state(&path.join("state"));
    let executor = Arc::new(
        Executor::build(SCRIPTED_MODEL_NAME, None)
            .unwrap_or_else(|error| panic!("scripted executor: {error}")),
    );
    let agent = Arc::new(AgentTool::new(
        Arc::clone(&database),
        secrets.clone(),
        Arc::clone(&executor),
    ));
    let pipeline = Arc::new(
        ToolPipeline::with_adapters(
            Arc::clone(&database),
            None,
            jarvis_tools::WorkspacePolicy::default(),
            vec![(
                AgentTool::definitions().unwrap_or_else(|error| panic!("definitions: {error}")),
                Arc::clone(&agent) as Arc<dyn ToolExecutor>,
            )],
            secrets.clone(),
        )
        .unwrap_or_else(|error| panic!("compose the pipeline: {error}")),
    );
    agent.bind(&pipeline);
    Fixture {
        _profile: profile,
        runs: RunService::new(Arc::clone(&database), secrets),
        database,
        pipeline,
        executor,
    }
}

async fn start(fixture: &Fixture, objective: &str) -> jarvis_protocol::RunReply {
    fixture
        .runs
        .start(&StartRunRequest {
            objective: objective.to_owned(),
            session_id: None,
            idempotency_key: None,
        })
        .await
        .unwrap_or_else(|error| panic!("start: {error:?}"))
}

fn parent_model(turns: Vec<Turn>) -> jarvis_models::ScriptedModel {
    scripted("scripted", "scripted-small", turns)
        .unwrap_or_else(|error| panic!("scripted model: {error}"))
}

async fn drive_parent(
    fixture: &Fixture,
    model: &jarvis_models::ScriptedModel,
    run_id: &str,
) -> jarvis_storage::StoredRun {
    execute_run_with_tools(
        &fixture.database,
        model,
        fixture.executor.model_id(),
        Some(&fixture.pipeline),
        run_id,
    )
    .await
    .unwrap_or_else(|error| panic!("drive: {error}"))
}

fn tool_results(model: &jarvis_models::ScriptedModel) -> Vec<String> {
    model
        .seen_messages()
        .last()
        .map(|messages| {
            messages
                .iter()
                .filter(|message| message.role() == Role::Tool)
                .map(|message| message.text().clone())
                .collect()
        })
        .unwrap_or_default()
}

/// **A parent run delegates, the sub-agent runs as an ordinary run, and its answer comes back fenced.**
#[tokio::test]
async fn a_delegated_task_runs_as_a_sub_agent_and_its_answer_is_fenced() {
    let fixture = fixture().await;
    let parent = start(&fixture, "split this work").await;
    let model = parent_model(vec![
        Turn::tool_call("c1", DELEGATE_TOOL, r#"{"task":"say hello"}"#),
        Turn::answer("The sub-agent finished."),
    ]);
    let settled = drive_parent(&fixture, &model, &parent.run_id).await;
    assert_eq!(settled.terminal_outcome(), Some(RunOutcome::Succeeded));

    let results = tool_results(&model);
    assert_eq!(results.len(), 1, "{results:?}");
    assert!(
        results[0].contains(r#""outcome":"completed""#),
        "{}",
        results[0]
    );
    // The sub-agent's answer (the scripted executor's fixed one) arrives inside the untrusted-data fence.
    assert!(
        results[0].contains(jarvis_core::FENCE_OPEN),
        "{}",
        results[0]
    );
    assert!(
        results[0].contains("No language model is configured"),
        "{}",
        results[0]
    );

    // The sub-agent is an ordinary, settled run in the same workspace, marked as one.
    let recent = jarvis_storage::read_recent_runs(&fixture.database, &parent.workspace_id, 10)
        .await
        .unwrap_or_else(|error| panic!("read runs: {error}"));
    let children: Vec<_> = recent
        .iter()
        .filter(|run| is_delegated_objective(run.objective()))
        .collect();
    assert_eq!(children.len(), 1);
    assert!(children[0].objective().ends_with("say hello"));
}

/// **Delegation is one level deep: a sub-agent is offered no delegation tool and is refused one it names anyway.**
#[tokio::test]
async fn a_sub_agent_cannot_delegate() {
    let fixture = fixture().await;
    let offered = |sub_agent: bool| -> Vec<String> {
        crate::executor::tool_specs_for_test(&fixture.pipeline, sub_agent)
    };
    assert!(
        offered(false).iter().any(|name| name == DELEGATE_TOOL),
        "a parent must be offered delegation"
    );
    assert!(
        !offered(true)
            .iter()
            .any(|name| name.starts_with(AGENT_TOOL_PREFIX)),
        "a sub-agent must be offered no delegation tool: {:?}",
        offered(true)
    );

    // And a call it makes anyway is refused, because its authority has no `agent.delegate` scope.
    let child = start(&fixture, &format!("{DELEGATED_NOTICE}try to delegate")).await;
    let model = parent_model(vec![
        Turn::tool_call("c1", DELEGATE_TOOL, r#"{"task":"again"}"#),
        Turn::answer("I could not."),
    ]);
    let settled = drive_parent(&fixture, &model, &child.run_id).await;
    assert_eq!(settled.terminal_outcome(), Some(RunOutcome::Succeeded));
    let results = tool_results(&model);
    assert!(
        results[0].contains("refused") || results[0].contains("could not be completed"),
        "a sub-agent's call to delegate must be refused: {results:?}"
    );
    let recent = jarvis_storage::read_recent_runs(&fixture.database, &child.workspace_id, 10)
        .await
        .unwrap_or_else(|error| panic!("read runs: {error}"));
    assert_eq!(
        recent
            .iter()
            .filter(|run| is_delegated_objective(run.objective()))
            .count(),
        1,
        "no second sub-agent may have been started"
    );
}

/// **`result` reads only sub-agents.** Asked for an ordinary run, it refuses and shows nothing of it.
#[tokio::test]
async fn result_refuses_a_run_that_is_not_a_sub_agent() {
    let fixture = fixture().await;
    let ordinary = start(&fixture, "an ordinary private run").await;
    let parent = start(&fixture, "collect it").await;
    let model = parent_model(vec![
        Turn::tool_call(
            "c1",
            RESULT_TOOL,
            &format!(r#"{{"run_id":"{}"}}"#, ordinary.run_id),
        ),
        Turn::answer("ok"),
    ]);
    drive_parent(&fixture, &model, &parent.run_id).await;
    let results = tool_results(&model);
    assert!(results[0].contains(r#""outcome":"refused""#), "{results:?}");
    assert!(!results[0].contains("private"), "{results:?}");
    let _ = (SystemClock, UtcTimestamp::now(&SystemClock));
}

/// **A sub-agent parked for approval is reported as waiting for the person, never approved on the parent's behalf.**
#[tokio::test]
async fn a_parked_sub_agent_is_reported_as_waiting_for_the_user() {
    let fixture = fixture().await;
    let child = start(&fixture, &format!("{DELEGATED_NOTICE}do something held")).await;
    let mut run = jarvis_storage::find_run(&fixture.database, &child.run_id)
        .await
        .unwrap_or_else(|error| panic!("read the child: {error}"));
    for next in [
        jarvis_core::RunState::ContextBuilding,
        jarvis_core::RunState::Planning,
        jarvis_core::RunState::AwaitingApproval,
    ] {
        run = jarvis_storage::transition_run(
            &fixture.database,
            run.id(),
            run.expectation(),
            &jarvis_core::RunTransition::new(next, next.required_outcome(), None)
                .unwrap_or_else(|error| panic!("transition: {error}")),
            jarvis_core::UtcTimestamp::now(&jarvis_core::SystemClock),
        )
        .await
        .unwrap_or_else(|error| panic!("advance to {next:?}: {error}"));
    }

    let parent = start(&fixture, "collect it").await;
    let model = parent_model(vec![
        Turn::tool_call(
            "c1",
            RESULT_TOOL,
            &format!(r#"{{"run_id":"{}","wait_seconds":5}}"#, child.run_id),
        ),
        Turn::answer("It is waiting."),
    ]);
    drive_parent(&fixture, &model, &parent.run_id).await;
    let results = tool_results(&model);
    assert!(
        results[0].contains(r#""outcome":"waiting_for_approval""#),
        "{results:?}"
    );
    assert_eq!(
        jarvis_storage::find_run(&fixture.database, &child.run_id)
            .await
            .unwrap_or_else(|error| panic!("read the child: {error}"))
            .state(),
        jarvis_core::RunState::AwaitingApproval,
        "reading a sub-agent must not move it"
    );
}
