//! Tests for connecting an account, and for resuming from a stored cursor.
//!
//! The load-bearing one is the **duplicate refusal**, because the cost it prevents is invisible: two accounts
//! for one mailbox look exactly like two mailboxes until a push delivery cannot be routed.
//!
//! Falsification record in `TODO.md`.

use super::*;
use crate::google::pubsub::PubsubNotification;
use crate::google::request::GmailProfile;
use crate::google::routing::{DeliveryRoute, route_delivery};

fn must<T, E: std::fmt::Display>(result: Result<T, E>, what: &str) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("{what}: {error}"),
    }
}

fn must_err<T, E>(result: Result<T, E>, what: &str) -> E {
    match result {
        Ok(_) => panic!("{what}: the call unexpectedly succeeded"),
        Err(error) => error,
    }
}

fn reference(value: &str) -> AccountReference {
    must(AccountReference::new(value), "a valid account reference")
}

fn instant(seconds: i64) -> UtcTimestamp {
    must(
        UtcTimestamp::from_unix_nanos(i128::from(seconds) * 1_000_000_000),
        "a representable instant",
    )
}

fn profile(address: &str) -> GmailProfile {
    GmailProfile {
        email_address: address.to_owned(),
        history_id: Some("1234567890".to_owned()),
    }
}

fn scopes() -> Vec<String> {
    vec!["https://www.googleapis.com/auth/gmail.readonly".to_owned()]
}

fn connect(
    reference_value: &str,
    address: &str,
    existing: &[VerifiedAccount],
) -> Result<VerifiedAccount, ConnectError> {
    establish_account(
        reference(reference_value),
        &profile(address),
        AuthMethod::OAuthPkce,
        &scopes(),
        instant(1_700_000_000),
        existing,
    )
}

#[test]
fn connecting_a_new_mailbox_stores_the_providers_own_identity() {
    // The whole point of the read `ADR-0096` added: the address becomes the account's `provider_account_id`,
    // which is the value a push delivery is later matched against. So this call is what makes routing able to
    // find the account at all.
    let account = must(
        connect("acct-1", "person@example.invalid", &[]),
        "a first account",
    );
    assert_eq!(account.provider_account_id(), "person@example.invalid");
    assert_eq!(account.reference(), &reference("acct-1"));
    assert_eq!(account.method(), AuthMethod::OAuthPkce);
    assert_eq!(account.scopes(), scopes().as_slice());
    assert_eq!(account.verified_at(), instant(1_700_000_000));
    // **No display name, because `users.getProfile` returns none.** Passing the address as one would invent a
    // provider statement — the field `VerifiedAccount` exists to keep honest.
    assert_eq!(account.display_name(), None);

    // And the account it produced is routable, which is the property the whole path needs: a notification
    // naming that address resolves to exactly this account.
    let route = route_delivery(
        &PubsubNotification {
            email_address: "person@example.invalid".to_owned(),
            history_id: "9".to_owned(),
        },
        std::slice::from_ref(&account),
    );
    assert_eq!(route, DeliveryRoute::Exact(reference("acct-1")));
}

#[test]
fn a_second_account_for_the_same_address_is_refused_because_it_breaks_routing() {
    // **The finding, as a test.** `ADR-0097` named `DeliveryRoute::Ambiguous` as reachable when a reconnect
    // mints a new reference without retiring the old row — and this is the step where that is either prevented
    // or created. A duplicate is refused because of what it costs: routing stops being decidable, so every
    // notification for that mailbox becomes unactionable, while the duplicate itself is **invisible** (two
    // cursors, two schedules, a quota budget paid twice look exactly like two mailboxes).
    let existing = [must(
        connect("acct-1", "person@example.invalid", &[]),
        "a first account",
    )];
    let error = must_err(
        connect("acct-2", "person@example.invalid", &existing),
        "a duplicate address must be refused",
    );
    // The refusal names the **holder**, which is what makes it actionable — "this address is taken" without
    // saying by what leaves an operator searching.
    assert_eq!(error.holder(), Some(&reference("acct-1")));
    assert_eq!(
        error,
        ConnectError::AddressAlreadyConnected {
            holder: reference("acct-1")
        }
    );

    // **And the refusal is what keeps routing single-valued — asserted through the router rather than as a
    // claim.** Had the duplicate been created, this same notification would be `Ambiguous` and would require a
    // person. The test walks that counterfactual by constructing the duplicate directly, so the consequence is
    // demonstrated rather than described.
    let duplicate = [existing[0].clone(), existing[0].clone()];
    assert_eq!(
        route_delivery(
            &PubsubNotification {
                email_address: "person@example.invalid".to_owned(),
                history_id: "9".to_owned(),
            },
            &duplicate,
        ),
        DeliveryRoute::Ambiguous { accounts: 2 },
        "a duplicate that was allowed would make the delivery unroutable"
    );
}

#[test]
fn the_duplicate_check_ignores_case_and_that_is_the_opposite_direction_from_routing() {
    // **The two conservative choices point in opposite directions, and this test asserts both against one pair
    // of spellings so a change to either shows up as a contradiction rather than as silent drift.** Routing
    // refuses to *act* on a case-only near-match; connecting refuses to *create* one. Both are the same
    // restraint — neither acts on an uncertain case-match — because a person being asked is recoverable where a
    // wrong automatic action is not.
    let existing = [must(
        connect("acct-1", "person@example.invalid", &[]),
        "a first account",
    )];
    // Connecting a differently-cased spelling of the SAME address is refused, so the duplicate cannot be made.
    let error = must_err(
        connect("acct-2", "Person@Example.Invalid", &existing),
        "a case-only variant must not create a second account",
    );
    assert_eq!(
        error.holder(),
        Some(&reference("acct-1")),
        "the refusal must name the account already holding the address"
    );

    // And routing the SAME spelling does not act either — it reports, and waits for a person.
    let route = route_delivery(
        &PubsubNotification {
            email_address: "Person@Example.Invalid".to_owned(),
            history_id: "9".to_owned(),
        },
        &existing,
    );
    assert_eq!(route, DeliveryRoute::CaseDiffers { accounts: 1 });
    assert!(
        !route.may_be_applied_automatically(),
        "routing must not act on the near-match it refuses to create"
    );
}

#[test]
fn an_identity_the_provider_returns_but_this_platform_cannot_store_is_its_own_refusal() {
    // The two refusals have different **subjects**: one is about the account set and one about the identity.
    // Reporting an unusable identifier as a duplicate would send an operator looking for an account that does
    // not exist. The reference bound is 256 characters, so an oversized address is the reachable case.
    let oversized = format!("{}@example.invalid", "a".repeat(300));
    let error = must_err(
        connect("acct-1", &oversized, &[]),
        "an identity this platform cannot store must be refused",
    );
    assert!(
        matches!(error, ConnectError::IdentityUnusable { .. }),
        "an unstorable identity must be its own refusal, not a duplicate: {error:?}"
    );
    // **No holder is reported**, because there is none — returning a fabricated one to make the signature
    // uniform is exactly what the accessor exists to avoid saying.
    assert_eq!(error.holder(), None);
    // And the reason names the bound rather than the address, so the diagnostic does not print a person's
    // mailbox — the value `VerifiedAccount`'s own `Debug` redacts (`ADR-0091`).
    let rendered = error.to_string();
    assert!(
        !rendered.contains("aaaaaaaa"),
        "the refusal must not print the address it could not store: {rendered}"
    );
}

#[test]
fn a_cursor_with_a_position_resumes_incrementally_and_names_the_kind_with_it() {
    // A position and its **kind** travel together, so a caller cannot take the token without seeing what may be
    // concluded from it: a `MonotonicMarker` has detectable staleness and a defined recovery, while an
    // `OpaqueToken` may not be validated at all and only the provider's refusal is authoritative.
    let gmail = must(
        SyncCursor::new(
            SyncCursorKind::MonotonicMarker,
            Some("1234567890".to_owned()),
            reference("acct-1"),
            "1.0.0",
            instant(1_700_000_000),
        ),
        "a Gmail cursor",
    );
    let point = resume_from(&gmail);
    assert_eq!(point.position(), Some("1234567890"));
    assert!(!point.requires_full_sync());
    assert_eq!(
        point,
        ResumePoint::FromPosition {
            position: "1234567890".to_owned(),
            kind: SyncCursorKind::MonotonicMarker
        }
    );

    // Calendar's is opaque, and the kind that arrives is Calendar's — so a caller does not have to remember
    // which API it is looking at to know whether the position can be checked.
    let calendar = must(
        SyncCursor::new(
            SyncCursorKind::OpaqueToken,
            Some("CAESAB".to_owned()),
            reference("acct-1"),
            "1.0.0",
            instant(1_700_000_000),
        ),
        "a Calendar cursor",
    );
    assert_eq!(
        resume_from(&calendar),
        ResumePoint::FromPosition {
            position: "CAESAB".to_owned(),
            kind: SyncCursorKind::OpaqueToken
        }
    );
}

#[test]
fn a_start_cursor_is_a_full_sync_decision_rather_than_a_missing_value() {
    // `SyncCursorKind::Start`'s doc is explicit that a full sync "is a decision with consequences (cost, time,
    // possibly a rate-limit budget), and an absent value would make it the default a caller stumbles into." So
    // the cursor kind is reported as the decision it is, and **both** of its documented causes land here:
    // "the connector has never synced, or its position was discarded".
    let start = must(
        SyncCursor::new(
            SyncCursorKind::Start,
            None,
            reference("acct-1"),
            "1.0.0",
            instant(1_700_000_000),
        ),
        "a start cursor",
    );
    let point = resume_from(&start);
    assert_eq!(point, ResumePoint::FullSync);
    assert!(point.requires_full_sync());
    assert_eq!(point.position(), None);
    // And a fresh account and a discarded position are deliberately **the same** answer, because the cursor
    // carries nothing that separates them and the caller that discarded the position already knows why — the
    // restraint `ADR-0067` records for a status whose cause the provider publishes no code for.
    assert_eq!(
        resume_from(&start),
        resume_from(&must(
            SyncCursor::new(
                SyncCursorKind::Start,
                None,
                reference("acct-2"),
                "1.0.0",
                instant(1_800_000_000),
            ),
            "another start cursor",
        )),
        "a never-synced account and a discarded position must resume identically"
    );
}
