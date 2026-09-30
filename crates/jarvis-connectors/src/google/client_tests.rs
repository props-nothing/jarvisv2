//! Tests for the Google client's decisions.
//!
//! Every fixture is **built from `docs/research/integrations/google.md`** rather than captured from the wire,
//! because no request has been sent to Google. So these tests prove that the code implements the record; they
//! prove nothing about whether the record matches Google, and the manifest's
//! `CompatibilityVerdict::Unverified` is the declaration of exactly that gap.
//!
//! The falsification record is in `TODO.md`.

use super::*;
use crate::google::transport::RetryAfter;
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

/// One instant, so a test compares cursors without restating a timestamp.
fn now() -> UtcTimestamp {
    UtcTimestamp::from_unix_nanos(1_774_000_000_500_000_000)
        .unwrap_or_else(|_| panic!("a representable instant"))
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
    let throttled = classify(
        GoogleApi::Gmail,
        403,
        GoogleErrorReason::RateLimitExceeded,
        None,
        None,
    );
    assert_eq!(throttled.class, RetryClass::Throttled);
    assert!(throttled.guidance.permits_retry());

    let user_throttled = classify(
        GoogleApi::Gmail,
        403,
        GoogleErrorReason::UserRateLimitExceeded,
        None,
        None,
    );
    assert_eq!(user_throttled.class, RetryClass::Throttled);

    let limit = classify(
        GoogleApi::Gmail,
        403,
        GoogleErrorReason::DailyLimitExceeded,
        None,
        None,
    );
    assert_eq!(
        limit.class,
        RetryClass::Permanent,
        "the daily threshold cannot be raised, so a retry is not the remedy"
    );
    assert!(!limit.guidance.permits_retry());

    let policy = classify(
        GoogleApi::Gmail,
        403,
        GoogleErrorReason::DomainPolicy,
        None,
        None,
    );
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
    assert!(GoogleErrorReason::DomainPolicy.needs_a_person());
    assert!(GoogleErrorReason::DailyLimitExceeded.needs_a_person());
    // And the throttling reasons do not, which is what makes the predicate discriminating rather than "true".
    assert!(!GoogleErrorReason::RateLimitExceeded.needs_a_person());
    assert!(!GoogleErrorReason::UserRateLimitExceeded.needs_a_person());
}

#[test]
fn a_429_honours_a_stated_delay_and_falls_back_to_the_documented_floor() {
    // Google documents three distinct causes behind one 429 and states the remedy is to wait. A stated retry
    // time is the provider's own answer, so it is used; an absent one falls back to the documented floor of at
    // least one second rather than to zero, which is the value the guidance explicitly rules out.
    let stated = classify(
        GoogleApi::Gmail,
        429,
        GoogleErrorReason::Unrecognised,
        Some(RetryAfter::Seconds(37)),
        None,
    );
    assert_eq!(stated.class, RetryClass::Throttled);
    assert_eq!(stated.guidance, RetryGuidance::RetryAfterSeconds(37));

    let unstated = classify(
        GoogleApi::Gmail,
        429,
        GoogleErrorReason::Unrecognised,
        None,
        None,
    );
    assert_eq!(unstated.class, RetryClass::Throttled);
    assert_eq!(
        unstated.guidance,
        RetryGuidance::BackoffSeconds(GOOGLE_RETRY_FLOOR_SECONDS)
    );

    // **A stated delay this client could not read is neither of the above.** The wait equals the absent case —
    // both are the floor — so the guidance variant is what keeps the two apart, and asserting *both* here is
    // what makes the distinction observable. A `BackoffSeconds` here would tell an operator the provider stated
    // nothing when it stated a time (`ADR-0076`), and the failure message would name the wrong document.
    let unreadable = classify(
        GoogleApi::Gmail,
        429,
        GoogleErrorReason::Unrecognised,
        Some(RetryAfter::NotSeconds),
        None,
    );
    assert_eq!(unreadable.class, RetryClass::Throttled);
    assert_eq!(
        unreadable.guidance,
        RetryGuidance::BackoffAfterUnreadableDelay(GOOGLE_RETRY_FLOOR_SECONDS)
    );
    assert_ne!(
        unreadable.guidance, unstated.guidance,
        "a stated-but-unreadable delay must not be reported as an absent one"
    );
    assert_eq!(
        unreadable.guidance.delay_seconds(),
        unstated.guidance.delay_seconds(),
        "the wait is the same floor; only what it says about the provider differs"
    );
    assert_eq!(
        GOOGLE_RETRY_FLOOR_SECONDS, 1,
        "the documented floor is one second"
    );
}

#[test]
fn a_server_fault_backs_off_and_a_bad_request_does_not() {
    for status in [500, 502, 503, 504] {
        let decision = classify(
            GoogleApi::Gmail,
            status,
            GoogleErrorReason::BackendError,
            None,
            None,
        );
        assert_eq!(decision.class, RetryClass::ProviderFault, "{status}");
        assert!(decision.guidance.permits_retry(), "{status}");
    }
    let bad = classify(
        GoogleApi::Gmail,
        400,
        GoogleErrorReason::BadRequest,
        None,
        None,
    );
    assert_eq!(bad.class, RetryClass::Permanent);
    assert_eq!(bad.guidance, RetryGuidance::DoNotRetry);
}

#[test]
fn an_expired_token_reauthenticates_rather_than_retrying() {
    // `authError` is documented as a 401 and "refresh the access token", so the class is `Authentication` and
    // the guidance is never a retry — retrying a request with the same expired token just spends the budget.
    let decision = classify(
        GoogleApi::Gmail,
        401,
        GoogleErrorReason::AuthError,
        None,
        None,
    );
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
    let decision = classify(
        GoogleApi::Gmail,
        418,
        GoogleErrorReason::Unrecognised,
        None,
        None,
    );
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
    let decision = classify(
        GoogleApi::Gmail,
        503,
        GoogleErrorReason::BackendError,
        None,
        Some(id),
    );
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
    let body: GoogleErrorBody = match serde_json::from_str(
        r#"{"error":{"code":403,"errors":[{"domain":"usageLimits","reason":"userRateLimitExceeded"}]}}"#,
    ) {
        Ok(body) => body,
        Err(error) => panic!("the documented body must parse: {error}"),
    };
    assert_eq!(body.reason(), Some("userRateLimitExceeded"));
    assert_eq!(
        GoogleErrorReason::parse("userRateLimitExceeded"),
        GoogleErrorReason::UserRateLimitExceeded
    );

    let without: GoogleErrorBody =
        match serde_json::from_str(r#"{"error":{"code":403,"errors":[{"domain":"global"}]}}"#) {
            Ok(body) => body,
            Err(error) => panic!("a reasonless body must still parse: {error}"),
        };
    assert_eq!(without.reason(), None);

    // An empty `errors` array parses too, and an unknown reason becomes `Unrecognised` rather than a failure.
    let empty: GoogleErrorBody = match serde_json::from_str(r#"{"error":{"code":403}}"#) {
        Ok(body) => body,
        Err(error) => panic!("a body with no errors must parse: {error}"),
    };
    assert_eq!(empty.reason(), None);
    assert_eq!(
        GoogleErrorReason::parse("somethingNewIn2027"),
        GoogleErrorReason::Unrecognised
    );
    assert_eq!(GoogleErrorReason::Unrecognised.as_str(), None);
}

#[test]
fn a_reason_is_matched_exactly_and_never_case_insensitively() {
    // A case-insensitive match would accept `DailyLimitExceeded`, which Google never sends, and fold it into
    // the documented case — inventing agreement with a response the provider does not produce.
    assert_eq!(
        GoogleErrorReason::parse("DailyLimitExceeded"),
        GoogleErrorReason::Unrecognised
    );
    assert_eq!(
        GoogleErrorReason::parse("dailyLimitExceeded"),
        GoogleErrorReason::DailyLimitExceeded
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
fn a_gmail_history_404_carries_no_information_that_could_distinguish_two_causes() {
    // **This test previously asserted the opposite claim, and the reversal is the finding.**
    //
    // An earlier version was named `a_gmail_history_404_is_pruned_history_and_not_a_missing_account` and
    // asserted `gmail_history_status_is_pruned(404)` -- that a 404 on `history.list` IS pruned history. Finding
    // 2 in the research record says that distinction cannot be made, and the error guide confirms it: 404 is
    // documented as "the requested resource couldn't be found" with **no `reason` code**, so the same status
    // with the same code is what an absent mailbox returns. The old predicate claimed knowledge the response
    // does not carry, and a caller reading it would believe two cases had been distinguished when they had not.
    assert!(gmail_history_status_cannot_prove_usable(404));
    assert!(!gmail_history_status_cannot_prove_usable(200));
    assert!(!gmail_history_status_cannot_prove_usable(403));
    // The name is the content: "cannot prove usable" is what is readable, and `404` is the one status where a
    // resync may be warranted. A 429 or a 5xx is retryable, and a resync would discard a working store over a
    // transient failure -- so those must NOT be read as a dead cursor.
    for status in [429, 500, 502, 503, 504] {
        assert!(
            !gmail_history_status_cannot_prove_usable(status),
            "{status} is retryable, so a resync would discard a working store"
        );
    }
    // And the ambiguity is real rather than theoretical, which is why it cannot be resolved by a predicate:
    // the classifier reads the SAME status as permanent, so nothing in this crate invents a reason code for it.
    assert_eq!(
        classify(
            GoogleApi::Gmail,
            404,
            GoogleErrorReason::Unrecognised,
            None,
            None
        )
        .class,
        RetryClass::Permanent
    );
}

#[test]
fn a_410_is_a_known_state_for_calendar_and_unclassified_for_gmail() {
    // **The finding this change is about.** The two APIs publish different status sets: Calendar's error page
    // documents `410 Gone` as a dead sync token whose remedy is "wipe the store and re-sync", while Gmail's
    // page has **no `410` subsection at all**. Before the classifier took an API, a Calendar `410` fell to the
    // catch-all and reached a caller as `unknown`/"reconcile" — telling it to *establish what happened* when
    // the provider had already said exactly what had.
    let calendar = classify(
        GoogleApi::Calendar,
        410,
        GoogleErrorReason::Unrecognised,
        None,
        None,
    );
    assert_eq!(
        calendar.class,
        RetryClass::Permanent,
        "a dead sync token is a known, definite state for Calendar"
    );
    assert_ne!(
        calendar.class,
        RetryClass::Unknown,
        "the whole defect: a Calendar 410 must not read as unclassified"
    );
    assert_eq!(calendar.guidance, RetryGuidance::DoNotRetry);

    // And the same status from the other API stays unclassified, because nothing documents it there. This is
    // the control that makes the assertion above about the API rather than about the status.
    let gmail = classify(
        GoogleApi::Gmail,
        410,
        GoogleErrorReason::Unrecognised,
        None,
        None,
    );
    assert_eq!(
        gmail.class,
        RetryClass::Unknown,
        "Gmail documents no 410, so it stays the fail-closed answer"
    );
    assert_eq!(gmail.guidance, RetryGuidance::Reconcile);
    assert_ne!(
        calendar, gmail,
        "the same status must classify differently for the two APIs"
    );
    // Neither permits a retry, so the divergence is about the *diagnosis* a caller reads rather than about a
    // retry that one would allow and the other refuse.
    assert!(!calendar.guidance.permits_retry());
    assert!(!gmail.guidance.permits_retry());
}

#[test]
fn the_shared_arms_do_not_depend_on_the_api_because_the_pages_agree_where_they_overlap() {
    // The other half of the change: taking an API must not fork everything. `401`, the `5xx` family and the
    // throttling reasons are documented the same way by both pages — Calendar's own error page says
    // "`rateLimitExceeded` errors can return either `403` or `429` error codes—currently they are functionally
    // similar" — so both APIs must give the same answer there. A table per API would have silently allowed
    // these to drift.
    for api in [GoogleApi::Gmail, GoogleApi::Calendar] {
        for status in [401u16, 429, 500, 502, 503, 504] {
            let decision = classify(api, status, GoogleErrorReason::Unrecognised, None, None);
            let other = classify(
                if api == GoogleApi::Gmail {
                    GoogleApi::Calendar
                } else {
                    GoogleApi::Gmail
                },
                status,
                GoogleErrorReason::Unrecognised,
                None,
                None,
            );
            assert_eq!(
                decision, other,
                "{api:?} and the other API must agree on {status}, which both pages document the same way"
            );
        }
        // And the 403 reason split is shared too, which is the one the classifier was built around.
        let throttled = classify(api, 403, GoogleErrorReason::RateLimitExceeded, None, None);
        assert_eq!(throttled.class, RetryClass::Throttled, "{api:?}");
        let policy = classify(api, 403, GoogleErrorReason::DomainPolicy, None, None);
        assert_eq!(policy.guidance, RetryGuidance::DoNotRetry, "{api:?}");
    }
}

#[test]
fn a_calendar_404_keeps_the_documented_divergence_rather_than_splitting_the_arm() {
    // **A divergence recorded rather than resolved silently.** Calendar's error page suggests "use exponential
    // backoff" for a `404`; Gmail's summary states no action. The crate keeps `DoNotRetry` for both, because
    // Calendar's own two documented causes are "the requested resource … has never existed" and "accessing a
    // calendar that the user can not access" — and neither is repaired by sending the identical request again,
    // so a retry would fail the same way until its budget ran out.
    //
    // Asserting the divergence HERE is what makes it a decision: if either page's guidance changed, or if a
    // future reader "fixed" the arm to match Calendar's sentence, this test names which document wins.
    let calendar = classify(
        GoogleApi::Calendar,
        404,
        GoogleErrorReason::Unrecognised,
        None,
        None,
    );
    let gmail = classify(
        GoogleApi::Gmail,
        404,
        GoogleErrorReason::Unrecognised,
        None,
        None,
    );
    assert_eq!(
        calendar, gmail,
        "the arm is deliberately shared despite the pages differing"
    );
    assert_eq!(calendar.class, RetryClass::Permanent);
    assert!(
        !calendar.guidance.permits_retry(),
        "the 404 arm refuses a retry even though Calendar's page suggests backoff: retrying an identical \
         request against a missing resource fails identically, and the divergence is recorded in `ADR-0082`"
    );
}

#[test]
fn a_history_status_becomes_the_signal_the_cursor_decision_consumes() {
    // **This is the producer that was missing.** `ADR-0066` made the signal a parameter so the inference from
    // a 404 would be a caller's explicit act — but nothing could BUILD the signal from a status, so the
    // `CursorUnusable` remedy (and so `SyncAdvance::HistoryPruned`) was still unreachable outside a fixture.
    let refused = classify(
        GoogleApi::Gmail,
        429,
        GoogleErrorReason::Unrecognised,
        Some(RetryAfter::Seconds(30)),
        None,
    );
    let not_found = classify(
        GoogleApi::Gmail,
        404,
        GoogleErrorReason::Unrecognised,
        None,
        None,
    );

    // A 404 is the dead-cursor signal, and it is the SAME status the classifier reads as permanent — so the
    // two are asserted together, because that coincidence is exactly the ambiguity the predicate names.
    assert_eq!(
        gmail_history_signal(404, None, not_found.clone()),
        SyncSignal::CursorUnusable
    );
    assert_eq!(not_found.class, RetryClass::Permanent);

    // A 200 advances, carrying the position the response stated.
    assert_eq!(
        gmail_history_signal(200, Some("12347"), refused.clone()),
        SyncSignal::Advanced {
            history_id: Some("12347".to_owned())
        }
    );
    // A 200 with no id is an unchanged mailbox — an ordinary outcome that advances nothing, not a refusal.
    assert_eq!(
        gmail_history_signal(200, None, refused.clone()),
        SyncSignal::Advanced { history_id: None }
    );

    // **The retryable family must not become a dead cursor.** A resync on a 429 or a 5xx discards a working
    // store — the opposite mistake, and a far more expensive one. Asserted with the control that the same
    // status carried into `Refused` preserves the classification rather than swallowing it.
    for status in [400, 403, 429, 500, 502, 503, 504] {
        let decision = classify(
            GoogleApi::Gmail,
            status,
            GoogleErrorReason::Unrecognised,
            Some(RetryAfter::Seconds(30)),
            None,
        );
        assert_eq!(
            gmail_history_signal(status, None, decision.clone()),
            SyncSignal::Refused(decision),
            "{status} is not a dead cursor"
        );
    }

    // And the signal really does drive the decision end to end: the 404 path reaches the resync remedy.
    let outcome = must(
        advance_gmail_history(
            &cursor(),
            &gmail_history_signal(
                404,
                None,
                classify(
                    GoogleApi::Gmail,
                    404,
                    GoogleErrorReason::Unrecognised,
                    None,
                    None,
                ),
            ),
            &account(),
            &version(),
            now(),
        ),
        "a 404 must produce an outcome",
    );
    assert_eq!(outcome.advance, SyncAdvance::HistoryPruned);
    assert!(outcome.cursor.is_none());
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
        classify(
            GoogleApi::Gmail,
            400,
            GoogleErrorReason::BadRequest,
            None,
            None
        )
        .class,
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
        advance_gmail_history(
            &start,
            &SyncSignal::Advanced {
                history_id: Some("1234567999".to_owned()),
            },
            &account,
            &version,
            now,
        ),
        "a forward advance must succeed",
    );
    assert_eq!(advanced.advance, SyncAdvance::Advanced);
    assert_eq!(
        advanced.cursor.as_ref().and_then(SyncCursor::token),
        Some("1234567999")
    );

    // An unchanged mailbox is an ordinary outcome and keeps the previous cursor rather than inventing one.
    let unchanged = must(
        advance_gmail_history(
            &start,
            &SyncSignal::Advanced { history_id: None },
            &account,
            &version,
            now,
        ),
        "no new id must be accepted",
    );
    assert_eq!(unchanged.advance, SyncAdvance::Advanced);
    assert_eq!(
        unchanged.cursor.as_ref().and_then(SyncCursor::token),
        Some("1234567890")
    );

    // A smaller id is refused. `historyId` increases, so a smaller value is a stale or foreign response, and
    // storing it would silently re-walk history the connector has already processed — a repeat for a connector
    // that acts on changes.
    let backwards = advance_gmail_history(
        &start,
        &SyncSignal::Advanced {
            history_id: Some("1234567000".to_owned()),
        },
        &account,
        &version,
        now,
    );
    assert!(
        backwards.is_err(),
        "a backwards historyId must be refused rather than stored"
    );
    // The positive control on the ordering check: one id forward IS accepted, so the refusal above is the
    // ordering rule and not a check that refuses every change.
    assert_eq!(advanced.advance, SyncAdvance::Advanced);
}

#[test]
fn a_gmail_cursor_that_cannot_be_used_requires_a_resync_and_carries_no_cursor() {
    // The documented remedy, now REACHABLE. Before `SyncSignal`, `advance_gmail_history` took only the new
    // history id, so `HistoryPruned` could not be produced by any input at all — the remedy was named in a
    // variant that nothing could construct.
    let outcome = must(
        advance_gmail_history(
            &cursor(),
            &SyncSignal::CursorUnusable,
            &account(),
            &version(),
            now(),
        ),
        "a dead cursor is an outcome, not an error",
    );
    assert_eq!(outcome.advance, SyncAdvance::HistoryPruned);
    assert!(
        outcome.cursor.is_none(),
        "carrying the rejected cursor forward would invite a resume from a dead position"
    );
    // And the same signal on Calendar names Calendar's own reason, so the shared variant does not lose the
    // distinction between Gmail's pruned history and Calendar's invalidated token.
    let calendar = must(
        advance_calendar_sync(
            &must(
                SyncCursor::new(
                    SyncCursorKind::OpaqueToken,
                    Some("CPDAlvWDx70CEPDAlvWDx70CGAU=".to_owned()),
                    account(),
                    "1.0.0",
                    now(),
                ),
                "an opaque cursor",
            ),
            &SyncSignal::CursorUnusable,
            &account(),
            &version(),
            now(),
        ),
        "a dead Calendar token is an outcome, not an error",
    );
    assert_eq!(calendar.advance, SyncAdvance::TokenInvalidated);
    assert!(calendar.cursor.is_none());
}

#[test]
fn a_refused_advance_carries_the_decision_and_keeps_the_previous_cursor() {
    // A refusal is neither an advance nor a dead cursor. The **decision** is carried so a caller can report
    // *why* — and the **previous cursor is kept**, because a refusal says the request failed, not that the
    // position is unusable.
    //
    // **This test used to assert the opposite**, and its old comment justified it as "so a caller cannot store a
    // new position on the strength of a failure". That reasoning is sound for a *new* position and wrong here:
    // the previous cursor is not a new position, it is the caller's existing one, and a transient `429` said
    // nothing about it. Dropping it restarts the sync — which for Calendar is a full wipe of the sync token, and
    // with a `429` on a push-driven incremental sync that is routine rather than exceptional (`ADR-0090`).
    let decision = classify(
        GoogleApi::Gmail,
        429,
        GoogleErrorReason::Unrecognised,
        Some(RetryAfter::Seconds(30)),
        None,
    );
    let previous = cursor();
    let outcome = must(
        advance_gmail_history(
            &previous,
            &SyncSignal::Refused(decision.clone()),
            &account(),
            &version(),
            now(),
        ),
        "a refusal is an outcome, not an error",
    );
    assert_eq!(outcome.advance, SyncAdvance::Refused(decision));
    assert_eq!(
        outcome.cursor.as_ref(),
        Some(&previous),
        "a refusal must not discard a cursor the provider never rejected"
    );
}

#[test]
fn the_three_gmail_signals_differ_in_whether_they_carry_a_cursor_forward() {
    // **The three outcomes together, because the distinction is what carries the weight.** Asserted in one test
    // so a change that collapsed two of them is visible: a dead cursor carries **nothing**, a refusal carries
    // the **previous** one, and an advance carries the provider's new one. Collapsing `Refused` into
    // `CursorUnusable` discards a working store; collapsing `CursorUnusable` into `Refused` resumes from a
    // position the provider rejected, which is the defect `ADR-0066` was written around.
    let previous = cursor();

    let usable = must(
        advance_gmail_history(
            &previous,
            &SyncSignal::CursorUnusable,
            &account(),
            &version(),
            now(),
        ),
        "a dead cursor is an outcome",
    );
    assert_eq!(usable.advance, SyncAdvance::HistoryPruned);
    assert!(
        usable.cursor.is_none(),
        "a rejected position must not be carried forward"
    );

    let refused = must(
        advance_gmail_history(
            &previous,
            &SyncSignal::Refused(classify(
                GoogleApi::Gmail,
                503,
                GoogleErrorReason::BackendError,
                None,
                None,
            )),
            &account(),
            &version(),
            now(),
        ),
        "a refusal is an outcome",
    );
    assert!(matches!(refused.advance, SyncAdvance::Refused(_)));
    assert_eq!(refused.cursor.as_ref(), Some(&previous));

    let advanced = must(
        advance_gmail_history(
            &previous,
            &SyncSignal::Advanced {
                history_id: Some("1234567891".to_owned()),
            },
            &account(),
            &version(),
            now(),
        ),
        "an advance is an outcome",
    );
    assert_eq!(advanced.advance, SyncAdvance::Advanced);
    assert_ne!(
        advanced.cursor.as_ref().and_then(|cursor| cursor.token()),
        previous.token(),
        "an advance must store the provider's new position, not the old one"
    );

    // And the unchanged-mailbox case, which is the fourth shape: no new position means the previous cursor is
    // kept verbatim — the same answer as a refusal, for the same reason (nothing was learned about the
    // position), which is why both carry it rather than only one.
    let unchanged = must(
        advance_gmail_history(
            &previous,
            &SyncSignal::Advanced { history_id: None },
            &account(),
            &version(),
            now(),
        ),
        "an unchanged mailbox is an outcome",
    );
    assert_eq!(unchanged.advance, SyncAdvance::Advanced);
    assert_eq!(unchanged.cursor.as_ref(), Some(&previous));
}

#[test]
fn a_refused_calendar_advance_keeps_the_sync_token() {
    // The Calendar half of `ADR-0090`, and it matters more here than for Gmail: discarding the token costs a
    // **full wipe and resync** of the store, so a single throttled incremental sync would discard everything the
    // walk had built. A refusal says the request failed; it says nothing about the token.
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
    let decision = classify(
        GoogleApi::Calendar,
        429,
        GoogleErrorReason::Unrecognised,
        Some(RetryAfter::Seconds(30)),
        None,
    );
    let outcome = must(
        advance_calendar_sync(
            &opaque,
            &SyncSignal::Refused(decision.clone()),
            &account,
            &version,
            now,
        ),
        "a refusal is an outcome, not an error",
    );
    assert_eq!(outcome.advance, SyncAdvance::Refused(decision));
    assert_eq!(
        outcome.cursor.as_ref(),
        Some(&opaque),
        "a throttled incremental sync must not cost the whole sync token"
    );

    // The control: a **410** still discards it, so the change above is about a refusal and not about keeping
    // tokens generally. Without this a function that always returned `Some(previous)` would pass.
    let invalidated = must(
        advance_calendar_sync(
            &opaque,
            &SyncSignal::CursorUnusable,
            &account,
            &version,
            now,
        ),
        "a dead token is an outcome",
    );
    assert_eq!(invalidated.advance, SyncAdvance::TokenInvalidated);
    assert!(
        invalidated.cursor.is_none(),
        "an invalidated token must not be carried forward"
    );
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
        advance_calendar_sync(
            &opaque,
            &SyncSignal::Advanced {
                history_id: Some("AAA=".to_owned()),
            },
            &account,
            &version,
            now,
        ),
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
    // limits or the page cap would change what a full sync costs without anything failing.
    assert_eq!(GMAIL_MAX_RESULTS_CAP, 500);
    assert_eq!(GMAIL_BATCH_HARD_LIMIT, 100);
    assert_eq!(GMAIL_BATCH_RECOMMENDED, 50);
    assert_eq!(GOOGLE_MAX_BACKOFF_SECONDS, 64);
    assert_eq!(GOOGLE_RETRY_FLOOR_SECONDS, 1);
    assert_eq!(GOOGLE_API_HOST, "www.googleapis.com");
    assert!(GMAIL_API_BASE.ends_with("/gmail/v1"));
    assert!(CALENDAR_API_BASE.ends_with("/calendar/v3"));
}

#[test]
fn the_batch_recommendation_is_below_the_hard_limit_and_the_hard_limit_below_the_page_cap() {
    // **The two batch figures are not interchangeable, and neither is the page cap.** The batch reference
    // states a hard ceiling of 100 and *recommends* no more than 50 because larger batches trigger throttling;
    // a single 50 constant documented as "the largest Gmail accepts" conflated a **refusal** with a
    // **slowdown**, and a caller reading it as the ceiling would never use the 50–100 range that is permitted.
    //
    // Bound to locals first, because `assert!` on two constants is a *constant assertion* — the trap this
    // workspace records.
    let recommended = GMAIL_BATCH_RECOMMENDED;
    let hard = GMAIL_BATCH_HARD_LIMIT;
    let page = GMAIL_MAX_RESULTS_CAP;
    assert!(
        recommended < hard,
        "the recommendation must be strictly below the hard limit, or it says nothing"
    );
    assert!(
        hard < page,
        "a batch is not a page; confusing them would ask for 500 sub-requests at once"
    );
    // And the recommendation is exactly half the hard limit, which is the documented pair rather than a
    // coincidence: a change to either figure alone should be a deliberate edit.
    assert_eq!(
        hard,
        recommended * 2,
        "the published pair is 100 accepted and 50 recommended"
    );
}

#[test]
fn a_batch_size_above_the_hard_limit_is_refused_and_one_above_the_recommendation_is_not() {
    // **The distinction between the two figures, asserted in both directions.** A size above the hard limit
    // is refused because the provider would reject the request; a size above the recommendation is
    // **permitted** and merely invites throttling, so refusing it would be stricter than Google and would hide
    // the 50–100 range the API actually accepts.
    assert!(matches!(batch_plan(1, 0), Err(BatchPlanError::EmptyBatch)));
    assert!(matches!(
        batch_plan(1, GMAIL_BATCH_HARD_LIMIT + 1),
        Err(BatchPlanError::AboveHardLimit { .. })
    ));
    // Exactly the hard limit is accepted — the boundary is inclusive, which is what "limited to 100" means.
    assert!(batch_plan(1, GMAIL_BATCH_HARD_LIMIT).is_ok());

    // Above the recommendation is allowed and **reported**, not refused.
    let unwise = must(
        batch_plan(101, 100),
        "100 is the hard limit and is permitted",
    );
    assert!(!unwise.is_within_recommendation());
    assert_eq!(unwise.batch_size, 100);
    // The control: a size at the recommendation reports itself as within it, so the predicate is not
    // false for everything.
    let recommended = must(
        batch_plan(101, GMAIL_BATCH_RECOMMENDED),
        "50 is recommended",
    );
    assert!(recommended.is_within_recommendation());
}

#[test]
fn the_batch_plan_counts_the_partial_final_batch_rather_than_dropping_it() {
    // **The arithmetic a naive planner gets wrong.** 101 calls at 50 per batch is three requests, not two, and
    // the third is partial. A planner that used floor division would drop the remainder's request and silently
    // skip part of a sync.
    let plan = must(batch_plan(101, 50), "a valid plan");
    assert_eq!(
        plan.requests, 3,
        "101 calls at 50 per batch is three requests"
    );
    assert_eq!(
        plan.final_batch_size, 1,
        "the last batch holds the remainder"
    );

    // An exact division has no partial batch, reported as zero rather than as a repeated full one.
    let exact = must(batch_plan(100, 50), "a valid plan");
    assert_eq!(exact.requests, 2);
    assert_eq!(exact.final_batch_size, 0);

    // One and zero calls are the boundaries: one call is one request, and nothing to send is no requests.
    let single = must(batch_plan(1, 50), "a valid plan");
    assert_eq!(single.requests, 1);
    assert_eq!(single.final_batch_size, 1);
    let empty = must(batch_plan(0, 50), "a valid plan");
    assert_eq!(empty.requests, 0);
    assert_eq!(empty.final_batch_size, 0);

    // And a full first sync's shape, using the record's own `5 + 20N` framing: 1,000 messages means 1,001
    // calls once the listing call is counted, which is 21 requests of 50 with the last holding one.
    let sync = must(batch_plan(1_001, GMAIL_BATCH_RECOMMENDED), "a valid plan");
    assert_eq!(sync.requests, 21);
    assert_eq!(sync.final_batch_size, 1);
    assert!(sync.is_within_recommendation());
}

#[test]
fn a_batch_of_one_is_permitted_because_chopping_is_a_caller_choice() {
    // The lower boundary, asserted so the two upper bounds are not the only ones in play: a size of 1 is
    // within the recommendation and divides `n` calls into `n` requests. A caller pacing very conservatively
    // is permitted to do that, and this test is what keeps a future lower bound from being added silently.
    let singles = must(batch_plan(3, 1), "a valid plan");
    assert_eq!(singles.requests, 3);
    assert_eq!(singles.final_batch_size, 0);
    assert!(singles.is_within_recommendation());
}
