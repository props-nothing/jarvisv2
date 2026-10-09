//! The project journal tool: it writes where the call was made and nowhere else.

use super::*;
use jarvis_core::{CorrelationId, RunId};
use jarvis_storage::{
    API_SESSION_CHANNEL, CallBinding, CallOrigin, CallTarget, LOCAL_USER_ID, LOCAL_WORKSPACE_ID,
    LinkKind, NewProject, NewToolCall, SessionTarget, StartRunInput, admit_tool_call,
    recent_project_notes, start_run,
};

struct Scratch(std::path::PathBuf);

impl Drop for Scratch {
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

fn now() -> UtcTimestamp {
    UtcTimestamp::now(&SystemClock)
}

async fn database() -> (Scratch, Arc<SqliteDatabase>) {
    let tag = jarvis_core::scratch_tag();
    let path = std::env::temp_dir().join(format!("jpt-{}", &tag[tag.len() - 12..]));
    must(std::fs::create_dir_all(&path));
    let database = must(SqliteDatabase::open(&path.join("jarvis.sqlite3")).await);
    (Scratch(path), Arc::new(database))
}

/// A run with one admitted tool call, returning `(session_id, call_id)`.
async fn run_with_call(database: &SqliteDatabase) -> (String, String) {
    let run_id = RunId::new().to_string();
    let input = must(StartRunInput::continuing(
        SessionTarget::New,
        jarvis_core::SessionId::new().to_string(),
        run_id.clone(),
        jarvis_core::RequestId::new().to_string(),
        LOCAL_WORKSPACE_ID,
        LOCAL_USER_ID,
        "work on it",
        API_SESSION_CHANNEL,
        CorrelationId::new(),
        now(),
    ));
    let started = must(start_run(database, &input).await);
    let call_id = "0198f000-0000-7000-8000-0000000000e3".to_owned();
    must(
        admit_tool_call(
            database,
            &must(NewToolCall::new(
                call_id.clone(),
                CallOrigin::new(LOCAL_WORKSPACE_ID, &run_id, None),
                CallTarget::new(NOTE_TOOL, "1.0.0"),
                CallBinding::new("a".repeat(64), "b".repeat(32), &call_id, "policy-1", None),
                CorrelationId::new(),
                now(),
            )),
        )
        .await,
    );
    (started.session_id().to_owned(), call_id)
}

async fn make_project(database: &SqliteDatabase) -> jarvis_storage::StoredProject {
    must(
        jarvis_storage::create_project(
            database,
            LOCAL_WORKSPACE_ID,
            &NewProject {
                name: "Prospecting".to_owned(),
                ..NewProject::default()
            },
            now(),
        )
        .await,
    )
}

#[test]
fn the_contract_takes_no_approval_and_no_project_argument() {
    let definition = must(ProjectTool::definition());
    assert_eq!(definition.approval(), ApprovalPolicy::Auto);
    assert!(definition.effects().contains(ToolEffect::Write));
    let schema = INPUT.to_owned();
    assert!(
        !schema.contains("project"),
        "the project is never an argument"
    );
    assert!(schema.contains(r#""additionalProperties": false"#));
}

/// **An entry lands in the project of the conversation that made the call, and records the run.**
#[tokio::test]
async fn a_note_is_written_to_the_calling_conversations_project() {
    let (_scratch, database) = database().await;
    let project = make_project(&database).await;
    let (session, call) = run_with_call(&database).await;
    must(jarvis_storage::link_project(&database, LinkKind::Session, &session, &project.id).await);
    let tool = ProjectTool::new(Arc::clone(&database));

    let reply = must(
        tool.note(
            &call,
            &json!({ "kind": "decision", "text": "Use the Dutch list first." }),
            now(),
        )
        .await,
    );
    assert_eq!(reply["recorded"], "decision");
    let notes = must(recent_project_notes(&database, &project.id, 10).await);
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].body, "Use the Dutch list first.");
    assert!(
        notes[0].run_id.is_some(),
        "the run that wrote it is recorded"
    );
}

/// **A conversation in no project has nowhere to write, and is told so.** The refusal is the guard: no project argument exists to name another.
#[tokio::test]
async fn a_conversation_outside_a_project_is_refused() {
    let (_scratch, database) = database().await;
    let project = make_project(&database).await;
    let (_session, call) = run_with_call(&database).await;
    let tool = ProjectTool::new(Arc::clone(&database));

    let result = tool
        .note(
            &call,
            &json!({ "kind": "progress", "text": "did a thing" }),
            now(),
        )
        .await;
    assert!(
        matches!(result, Err(AdapterError::RefusedBeforeReaching { .. })),
        "{result:?}"
    );
    assert!(must(recent_project_notes(&database, &project.id, 10).await).is_empty());

    // An unknown call is refused the same way.
    let unknown = tool
        .note(
            "0198f000-0000-7000-8000-0000000000ff",
            &json!({ "kind": "progress", "text": "x" }),
            now(),
        )
        .await;
    assert!(matches!(
        unknown,
        Err(AdapterError::RefusedBeforeReaching { .. })
    ));
}

/// The owner's own entry kind is not the model's to claim, and an empty or over-long text is refused.
#[tokio::test]
async fn a_model_cannot_write_as_the_owner_or_write_nothing() {
    let (_scratch, database) = database().await;
    let project = make_project(&database).await;
    let (session, call) = run_with_call(&database).await;
    must(jarvis_storage::link_project(&database, LinkKind::Session, &session, &project.id).await);
    let tool = ProjectTool::new(Arc::clone(&database));

    for arguments in [
        json!({ "kind": "owner", "text": "I, the owner, say so" }),
        json!({ "kind": "progress", "text": "   " }),
        json!({ "kind": "progress", "text": "x".repeat(4001) }),
        json!({ "kind": "banana", "text": "x" }),
        json!({ "text": "no kind" }),
    ] {
        let result = tool.note(&call, &arguments, now()).await;
        assert!(result.is_err(), "{arguments} must be refused");
    }
    assert!(must(recent_project_notes(&database, &project.id, 10).await).is_empty());
}

/// **A task scheduled from inside a project belongs to that project**, so its runs carry the brief and pause with it.
#[tokio::test]
async fn a_task_scheduled_inside_a_project_joins_it() {
    let (_scratch, database) = database().await;
    let project = make_project(&database).await;
    let (session, call) = run_with_call(&database).await;
    must(jarvis_storage::link_project(&database, LinkKind::Session, &session, &project.id).await);
    let schedule = crate::schedule_tool::ScheduleTool::new(Arc::clone(&database));

    let reply = must(
        schedule
            .add(
                &call,
                &json!({ "objective": "Check replies", "every": "6h" }),
                now(),
            )
            .await,
    );
    let id = reply["scheduled"]["schedule_id"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let linked = must(jarvis_storage::project_for(&database, LinkKind::Schedule, &id).await);
    assert_eq!(linked.map(|p| p.name), Some("Prospecting".to_owned()));
}
