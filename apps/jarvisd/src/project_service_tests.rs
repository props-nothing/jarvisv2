//! Projects through the real routes, the run start and the scheduler.

use std::sync::Arc;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use jarvis_core::{ClientCredential, SystemClock, UtcTimestamp};
use jarvis_protocol::{ProjectDetailReply, ProjectListReply, ProjectReply, RunReply};
use jarvis_storage::{LOCAL_WORKSPACE_ID, LinkKind, SqliteDatabase};
use tower::ServiceExt;

use crate::gateway::{GatewayState, router};

struct Fixture {
    app: Router,
    state: GatewayState,
    database: Arc<SqliteDatabase>,
    credential: String,
    directory: std::path::PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        jarvis_core::remove_scratch_dir(&self.directory);
    }
}

fn must<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("expected success, got {error:?}"),
    }
}

/// A gateway with no executor, so a started run stays `received` and nothing is sent to a model.
async fn fixture() -> Fixture {
    let directory =
        std::env::temp_dir().join(format!("jarvis-project-svc-{}", jarvis_core::scratch_tag()));
    must(std::fs::create_dir_all(&directory));
    let database = Arc::new(must(
        SqliteDatabase::open(&directory.join("jarvis.sqlite3")).await,
    ));
    let credential = must(ClientCredential::generate());
    let presented = credential.expose().to_owned();
    let state = GatewayState::new(Arc::clone(&database), credential);
    Fixture {
        app: router(state.clone()),
        state,
        database,
        credential: presented,
        directory,
    }
}

async fn call(
    fixture: &Fixture,
    method: &str,
    path: &str,
    authenticated: bool,
    body: Option<&str>,
) -> (StatusCode, String) {
    let mut builder = Request::builder().method(method).uri(path);
    if authenticated {
        builder = builder.header(
            header::AUTHORIZATION,
            format!("Bearer {}", fixture.credential),
        );
    }
    if body.is_some() {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
    }
    let request = must(builder.body(Body::from(body.unwrap_or_default().to_owned())));
    let response = must(fixture.app.clone().oneshot(request).await);
    let status = response.status();
    let bytes = must(axum::body::to_bytes(response.into_body(), 1 << 20).await);
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn send(
    fixture: &Fixture,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> (StatusCode, String) {
    call(fixture, method, path, true, body).await
}

fn parse<T: serde::de::DeserializeOwned>(text: &str) -> T {
    must(serde_json::from_str(text))
}

async fn make(fixture: &Fixture, name: &str) -> ProjectReply {
    let body =
        format!(r#"{{"name":"{name}","goal":"Win","guidance":"Be brief","folder":"sales"}}"#);
    let (status, text) = send(fixture, "POST", "/api/v1/projects", Some(&body)).await;
    assert_eq!(status, StatusCode::CREATED, "{text}");
    parse(&text)
}

async fn start(fixture: &Fixture, body: &str) -> (StatusCode, String) {
    send(fixture, "POST", "/api/v1/runs", Some(body)).await
}

#[tokio::test]
async fn the_project_routes_refuse_a_caller_without_the_credential() {
    let fixture = fixture().await;
    for (method, path) in [
        ("GET", "/api/v1/projects"),
        ("POST", "/api/v1/projects"),
        ("DELETE", "/api/v1/projects/x"),
    ] {
        let (status, _) = call(&fixture, method, path, false, Some("{}")).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {path}");
    }
}

#[tokio::test]
async fn a_project_is_created_read_changed_noted_and_deleted() {
    let fixture = fixture().await;
    let project = make(&fixture, "Prospecting").await;
    assert_eq!(project.status, "active");

    let (_, text) = send(&fixture, "GET", "/api/v1/projects", None).await;
    assert_eq!(parse::<ProjectListReply>(&text).total, 1);

    let (status, text) = send(
        &fixture,
        "PATCH",
        "/api/v1/projects/prospecting",
        Some(r#"{"status":"paused","goal":"Win more"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let changed: ProjectReply = parse(&text);
    assert_eq!(
        (changed.status.as_str(), changed.goal.as_str()),
        ("paused", "Win more")
    );

    let (status, _) = send(
        &fixture,
        "POST",
        "/api/v1/projects/Prospecting/notes",
        Some(r#"{"text":"Start with the Dutch list"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (_, text) = send(
        &fixture,
        "GET",
        &format!("/api/v1/projects/{}", project.project_id),
        None,
    )
    .await;
    let detail: ProjectDetailReply = parse(&text);
    assert_eq!(detail.notes.len(), 1);
    assert_eq!(detail.notes[0].kind, "owner");

    let (status, _) = send(&fixture, "DELETE", "/api/v1/projects/Prospecting", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = send(&fixture, "GET", "/api/v1/projects/Prospecting", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn bad_input_is_refused_with_the_right_status() {
    let fixture = fixture().await;
    make(&fixture, "P").await;
    let (status, _) = send(
        &fixture,
        "POST",
        "/api/v1/projects",
        Some(r#"{"name":"p"}"#),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "a name is unique ignoring case"
    );
    let (status, _) = send(
        &fixture,
        "POST",
        "/api/v1/projects",
        Some(r#"{"name":"Q","folder":"../x"}"#),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "a folder cannot leave the root"
    );
    let (status, _) = send(
        &fixture,
        "PATCH",
        "/api/v1/projects/P",
        Some(r#"{"status":"frozen"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = send(
        &fixture,
        "POST",
        "/api/v1/projects/P/notes",
        Some(r#"{"text":"x","kind":"order"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    // A project cannot be used to grant authority: unknown fields are refused, not ignored.
    let (status, _) = send(
        &fixture,
        "POST",
        "/api/v1/projects",
        Some(r#"{"name":"R","tools":["jarvis.command.run"]}"#),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn a_run_started_for_a_project_binds_its_conversation_to_it() {
    let fixture = fixture().await;
    let project = make(&fixture, "Prospecting").await;
    let (status, text) = start(
        &fixture,
        r#"{"objective":"look","project_id":"prospecting"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{text}");
    let run: RunReply = parse(&text);
    let linked = must(
        jarvis_storage::project_for(&fixture.database, LinkKind::Session, &run.session_id).await,
    );
    assert_eq!(linked.map(|p| p.id), Some(project.project_id));

    // A follow-up in the same conversation needs no project in the request.
    let follow = format!(
        r#"{{"objective":"more","session_id":"{}"}}"#,
        run.session_id
    );
    assert_eq!(start(&fixture, &follow).await.0, StatusCode::CREATED);
}

#[tokio::test]
async fn a_refused_project_start_leaves_no_run_behind() {
    let fixture = fixture().await;
    let one = make(&fixture, "One").await;
    let two = make(&fixture, "Two").await;
    let before =
        must(jarvis_storage::read_recent_runs(&fixture.database, LOCAL_WORKSPACE_ID, 50).await)
            .len();

    let (status, _) = start(&fixture, r#"{"objective":"x","project_id":"Nope"}"#).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (_, text) = start(
        &fixture,
        &format!(r#"{{"objective":"x","project_id":"{}"}}"#, one.project_id),
    )
    .await;
    let run: RunReply = parse(&text);
    let body = format!(
        r#"{{"objective":"y","session_id":"{}","project_id":"{}"}}"#,
        run.session_id, two.project_id
    );
    assert_eq!(
        start(&fixture, &body).await.0,
        StatusCode::CONFLICT,
        "a conversation is not moved to another project"
    );

    let after =
        must(jarvis_storage::read_recent_runs(&fixture.database, LOCAL_WORKSPACE_ID, 50).await)
            .len();
    assert_eq!(after, before + 1, "only the one valid start created a run");
}

fn now() -> UtcTimestamp {
    UtcTimestamp::now(&SystemClock)
}

#[tokio::test]
async fn a_schedule_of_a_project_fires_into_it_and_waits_while_the_project_is_not_active() {
    let fixture = fixture().await;
    let project = make(&fixture, "Prospecting").await;
    let body = format!(
        r#"{{"objective":"check","every":"1h","project_id":"{}"}}"#,
        project.project_id
    );
    let (status, text) = send(&fixture, "POST", "/api/v1/schedules", Some(&body)).await;
    assert_eq!(status, StatusCode::CREATED, "{text}");
    let schedule: jarvis_protocol::ScheduleReply = parse(&text);
    assert_eq!(schedule.project.as_deref(), Some("Prospecting"));

    send(
        &fixture,
        "PATCH",
        "/api/v1/projects/Prospecting",
        Some(r#"{"status":"paused"}"#),
    )
    .await;
    let later = must(UtcTimestamp::from_unix_nanos(
        now().unix_nanos() + 3_700_000_000_000,
    ));
    let report = must(crate::schedule_service::tick(&fixture.state, later).await);
    assert_eq!(
        (report.started, report.skipped),
        (0, 1),
        "a paused project's task does not fire"
    );

    send(
        &fixture,
        "PATCH",
        "/api/v1/projects/Prospecting",
        Some(r#"{"status":"active"}"#),
    )
    .await;
    let much_later = must(UtcTimestamp::from_unix_nanos(
        later.unix_nanos() + 3_700_000_000_000,
    ));
    let report = must(crate::schedule_service::tick(&fixture.state, much_later).await);
    assert_eq!(report.started, 1);
    let fired = must(
        jarvis_storage::find_schedule(&fixture.database, LOCAL_WORKSPACE_ID, &schedule.schedule_id)
            .await,
    );
    let session = fired.session_id().unwrap_or_default().to_owned();
    let linked =
        must(jarvis_storage::project_for(&fixture.database, LinkKind::Session, &session).await);
    assert_eq!(linked.map(|p| p.name), Some("Prospecting".to_owned()));

    let (status, _) = send(
        &fixture,
        "POST",
        "/api/v1/schedules",
        Some(r#"{"objective":"c","every":"1h","project_id":"Nope"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn an_existing_schedule_can_be_filed_under_a_project_and_taken_out_again() {
    let fixture = fixture().await;
    let project = make(&fixture, "Prospecting").await;
    let (_, text) = send(
        &fixture,
        "POST",
        "/api/v1/schedules",
        Some(r#"{"objective":"check","every":"1h"}"#),
    )
    .await;
    let schedule: jarvis_protocol::ScheduleReply = parse(&text);
    assert!(schedule.project.is_none());
    let path = format!("/api/v1/schedules/{}/project", schedule.schedule_id);

    let (status, text) = send(
        &fixture,
        "POST",
        &path,
        Some(&format!(r#"{{"project_id":"{}"}}"#, project.project_id)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert_eq!(
        parse::<jarvis_protocol::ScheduleReply>(&text)
            .project
            .as_deref(),
        Some("Prospecting")
    );
    let linked = must(
        jarvis_storage::project_for(&fixture.database, LinkKind::Schedule, &schedule.schedule_id)
            .await,
    );
    assert!(linked.is_some());

    let (status, _) = send(&fixture, "POST", &path, Some(r#"{"project_id":"Nope"}"#)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, text) = send(&fixture, "POST", &path, Some("{}")).await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert!(
        parse::<jarvis_protocol::ScheduleReply>(&text)
            .project
            .is_none()
    );
    let (status, _) = send(
        &fixture,
        "POST",
        "/api/v1/schedules/00000000-0000-0000-0000-000000000000/project",
        Some("{}"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_project_that_used_its_daily_cap_gets_no_more_scheduled_runs() {
    let fixture = fixture().await;
    let body = r#"{"name":"Capped","goal":"g","daily_run_limit":1}"#;
    let (status, text) = send(&fixture, "POST", "/api/v1/projects", Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{text}");
    let project: ProjectReply = parse(&text);
    assert_eq!((project.daily_run_limit, project.runs_today), (1, 0));

    // The owner's own message counts towards the day, and is never refused by the cap.
    let (status, _) = start(&fixture, r#"{"objective":"look","project_id":"Capped"}"#).await;
    assert_eq!(status, StatusCode::CREATED);
    let (_, text) = send(&fixture, "GET", "/api/v1/projects/Capped", None).await;
    let detail: ProjectDetailReply = parse(&text);
    assert_eq!(detail.project.runs_today, 1);

    let task = r#"{"objective":"check","every":"1h","project_id":"Capped"}"#;
    send(&fixture, "POST", "/api/v1/schedules", Some(task)).await;
    let later = must(UtcTimestamp::from_unix_nanos(
        now().unix_nanos() + 3_700_000_000_000,
    ));
    let report = must(crate::schedule_service::tick(&fixture.state, later).await);
    assert_eq!(
        (report.started, report.skipped),
        (0, 1),
        "the cap holds the fire"
    );

    // Raising the cap lets the next fire through.
    let (status, _) = send(
        &fixture,
        "PATCH",
        "/api/v1/projects/Capped",
        Some(r#"{"daily_run_limit":5}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let much_later = must(UtcTimestamp::from_unix_nanos(
        later.unix_nanos() + 3_700_000_000_000,
    ));
    assert_eq!(
        must(crate::schedule_service::tick(&fixture.state, much_later).await).started,
        1
    );

    let (status, _) = send(
        &fixture,
        "POST",
        "/api/v1/projects",
        Some(r#"{"name":"Huge","daily_run_limit":5000}"#),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "a cap beyond the maximum is refused"
    );
}

/// Records what one model call would: a `usage_updated` event on the run.
async fn record_usage(fixture: &Fixture, run_id: &str, input: u64, output: u64) {
    let payload =
        format!(r#"{{"input_tokens":{input},"output_tokens":{output},"cached_input_tokens":0}}"#);
    let event = must(jarvis_storage::NewRunEvent::new(
        jarvis_core::RunId::new().to_string(),
        run_id,
        jarvis_core::RunEventKind::UsageUpdated,
        None,
        must(jarvis_core::RunEventPayload::new(&payload)),
        jarvis_core::CorrelationId::new(),
        now(),
    ));
    must(jarvis_storage::append_run_event(&fixture.database, &event).await);
}

#[tokio::test]
async fn a_project_that_used_its_daily_tokens_gets_no_more_scheduled_runs_and_only_its_own_tokens_count()
 {
    let fixture = fixture().await;
    let body = r#"{"name":"Metered","goal":"g","daily_token_limit":1000}"#;
    let (status, text) = send(&fixture, "POST", "/api/v1/projects", Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{text}");
    let project: ProjectReply = parse(&text);
    assert_eq!((project.daily_token_limit, project.tokens_today), (1000, 0));

    // A run in another project, however large, is not this project's spend.
    make(&fixture, "Other").await;
    let (_, other) = start(&fixture, r#"{"objective":"big","project_id":"Other"}"#).await;
    let other: RunReply = parse(&other);
    record_usage(&fixture, &other.run_id, 900_000, 5_000).await;

    let (status, mine) = start(&fixture, r#"{"objective":"look","project_id":"Metered"}"#).await;
    assert_eq!(status, StatusCode::CREATED, "{mine}");
    let mine: RunReply = parse(&mine);
    record_usage(&fixture, &mine.run_id, 600, 50).await;
    let (_, text) = send(&fixture, "GET", "/api/v1/projects/Metered", None).await;
    let detail: ProjectDetailReply = parse(&text);
    assert_eq!(
        detail.project.tokens_today, 650,
        "input plus output, this project only"
    );

    // More usage takes the project over its cap before the task first fires, so a held fire can only be the token cap.
    record_usage(&fixture, &mine.run_id, 400, 0).await;
    let task = r#"{"objective":"check","every":"1h","project_id":"Metered"}"#;
    send(&fixture, "POST", "/api/v1/schedules", Some(task)).await;
    let hour = 3_700_000_000_000_i128;
    let first = must(UtcTimestamp::from_unix_nanos(now().unix_nanos() + hour));
    let report = must(crate::schedule_service::tick(&fixture.state, first).await);
    assert_eq!(
        (report.started, report.skipped),
        (0, 1),
        "the token cap holds the fire"
    );

    // The owner's own message is never refused by the cap.
    let (status, _) = start(
        &fixture,
        r#"{"objective":"still me","project_id":"Metered"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    // Raising the cap above what was used lets the next fire through.
    let (status, _) = send(
        &fixture,
        "PATCH",
        "/api/v1/projects/Metered",
        Some(r#"{"daily_token_limit":5000}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let second = must(UtcTimestamp::from_unix_nanos(first.unix_nanos() + hour));
    assert_eq!(
        must(crate::schedule_service::tick(&fixture.state, second).await).started,
        1
    );
    let (status, _) = send(
        &fixture,
        "POST",
        "/api/v1/projects",
        Some(r#"{"name":"Huge","daily_token_limit":3000000000}"#),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "a cap beyond the maximum is refused"
    );
}
#[tokio::test]
async fn a_run_a_restart_cut_off_is_continued_once_in_its_conversation_and_never_again() {
    let fixture = fixture().await;
    make(&fixture, "Prospecting").await;
    let (status, text) = start(
        &fixture,
        r#"{"objective":"find five leads","project_id":"Prospecting"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let first: RunReply = parse(&text);

    // A restart: the run was never driven to the end, so recovery settles it as interrupted.
    let settled = must(jarvis_storage::recover_interrupted_runs(&fixture.database, now()).await);
    assert_eq!(settled, vec![first.run_id.clone()]);
    assert_eq!(
        crate::resume::continue_interrupted(&fixture.state, &settled).await,
        1
    );

    let runs =
        must(jarvis_storage::read_recent_runs(&fixture.database, LOCAL_WORKSPACE_ID, 10).await);
    let continued = runs
        .iter()
        .find(|run| run.id() != first.run_id)
        .map(|run| (run.session_id().to_owned(), run.objective().to_owned()));
    let (session, objective) = continued.unwrap_or_else(|| panic!("a continuation was started"));
    assert_eq!(
        session, first.session_id,
        "it joins the conversation, and with it the project"
    );
    assert!(
        objective.starts_with(crate::resume::RESUME_NOTICE)
            && objective.ends_with("find five leads")
    );

    // It dies in turn: it is not continued again, and the original is no longer the newest run of its conversation.
    let again = must(jarvis_storage::recover_interrupted_runs(&fixture.database, now()).await);
    assert_eq!(again.len(), 1);
    assert_eq!(
        crate::resume::continue_interrupted(&fixture.state, &again).await,
        0
    );
    assert_eq!(
        crate::resume::continue_interrupted(&fixture.state, &settled).await,
        0
    );
}

#[tokio::test]
async fn the_digest_counts_runs_by_project_and_lists_what_was_decided_and_what_failed() {
    let fixture = fixture().await;
    make(&fixture, "Prospecting").await;
    let (status, _) = send(&fixture, "GET", "/api/v1/digest", None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, text) = start(
        &fixture,
        r#"{"objective":"find leads","project_id":"Prospecting"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{text}");
    start(&fixture, r#"{"objective":"just a chat"}"#).await;
    let note = r#"{"text":"Target dentists first.","kind":"decision"}"#;
    send(
        &fixture,
        "POST",
        "/api/v1/projects/Prospecting/notes",
        Some(note),
    )
    .await;
    send(
        &fixture,
        "POST",
        "/api/v1/projects/Prospecting/notes",
        Some(r#"{"text":"fiddling","kind":"progress"}"#),
    )
    .await;
    // Each model call records its own usage event; the digest adds them up.
    let run: RunReply = parse(&text);
    for (input, output) in [(1000, 40), (500, 10)] {
        let payload = format!(
            r#"{{"input_tokens":{input},"output_tokens":{output},"cached_input_tokens":0}}"#
        );
        let event = must(jarvis_storage::NewRunEvent::new(
            jarvis_core::RunId::new().to_string(),
            &run.run_id,
            jarvis_core::RunEventKind::UsageUpdated,
            None,
            must(jarvis_core::RunEventPayload::new(&payload)),
            jarvis_core::CorrelationId::new(),
            now(),
        ));
        must(jarvis_storage::append_run_event(&fixture.database, &event).await);
    }
    must(jarvis_storage::recover_interrupted_runs(&fixture.database, now()).await);

    let (status, text) = send(&fixture, "GET", "/api/v1/digest/48", None).await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let digest: jarvis_protocol::DigestReply = parse(&text);
    assert_eq!(
        (digest.hours, digest.runs, digest.failed, digest.succeeded),
        (48, 2, 2, 0)
    );
    assert_eq!((digest.input_tokens, digest.output_tokens), (1500, 50));
    assert_eq!(
        digest.projects.len(),
        1,
        "a plain chat belongs to no project"
    );
    assert_eq!(
        (digest.projects[0].name.as_str(), digest.projects[0].runs),
        ("Prospecting", 1)
    );
    let kinds: Vec<&str> = digest.projects[0]
        .highlights
        .iter()
        .map(|note| note.kind.as_str())
        .collect();
    assert_eq!(kinds, ["decision"], "progress chatter is not a highlight");
    assert_eq!(digest.problems.len(), 2);
    assert_eq!(digest.problems[0].error_code, "interrupted_by_restart");

    for bad in ["0", "99999"] {
        let (status, _) = send(&fixture, "GET", &format!("/api/v1/digest/{bad}"), None).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{bad}");
    }
    let (status, _) = call(&fixture, "GET", "/api/v1/digest", false, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_push_test_without_a_topic_is_refused_and_needs_the_credential() {
    let fixture = fixture().await;
    let (status, text) = send(&fixture, "POST", "/api/v1/push/test", Some("{}")).await;
    assert_eq!(status, StatusCode::CONFLICT, "{text}");
    assert!(text.contains("daemon.push_topic"));
    let (status, _) = call(&fixture, "POST", "/api/v1/push/test", false, Some("{}")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn the_owner_manages_contacts_over_the_api_including_lifting_a_do_not_contact() {
    let fixture = fixture().await;
    let add =
        r#"{"company":"Acme BV","person":"Eva","email":"eva@acme.nl","status":"do_not_contact"}"#;
    let (status, text) = send(&fixture, "POST", "/api/v1/contacts", Some(add)).await;
    assert_eq!(status, StatusCode::CREATED, "{text}");
    let made: jarvis_protocol::ContactReply = parse(&text);

    let (status, text) = send(
        &fixture,
        "POST",
        "/api/v1/contacts",
        Some(r#"{"email":"eva@acme.nl","status":"contacted"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert_eq!(
        parse::<jarvis_protocol::ContactReply>(&text).status,
        "contacted",
        "the owner may lift it"
    );

    let (_, text) = send(
        &fixture,
        "GET",
        "/api/v1/contacts?status=contacted&limit=10",
        None,
    )
    .await;
    let listed: jarvis_protocol::ContactListReply = parse(&text);
    assert_eq!(listed.contacts.len(), 1);
    assert_eq!(
        listed
            .counts
            .iter()
            .find(|count| count.status == "contacted")
            .map(|count| count.count),
        Some(1)
    );
    let (_, text) = send(&fixture, "GET", "/api/v1/contacts?status=won", None).await;
    assert!(
        parse::<jarvis_protocol::ContactListReply>(&text)
            .contacts
            .is_empty()
    );

    for bad in [
        "?status=maybe",
        "?limit=0",
        "?limit=5000",
        "?colour=red",
        "?limit=x",
    ] {
        let (status, _) = send(&fixture, "GET", &format!("/api/v1/contacts{bad}"), None).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{bad}");
    }
    let (status, _) = send(
        &fixture,
        "POST",
        "/api/v1/contacts",
        Some(r#"{"person":"nobody"}"#),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "a new contact needs a company"
    );

    let path = format!("/api/v1/contacts/{}", made.contact_id);
    assert_eq!(
        send(&fixture, "DELETE", &path, None).await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        send(&fixture, "DELETE", &path, None).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&fixture, "GET", "/api/v1/contacts", false, None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
}

/// **A long run keeps its answer.**
///
/// The answer used to be looked for inside the first page of a run''s events (a thousand), so a run that streamed more fragments than that
/// before its final answer lost the answer: the parent that delegated it, the owner''s notification and the transcript all saw nothing.
#[tokio::test]
async fn an_answer_is_found_even_when_the_run_streamed_more_events_than_one_page() {
    let fixture = fixture().await;
    let (status, text) = start(&fixture, r#"{"objective":"long research"}"#).await;
    assert_eq!(status, StatusCode::CREATED, "{text}");
    let run: RunReply = parse(&text);
    let append = |kind, payload: &'static str| {
        let event = must(jarvis_storage::NewRunEvent::new(
            jarvis_core::RunId::new().to_string(),
            &run.run_id,
            kind,
            None,
            must(jarvis_core::RunEventPayload::new(payload)),
            jarvis_core::CorrelationId::new(),
            now(),
        ));
        let database = fixture.database.clone();
        async move { must(jarvis_storage::append_run_event(&database, &event).await) }
    };
    for _ in 0..1_300 {
        append(
            jarvis_core::RunEventKind::OutputDelta,
            r#"{"text":"fragment "}"#,
        )
        .await;
    }
    let stored = must(jarvis_storage::find_run(&fixture.database, &run.run_id).await);
    assert_eq!(
        must(crate::executor::last_answer(&fixture.state.database_handle(), &stored).await),
        None,
        "no answer yet"
    );
    append(
        jarvis_core::RunEventKind::OutputCompleted,
        r#"{"text":"first answer"}"#,
    )
    .await;
    append(
        jarvis_core::RunEventKind::OutputCompleted,
        r#"{"text":"the final answer"}"#,
    )
    .await;
    assert_eq!(
        must(crate::executor::last_answer(&fixture.state.database_handle(), &stored).await)
            .as_deref(),
        Some("the final answer"),
        "the latest answer, past the first thousand events"
    );
}
