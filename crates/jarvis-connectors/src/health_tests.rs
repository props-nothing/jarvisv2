//! Tests for the health contract.
//!
//! The two assertions that matter are that a health state cannot exist without a probe, and that staleness
//! is measured against a **supplied instant** rather than the system clock. The second is what makes
//! `security.md`'s "stale evidence fails closed" testable at all: a value that asked the clock about its own
//! age could not be checked against a known time.

use super::*;

fn must<T, E: std::fmt::Display>(result: Result<T, E>, what: &str) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("{what}: {error}"),
    }
}

fn at(seconds: i64) -> UtcTimestamp {
    must(
        UtcTimestamp::from_unix_nanos(i128::from(seconds) * 1_000_000_000),
        "a valid instant",
    )
}

fn signal(probe: HealthProbe, succeeded: bool, at_seconds: i64) -> HealthSignal {
    must(
        HealthSignal::new(probe, succeeded, None, at(at_seconds)),
        "a valid health signal",
    )
}

#[test]
fn every_health_state_carries_the_observation_behind_it() {
    // A state on its own is a claim, and `A10` requires that onboarding "verifies provider identity and
    // connectivity before saving" — a statement about evidence. So a health value cannot be built without a
    // probe, which is checked by construction: the only fields are signals, and the `Unknown` state carries
    // a probe of `None` so the value says WHAT was not checked.
    let observed = signal(HealthProbe::Identity, true, 1_700_000_000);
    let states = [
        ConnectorHealth::Connected {
            signal: observed.clone(),
        },
        ConnectorHealth::Degraded {
            signal: observed.clone(),
            retry_after: Some(at(1_700_000_060)),
        },
        ConnectorHealth::NeedsReauth {
            reason: ReauthReason::Revoked,
            missing_scopes: Vec::new(),
            signal: observed.clone(),
        },
        ConnectorHealth::Disconnected {
            signal: observed.clone(),
        },
        ConnectorHealth::Unknown {
            signal: must(
                HealthSignal::new(HealthProbe::None, false, None, at(0)),
                "an unknown signal",
            ),
        },
    ];
    for state in &states {
        // The signal must be readable from every variant through the accessor, rather than only by matching —
        // a caller that had to match every variant would break on a new one.
        assert!(
            !state.signal().probe().as_str().is_empty(),
            "every state must expose which probe produced it"
        );
        // `Display` uses the probe's own vocabulary, so an operator sees which check produced the state.
        let rendered = state.to_string();
        assert!(!rendered.is_empty(), "every state must render");
    }
    // The `Connected` state's signal is the one it was built with, which pins that the field is not dropped
    // by a constructor somewhere.
    match &states[0] {
        ConnectorHealth::Connected { signal } => {
            assert_eq!(signal.probe(), HealthProbe::Identity);
            assert_eq!(signal.observed_at(), at(1_700_000_000));
        }
        other => panic!("expected a connected state, got {other:?}"),
    }
    // The `Unknown` state's probe is `None`, which is the value that says nothing was checked rather than
    // reporting one of the definite checks.
    match &states[4] {
        ConnectorHealth::Unknown { signal } => assert_eq!(signal.probe(), HealthProbe::None),
        other => panic!("expected an unknown state, got {other:?}"),
    }
}

#[test]
fn only_connected_and_degraded_permit_a_call_and_unknown_does_not() {
    // `security.md` fails closed, so `Unknown` — the state an unchecked account is in — permits nothing. This
    // is the assertion that stops a platform defaulting an untested account into service.
    let observed = signal(HealthProbe::Read, true, 1_700_000_000);
    let permitted = [
        ConnectorHealth::Connected {
            signal: observed.clone(),
        },
        ConnectorHealth::Degraded {
            signal: observed.clone(),
            retry_after: None,
        },
    ];
    for state in &permitted {
        assert!(state.permits_calls(), "{state:?} must permit calls");
    }
    let refused = [
        ConnectorHealth::NeedsReauth {
            reason: ReauthReason::Expired,
            missing_scopes: Vec::new(),
            signal: observed.clone(),
        },
        ConnectorHealth::Disconnected {
            signal: observed.clone(),
        },
        ConnectorHealth::Unknown {
            signal: signal(HealthProbe::None, false, 0),
        },
    ];
    for state in &refused {
        assert!(!state.permits_calls(), "{state:?} must not permit calls");
    }
    // A `Degraded` account still permits calls, which is the deliberate over-permissive direction: a rate
    // limit is not a revocation, and refusing everything would make an ordinary throttle look like a
    // disconnect.
    assert!(
        ConnectorHealth::Degraded {
            signal: observed.clone(),
            retry_after: None
        }
        .permits_calls()
    );
    // Only a reauth state needs a user.
    for state in permitted.iter().chain(refused.iter()) {
        let needs = state.needs_user();
        assert_eq!(
            needs,
            matches!(state, ConnectorHealth::NeedsReauth { .. }),
            "{state:?} needs_a_user disagrees with its variant"
        );
    }
}

#[test]
fn staleness_is_measured_against_a_supplied_instant_at_both_bounds() {
    // The stale-evidence rule. `is_fresh_at` takes the instant rather than consulting a clock, so the
    // boundary is testable — and the boundary is asserted on both sides so an off-by-one that refused
    // everything would fail.
    let state = ConnectorHealth::Connected {
        signal: signal(HealthProbe::Refresh, true, 1_000_000),
    };
    assert!(
        state.is_fresh_at(at(1_000_000), 0),
        "an instant is fresh at itself"
    );
    assert!(
        state.is_fresh_at(at(1_000_300), 300),
        "exactly the freshness bound must be fresh"
    );
    assert!(
        !state.is_fresh_at(at(1_000_301), 300),
        "one second beyond the bound must not be fresh"
    );
    assert!(
        !state.is_fresh_at(at(2_000_000), 300),
        "a long-elapsed observation must not be fresh"
    );
    // A future observation is NOT fresh: it means a clock moved backwards, and treating it as fresh would let
    // an arbitrarily old state look current after a clock adjustment.
    assert!(
        !state.is_fresh_at(at(999_999), 300),
        "an observation from the future must not be fresh"
    );
    // And `permits_calls_at` is the combined answer a caller should use, because calling `permits_calls` on an
    // unfresh state is the defect the pair exists to prevent.
    assert!(state.permits_calls_at(at(1_000_300), 300));
    assert!(!state.permits_calls_at(at(1_000_301), 300));
    // A state that does not permit calls cannot be made to permit them by freshness.
    let reauth = ConnectorHealth::NeedsReauth {
        reason: ReauthReason::Revoked,
        missing_scopes: Vec::new(),
        signal: signal(HealthProbe::Refresh, false, 1_000_000),
    };
    assert!(!reauth.permits_calls_at(at(1_000_000), 300));
}

#[test]
fn a_health_detail_may_not_carry_a_control_character_because_a_terminal_renders_it() {
    // `doctor` renders a detail into an operator's terminal, where an escape sequence can rewrite the
    // surrounding lines — so a control character is refused rather than escaped, which is the same rule the
    // isolation module applies to untrusted text.
    for unusable in ["", "   ", "failed\u{7}", "line\nbreak", "\u{1b}[31mred"] {
        assert!(
            HealthSignal::new(HealthProbe::Read, false, Some(unusable.to_owned()), at(0)).is_err(),
            "a detail of `{}` must be refused",
            unusable.escape_debug()
        );
    }
    assert!(HealthSignal::new(HealthProbe::Read, false, None, at(0)).is_ok());
    assert!(
        HealthSignal::new(
            HealthProbe::Read,
            false,
            Some("token endpoint returned 400".to_owned()),
            at(0),
        )
        .is_ok()
    );
    assert!(
        HealthSignal::new(
            HealthProbe::Read,
            false,
            Some("x".repeat(MAX_HEALTH_DETAIL_CHARS + 1)),
            at(0),
        )
        .is_err()
    );
    let signal = HealthSignal::succeeded(HealthProbe::Identity, at(1_700_000_000));
    assert!(signal.is_success());
    assert_eq!(signal.detail(), None);
    assert_eq!(signal.probe(), HealthProbe::Identity);
}

#[test]
fn a_reauth_reason_says_whether_a_user_can_resolve_it() {
    // The interesting column is `provider_refused`: if the provider rejected the client itself, a user
    // reconnecting hits the same refusal, so a message telling them to reconnect would be a loop.
    // `P3-008i` records this shape of defect — a message must not assert a cause it cannot know.
    let table = [
        (ReauthReason::Revoked, true),
        (ReauthReason::Expired, true),
        (ReauthReason::ScopeLoss, true),
        (ReauthReason::ProviderRefused, false),
    ];
    for (reason, resolvable) in table {
        assert_eq!(
            reason.is_user_resolvable(),
            resolvable,
            "{reason:?} resolvable by a user"
        );
        assert!(!reason.as_str().is_empty());
    }
    // Every reason has a distinct code, so a stored value cannot be ambiguous.
    let codes: Vec<&str> = table.iter().map(|(reason, _)| reason.as_str()).collect();
    let mut unique = codes.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(
        unique.len(),
        codes.len(),
        "the reason codes must be distinct"
    );
    // And the accessor on the state reports the reason only when there is one.
    let with_reason = ConnectorHealth::NeedsReauth {
        reason: ReauthReason::ScopeLoss,
        missing_scopes: vec!["read".to_owned()],
        signal: signal(HealthProbe::Read, false, 0),
    };
    assert_eq!(with_reason.reauth_reason(), Some(ReauthReason::ScopeLoss));
    let without = ConnectorHealth::Connected {
        signal: signal(HealthProbe::Read, true, 0),
    };
    assert_eq!(without.reauth_reason(), None);
}

#[test]
fn the_missing_scopes_on_a_reauth_state_are_reachable_and_empty_for_every_other_state() {
    // **The reading that did not exist.** `NeedsReauth`'s `missing_scopes` was documented as "the list a reauth
    // prompt needs" and written by every construction — and read by nothing, because `diagnostics_for` took a
    // SEPARATE list from its caller. So a caller that built a `ScopeLoss` state and passed an empty shortfall
    // reported no missing scopes at all. The accessor is the reader that closes that (`ADR-0116`).
    let scope_loss = ConnectorHealth::NeedsReauth {
        reason: ReauthReason::ScopeLoss,
        missing_scopes: vec!["calendar.readonly".to_owned(), "gmail.readonly".to_owned()],
        signal: signal(HealthProbe::Read, false, 0),
    };
    assert_eq!(
        scope_loss.missing_scopes(),
        ["calendar.readonly", "gmail.readonly"],
        "the state's own list must be reachable rather than dropped"
    );
    // A reauth for another reason still carries the field, and an empty one is not a missing accessor: the two
    // are told apart by matching the state, and both return a slice so a caller branches on `is_empty`.
    let revoked = ConnectorHealth::NeedsReauth {
        reason: ReauthReason::Revoked,
        missing_scopes: Vec::new(),
        signal: signal(HealthProbe::Refresh, false, 0),
    };
    assert!(revoked.missing_scopes().is_empty());
    assert_eq!(revoked.reauth_reason(), Some(ReauthReason::Revoked));
    // Every state that carries no list returns an **empty slice**, not a panic and not a default: an absent
    // list and an empty one call for the same action, which is why the return is a slice rather than an
    // `Option<&Vec>`.
    let others = [
        ConnectorHealth::Connected {
            signal: signal(HealthProbe::Identity, true, 0),
        },
        ConnectorHealth::Degraded {
            signal: signal(HealthProbe::RateLimit, false, 0),
            retry_after: None,
        },
        ConnectorHealth::Disconnected {
            signal: signal(HealthProbe::None, false, 0),
        },
        ConnectorHealth::Unknown {
            signal: signal(HealthProbe::None, false, 0),
        },
    ];
    for state in &others {
        assert!(
            state.missing_scopes().is_empty(),
            "{state:?} carries no scope list, so the accessor must answer empty"
        );
    }
}

#[test]
fn an_inconclusive_probe_is_not_evidence_and_must_not_replace_an_observation() {
    // The distinction that stops a network problem from entering reauth: "we could not reach the provider" is
    // not evidence that anything is wrong with the ACCOUNT, so an inconclusive probe must leave the previous
    // state alone — whose age the staleness rule then reports.
    assert!(ProbeOutcome::Succeeded.is_evidence());
    assert!(
        ProbeOutcome::Failed {
            detail: "the token endpoint refused".to_owned()
        }
        .is_evidence()
    );
    assert!(
        !ProbeOutcome::Inconclusive {
            detail: "the host was unreachable".to_owned()
        }
        .is_evidence(),
        "an inconclusive probe is not evidence, so it must not update a health state"
    );
    // The three outcomes are distinct values, which is what lets a caller branch on them rather than parsing
    // a string — `P3-008c`'s rule about not deriving a decision from message text.
    let outcomes = [
        ProbeOutcome::Succeeded,
        ProbeOutcome::Failed {
            detail: "x".to_owned(),
        },
        ProbeOutcome::Inconclusive {
            detail: "x".to_owned(),
        },
    ];
    for (index, first) in outcomes.iter().enumerate() {
        for (other_index, second) in outcomes.iter().enumerate() {
            assert_eq!(
                index == other_index,
                first == second,
                "{first:?} and {second:?} must differ"
            );
        }
    }
}
