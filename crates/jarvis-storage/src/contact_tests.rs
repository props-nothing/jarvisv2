//! Contact storage: saving is an upsert, search is literal, and bad input is refused.

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
            std::env::temp_dir().join(format!("jarvis-contacts-{}", jarvis_core::scratch_tag()));
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

fn lead(company: &str, person: &str, email: &str) -> ContactInput {
    ContactInput {
        company: Some(company.to_owned()),
        person: Some(person.to_owned()),
        email: (!email.is_empty()).then(|| email.to_owned()),
        ..ContactInput::default()
    }
}

async fn save(
    database: &SqliteDatabase,
    input: &ContactInput,
    second: i128,
) -> (StoredContact, bool) {
    must(save_contact(database, LOCAL_WORKSPACE_ID, None, input, true, at(second)).await)
}

#[tokio::test]
async fn saving_the_same_lead_twice_updates_it_by_address_or_by_company_and_person() {
    let (_directory, database) = database().await;
    let (first, created) = save(&database, &lead("Acme BV", "Eva Jansen", "eva@acme.nl"), 0).await;
    assert!(created && first.status == ContactStatus::New);

    let update = ContactInput {
        email: Some("EVA@acme.nl".to_owned()),
        status: Some(ContactStatus::Contacted),
        notes: Some("Sent the intro.".to_owned()),
        ..ContactInput::default()
    };
    let (again, created) = save(&database, &update, 5).await;
    assert!(!created, "the address is matched without regard to case");
    assert_eq!(
        (again.id.as_str(), again.status),
        (first.id.as_str(), ContactStatus::Contacted)
    );
    assert_eq!(
        (again.company.as_str(), again.person.as_str()),
        ("Acme BV", "Eva Jansen")
    );
    assert_eq!(again.notes, "Sent the intro.");
    assert_ne!(again.updated_at, again.created_at);

    let (bare, created) = save(&database, &lead("Beta", "Tom", ""), 6).await;
    assert!(created);
    let (found, created) = save(&database, &lead("BETA", "tom", "tom@beta.nl"), 7).await;
    assert!(
        !created,
        "company and person match a contact that has no address yet"
    );
    assert_eq!((found.id, found.email.as_str()), (bare.id, "tom@beta.nl"));
    let stats = must(contact_stats(&database, LOCAL_WORKSPACE_ID).await);
    assert_eq!(stats.iter().map(|(_, held)| held).sum::<u32>(), 2);
}

#[tokio::test]
async fn a_new_contact_needs_a_company_and_every_field_is_bounded() {
    let (_directory, database) = database().await;
    for (input, field) in [
        (ContactInput::default(), "company"),
        (lead("", "x", ""), "company"),
        (lead("Acme", "a\u{0007}b", ""), "person"),
        (lead(&"x".repeat(201), "", ""), "company"),
        (lead("Acme", "", "not an address"), "email"),
        (lead("Acme", "", "a b@c.nl"), "email"),
    ] {
        let refused = save_contact(&database, LOCAL_WORKSPACE_ID, None, &input, false, at(0)).await;
        assert!(
            matches!(refused, Err(DatabaseError::InvalidContact { field: named }) if named == field),
            "{field}: {refused:?}"
        );
    }
    let notes = ContactInput {
        notes: Some("line one\nline two".to_owned()),
        ..lead("Acme", "", "")
    };
    assert_eq!(
        save(&database, &notes, 1).await.0.notes,
        "line one\nline two",
        "notes may span lines"
    );
}

#[tokio::test]
async fn search_matches_text_literally_and_filters_by_status() {
    let (_directory, database) = database().await;
    save(&database, &lead("Acme BV", "Eva", "eva@acme.nl"), 0).await;
    let done = ContactInput {
        status: Some(ContactStatus::DoNotContact),
        ..lead("100% Pure", "Tom", "tom@pure.nl")
    };
    save(&database, &done, 1).await;

    let find = |query: Option<&'static str>, status| {
        let database = &database;
        async move { must(search_contacts(database, LOCAL_WORKSPACE_ID, query, status, 50).await) }
    };
    assert_eq!(find(Some("acme"), None).await.len(), 1);
    assert_eq!(
        find(Some("EVA@"), None).await.len(),
        1,
        "case does not matter"
    );
    assert_eq!(
        find(Some("100%"), None).await.len(),
        1,
        "a percent sign is a character, not a wildcard"
    );
    assert!(find(Some("%"), Some(ContactStatus::New)).await.is_empty());
    assert_eq!(
        find(None, Some(ContactStatus::DoNotContact)).await[0].company,
        "100% Pure"
    );
    assert_eq!(find(None, None).await.len(), 2);
    assert_eq!(
        find(Some("   "), None).await.len(),
        2,
        "a blank query is no query"
    );
}

#[tokio::test]
async fn a_contact_can_be_deleted_and_stats_count_every_status() {
    let (_directory, database) = database().await;
    let (made, _) = save(&database, &lead("Acme", "", ""), 0).await;
    let stats = must(contact_stats(&database, LOCAL_WORKSPACE_ID).await);
    assert_eq!(
        stats.len(),
        ContactStatus::ALL.len(),
        "empty statuses are listed too"
    );
    assert_eq!(stats[0], (ContactStatus::New, 1));
    assert!(must(
        delete_contact(&database, LOCAL_WORKSPACE_ID, &made.id).await
    ));
    assert!(!must(
        delete_contact(&database, LOCAL_WORKSPACE_ID, &made.id).await
    ));
}

#[tokio::test]
async fn only_the_owner_can_lift_do_not_contact() {
    let (_directory, database) = database().await;
    let barred = ContactInput {
        status: Some(ContactStatus::DoNotContact),
        ..lead("Acme", "Eva", "eva@acme.nl")
    };
    save(&database, &barred, 0).await;
    let revive = ContactInput {
        status: Some(ContactStatus::New),
        ..lead("Acme", "Eva", "eva@acme.nl")
    };
    let refused = save_contact(&database, LOCAL_WORKSPACE_ID, None, &revive, false, at(1)).await;
    assert!(
        matches!(
            refused,
            Err(DatabaseError::InvalidContact { field: "status" })
        ),
        "{refused:?}"
    );
    let notes = ContactInput {
        notes: Some("asked not to be contacted".to_owned()),
        ..lead("Acme", "Eva", "eva@acme.nl")
    };
    let (kept, _) =
        must(save_contact(&database, LOCAL_WORKSPACE_ID, None, &notes, false, at(2)).await);
    assert_eq!(
        kept.status,
        ContactStatus::DoNotContact,
        "other changes do not touch the status"
    );
    let (lifted, _) = save(&database, &revive, 3).await;
    assert_eq!(lifted.status, ContactStatus::New, "the owner can");
}
