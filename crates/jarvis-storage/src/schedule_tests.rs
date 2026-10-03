//! Scheduled-task storage: creation, listing, the claim, and what a restart does.

use std::{
    fs,
    path::{Path, PathBuf},
};

use super::*;
use crate::{DEFAULT_DATABASE_FILENAME, LOCAL_WORKSPACE_ID, SqliteDatabase};
use jarvis_core::{Cadence, UtcTimestamp};

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("jarvis-schedules-{}", jarvis_core::scratch_tag()));
        must(fs::create_dir_all(&path));
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDirectory {
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

/// An instant `seconds` after a fixed epoch.
fn at(seconds: i128) -> UtcTimestamp {
    must(UtcTimestamp::from_unix_nanos(
        1_774_000_000_000_000_000 + seconds * 1_000_000_000,
    ))
}

async fn database() -> (TestDirectory, SqliteDatabase) {
    let directory = TestDirectory::new();
    let database =
        must(SqliteDatabase::open(&directory.path().join(DEFAULT_DATABASE_FILENAME)).await);
    (directory, database)
}

const SESSION: &str = "0198f000-0000-7000-8000-000000000003";

/// Inserts the session a recorded run names, because the column is foreign-keyed.
async fn seed_session(database: &SqliteDatabase) {
    must(
        sqlx::query(
            "INSERT INTO sessions (id, workspace_id, user_id, channel, status, created_at, updated_at, version) \
             VALUES (?1, ?2, ?3, 'api', 'active', ?4, ?4, 1)",
        )
        .bind(SESSION)
        .bind(LOCAL_WORKSPACE_ID)
        .bind(crate::LOCAL_USER_ID)
        .bind(at(0).to_string())
        .execute(database.pool())
        .await
        .map(|_| ()),
    );
}

fn hourly() -> Cadence {
    must(Cadence::every_seconds(3600))
}

#[tokio::test]
async fn a_schedule_round_trips_and_first_fires_one_interval_after_creation() {
    let (_directory, database) = database().await;
    let created = must(
        create_schedule(
            &database,
            LOCAL_WORKSPACE_ID,
            "check the weather",
            hourly(),
            at(0),
        )
        .await,
    );
    assert_eq!(created.objective(), "check the weather");
    assert_eq!(created.cadence(), hourly());
    assert!(created.enabled());
    assert_eq!(created.next_run(), at(3600));
    assert_eq!(created.fire_count(), 0);
    assert!(created.session_id().is_none());

    let listed = must(list_schedules(&database, LOCAL_WORKSPACE_ID).await);
    assert_eq!(listed, vec![created]);
    database.close().await;
}

#[tokio::test]
async fn nothing_is_due_before_its_time_and_a_task_is_claimed_when_it_is() {
    let (_directory, database) = database().await;
    let schedule = must(create_schedule(&database, LOCAL_WORKSPACE_ID, "a", hourly(), at(0)).await);

    assert!(must(claim_due_schedules(&database, at(3599), 10).await).is_empty());
    let claimed = must(claim_due_schedules(&database, at(3600), 10).await);
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].id(), schedule.id());
    database.close().await;
}

/// **The guarded claim is what makes a fire happen at most once.** Two passes over the same due task: only the
/// first gets it. This is the property a restart racing a slow tick depends on.
#[tokio::test]
async fn a_due_task_is_claimed_by_exactly_one_pass() {
    let (_directory, database) = database().await;
    must(create_schedule(&database, LOCAL_WORKSPACE_ID, "a", hourly(), at(0)).await);

    let first = must(claim_due_schedules(&database, at(4000), 10).await);
    let second = must(claim_due_schedules(&database, at(4000), 10).await);
    assert_eq!(first.len(), 1);
    assert!(
        second.is_empty(),
        "a second pass must not fire what the first claimed"
    );
    database.close().await;
}

/// **A backlog is skipped, not replayed.** The machine was off for ten hours; one fire is owed and the next is an
/// hour after *waking*, not eleven catch-up fires.
#[tokio::test]
async fn a_long_absence_owes_one_fire_and_the_next_is_measured_from_waking() {
    let (_directory, database) = database().await;
    let schedule = must(create_schedule(&database, LOCAL_WORKSPACE_ID, "a", hourly(), at(0)).await);

    let woke = at(36_000);
    assert_eq!(
        must(claim_due_schedules(&database, woke, 10).await).len(),
        1
    );
    assert!(must(claim_due_schedules(&database, woke, 10).await).is_empty());
    let after = must(find_schedule(&database, LOCAL_WORKSPACE_ID, schedule.id()).await);
    assert_eq!(after.next_run(), at(36_000 + 3600));
    database.close().await;
}

#[tokio::test]
async fn a_one_off_fires_once_and_is_then_finished() {
    let (_directory, database) = database().await;
    let once = must(Cadence::once_at(at(500), at(0)));
    let schedule =
        must(create_schedule(&database, LOCAL_WORKSPACE_ID, "remind me", once, at(0)).await);

    assert_eq!(
        must(claim_due_schedules(&database, at(500), 10).await).len(),
        1
    );
    assert!(must(claim_due_schedules(&database, at(9999), 10).await).is_empty());
    let after = must(find_schedule(&database, LOCAL_WORKSPACE_ID, schedule.id()).await);
    assert!(!after.enabled(), "a fired one-off is finished");

    seed_session(&database).await;
    must(record_schedule_run(&database, schedule.id(), "run-1", SESSION, at(500)).await);
    let resumed =
        set_schedule_enabled(&database, LOCAL_WORKSPACE_ID, schedule.id(), true, at(600)).await;
    assert!(
        matches!(resumed, Err(DatabaseError::ScheduleFinished)),
        "a one-off that already fired cannot be resumed: {resumed:?}"
    );
    database.close().await;
}

#[tokio::test]
async fn a_paused_task_never_fires_and_resuming_waits_a_full_interval() {
    let (_directory, database) = database().await;
    let schedule = must(create_schedule(&database, LOCAL_WORKSPACE_ID, "a", hourly(), at(0)).await);
    must(set_schedule_enabled(&database, LOCAL_WORKSPACE_ID, schedule.id(), false, at(10)).await);
    assert!(must(claim_due_schedules(&database, at(99_999), 10).await).is_empty());

    // Resumed a week later: it does not fire at once because it is "overdue".
    let week = 7 * 24 * 3600;
    let resumed = must(
        set_schedule_enabled(&database, LOCAL_WORKSPACE_ID, schedule.id(), true, at(week)).await,
    );
    assert!(resumed.enabled());
    assert_eq!(resumed.next_run(), at(week + 3600));
    assert!(must(claim_due_schedules(&database, at(week), 10).await).is_empty());
    database.close().await;
}

#[tokio::test]
async fn a_recorded_run_sets_the_session_the_next_fire_continues() {
    let (_directory, database) = database().await;
    let schedule = must(create_schedule(&database, LOCAL_WORKSPACE_ID, "a", hourly(), at(0)).await);
    // Sessions are foreign-keyed, so the fixture inserts the one it names.
    must(
        sqlx::query(
            "INSERT INTO sessions (id, workspace_id, user_id, channel, status, created_at, updated_at, version) \
             VALUES (?1, ?2, ?3, 'api', 'active', ?4, ?4, 1)",
        )
        .bind("0198f000-0000-7000-8000-000000000003")
        .bind(LOCAL_WORKSPACE_ID)
        .bind(crate::LOCAL_USER_ID)
        .bind(at(0).to_string())
        .execute(database.pool())
        .await
        .map(|_| ()),
    );
    must(
        record_schedule_run(
            &database,
            schedule.id(),
            "0198f000-0000-7000-8000-0000000000c3",
            "0198f000-0000-7000-8000-000000000003",
            at(3600),
        )
        .await,
    );
    let after = must(find_schedule(&database, LOCAL_WORKSPACE_ID, schedule.id()).await);
    assert_eq!(
        after.session_id(),
        Some("0198f000-0000-7000-8000-000000000003")
    );
    assert_eq!(
        after.last_run_id(),
        Some("0198f000-0000-7000-8000-0000000000c3")
    );
    assert_eq!(after.fire_count(), 1);
    assert_eq!(after.last_fired_at(), Some(at(3600)));

    must(record_schedule_skip(&database, schedule.id()).await);
    let skipped = must(find_schedule(&database, LOCAL_WORKSPACE_ID, schedule.id()).await);
    assert_eq!(skipped.skipped_count(), 1);
    database.close().await;
}

#[tokio::test]
async fn a_workspace_holds_a_bounded_number_of_tasks() {
    let (_directory, database) = database().await;
    for index in 0..MAX_SCHEDULES_PER_WORKSPACE {
        must(
            create_schedule(
                &database,
                LOCAL_WORKSPACE_ID,
                &format!("task {index}"),
                hourly(),
                at(0),
            )
            .await,
        );
    }
    let refused = create_schedule(
        &database,
        LOCAL_WORKSPACE_ID,
        "one too many",
        hourly(),
        at(0),
    )
    .await;
    assert!(
        matches!(refused, Err(DatabaseError::ScheduleLimit)),
        "{refused:?}"
    );
    database.close().await;
}

#[tokio::test]
async fn a_task_in_another_workspace_is_indistinguishable_from_none() {
    let (_directory, database) = database().await;
    let schedule = must(create_schedule(&database, LOCAL_WORKSPACE_ID, "a", hourly(), at(0)).await);
    let elsewhere = "0198f000-0000-7000-8000-00000000ffff";
    assert!(matches!(
        find_schedule(&database, elsewhere, schedule.id()).await,
        Err(DatabaseError::ScheduleNotFound)
    ));
    assert!(matches!(
        delete_schedule(&database, elsewhere, schedule.id()).await,
        Err(DatabaseError::ScheduleNotFound)
    ));
    must(delete_schedule(&database, LOCAL_WORKSPACE_ID, schedule.id()).await);
    assert!(must(list_schedules(&database, LOCAL_WORKSPACE_ID).await).is_empty());
    database.close().await;
}

/// Survives a reopen of the database file: a schedule is durable, and a restart owes only what is due.
#[tokio::test]
async fn a_schedule_survives_a_restart() {
    let directory = TestDirectory::new();
    let path = directory.path().join(DEFAULT_DATABASE_FILENAME);
    let first = must(SqliteDatabase::open(&path).await);
    let schedule = must(create_schedule(&first, LOCAL_WORKSPACE_ID, "a", hourly(), at(0)).await);
    first.close().await;

    let second = must(SqliteDatabase::open(&path).await);
    let after = must(find_schedule(&second, LOCAL_WORKSPACE_ID, schedule.id()).await);
    assert_eq!(after, schedule);
    assert_eq!(
        must(claim_due_schedules(&second, at(3600), 10).await).len(),
        1
    );
    second.close().await;
}
