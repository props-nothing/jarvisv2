//! Tests for rate limits and retry classification.
//!
//! The central assertion is the retry table: **the class and the idempotency together** decide, and `Unknown`
//! never retries whatever the idempotency. A test that only checked one column would pass for an
//! implementation that ignored the other, which is exactly the defect that sends a second effect.

use super::*;

fn must<T, E: std::fmt::Display>(result: Result<T, E>, what: &str) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("{what}: {error}"),
    }
}

fn limit(per_window: u32, window_seconds: u32, burst: u32) -> RateLimit {
    must(
        RateLimit::new(
            per_window,
            window_seconds,
            burst,
            RateLimitUnit::Requests,
            RateLimitScope::PerAccount,
            RateLimitEvidence::Documented,
        ),
        "a valid rate limit",
    )
}

#[test]
fn only_a_documented_limit_may_be_planned_against() {
    // Planning against an observed limit means the plan is derived from the workload that produced the
    // observation rather than from the provider's rules.
    assert!(RateLimitEvidence::Documented.is_plannable());
    assert!(!RateLimitEvidence::Observed.is_plannable());
    assert!(!RateLimitEvidence::Unknown.is_plannable());
    let codes: Vec<&str> = [
        RateLimitEvidence::Documented,
        RateLimitEvidence::Observed,
        RateLimitEvidence::Unknown,
    ]
    .iter()
    .map(|evidence| evidence.as_str())
    .collect();
    let mut unique = codes.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(
        unique.len(),
        codes.len(),
        "the evidence codes must be distinct"
    );
}

#[test]
fn a_rate_limit_refuses_a_zero_or_unbounded_shape() {
    // Zero is refused rather than read as "unlimited", which is the reasoning every other bound in this
    // workspace records: a zero read as unlimited is an unbounded budget.
    for (per_window, window_seconds, burst) in [
        (0, 60, 10),
        (MAX_RATE_LIMIT_PER_WINDOW + 1, 60, 10),
        (100, 0, 10),
        (100, 60, 0),
        (100, 60, MAX_RATE_LIMIT_BURST + 1),
    ] {
        assert!(
            matches!(
                RateLimit::new(
                    per_window,
                    window_seconds,
                    burst,
                    RateLimitUnit::Requests,
                    RateLimitScope::Global,
                    RateLimitEvidence::Observed
                ),
                Err(RateLimitError::Shape { .. })
            ),
            "({per_window}, {window_seconds}, {burst}) must be refused"
        );
    }
    // Both ends of each bound are accepted, so the checks are boundaries rather than over-restrictions.
    assert!(
        RateLimit::new(
            MAX_RATE_LIMIT_PER_WINDOW,
            1,
            MAX_RATE_LIMIT_BURST,
            RateLimitUnit::Requests,
            RateLimitScope::PerUser,
            RateLimitEvidence::Documented
        )
        .is_ok()
    );
    assert!(
        RateLimit::new(
            1,
            1,
            1,
            RateLimitUnit::Requests,
            RateLimitScope::PerClient,
            RateLimitEvidence::Documented
        )
        .is_ok()
    );
}

#[test]
fn the_sustained_rate_rounds_down_so_a_caller_stays_inside_the_limit() {
    // The rounding direction is deliberate: a rate that under-estimates keeps a caller inside the provider's
    // limit, while rounding up would exceed it by a fraction on every window. `P3-002`'s unreachable-bound
    // lesson applies — the arithmetic has to be the one that binds.
    assert_eq!(limit(250, 60, 50).sustained_per_second(), 4);
    assert_eq!(limit(300, 60, 50).sustained_per_second(), 5);
    assert_eq!(limit(59, 60, 50).sustained_per_second(), 0);
    assert_eq!(limit(1, 3_600, 1).sustained_per_second(), 0);
    // And the burst is asserted on both sides, so a limit with no usable burst is visible.
    let limit = limit(250, 60, 50);
    assert!(limit.admits_burst(50));
    assert!(!limit.admits_burst(51));
    assert!(limit.admits_burst(1));
}

#[test]
fn a_shared_budget_is_distinguishable_from_a_per_account_one() {
    // A provider limit is usually per account while a JARVIS budget is per workspace, so a connector that
    // conflated them would let one workspace exhaust a shared account's quota and report the refusal as that
    // workspace's own limit.
    assert!(RateLimitScope::Global.is_shared_between_accounts());
    assert!(RateLimitScope::PerClient.is_shared_between_accounts());
    assert!(!RateLimitScope::PerAccount.is_shared_between_accounts());
    assert!(!RateLimitScope::PerUser.is_shared_between_accounts());
    for scope in [
        RateLimitScope::PerAccount,
        RateLimitScope::PerUser,
        RateLimitScope::PerClient,
        RateLimitScope::Global,
    ] {
        assert!(!scope.as_str().is_empty());
    }
    // The budget outcome that means "another account spent it" is separate, because the remedy is not this
    // account's.
    assert!(BudgetOutcome::Allowed.is_allowed());
    assert!(!BudgetOutcome::SharedBudgetSpent.is_allowed());
    assert!(!BudgetOutcome::Exhausted.is_allowed());
    assert!(!BudgetOutcome::WaitForSeconds(5).is_allowed());
}

#[test]
fn only_a_short_stated_delay_is_worth_waiting_and_retrying() {
    // The whole reason `Exhausted` and `SharedBudgetSpent` are separate variants: a scheduler that retried
    // them in a loop would spin for the length of a quota window.
    assert!(BudgetOutcome::WaitForSeconds(5).should_wait_and_retry());
    assert!(
        !BudgetOutcome::Exhausted.should_wait_and_retry(),
        "a quota window is minutes or hours, not a delay to spin on"
    );
    assert!(
        !BudgetOutcome::SharedBudgetSpent.should_wait_and_retry(),
        "another account's spending is not this caller's delay"
    );
    assert!(!BudgetOutcome::Allowed.should_wait_and_retry());
}

#[test]
fn the_retry_decision_needs_both_the_class_and_the_idempotency() {
    // The central table. Each row is asserted in both columns, because a test that checked only one would
    // pass for an implementation that ignored the other — and ignoring the idempotency is what sends a
    // second effect.
    use crate::manifest::ProviderIdempotency;

    let table = [
        // class, idempotent, not idempotent, unknown
        (RetryClass::Transient, true, false, false),
        (RetryClass::Throttled, true, false, false),
        (RetryClass::ProviderFault, true, false, false),
        // A permanent refusal never retries, whatever the provider says about idempotency.
        (RetryClass::Permanent, false, false, false),
        // A request needing new authorization will fail identically until a user acts.
        (RetryClass::Authentication, false, false, false),
        // The dangerous class: the question it asks — "did the effect happen" — has not been answered, and a
        // retry answers it by making it happen twice.
        (RetryClass::Unknown, false, false, false),
    ];
    for (class, declared, not_idempotent, unknown) in table {
        assert_eq!(
            class.permits_automatic_retry(ProviderIdempotency::Declared),
            declared,
            "{class:?} with a declared idempotency"
        );
        assert_eq!(
            class.permits_automatic_retry(ProviderIdempotency::ProviderKey),
            declared,
            "{class:?} with a provider key"
        );
        assert_eq!(
            class.permits_automatic_retry(ProviderIdempotency::NotIdempotent),
            not_idempotent,
            "{class:?} with a documented duplication"
        );
        assert_eq!(
            class.permits_automatic_retry(ProviderIdempotency::Unknown),
            unknown,
            "{class:?} with an unknown idempotency"
        );
    }
    // `Unknown` refuses in EVERY column, which is the property a per-column table alone cannot state.
    for idempotency in [
        ProviderIdempotency::Declared,
        ProviderIdempotency::ProviderKey,
        ProviderIdempotency::Unknown,
        ProviderIdempotency::NotIdempotent,
    ] {
        assert!(
            !RetryClass::Unknown.permits_automatic_retry(idempotency),
            "an unknown outcome must never retry automatically, whatever the idempotency"
        );
    }
    // Only an authentication refusal needs a user.
    for class in [
        RetryClass::Transient,
        RetryClass::Throttled,
        RetryClass::Permanent,
        RetryClass::ProviderFault,
        RetryClass::Unknown,
    ] {
        assert!(!class.needs_user(), "{class:?} must not need a user");
    }
    assert!(RetryClass::Authentication.needs_user());
}

#[test]
fn the_guidance_and_the_class_travel_together_so_a_delay_cannot_be_missed() {
    // `P3-006a`'s shape: a class a caller could act on without reading the delay would retry immediately,
    // which for a `Throttled` response is the one thing the provider asked it not to do.
    let throttled = RetryDecision {
        class: RetryClass::Throttled,
        guidance: RetryGuidance::RetryAfterSeconds(30),
        provider_request_id: None,
    };
    assert_eq!(throttled.guidance.delay_seconds(), Some(30));
    assert!(throttled.guidance.permits_retry());
    // An `Unknown` outcome's guidance is `Reconcile`, and it is the only guidance that is not a retry and not
    // a refusal — because the instruction is to establish what happened first.
    let unknown = RetryDecision {
        class: RetryClass::Unknown,
        guidance: RetryGuidance::Reconcile,
        provider_request_id: None,
    };
    assert!(!unknown.guidance.permits_retry());
    assert_eq!(unknown.guidance.delay_seconds(), None);
    assert!(
        !unknown
            .class
            .permits_automatic_retry(crate::manifest::ProviderIdempotency::Declared)
    );
    // Every guidance either states a delay or refuses, and the two sets are complements — so a guidance
    // cannot be one that permits a retry without saying when. The variants are listed explicitly because a
    // property asserted over an incomplete set of variants is a property about the list, not about the type.
    for guidance in [
        RetryGuidance::RetryAfterSeconds(1),
        RetryGuidance::BackoffSeconds(1),
        RetryGuidance::BackoffAfterUnreadableDelay(1),
        RetryGuidance::DeferSeconds(1),
        RetryGuidance::DoNotRetry,
        RetryGuidance::Reauthenticate,
        RetryGuidance::Reconcile,
    ] {
        assert_eq!(
            guidance.permits_retry(),
            guidance.delay_seconds().is_some(),
            "{guidance:?} must state a delay exactly when it permits a retry"
        );
    }
    // And every variant's two accessors are disjoint except where the retry itself states the delay, so no
    // caller can read a "when" without knowing whether it is a retry or a deferral.
    for guidance in [
        RetryGuidance::RetryAfterSeconds(1),
        RetryGuidance::BackoffSeconds(1),
        RetryGuidance::BackoffAfterUnreadableDelay(1),
        RetryGuidance::DeferSeconds(1),
        RetryGuidance::DoNotRetry,
        RetryGuidance::Reauthenticate,
        RetryGuidance::Reconcile,
    ] {
        assert!(
            !(guidance.delay_seconds().is_some() && guidance.deferred_seconds().is_some()),
            "{guidance:?} must not answer both accessors"
        );
    }
}

#[test]
fn a_provider_request_identifier_may_not_carry_a_control_character() {
    // It is the string an operator reads back to a vendor's support, so a newline forges a log line and a
    // control character rewrites a terminal.
    for unusable in ["", "req\n1", "req\u{0}1", "req\u{1b}[31m"] {
        assert!(
            ProviderRequestId::new(unusable).is_err(),
            "`{}` must be refused as a request identifier",
            unusable.escape_debug()
        );
    }
    let identifier = must(
        ProviderRequestId::new("req-abc-123"),
        "a request identifier",
    );
    assert_eq!(identifier.as_str(), "req-abc-123");
    assert_eq!(identifier.to_string(), "req-abc-123");
    assert!(
        ProviderRequestId::new("x".repeat(ProviderRequestId::MAX_BYTES + 1)).is_err(),
        "an unbounded identifier must be refused"
    );
    assert!(ProviderRequestId::new("x".repeat(ProviderRequestId::MAX_BYTES)).is_ok());
}

#[test]
fn a_retry_delay_longer_than_a_caller_may_hold_is_deferred_rather_than_clamped() {
    // A provider asking for longer than an hour is describing a quota window rather than a transient limit, and
    // clamping would silently retry sooner than the provider asked — the direction that gets a caller blocked.
    // So the refusal is a **variant**, and this test asserts both sides of the bound, because a bound asserted
    // on one side only would pass for a constructor that never applied it at all.
    let at_the_ceiling = RetryGuidance::for_stated_delay(MAX_RETRY_AFTER_SECONDS);
    assert_eq!(
        at_the_ceiling,
        RetryGuidance::RetryAfterSeconds(MAX_RETRY_AFTER_SECONDS),
        "a delay the caller may hold is an ordinary stated delay"
    );
    assert!(at_the_ceiling.permits_retry());

    let above = RetryGuidance::for_stated_delay(MAX_RETRY_AFTER_SECONDS + 1);
    assert_eq!(
        above,
        RetryGuidance::DeferSeconds(MAX_RETRY_AFTER_SECONDS + 1),
        "one second above the ceiling must defer, not retry"
    );
    // **The load-bearing assertion.** `delay_seconds()` answers "how long before the automatic retry", so it
    // must be `None` for a deferral — a caller that read a number here would wait out a whole quota window
    // inside a retry loop. The provider's number is not lost; it moves to `deferred_seconds`.
    assert!(
        !above.permits_retry(),
        "a deferral is not an automatic retry"
    );
    assert_eq!(above.delay_seconds(), None);
    assert_eq!(
        above.deferred_seconds(),
        Some(MAX_RETRY_AFTER_SECONDS + 1),
        "the stated delay must remain readable for whoever schedules the deferral"
    );
    // And the two accessors are complements in the direction that matters: a deferral states a delay through
    // exactly one of them, so a scheduler cannot miss it by reading the wrong one.
    assert_eq!(above.delay_seconds().is_some(), above.permits_retry());
    assert_eq!(
        above.deferred_seconds().is_some(),
        !above.permits_retry() && above != RetryGuidance::DoNotRetry,
        "only the deferral states a delay while refusing the retry"
    );
    // A value far above the ceiling is still a deferral rather than a clamp or an overflow.
    assert_eq!(
        RetryGuidance::for_stated_delay(u32::MAX),
        RetryGuidance::DeferSeconds(u32::MAX)
    );
}
