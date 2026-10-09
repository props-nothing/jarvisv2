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
