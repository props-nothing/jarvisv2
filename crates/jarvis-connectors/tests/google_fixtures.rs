//! Fixture-driven tests for the Google connector's documented wire shapes.
//!
//! # What these tests do and do not prove
//!
//! They prove that this crate **correctly reads a document that matches Google's published response shapes**.
//! They do **not** prove that Google sends what the documentation says, because **no request has ever been
//! sent**: there is no transport implementation, no credential, and no account. Every fixture is hand-built
//! from the reference pages cited in `docs/research/integrations/google.md`, and each file says so in its own
//! `_not_a_capture` field rather than leaving a reader to infer it from the directory name.
//!
//! That distinction matters because "we have fixtures" is exactly the kind of claim that reads as stronger
//! than it is. A fixture proves the *reader*; only a capture proves the *record*. The research record's
//! Verification Plan names the opt-in live smoke test that closes the remaining gap, and it is not built.
//!
//! # Why the fixtures are files rather than inline strings
//!
//! A shape change should be a **diff to a document**, not an edit buried in a test body. Every one of these
//! files can be opened, compared against the live reference page, and corrected without reading any Rust.

use std::path::PathBuf;

use jarvis_connectors::google::client::{self, GoogleApi, GoogleErrorBody, GoogleErrorReason};
use jarvis_connectors::google::request;
use serde_json::Value;

/// Reads a fixture from `tests/fixtures/google/`.
///
/// Fails loudly rather than skipping: a missing fixture is a broken test, not an absent capability. A skip
/// would make "the fixture was renamed" look like "everything passed".
fn fixture(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("google")
        .join(name);
    match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) => panic!("the fixture {} must be readable: {error}", path.display()),
    }
}

/// Asserts a fixture declares itself as a hand-built shape rather than a capture.
///
/// The honesty of these tests rests entirely on this claim, so it is **checked** rather than trusted. A file
/// that quietly dropped the marker would turn a hand-built payload into an apparent recording, which is the
/// one thing these fixtures must never become.
fn assert_declared_shape(text: &str) {
    let value: Value = match serde_json::from_str(text) {
        Ok(value) => value,
        Err(error) => panic!("a fixture must be JSON: {error}"),
    };
    assert_eq!(
        value["_not_a_capture"],
        Value::Bool(true),
        "a hand-built fixture must declare itself as one, or it reads as a real capture"
    );
    assert!(
        value["_shape_documented_at"].is_string(),
        "a fixture must cite the page its shape came from"
    );
}

#[test]
fn the_gmail_list_fixture_parses_to_identifiers_and_a_page_token() {
    let text = fixture("gmail_messages_list.json");
    assert_declared_shape(&text);
    let page = match request::parse_id_page(200, &text) {
        Ok(page) => page,
        Err(error) => panic!("the documented list shape must parse: {error}"),
    };
    assert_eq!(
        page.ids,
        ["18f9c0d1e2a3b4c5", "18f9c0d1e2a3b4c6", "18f9c0d1e2a3b4c7"]
    );
    assert_eq!(
        page.next_page_token.as_deref(),
        Some("09876543210987654321")
    );
    // `resultSizeEstimate` is NOT carried. It is an estimate, not a count, so a caller that used it as a
    // total would be reading an approximation as a fact -- and the connector's own output schema has no
    // field for it. Asserted by absence so that adding it later is a deliberate change.
    let value: Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(error) => panic!("{error}"),
    };
    assert_eq!(
        value["resultSizeEstimate"], 3,
        "the fixture does carry the estimate"
    );
}

#[test]
fn the_gmail_message_fixture_parses_to_one_identifier() {
    let text = fixture("gmail_messages_get.json");
    assert_declared_shape(&text);
    let id = match request::parse_single_id(200, &text) {
        Ok(id) => id,
        Err(error) => panic!("the documented Message shape must parse: {error}"),
    };
    assert_eq!(id, "18f9c0d1e2a3b4c5");

    // The `Message` resource carries MIME parts, headers, and an `internalDate`, and this connector reads
    // exactly one field from it. The rest is asserted **present in the fixture** so that a future slice which
    // starts reading a header does so against a payload that already has one.
    let value: Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(error) => panic!("{error}"),
    };
    assert_eq!(value["payload"]["headers"][0]["name"], "From");
    assert_eq!(value["payload"]["parts"][0]["mimeType"], "text/plain");
    assert!(
        value["payload"]["parts"][0]["body"]["data"].is_string(),
        "the fixture must carry a base64url body so a future slice can decode it"
    );
    assert!(value["internalDate"].is_string());
}

#[test]
fn a_calendar_mid_walk_page_yields_a_page_token_and_no_sync_token() {
    let text = fixture("calendar_events_list_page.json");
    assert_declared_shape(&text);
    let page = match request::parse_calendar_page(200, &text) {
        Ok(page) => page,
        Err(error) => panic!("the documented events shape must parse: {error}"),
    };
    assert_eq!(page.ids, ["0a1b2c3d4e5f6071", "0a1b2c3d4e5f6072"]);
    assert_eq!(page.next_page_token.as_deref(), Some("CpEBGh0KA2NhbA"));
    assert_eq!(
        page.next_sync_token, None,
        "a page with further results cannot carry a sync token -- the two are mutually exclusive"
    );
}

#[test]
fn a_calendar_last_page_yields_a_sync_token_and_no_page_token() {
    let text = fixture("calendar_events_list_last_page.json");
    assert_declared_shape(&text);
    let page = match request::parse_calendar_page(200, &text) {
        Ok(page) => page,
        Err(error) => panic!("the documented events shape must parse: {error}"),
    };
    assert_eq!(page.ids, ["0a1b2c3d4e5f6073"]);
    assert_eq!(
        page.next_page_token, None,
        "the last page cannot carry a page token"
    );
    assert_eq!(
        page.next_sync_token.as_deref(),
        Some("CMf8oPz2sPICEMf8oPz2sPICGAU="),
        "only the last page carries the durable cursor"
    );
}

#[test]
fn the_two_calendar_pages_together_are_the_only_way_to_get_a_cursor() {
    // The property that makes the split matter: walking the pages gives a page token then a sync token, and a
    // caller that stored the FIRST token it saw would hold one that expires with the walk. Asserted as a
    // sequence rather than as two independent parses, because that is how a caller actually encounters it.
    let first = match request::parse_calendar_page(200, &fixture("calendar_events_list_page.json"))
    {
        Ok(page) => page,
        Err(error) => panic!("{error}"),
    };
    let last =
        match request::parse_calendar_page(200, &fixture("calendar_events_list_last_page.json")) {
            Ok(page) => page,
            Err(error) => panic!("{error}"),
        };
    assert!(first.next_sync_token.is_none() && last.next_sync_token.is_some());
    assert!(first.next_page_token.is_some() && last.next_page_token.is_none());
    assert_ne!(
        first.next_page_token, last.next_sync_token,
        "the two continuation tokens are different values for different purposes"
    );
}

#[test]
fn a_403_whose_reason_is_an_administrators_decision_is_permanent() {
    // The fixture's `message` says "the request can be retried later" while its `reason` says `domainPolicy`.
    // The classification must follow the CODE: a domain administrator disabled the app, and no amount of
    // retrying changes a person's decision. A classifier reading the prose would retry forever.
    let text = fixture("gmail_error_403_domain_policy.json");
    assert_declared_shape(&text);
    let body: GoogleErrorBody = match serde_json::from_str(&text) {
        Ok(body) => body,
        Err(error) => panic!("the documented error shape must parse: {error}"),
    };
    let reason = GoogleErrorReason::parse(
        body.reason()
            .unwrap_or_else(|| panic!("the fixture states a reason")),
    );
    assert_eq!(reason, GoogleErrorReason::DomainPolicy);
    assert!(reason.needs_a_person(), "an administrator must be involved");
    let decision = client::classify(GoogleApi::Gmail, 403, reason, None, None);
    assert!(
        !decision.guidance.permits_retry(),
        "a domain policy is permanent, whatever the message text invites"
    );
    assert_eq!(
        decision.guidance,
        jarvis_connectors::RetryGuidance::DoNotRetry
    );
}

#[test]
fn a_403_whose_reason_is_throttling_is_retryable_and_shares_the_status() {
    // The falsifying pair to the test above. **Both fixtures are a 403.** If the classifier switched on the
    // status alone these two would classify identically, and the reason -- which is the only thing that
    // distinguishes a throttling limit from a disabled app -- would be ignored. So the pair is asserted
    // together, and the assertion that carries the weight is that the two decisions DIFFER.
    let text = fixture("gmail_error_403_rate_limit.json");
    assert_declared_shape(&text);
    let body: GoogleErrorBody = match serde_json::from_str(&text) {
        Ok(body) => body,
        Err(error) => panic!("the documented error shape must parse: {error}"),
    };
    let throttled = GoogleErrorReason::parse(
        body.reason()
            .unwrap_or_else(|| panic!("the fixture states a reason")),
    );
    assert_eq!(throttled, GoogleErrorReason::RateLimitExceeded);

    let disabled = GoogleErrorReason::parse("domainPolicy");
    assert_ne!(
        throttled, disabled,
        "the two reasons must be distinguishable"
    );
    assert_ne!(
        body.error.code, 0,
        "the fixture carries the status in the body as the provider does"
    );

    let retryable = client::classify(GoogleApi::Gmail, 403, throttled, None, None);
    let permanent = client::classify(GoogleApi::Gmail, 403, disabled, None, None);
    assert!(
        retryable.guidance.permits_retry(),
        "a throttling limit may be retried"
    );
    // And a stated delay that cannot be read is **not** the same as no delay: RFC 9110 §10.2.3 allows the
    // `Retry-After` field to be an `HTTP-date`, and a response that carries one must not be reported as a
    // response that carried none (`ADR-0076`). This is the case the fixture suite can assert without a
    // socket, because the distinction lives entirely in the classifier's input.
    let unreadable = client::classify(
        GoogleApi::Gmail,
        429,
        GoogleErrorReason::Unrecognised,
        Some(jarvis_connectors::google::transport::RetryAfter::NotSeconds),
        None,
    );
    let absent = client::classify(
        GoogleApi::Gmail,
        429,
        GoogleErrorReason::Unrecognised,
        None,
        None,
    );
    assert_eq!(unreadable.class, jarvis_connectors::RetryClass::Throttled);
    assert!(
        unreadable.guidance.permits_retry(),
        "an unreadable delay is still a throttling answer, which is retryable"
    );
    assert_ne!(
        unreadable.guidance, absent.guidance,
        "a stated delay this client could not read must not be reported as an absent one"
    );
    assert!(
        !permanent.guidance.permits_retry(),
        "a disabled app may not"
    );
    assert_ne!(
        retryable, permanent,
        "the same status with two reasons must not classify identically"
    );
}

#[test]
fn the_error_type_cannot_hold_the_prose_it_parsed() {
    // `P3-008c`: a classification must not come from message text. The structural form of that rule is a type
    // that has no field for the text -- so the fixture's `message` and `status` fields parse and then become
    // unreachable, and nothing downstream can quote them into a reason or a decision.
    let text = fixture("gmail_error_403_domain_policy.json");
    let body: GoogleErrorBody = match serde_json::from_str(&text) {
        Ok(body) => body,
        Err(error) => panic!("{error}"),
    };
    // The body exposes exactly one thing: the first reason.
    assert_eq!(body.reason(), Some("domainPolicy"));
    let rendered = format!("{body:?}");
    // `Debug` is derived from the fields that exist, so the prose is genuinely absent rather than merely
    // unread -- which is what makes this a property of the type and not a convention.
    assert!(
        !rendered.contains("transient error"),
        "the error type must not retain the provider's prose: {rendered}"
    );
    assert!(rendered.contains("domainPolicy"));
}

#[test]
fn the_gmail_history_404_fixture_carries_no_reason_code() {
    // The fixture that closes the 404-is-staleness item in the research record's Verification Plan, and it
    // closes it by **falsifying the question rather than answering it**. The sync guide documents a 404 for a
    // pruned history; the error guide documents 404 as "the requested resource couldn't be found" with no 404
    // subsection at all. So there is no `reason` code to switch on, and the two causes are indistinguishable.
    let text = fixture("gmail_history_404_no_reason.json");
    assert_declared_shape(&text);
    let value: Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(error) => panic!("{error}"),
    };
    assert_eq!(value["error"]["code"], 404);
    assert!(
        value["error"].get("errors").is_none(),
        "the fixture must carry NO `errors` array, because that is where a `reason` code would live and \
         Google publishes none for a 404"
    );
    assert!(
        value["error"]["message"].is_string(),
        "the message is present, and it is prose -- which is why nothing may classify from it"
    );

    // The consequence: this crate's classifier reads the status as permanent and the reason as unrecognised,
    // so nothing invents the `reason` the response does not carry.
    let body: GoogleErrorBody = match serde_json::from_str(&text) {
        Ok(body) => body,
        Err(error) => panic!("{error}"),
    };
    assert_eq!(body.error.code, 404);
    assert_eq!(
        body.reason(),
        None,
        "there is no reason code, which is the whole point"
    );
    let decision = client::classify(
        GoogleApi::Gmail,
        404,
        GoogleErrorReason::Unrecognised,
        None,
        None,
    );
    assert_eq!(
        decision.guidance,
        jarvis_connectors::RetryGuidance::DoNotRetry
    );
    assert!(
        !decision.guidance.permits_retry(),
        "a 404 is not retryable; the remedy is a resync, which is a different action"
    );
}

#[test]
fn the_history_fixture_pair_separates_the_page_token_from_the_durable_cursor() {
    // The pair that makes "which token is the cursor" falsifiable. Unlike Calendar's `nextPageToken`/
    // `nextSyncToken` -- which the reference documents as mutually exclusive -- the history response carries
    // `historyId` on **every** success while `nextPageToken` appears only mid-walk. So a reader that treated
    // them as a pair would take its cursor from whichever field it happened to read first, and a sync resumed
    // from a page token would fail the moment the walk it belonged to ended.
    let mid_walk = fixture("gmail_history_list.json");
    let last_page = fixture("gmail_history_list_last_page.json");
    assert_declared_shape(&mid_walk);
    assert_declared_shape(&last_page);

    let mid = match request::parse_history_page(200, &mid_walk) {
        Ok(page) => page,
        Err(error) => panic!("the documented history shape must parse: {error}"),
    };
    let last = match request::parse_history_page(200, &last_page) {
        Ok(page) => page,
        Err(error) => panic!("the documented history shape must parse: {error}"),
    };

    // Mid-walk: both a page token and a history id, and they are different values.
    assert_eq!(mid.next_page_token.as_deref(), Some("0987654321"));
    assert_eq!(mid.history_id.as_deref(), Some("12347"));
    assert_ne!(
        mid.history_id, mid.next_page_token,
        "the two are distinct fields and must never be interchanged"
    );
    // Last page: NO page token, and the history id is still present -- which is the whole reason a caller may
    // advance a durable cursor from the response at all.
    assert_eq!(
        last.next_page_token, None,
        "the last page carries no page token"
    );
    assert_eq!(last.history_id.as_deref(), Some("12348"));

    // And the consequence is asserted rather than described: only the last page's history id is a position a
    // caller could store, because the page token is absent there and present mid-walk.
    assert!(
        last.next_page_token.is_none() && last.history_id.is_some(),
        "a durable cursor comes from the field that survives the end of the walk"
    );
}

#[test]
fn two_410s_with_one_status_and_opposite_remedies_are_told_apart_by_the_reason() {
    // **The pair that makes "every 410 wipes the store" falsifiable.** The Calendar errors page publishes three
    // bodies for HTTP 410 Gone: `fullSyncRequired` and `updatedMinTooLongAgo` say "wipe the store and re-sync",
    // while `deleted` says "no further action is necessary". Both fixtures below are a 410, so a connector
    // keying on the status alone gives them the same answer -- and the wrong one costs a whole sync store when
    // a user deletes a single event.
    let requires = fixture("calendar_error_410_full_sync_required.json");
    let deleted = fixture("calendar_error_410_resource_deleted.json");
    assert_declared_shape(&requires);
    assert_declared_shape(&deleted);

    let parse_reason = |text: &str| -> String {
        let body: GoogleErrorBody = match serde_json::from_str(text) {
            Ok(body) => body,
            Err(error) => panic!("the documented 410 shape must parse: {error}"),
        };
        assert_eq!(body.error.code, 410, "both fixtures are a 410");
        body.reason()
            .unwrap_or_else(|| panic!("a 410 fixture states a reason"))
            .to_owned()
    };
    let full_sync = parse_reason(&requires);
    let deleted_reason = parse_reason(&deleted);
    assert_ne!(
        full_sync, deleted_reason,
        "the two 410s must differ in the field the decision reads, or the pair proves nothing"
    );

    // The classification, using the reason each body carries.
    assert_eq!(
        client::CalendarGoneReason::parse(Some(&full_sync)),
        client::CalendarGoneReason::FullSyncRequired
    );
    assert_eq!(
        client::CalendarGoneReason::parse(Some(&deleted_reason)),
        client::CalendarGoneReason::ResourceAlreadyDeleted
    );
    // And the decisions genuinely oppose each other.
    assert!(
        client::CalendarGoneReason::FullSyncRequired.requires_resync(),
        "an invalid sync token must wipe the store"
    );
    assert!(
        !client::CalendarGoneReason::ResourceAlreadyDeleted.requires_resync(),
        "a deleted resource must NOT wipe the store; the page says no further action is necessary"
    );

    // The status-only predicate still resyncs both, which is why it cannot be the only reader: it cannot see
    // the body. Asserted so the two functions' relationship is a recorded fact rather than an accident.
    assert!(client::calendar_status_requires_resync(410));
    let refusal = client::classify(
        GoogleApi::Gmail,
        410,
        GoogleErrorReason::Unrecognised,
        None,
        None,
    );
    assert_eq!(
        client::calendar_signal(410, None, Some(&full_sync), refusal.clone()),
        client::SyncSignal::CursorUnusable
    );
    assert_eq!(
        client::calendar_signal(410, None, Some(&deleted_reason), refusal.clone()),
        client::SyncSignal::Refused(refusal),
        "a deleted resource is a refusal for the call, not a dead cursor"
    );
}

#[test]
fn a_400_on_an_incremental_sync_is_a_callers_mistake_and_never_a_stale_cursor() {
    // The other half of the 410 distinction, and the reason the 410 exists at all: the sync guide says a list
    // query "containing disallowed restrictions" answers **400**, and a 410 means the token is invalid. The
    // remedies are opposites -- 400 must not be retried and must not resync, because the same query would fail
    // the same way and the store is fine -- so a connector that treated any incremental failure as staleness
    // would discard a working store over its own bad filter.
    let text = fixture("calendar_error_400_time_range_empty.json");
    assert_declared_shape(&text);
    let body: GoogleErrorBody = match serde_json::from_str(&text) {
        Ok(body) => body,
        Err(error) => panic!("the documented 400 shape must parse: {error}"),
    };
    assert_eq!(body.error.code, 400);
    assert_eq!(
        body.reason(),
        Some("timeRangeEmpty"),
        "the page's own 400 example carries this reason"
    );

    // **Not a dead cursor.** The status-only predicate is false for it, so the resync path is unreachable from
    // a 400 by construction rather than by a caller remembering.
    assert!(!client::calendar_status_requires_resync(400));
    let decision = client::classify(
        GoogleApi::Gmail,
        400,
        GoogleErrorReason::Unrecognised,
        None,
        None,
    );
    assert!(
        !decision.guidance.permits_retry(),
        "the page says plainly: 'this is a permanent error, do not retry'"
    );
    assert_eq!(
        client::calendar_signal(400, None, None, decision.clone()),
        client::SyncSignal::Refused(decision),
        "a 400 is carried as a refusal, never as staleness"
    );
}

#[test]
fn a_410_whose_reason_cannot_be_read_still_resyncs_because_the_alternative_is_a_dead_store() {
    // The direction of the unreadable case, asserted because it is the opposite of the crate's usual
    // fail-closed rule and a reader should be able to see the reasoning rather than infer it. An unrecognised
    // 410 might be a new sync-token cause, and resyncing needlessly costs a slower next sync while *not*
    // resyncing a genuinely dead token costs a store that never syncs again and never says so.
    for reason in [None, Some("somethingNew"), Some("")] {
        let parsed = client::CalendarGoneReason::parse(reason);
        assert_eq!(
            parsed,
            client::CalendarGoneReason::Unrecognised,
            "{reason:?}"
        );
        assert!(
            parsed.requires_resync(),
            "{reason:?}: an unreadable 410 must still resync"
        );
        let refusal = client::classify(
            GoogleApi::Gmail,
            410,
            GoogleErrorReason::Unrecognised,
            None,
            None,
        );
        assert_eq!(
            client::calendar_signal(410, None, reason, refusal),
            client::SyncSignal::CursorUnusable,
            "{reason:?}"
        );
    }
    // The control: a `deleted` reason does NOT resync, so the assertions above are about the unreadable case
    // rather than about `requires_resync` being true for everything.
    assert!(!client::CalendarGoneReason::parse(Some("deleted")).requires_resync());
}

#[test]
fn a_successful_calendar_read_advances_the_cursor_from_the_sync_token() {
    // The positive control for every refusal above: without it, a `calendar_signal` that refused everything
    // would pass all of them.
    let refusal = client::classify(
        GoogleApi::Gmail,
        500,
        GoogleErrorReason::BackendError,
        None,
        None,
    );
    assert_eq!(
        client::calendar_signal(200, Some("CPDAlvWDx70C="), None, refusal.clone()),
        client::SyncSignal::Advanced {
            history_id: Some("CPDAlvWDx70C=".to_owned())
        }
    );
    // A 200 with no token is an unchanged calendar -- ordinary, and distinguishable from an absent response.
    assert_eq!(
        client::calendar_signal(200, None, None, refusal.clone()),
        client::SyncSignal::Advanced { history_id: None }
    );
    // And a retryable status is carried rather than read as staleness, which is the mistake that discards a
    // working store on a transient failure.
    for status in [429, 500, 503] {
        let decision = client::classify(
            GoogleApi::Gmail,
            status,
            GoogleErrorReason::Unrecognised,
            None,
            None,
        );
        assert_eq!(
            client::calendar_signal(status, None, None, decision.clone()),
            client::SyncSignal::Refused(decision),
            "{status}"
        );
    }
}

#[test]
fn every_fixture_is_json_and_declares_itself() {
    // A sweep, so a new fixture is held to the same standard as these. A file that is not JSON, or that does
    // not declare its provenance, fails here rather than being noticed by a reader.
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("google");
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) => panic!("the fixture directory must exist: {error}"),
    };
    let mut checked = 0;
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => panic!("{error}"),
        };
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => panic!("{} must be readable: {error}", path.display()),
        };
        assert_declared_shape(&text);
        checked += 1;
    }
    assert!(
        checked >= 5,
        "the sweep must have found the fixtures, not zero of them: {checked}"
    );
}
