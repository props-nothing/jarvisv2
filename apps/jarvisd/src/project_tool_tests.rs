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
fn the_note_takes_no_approval_and_no_project_argument_and_the_managing_tools_ask_where_they_persist()
 {
    let definitions = must(ProjectTool::definitions());
    let by_id = |id: &str| {
        definitions
            .iter()
            .find(|definition| definition.id().to_string() == id)
            .unwrap_or_else(|| panic!("{id} must be defined"))
    };
    assert_eq!(by_id(NOTE_TOOL).approval(), ApprovalPolicy::Auto);
    assert!(by_id(NOTE_TOOL).effects().contains(ToolEffect::Write));
    assert!(
        !NOTE_INPUT.contains("project"),
        "a note never names its project"
    );
    assert!(NOTE_INPUT.contains(r#""additionalProperties": false"#));
    // Standing instructions that every later run is given are planted only with the owner's yes.
    for tool in [CREATE_TOOL, UPDATE_TOOL, ASSIGN_TOOL] {
        assert_eq!(by_id(tool).approval(), ApprovalPolicy::Ask, "{tool}");
    }
    for tool in [LIST_TOOL, USE_TOOL] {
        assert_eq!(by_id(tool).approval(), ApprovalPolicy::Auto, "{tool}");
    }
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

/// **Asked to make a project, a model can: the project exists, the conversation belongs to it, and the journal works at once.**
#[tokio::test]
async fn a_model_can_create_a_project_and_then_journal_in_it() {
    let (_scratch, database) = database().await;
    let (session, call) = run_with_call(&database).await;
    let tool = ProjectTool::new(Arc::clone(&database));

    let reply = must(
        tool.create(
            &call,
            &json!({ "name": "Prospecting", "goal": "Book demos", "guidance": "Write in Dutch.", "folder": "sales" }),
            now(),
        )
        .await,
    );
    assert_eq!(reply["this_conversation_now_belongs_to_it"], true);
    let linked = must(jarvis_storage::project_for(&database, LinkKind::Session, &session).await);
    assert_eq!(linked.map(|p| p.name), Some("Prospecting".to_owned()));
    must(
        tool.note(
            &call,
            &json!({ "kind": "progress", "text": "Created." }),
            now(),
        )
        .await,
    );

    // A second project with the same name is refused and says what to do instead.
    let again = tool
        .create(&call, &json!({ "name": "prospecting" }), now())
        .await;
    assert!(
        matches!(again, Err(AdapterError::RefusedBeforeReaching { .. })),
        "{again:?}"
    );
}

/// **A conversation already in a project is not moved by creating or using another.**
#[tokio::test]
async fn a_conversation_in_a_project_is_not_moved_to_another() {
    let (_scratch, database) = database().await;
    let one = make_project(&database).await;
    let (session, call) = run_with_call(&database).await;
    must(jarvis_storage::link_project(&database, LinkKind::Session, &session, &one.id).await);
    let tool = ProjectTool::new(Arc::clone(&database));
    must(tool.create(&call, &json!({ "name": "Other" }), now()).await);
    let after = must(jarvis_storage::project_for(&database, LinkKind::Session, &session).await);
    assert_eq!(
        after.map(|p| p.id),
        Some(one.id.clone()),
        "creating another does not move the conversation"
    );
    let moved = tool
        .use_project(&call, &json!({ "project": "Other" }))
        .await;
    assert!(
        matches!(moved, Err(AdapterError::RefusedBeforeReaching { .. })),
        "{moved:?}"
    );
}

/// A conversation in no project can join an existing one, and is handed its brief.
#[tokio::test]
async fn a_model_can_join_an_existing_project_and_list_them() {
    let (_scratch, database) = database().await;
    let project = make_project(&database).await;
    let (session, call) = run_with_call(&database).await;
    let tool = ProjectTool::new(Arc::clone(&database));
    let listed = must(tool.list().await);
    assert_eq!(listed["projects"][0]["name"], "Prospecting");
    let used = must(
        tool.use_project(&call, &json!({ "project": "prospecting" }))
            .await,
    );
    assert_eq!(used["using"]["name"], "Prospecting");
    let linked = must(jarvis_storage::project_for(&database, LinkKind::Session, &session).await);
    assert_eq!(linked.map(|p| p.id), Some(project.id));
    assert!(
        tool.use_project(&call, &json!({ "project": "Nope" }))
            .await
            .is_err()
    );
}

/// Updating changes the named fields, defaults to the conversation's own project, and refuses a bad status or folder.
#[tokio::test]
async fn a_model_can_change_a_project_and_file_a_schedule_under_it() {
    let (_scratch, database) = database().await;
    let project = make_project(&database).await;
    let (session, call) = run_with_call(&database).await;
    must(jarvis_storage::link_project(&database, LinkKind::Session, &session, &project.id).await);
    let tool = ProjectTool::new(Arc::clone(&database));

    let changed = must(
        tool.update(
            &call,
            &json!({ "goal": "Ten demos", "status": "paused" }),
            now(),
        )
        .await,
    );
    assert_eq!(changed["updated"]["status"], "paused");
    assert!(
        tool.update(&call, &json!({ "status": "frozen" }), now())
            .await
            .is_err()
    );
    assert!(
        tool.update(&call, &json!({ "folder": "../x" }), now())
            .await
            .is_err()
    );
    let stored =
        must(jarvis_storage::find_project(&database, LOCAL_WORKSPACE_ID, "Prospecting").await);
    assert_eq!(stored.goal, "Ten demos");

    let schedule = must(
        jarvis_storage::create_schedule(
            &database,
            LOCAL_WORKSPACE_ID,
            "check replies",
            must(jarvis_core::Cadence::every_seconds(3600)),
            now(),
        )
        .await,
    );
    must(
        tool.assign_schedule(&call, &json!({ "schedule_id": schedule.id() }))
            .await,
    );
    let filed =
        must(jarvis_storage::project_for(&database, LinkKind::Schedule, schedule.id()).await);
    assert_eq!(filed.map(|p| p.name), Some("Prospecting".to_owned()));
    must(
        tool.assign_schedule(
            &call,
            &json!({ "schedule_id": schedule.id(), "remove": true }),
        )
        .await,
    );
    assert!(
        must(jarvis_storage::project_for(&database, LinkKind::Schedule, schedule.id()).await)
            .is_none()
    );
}
