//! The contact tools: they save, find and count, and a model cannot lift a do-not-contact.

use super::*;
use jarvis_core::{CorrelationId, RunId};
use jarvis_storage::{
    API_SESSION_CHANNEL, CallBinding, CallOrigin, CallTarget, LOCAL_USER_ID, LOCAL_WORKSPACE_ID,
    LinkKind, NewProject, NewToolCall, SessionTarget, StartRunInput, admit_tool_call, start_run,
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
    let path = std::env::temp_dir().join(format!("jct-{}", &tag[tag.len() - 12..]));
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
        "find leads",
        API_SESSION_CHANNEL,
        CorrelationId::new(),
        now(),
    ));
    let started = must(start_run(database, &input).await);
    let call_id = "0198f000-0000-7000-8000-0000000000e4".to_owned();
    must(
        admit_tool_call(
            database,
            &must(NewToolCall::new(
                call_id.clone(),
                CallOrigin::new(LOCAL_WORKSPACE_ID, &run_id, None),
                CallTarget::new(SAVE_TOOL, "1.0.0"),
                CallBinding::new("a".repeat(64), "b".repeat(32), &call_id, "policy-1", None),
                CorrelationId::new(),
                now(),
            )),
        )
        .await,
    );
    (started.session_id().to_owned(), call_id)
}

#[test]
fn the_contact_tools_need_no_approval_because_they_only_write_local_rows() {
    let definitions = must(ContactsTool::definitions());
    assert_eq!(definitions.len(), 3);
    for definition in &definitions {
        assert_eq!(definition.approval(), ApprovalPolicy::Auto);
    }
    let save = definitions
        .iter()
        .find(|definition| definition.id().to_string() == SAVE_TOOL)
        .unwrap_or_else(|| panic!("the save tool is defined"));
    assert!(save.effects().contains(ToolEffect::Write));
    assert!(SAVE_INPUT.contains(r#""additionalProperties": false"#));
}

#[tokio::test]
async fn a_lead_is_saved_updated_found_and_counted() {
    let (_scratch, database) = database().await;
    let tool = ContactsTool::new(Arc::clone(&database));
    let (_session, call) = run_with_call(&database).await;
    let made = must(
        tool.save(
            &call,
            &json!({ "company": "Acme BV", "person": "Eva", "email": "eva@acme.nl", "source": "kvk.nl" }),
            now(),
        )
        .await,
    );
    assert_eq!(made["saved"], "created");
    let again = must(
        tool.save(
            &call,
            &json!({ "email": "eva@acme.nl", "status": "contacted" }),
            now(),
        )
        .await,
    );
    assert_eq!(
        (again["saved"].as_str(), again["contact"]["status"].as_str()),
        (Some("updated"), Some("contacted"))
    );

    let found = must(
        tool.search(&json!({ "query": "acme", "status": "contacted" }))
            .await,
    );
    assert_eq!(found["count"], 1);
    let source = found["contacts"][0]["source"].as_str().unwrap_or_default();
    assert!(
        source.contains("kvk.nl") && source != "kvk.nl",
        "free text is fenced as data: {source}"
    );
    assert_eq!(
        must(tool.search(&json!({ "status": "won" })).await)["count"],
        0
    );

    let stats = must(tool.stats().await);
    assert_eq!(
        (&stats["total"], &stats["by_status"]["contacted"]),
        (&json!(1), &json!(1))
    );
    assert_eq!(stats["by_status"]["won"], 0);
}

#[tokio::test]
async fn a_model_cannot_lift_a_do_not_contact_and_bad_input_is_refused() {
    let (_scratch, database) = database().await;
    let tool = ContactsTool::new(Arc::clone(&database));
    let call = "no-such-call";
    must(
        tool.save(
            call,
            &json!({ "company": "Acme", "email": "eva@acme.nl", "status": "do_not_contact" }),
            now(),
        )
        .await,
    );
    let refused = tool
        .save(
            call,
            &json!({ "email": "eva@acme.nl", "status": "new" }),
            now(),
        )
        .await;
    assert!(
        matches!(&refused, Err(AdapterError::RefusedBeforeReaching { reason }) if reason.contains("only the owner")),
        "{refused:?}"
    );
    for bad in [
        json!({ "person": "nobody" }),
        json!({ "company": "Acme", "email": "not-an-address" }),
        json!({ "company": "Acme", "status": "maybe" }),
    ] {
        assert!(tool.save(call, &bad, now()).await.is_err(), "{bad}");
    }
}

#[tokio::test]
async fn a_contact_found_inside_a_project_is_filed_under_it() {
    let (_scratch, database) = database().await;
    let project = must(
        jarvis_storage::create_project(
            &database,
            LOCAL_WORKSPACE_ID,
            &NewProject {
                name: "Prospecting".to_owned(),
                ..NewProject::default()
            },
            now(),
        )
        .await,
    );
    let (session, call) = run_with_call(&database).await;
    must(jarvis_storage::link_project(&database, LinkKind::Session, &session, &project.id).await);
    let tool = ContactsTool::new(Arc::clone(&database));
    must(tool.save(&call, &json!({ "company": "Acme" }), now()).await);
    let filed = must(
        jarvis_storage::search_contacts(&database, LOCAL_WORKSPACE_ID, Some("acme"), None, 5).await,
    );
    let filed = filed.first().and_then(|contact| contact.project_id.clone());
    assert_eq!(filed.as_deref(), Some(project.id.as_str()));
}
