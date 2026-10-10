//! Project storage: validation, names, journal, links and what deleting removes.

use std::{
    fs,
    path::{Path, PathBuf},
};

use super::*;
use crate::{DEFAULT_DATABASE_FILENAME, LOCAL_WORKSPACE_ID, SqliteDatabase};
use jarvis_core::UtcTimestamp;

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("jarvis-projects-{}", jarvis_core::scratch_tag()));
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

const SESSION: &str = "11111111-1111-1111-1111-111111111111";
const SCHEDULE: &str = "22222222-2222-2222-2222-222222222222";

fn new(name: &str) -> NewProject {
    NewProject {
        name: name.to_owned(),
        goal: "Win customers".to_owned(),
        guidance: "Be brief.".to_owned(),
        folder: "sales".to_owned(),
        daily_run_limit: 0,
        daily_token_limit: 0,
    }
}

#[tokio::test]
async fn a_project_is_created_found_by_id_or_name_and_listed_active_first() {
    let (_directory, database) = database().await;
    let made =
        must(create_project(&database, LOCAL_WORKSPACE_ID, &new("Prospecting"), at(0)).await);
    assert_eq!(made.status, ProjectStatus::Active);
    assert_eq!(made.folder, "sales");
    assert_eq!(
        must(find_project(&database, LOCAL_WORKSPACE_ID, &made.id).await),
        made
    );
    assert_eq!(
        must(find_project(&database, LOCAL_WORKSPACE_ID, "prospecting").await),
        made
    );

    let other = must(create_project(&database, LOCAL_WORKSPACE_ID, &new("Archive"), at(1)).await);
    must(
        update_project(
            &database,
            LOCAL_WORKSPACE_ID,
            &other.id,
            &ProjectChanges {
                status: Some(ProjectStatus::Done),
                ..ProjectChanges::default()
            },
            at(2),
        )
        .await,
    );
    let names: Vec<String> = must(list_projects(&database, LOCAL_WORKSPACE_ID).await)
        .into_iter()
        .map(|project| project.name)
        .collect();
    assert_eq!(names, ["Prospecting", "Archive"]);
}

#[tokio::test]
async fn names_are_unique_ignoring_case_and_projects_are_workspace_scoped() {
    let (_directory, database) = database().await;
    must(create_project(&database, LOCAL_WORKSPACE_ID, &new("Prospecting"), at(0)).await);
    assert!(matches!(
        create_project(&database, LOCAL_WORKSPACE_ID, &new("prospecting"), at(1)).await,
        Err(DatabaseError::ProjectNameTaken)
    ));
    assert!(matches!(
        find_project(&database, "another-workspace", "Prospecting").await,
        Err(DatabaseError::ProjectNotFound)
    ));
}

#[tokio::test]
async fn unacceptable_fields_are_refused() {
    let (_directory, database) = database().await;
    for name in ["", "   ", &"n".repeat(81), "a\nb"] {
        assert!(matches!(
            create_project(&database, LOCAL_WORKSPACE_ID, &new(name), at(0)).await,
            Err(DatabaseError::InvalidProject { field: "name" })
        ));
    }
    // A folder that could leave the granted root fails closed.
    for folder in [
        "../secrets",
        "a/../../b",
        "/etc",
        "\\share",
        "C:\\Windows",
        "d:x",
    ] {
        let project = NewProject {
            folder: folder.to_owned(),
            ..new("P")
        };
        assert!(
            matches!(
                create_project(&database, LOCAL_WORKSPACE_ID, &project, at(0)).await,
                Err(DatabaseError::InvalidProject { field: "folder" })
            ),
            "{folder} must be refused"
        );
    }
    let long = NewProject {
        guidance: "g".repeat(12_001),
        ..new("P")
    };
    assert!(matches!(
        create_project(&database, LOCAL_WORKSPACE_ID, &long, at(0)).await,
        Err(DatabaseError::InvalidProject { field: "guidance" })
    ));
    for over in [MAX_DAILY_TOKEN_LIMIT + 1, u32::MAX] {
        let capped = NewProject {
            daily_token_limit: over,
            ..new("P")
        };
        assert!(matches!(
            create_project(&database, LOCAL_WORKSPACE_ID, &capped, at(0)).await,
            Err(DatabaseError::InvalidProject {
                field: "daily_token_limit"
            })
        ));
    }
    let ok = NewProject {
        folder: "a\\b\\".to_owned(),
        ..new("P")
    };
    assert_eq!(
        must(create_project(&database, LOCAL_WORKSPACE_ID, &ok, at(0)).await).folder,
        "a/b"
    );
}

#[tokio::test]
async fn update_changes_only_the_given_fields() {
    let (_directory, database) = database().await;
    let made = must(create_project(&database, LOCAL_WORKSPACE_ID, &new("P"), at(0)).await);
    let changed = must(
        update_project(
            &database,
            LOCAL_WORKSPACE_ID,
            "P",
            &ProjectChanges {
                goal: Some("New goal".to_owned()),
                status: Some(ProjectStatus::Paused),
                ..ProjectChanges::default()
            },
            at(5),
        )
        .await,
    );
    assert_eq!(changed.goal, "New goal");
    assert_eq!(changed.status, ProjectStatus::Paused);
    assert_eq!(changed.guidance, made.guidance);
    assert_ne!(changed.updated_at, made.updated_at);
}

#[tokio::test]
async fn the_journal_keeps_the_latest_notes_oldest_first_and_refuses_bad_entries() {
    let (_directory, database) = database().await;
    let project = must(create_project(&database, LOCAL_WORKSPACE_ID, &new("P"), at(0)).await);
    for index in 0..5 {
        must(
            add_project_note(
                &database,
                &project.id,
                NoteKind::Progress,
                &format!("step {index}"),
                Some("run-1"),
                at(index),
            )
            .await,
        );
    }
    let notes = must(recent_project_notes(&database, &project.id, 3).await);
    let bodies: Vec<&str> = notes.iter().map(|note| note.body.as_str()).collect();
    assert_eq!(bodies, ["step 2", "step 3", "step 4"]);
    assert_eq!(notes[0].run_id.as_deref(), Some("run-1"));
    for body in ["", "  ", &"x".repeat(MAX_NOTE_CHARS + 1)] {
        assert!(matches!(
            add_project_note(&database, &project.id, NoteKind::Next, body, None, at(9)).await,
            Err(DatabaseError::InvalidProject { field: "note" })
        ));
    }
}

#[tokio::test]
async fn links_resolve_repeat_and_never_move_silently() {
    let (_directory, database) = database().await;
    let one = must(create_project(&database, LOCAL_WORKSPACE_ID, &new("One"), at(0)).await);
    let two = must(create_project(&database, LOCAL_WORKSPACE_ID, &new("Two"), at(0)).await);
    assert!(must(project_for(&database, LinkKind::Session, SESSION).await).is_none());
    must(link_project(&database, LinkKind::Session, SESSION, &one.id).await);
    must(link_project(&database, LinkKind::Session, SESSION, &one.id).await);
    assert!(matches!(
        link_project(&database, LinkKind::Session, SESSION, &two.id).await,
        Err(DatabaseError::ProjectConflict)
    ));
    assert_eq!(
        must(project_for(&database, LinkKind::Session, SESSION).await).map(|p| p.id),
        Some(one.id.clone())
    );
    // The same id as a schedule is a different link.
    assert!(must(project_for(&database, LinkKind::Schedule, SESSION).await).is_none());
}

#[tokio::test]
async fn deleting_a_project_removes_its_notes_and_links() {
    let (_directory, database) = database().await;
    let project = must(create_project(&database, LOCAL_WORKSPACE_ID, &new("P"), at(0)).await);
    must(
        add_project_note(
            &database,
            &project.id,
            NoteKind::Owner,
            "hello",
            None,
            at(1),
        )
        .await,
    );
    must(link_project(&database, LinkKind::Schedule, SCHEDULE, &project.id).await);
    must(delete_project(&database, LOCAL_WORKSPACE_ID, "P").await);
    assert!(must(recent_project_notes(&database, &project.id, 10).await).is_empty());
    assert!(must(project_for(&database, LinkKind::Schedule, SCHEDULE).await).is_none());
    assert!(matches!(
        delete_project(&database, LOCAL_WORKSPACE_ID, "P").await,
        Err(DatabaseError::ProjectNotFound)
    ));
}

#[tokio::test]
async fn a_schedule_can_be_refiled_into_another_project_or_out_of_all() {
    let (_directory, database) = database().await;
    let one = must(create_project(&database, LOCAL_WORKSPACE_ID, &new("One"), at(0)).await);
    let two = must(create_project(&database, LOCAL_WORKSPACE_ID, &new("Two"), at(0)).await);
    must(set_project_link(&database, LinkKind::Schedule, SCHEDULE, Some(&one.id)).await);
    must(set_project_link(&database, LinkKind::Schedule, SCHEDULE, Some(&two.id)).await);
    let now_in = must(project_for(&database, LinkKind::Schedule, SCHEDULE).await);
    assert_eq!(now_in.map(|p| p.name), Some("Two".to_owned()));
    must(set_project_link(&database, LinkKind::Schedule, SCHEDULE, None).await);
    assert!(must(project_for(&database, LinkKind::Schedule, SCHEDULE).await).is_none());
    // Taking out of a project it was never in is fine.
    must(set_project_link(&database, LinkKind::Schedule, SCHEDULE, None).await);
}

#[tokio::test]
async fn earlier_important_notes_skip_routine_progress_and_stop_at_the_window() {
    let (_directory, database) = database().await;
    let project = must(create_project(&database, LOCAL_WORKSPACE_ID, &new("P"), at(0)).await);
    let mut ids = Vec::new();
    for (index, (kind, body)) in [
        (NoteKind::Progress, "routine one"),
        (NoteKind::Decision, "chose Dutch list"),
        (NoteKind::Progress, "routine two"),
        (NoteKind::Blocker, "needs a login"),
        (NoteKind::Progress, "recent"),
    ]
    .into_iter()
    .enumerate()
    {
        let made = must(
            add_project_note(
                &database,
                &project.id,
                kind,
                body,
                None,
                at(i64::try_from(index).unwrap_or(0).into()),
            )
            .await,
        );
        ids.push(made.id);
    }
    let earlier = must(earlier_important_notes(&database, &project.id, &ids[4], 10).await);
    let bodies: Vec<&str> = earlier.iter().map(|note| note.body.as_str()).collect();
    assert_eq!(bodies, ["chose Dutch list", "needs a login"]);
    let none = must(earlier_important_notes(&database, &project.id, &ids[1], 10).await);
    assert!(
        none.is_empty(),
        "nothing important precedes the first decision"
    );
}
