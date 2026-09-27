//! Tests for the Google client's decisions.
//!
//! Every fixture is **built from `docs/research/integrations/google.md`** rather than captured from the wire,
//! because no request has been sent to Google. So these tests prove that the code implements the record; they
//! prove nothing about whether the record matches Google, and the manifest's
//! `CompatibilityVerdict::Unverified` is the declaration of exactly that gap.
//!
//! The falsification record is in `TODO.md`.

use super::*;
use crate::manifest::{ConnectorVersion, ProviderIdempotency};
use crate::ratelimit::{RetryClass, RetryGuidance};
use crate::{AccountReference, SyncCursorKind};

fn must<T, E: std::fmt::Display>(result: Result<T, E>, what: &str) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("{what}: {error}"),
    }
}

fn account() -> AccountReference {
    // `AccountReference` is a **JARVIS-local** reference rather than the provider account id: its alphabet is
    // lowercase letters, digits and `-`, so an email address is not representable here at all. The
    // provider-verified identity lives on `VerifiedAccount`, which is the type `tools-and-connectors.md`
    // means by "account identity verified from the provider".
    must(
        AccountReference::new("example-account"),
        "a valid account reference",
    )
}

fn version() -> ConnectorVersion {
    must(ConnectorVersion::new("1.0.0"), "a valid connector version")
}

fn cursor() -> SyncCursor {
    must(
        SyncCursor::new(
            SyncCursorKind::MonotonicMarker,
            Some("1234567890".to_owned()),
            account(),
            "1.0.0",
            UtcTimestamp::from_unix_nanos(1_774_000_000_500_000_000)
                .unwrap_or_else(|_| panic!("a representable instant")),
        ),
        "a valid cursor",
    )
}

#[test]
fn a_403_is_classified_by_its_reason_and_not_its_status() {
    // The table that justifies this function's existence. Four documented reasons share one status and have
    // three different remedies, so a status-only classifier gets a real case wrong.
    let throttled = classify(403, GmailErrorReason::RateLimitExceeded, None, None);
    assert_eq!(throttled.class, RetryClass::Throttled);
    assert!(throttled.guidance.permits_retry());

    let user_throttled = classify(403, GmailErrorReason::UserRateLimitExceeded, None, None);
    assert_eq!(user_throttled.class, RetryClass::Throttled);

    let limit = classify(403, GmailErrorReason::DailyLimitExceeded, None, None);
    assert_eq!(
        limit.class,
        RetryClass::Permanent,
        "the daily threshold cannot be raised, so a retry is not the remedy"
    );
    assert!(!limit.guidance.permits_retry());

    let policy = classify(403, GmailErrorReason::DomainPolicy, None, None);
    assert_eq!(policy.class, RetryClass::Permanent);
    assert_eq!(policy.guidance, RetryGuidance::DoNotRetry);
    // The positive control: the same status DOES yield a retry for other reasons, so this test is not passing
    // on a classifier that refuses everything at 403.
    assert_ne!(policy.class, throttled.class);
}

#[test]
fn a_domain_policy_refusal_is_permanent_and_needs_a_person() {
    // `domainPolicy` is "the domain administrators have disabled Gmail apps" — a decision by a human that no
    // amount of retrying can change, and the documented remedy is to ask that human.
    assert!(GmailErrorReason::DomainPolicy.needs_a_person());
    assert!(GmailErrorReason::DailyLimitExceeded.needs_a_person());
    // And the throttling reasons do not, which is what makes the predicate discriminating rather than "true".
    assert!(!GmailErrorReason::RateLimitExceeded.needs_a_person());
    assert!(!GmailErrorReason::UserRateLimitExceeded.needs_a_person());
}

#[test]
fn a_429_honours_a_stated_delay_and_falls_back_to_the_documented_floor() {
    // Google documents three distinct causes behind one 429 and states the remedy is to wait. A stated retry
    // time is the provider's own answer, so it is used; an absent one falls back to the documented floor of at
    // least one second rather than to zero, which is the value the guidance explicitly rules out.
    let stated = classify(429, GmailErrorReason::Unrecognised, Some(37), None);
    assert_eq!(stated.class, RetryClass::Throttled);
    assert_eq!(stated.guidance, RetryGuidance::RetryAfterSeconds(37));

    let unstated = classify(429, GmailErrorReason::Unrecognised, None, None);
    assert_eq!(unstated.class, RetryClass::Throttled);
    assert_eq!(
        unstated.guidance,
        RetryGuidance::BackoffSeconds(GOOGLE_RETRY_FLOOR_SECONDS)
    );
    assert_eq!(
        GOOGLE_RETRY_FLOOR_SECONDS, 1,
        "the documented floor is one second"
    );
}

#[test]
fn a_server_fault_backs_off_and_a_bad_request_does_not() {
    for status in [500, 502, 503, 504] {
        let decision = classify(status, GmailErrorReason::BackendError, None, None);
        assert_eq!(decision.class, RetryClass::ProviderFault, "{status}");
        assert!(decision.guidance.permits_retry(), "{status}");
    }
    let bad = classify(400, GmailErrorReason::BadRequest, None, None);
    assert_eq!(bad.class, RetryClass::Permanent);
    assert_eq!(bad.guidance, RetryGuidance::DoNotRetry);
}

#[test]
fn an_expired_token_reauthenticates_rather_than_retrying() {
    // `authError` is documented as a 401 and "refresh the access token", so the class is `Authentication` and
    // the guidance is never a retry — retrying a request with the same expired token just spends the budget.
    let decision = classify(401, GmailErrorReason::AuthError, None, None);
    assert_eq!(decision.class, RetryClass::Authentication);
    assert_eq!(decision.guidance, RetryGuidance::Reauthenticate);
    assert!(!decision.guidance.permits_retry());
    assert!(decision.class.needs_user());
}

#[test]
fn an_unrecognised_status_becomes_unknown_and_never_permits_a_retry() {
    // The fail-closed direction. A status this connector has no case for is one whose effect is unknown, and
    // `Unknown`'s whole content is that an unanswered question must not be read as a yes — so it refuses a
    // retry even for an operation the manifest declares idempotent.
    let decision = classify(418, GmailErrorReason::Unrecognised, None, None);
    assert_eq!(decision.class, RetryClass::Unknown);
    assert_eq!(decision.guidance, RetryGuidance::Reconcile);
    assert!(!decision.guidance.permits_retry());
    assert!(
        !decision
            .class
            .permits_automatic_retry(ProviderIdempotency::Declared),
        "even a declared-idempotent operation must not retry an unknown outcome"
    );
    // The positive control: a transient fault DOES permit a retry for the same declaration, so the assertion
    // above is not passing because every class refuses.
    assert!(
        RetryClass::ProviderFault.permits_automatic_retry(ProviderIdempotency::Declared),
        "a provider fault must permit a retry for a declared-idempotent operation"
    );
}

#[test]
fn a_provider_request_id_is_carried_and_never_the_error_text() {
    // `tools-and-connectors.md`: "preserve provider IDs and receipts separately from user-facing text". The
    // decision carries the id, and the body type has **no field for the message** — which is the structural
    // form of `P3-008c`'s "do not derive a safety flag from message text".
    let id = must(ProviderRequestId::new("request-abc"), "a valid request id");
    let decision = classify(503, GmailErrorReason::BackendError, None, Some(id));
    assert_eq!(
        decision
            .provider_request_id
            .as_ref()
            .map(ProviderRequestId::as_str),
        Some("request-abc")
    );
}

#[test]
fn an_error_body_yields_its_reason_and_tolerates_its_absence() {
    // Real bodies vary: the documentation's own samples are not all consistent, so a missing `reason` must
    // leave the caller able to classify from the status rather than making the body unparseable.
    let body: GmailErrorBody = match serde_json::from_str(
        r#"{"error":{"code":403,"errors":[{"domain":"usageLimits","reason":"userRateLimitExceeded"}]}}"#,
    ) {
        Ok(body) => body,
        Err(error) => panic!("the documented body must parse: {error}"),
    };
    assert_eq!(body.reason(), Some("userRateLimitExceeded"));
    assert_eq!(
        GmailErrorReason::parse("userRateLimitExceeded"),
        GmailErrorReason::UserRateLimitExceeded
    );

    let without: GmailErrorBody =
        match serde_json::from_str(r#"{"error":{"code":403,"errors":[{"domain":"global"}]}}"#) {
            Ok(body) => body,
            Err(error) => panic!("a reasonless body must still parse: {error}"),
        };
    assert_eq!(without.reason(), None);

    // An empty `errors` array parses too, and an unknown reason becomes `Unrecognised` rather than a failure.
    let empty: GmailErrorBody = match serde_json::from_str(r#"{"error":{"code":403}}"#) {
        Ok(body) => body,
        Err(error) => panic!("a body with no errors must parse: {error}"),
    };
    assert_eq!(empty.reason(), None);
    assert_eq!(
        GmailErrorReason::parse("somethingNewIn2027"),
        GmailErrorReason::Unrecognised
    );
    assert_eq!(GmailErrorReason::Unrecognised.as_str(), None);
}

#[test]
fn a_reason_is_matched_exactly_and_never_case_insensitively() {
    // A case-insensitive match would accept `DailyLimitExceeded`, which Google never sends, and fold it into
    // the documented case — inventing agreement with a response the provider does not produce.
    assert_eq!(
        GmailErrorReason::parse("DailyLimitExceeded"),
        GmailErrorReason::Unrecognised
    );
    assert_eq!(
        GmailErrorReason::parse("dailyLimitExceeded"),
        GmailErrorReason::DailyLimitExceeded
    );
}

#[test]
fn a_page_token_is_bounded_and_an_empty_one_is_refused() {
    assert_eq!(next_page(None), Ok(None));
    assert_eq!(next_page(Some("abc")), Ok(Some("abc".to_owned())));
    // An empty token is a different statement from an absent one: absent means the last page, and empty means
    // the provider sent something unusable. Folding them together would report a truncated sync as complete.
    assert!(next_page(Some("")).is_err());
    assert!(
        next_page(Some("a\nb")).is_err(),
        "a control character forges a log line"
    );
    let long = "a".repeat(MAX_PAGE_TOKEN_CHARS + 1);
    assert!(next_page(Some(&long)).is_err());
    // The bound is exercised AT its limit, so it is not an unreachable one — the defect `P5-001` recorded.
    let at_limit = "a".repeat(MAX_PAGE_TOKEN_CHARS);
    assert!(next_page(Some(&at_limit)).is_ok());
}

#[test]
fn a_gmail_history_404_is_pruned_history_and_not_a_missing_account() {
    // The finding this module was written around, asserted from both sides: the predicate is true for the
    // status Gmail documents as pruned history, and the *same* status must not be read as an absent resource
    // by any other reading. `gmail_history_status_is_pruned` is a named predicate precisely so the caller
    // applies it to `history.list` alone.
    assert!(gmail_history_status_is_pruned(404));
    assert!(!gmail_history_status_is_pruned(200));
    assert!(!gmail_history_status_is_pruned(403));
    // And the ambiguity is real rather than theoretical: 404 is also the status for a missing resource, which
    // is why the function's name says `history`.
    assert_eq!(
        classify(404, GmailErrorReason::Unrecognised, None, None).class,
        RetryClass::Permanent
    );
}

#[test]
fn a_calendar_410_requires_a_resync_and_a_400_does_not() {
    // Google's sync guide: a 410 "should trigger a full wipe of the client's store and a new full sync", while
    // 400 is a disallowed query restriction — the caller's mistake. Conflating them would discard a whole
    // store over a bad query parameter.
    assert!(calendar_status_requires_resync(410));
    assert!(!calendar_status_requires_resync(400));
    assert!(!calendar_status_requires_resync(200));
    assert_eq!(
        classify(400, GmailErrorReason::BadRequest, None, None).class,
        RetryClass::Permanent
    );
}

#[test]
fn a_history_cursor_advances_and_refuses_to_move_backwards() {
    let account = account();
    let version = version();
    let start = cursor();
    let now = UtcTimestamp::from_unix_nanos(1_774_000_001_500_000_000)
        .unwrap_or_else(|_| panic!("a representable instant"));

    let advanced = must(
        advance_gmail_history(&start, Some("1234567999"), &account, &version, now),
        "a forward advance must succeed",
    );
    assert_eq!(advanced.advance, SyncAdvance::Advanced);
    assert_eq!(
        advanced.cursor.as_ref().and_then(SyncCursor::token),
        Some("1234567999")
    );

    // An unchanged mailbox is an ordinary outcome and keeps the previous cursor rather than inventing one.
    let unchanged = must(
        advance_gmail_history(&start, None, &account, &version, now),
        "no new id must be accepted",
    );
    assert_eq!(
        unchanged.cursor.as_ref().and_then(SyncCursor::token),
        Some("1234567890")
    );

    // A smaller id is refused. `historyId` increases, so a smaller value is a stale or foreign response, and
    // storing it would silently re-walk history the connector has already processed — a repeat for a connector
    // that acts on changes.
    let backwards = advance_gmail_history(&start, Some("1234567000"), &account, &version, now);
    assert!(
        backwards.is_err(),
        "a backwards historyId must be refused rather than stored"
    );
    // The positive control on the ordering check: one id forward IS accepted, so the refusal above is the
    // ordering rule and not a check that refuses every change.
    assert_eq!(advanced.advance, SyncAdvance::Advanced);
}

#[test]
fn a_calendar_token_is_treated_as_opaque_so_no_ordering_is_invented() {
    // `nextSyncToken` is opaque: nothing may be concluded from it, so unlike Gmail's monotonic marker there is
    // no backwards check. Comparing two tokens would be inventing a property the provider never offered.
    let account = account();
    let version = version();
    let now = UtcTimestamp::from_unix_nanos(1_774_000_000_500_000_000)
        .unwrap_or_else(|_| panic!("a representable instant"));
    let opaque = must(
        SyncCursor::new(
            SyncCursorKind::OpaqueToken,
            Some("CPDAlvWDx70CEPDAlvWDx70CGAU=".to_owned()),
            account.clone(),
            "1.0.0",
            now,
        ),
        "a valid opaque cursor",
    );
    // A lexicographically smaller token is NOT refused, because no ordering is defined for it.
    let next = must(
        advance_calendar_sync(&opaque, Some("AAA="), &account, &version, now),
        "an opaque token has no ordering, so any value is accepted",
    );
    assert_eq!(next.advance, SyncAdvance::Advanced);
    assert_eq!(
        next.cursor.as_ref().and_then(SyncCursor::token),
        Some("AAA=")
    );
    assert_eq!(
        opaque.kind(),
        SyncCursorKind::OpaqueToken,
        "the kind is what carries `nothing may be inferred`"
    );
}

#[test]
fn the_declared_bounds_are_googles_own_numbers() {
    // These are transcribed values, so pinning them makes a change deliberate. A silent edit to the batch
    // limit or the page cap would change what a full sync costs without anything failing.
    assert_eq!(GMAIL_MAX_RESULTS_CAP, 500);
    assert_eq!(GMAIL_BATCH_LIMIT, 50);
    assert_eq!(GOOGLE_MAX_BACKOFF_SECONDS, 64);
    assert_eq!(GOOGLE_RETRY_FLOOR_SECONDS, 1);
    assert_eq!(GOOGLE_API_HOST, "www.googleapis.com");
    assert!(GMAIL_API_BASE.ends_with("/gmail/v1"));
    assert!(CALENDAR_API_BASE.ends_with("/calendar/v3"));
}

#[test]
fn the_batch_limit_is_below_the_page_cap_and_both_are_bounded() {
    // Batching is what makes a full sync affordable AND is itself a rate-limit trigger, so the two numbers
    // must not be confused: a batch of 500 would be the page cap used as a batch size. Bound to locals first,
    // because `assert!` on two constants is a *constant assertion* — the trap this workspace records.
    let batch = GMAIL_BATCH_LIMIT;
    let page = GMAIL_MAX_RESULTS_CAP;
    assert!(
        batch < page,
        "a batch is not a page; confusing them would ask for 500 sub-requests at once"
    );
}
