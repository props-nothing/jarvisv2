//! Scheduled tasks through the real routes and the real scheduler pass.

use std::sync::Arc;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use jarvis_core::{Cadence, ClientCredential, SystemClock, UtcTimestamp};
use jarvis_protocol::{RunListReply, ScheduleListReply, ScheduleReply};
use jarvis_storage::{LOCAL_WORKSPACE_ID, SqliteDatabase};
use tower::ServiceExt;

use super::*;
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

/// A gateway over a fresh profile, with or without a scripted executor.
///
/// Without one, a started run stays `received` forever — which is exactly what the overlap test needs: a previous
/// run that has not finished.
async fn fixture(with_executor: bool) -> Fixture {
    let directory = std::env::temp_dir().join(format!(
        "jarvis-schedule-svc-{}",
        jarvis_core::scratch_tag()
    ));
    must(std::fs::create_dir_all(&directory));
    let database = Arc::new(must(
        SqliteDatabase::open(&directory.join("jarvis.sqlite3")).await,
    ));
    let credential = must(ClientCredential::generate());
    let presented = credential.expose().to_owned();
    let mut state = GatewayState::new(Arc::clone(&database), credential);
    if with_executor {
        let executor = must(crate::executor::Executor::build("scripted", None));
        state = state.with_executor(Arc::new(executor));
    }
    Fixture {
        app: router(state.clone()),
        state,
        database,
        credential: presented,
        directory,
    }
}

fn request(
    method: &str,
    path: &str,
    credential: Option<&str>,
    body: Option<&str>,
) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(credential) = credential {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {credential}"));
    }
    if body.is_some() {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
    }
    must(builder.body(Body::from(body.unwrap_or_default().to_owned())))
}

async fn send(
    fixture: &Fixture,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> (StatusCode, String) {
    let response = must(
        fixture
            .app
            .clone()
            .oneshot(request(method, path, Some(&fixture.credential), body))
            .await,
    );
    let status = response.status();
    let bytes = must(axum::body::to_bytes(response.into_body(), 1 << 20).await);
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

fn now() -> UtcTimestamp {
    UtcTimestamp::now(&SystemClock)
}

fn after(base: UtcTimestamp, seconds: i128) -> UtcTimestamp {
    must(UtcTimestamp::from_unix_nanos(
        base.unix_nanos() + seconds * 1_000_000_000,
    ))
}

/// Creates a minute-interval task whose first fire is due at `fire_at`.
async fn due_task(fixture: &Fixture, fire_at: UtcTimestamp) -> jarvis_storage::StoredSchedule {
    let created = after(fire_at, -60);
    must(
        jarvis_storage::create_schedule(
            &fixture.database,
            LOCAL_WORKSPACE_ID,
            "check the weather in Amsterdam",
            must(Cadence::every_seconds(60)),
            created,
        )
        .await,
    )
}

async fn find(fixture: &Fixture, id: &str) -> jarvis_storage::StoredSchedule {
    must(jarvis_storage::find_schedule(&fixture.database, LOCAL_WORKSPACE_ID, id).await)
}

async fn wait_for_settled(fixture: &Fixture, run_id: &str) {
    for _ in 0..100 {
        if must(jarvis_storage::find_run(&fixture.database, run_id).await)
            .state()
            .is_terminal()
        {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("the run did not settle");
}

/// **A due task starts an ordinary run, records it, and is then not due again.**
#[tokio::test]
async fn a_due_task_starts_a_run_and_is_not_due_twice() {
    let fixture = fixture(true).await;
    let fire_at = now();
    let task = due_task(&fixture, fire_at).await;

    let report = must(tick(&fixture.state, fire_at).await);
    assert_eq!(
        report,
        TickReport {
            started: 1,
            skipped: 0,
            failed: 0
        }
    );

    let fired = find(&fixture, task.id()).await;
    assert_eq!(fired.fire_count(), 1);
    let run_id = fired
        .last_run_id()
        .unwrap_or_else(|| panic!("a fired task records its run"))
        .to_owned();
    let run = must(jarvis_storage::find_run(&fixture.database, &run_id).await);
    assert!(
        run.objective().contains("check the weather in Amsterdam"),
        "the run carries the task's objective: {}",
        run.objective()
    );
    assert!(
        run.objective().contains("no one to ask"),
        "the model is told nobody is there to answer a question"
    );

    assert_eq!(
        must(tick(&fixture.state, fire_at).await),
        TickReport::default(),
        "a task is not due twice"
    );
}

/// **A recurring task has a conversation**: its second fire continues the first fire's session.
#[tokio::test]
async fn a_recurring_task_continues_one_session() {
    let fixture = fixture(true).await;
    let first_time = now();
    let task = due_task(&fixture, first_time).await;

    must(tick(&fixture.state, first_time).await);
    let first = find(&fixture, task.id()).await;
    let first_run = first.last_run_id().unwrap_or_default().to_owned();
    wait_for_settled(&fixture, &first_run).await;

    must(tick(&fixture.state, after(first_time, 61)).await);
    let second = find(&fixture, task.id()).await;
    assert_eq!(second.fire_count(), 2);
    assert_ne!(second.last_run_id(), first.last_run_id());
    assert_eq!(
        second.session_id(),
        first.session_id(),
        "the second fire must continue the first fire's conversation"
    );
}

/// **No pile-up: a fire is skipped while the previous run is still going.** With no executor the first run never
/// finishes, which is the same shape as one parked on an approval nobody has answered.
#[tokio::test]
async fn a_fire_is_skipped_while_the_previous_run_is_still_going() {
    let fixture = fixture(false).await;
    let first_time = now();
    let task = due_task(&fixture, first_time).await;

    assert_eq!(must(tick(&fixture.state, first_time).await).started, 1);
    let report = must(tick(&fixture.state, after(first_time, 61)).await);
    assert_eq!(
        report,
        TickReport {
            started: 0,
            skipped: 1,
            failed: 0
        }
    );

    let skipped = find(&fixture, task.id()).await;
    assert_eq!(
        skipped.fire_count(),
        1,
        "the skipped fire must not start a second run"
    );
    assert_eq!(
        skipped.skipped_count(),
        1,
        "and the skip is counted, so a person can see it"
    );
}

#[tokio::test]
async fn a_paused_task_does_not_fire() {
    let fixture = fixture(true).await;
    let fire_at = now();
    let task = due_task(&fixture, fire_at).await;
    let (status, _) = send(
        &fixture,
        "POST",
        &format!("/api/v1/schedules/{}/pause", task.id()),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        must(tick(&fixture.state, after(fire_at, 10_000)).await),
        TickReport::default()
    );
}

#[tokio::test]
async fn the_routes_create_list_pause_resume_and_delete() {
    let fixture = fixture(false).await;

    let (status, body) = send(
        &fixture,
        "POST",
        "/api/v1/schedules",
        Some(r#"{"objective":"summarise my day","every":"6h"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let created: ScheduleReply = must(serde_json::from_str(&body));
    assert_eq!(created.cadence, "every");
    assert_eq!(created.interval_seconds, Some(21_600));
    assert!(created.enabled);
    assert!(created.next_run_at.is_some());

    let (_, body) = send(&fixture, "GET", "/api/v1/schedules", None).await;
    let listed: ScheduleListReply = must(serde_json::from_str(&body));
    assert_eq!(listed.total, 1);
    assert_eq!(listed.schedules[0].schedule_id, created.schedule_id);

    let path = format!("/api/v1/schedules/{}", created.schedule_id);
    let (status, body) = send(&fixture, "POST", &format!("{path}/pause"), None).await;
    assert_eq!(status, StatusCode::OK);
    let paused: ScheduleReply = must(serde_json::from_str(&body));
    assert!(!paused.enabled);
    assert!(
        paused.next_run_at.is_none(),
        "a paused task has no next fire to show"
    );

    let (_, body) = send(&fixture, "POST", &format!("{path}/resume"), None).await;
    let resumed: ScheduleReply = must(serde_json::from_str(&body));
    assert!(resumed.enabled);

    let (status, _) = send(&fixture, "DELETE", &path, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = send(&fixture, "DELETE", &path, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_bad_schedule_is_refused_with_the_rule_it_broke() {
    let fixture = fixture(false).await;
    for (body, expected) in [
        (r#"{"objective":"x","every":"5s"}"#, "at least 60 seconds"),
        (
            r#"{"objective":"x","every":"tomorrow"}"#,
            "a whole number and a unit",
        ),
        (r#"{"objective":"x"}"#, "exactly one"),
        (
            r#"{"objective":"x","every":"1h","at":"2099-01-01T00:00:00Z"}"#,
            "exactly one",
        ),
        (
            r#"{"objective":"x","at":"2001-01-01T00:00:00Z"}"#,
            "in the future",
        ),
        (r#"{"objective":"   ","every":"1h"}"#, "needs an objective"),
    ] {
        let (status, text) = send(&fixture, "POST", "/api/v1/schedules", Some(body)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}: {text}");
        assert!(
            text.contains(expected),
            "{body}: expected {expected:?} in {text}"
        );
    }
    // A tool or scope is not something a schedule can carry: an unknown field is refused, not ignored.
    let (status, _) = send(
        &fixture,
        "POST",
        "/api/v1/schedules",
        Some(r#"{"objective":"x","every":"1h","approve":true}"#),
    )
    .await;
    assert!(status.is_client_error());
}

#[tokio::test]
async fn the_schedule_routes_need_the_credential() {
    let fixture = fixture(false).await;
    for (method, path) in [("GET", "/api/v1/schedules"), ("GET", "/api/v1/runs")] {
        let response = must(
            fixture
                .app
                .clone()
                .oneshot(request(method, path, None, None))
                .await,
        );
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "{method} {path}"
        );
    }
}

/// A scheduled run has nobody watching its stream, so the run list is how its answer is found.
#[tokio::test]
async fn the_run_list_shows_what_a_run_answered() {
    let fixture = fixture(true).await;
    let fire_at = now();
    let task = due_task(&fixture, fire_at).await;
    must(tick(&fixture.state, fire_at).await);
    let run_id = find(&fixture, task.id())
        .await
        .last_run_id()
        .unwrap_or_default()
        .to_owned();
    wait_for_settled(&fixture, &run_id).await;

    let (status, body) = send(&fixture, "GET", "/api/v1/runs?limit=5", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let listed: RunListReply = must(serde_json::from_str(&body));
    assert_eq!(listed.total, 1);
    assert_eq!(listed.runs[0].run_id, run_id);
    assert_eq!(
        listed.runs[0].objective, "[scheduled] check the weather in Amsterdam",
        "the framing line is for the model; a person sees the task"
    );
    assert!(
        listed.runs[0]
            .answer
            .as_deref()
            .is_some_and(|answer| !answer.is_empty()),
        "a settled run's answer must be listed: {:?}",
        listed.runs[0]
    );

    let (status, _) = send(&fixture, "GET", "/api/v1/runs?limit=0", None).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = send(&fixture, "GET", "/api/v1/runs?limit=51", None).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}
