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

use jarvis_connectors::google::client::{self, GmailErrorBody, GmailErrorReason};
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
    let body: GmailErrorBody = match serde_json::from_str(&text) {
        Ok(body) => body,
        Err(error) => panic!("the documented error shape must parse: {error}"),
    };
    let reason = GmailErrorReason::parse(
        body.reason()
            .unwrap_or_else(|| panic!("the fixture states a reason")),
    );
    assert_eq!(reason, GmailErrorReason::DomainPolicy);
    assert!(reason.needs_a_person(), "an administrator must be involved");
    let decision = client::classify(403, reason, None, None);
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
    let body: GmailErrorBody = match serde_json::from_str(&text) {
        Ok(body) => body,
        Err(error) => panic!("the documented error shape must parse: {error}"),
    };
    let throttled = GmailErrorReason::parse(
        body.reason()
            .unwrap_or_else(|| panic!("the fixture states a reason")),
    );
    assert_eq!(throttled, GmailErrorReason::RateLimitExceeded);

    let disabled = GmailErrorReason::parse("domainPolicy");
    assert_ne!(
        throttled, disabled,
        "the two reasons must be distinguishable"
    );
    assert_ne!(
        body.error.code, 0,
        "the fixture carries the status in the body as the provider does"
    );

    let retryable = client::classify(403, throttled, None, None);
    let permanent = client::classify(403, disabled, None, None);
    assert!(
        retryable.guidance.permits_retry(),
        "a throttling limit may be retried"
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
    let body: GmailErrorBody = match serde_json::from_str(&text) {
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
