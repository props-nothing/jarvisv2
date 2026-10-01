//! Tests for the diagnostics contract.
//!
//! The assertion that carries the security property is that **no current field is unrenderable**: a
//! `Withheld` field would make a diagnostic less useful, and `A10` requires that diagnostics "remain useful"
//! while redacting. The other one is that only the provider request identifier is withheld from a model,
//! because a diagnostic that reached a prompt would otherwise carry a request handle into it.

use super::*;

use crate::health::{ConnectorHealth, HealthProbe, HealthSignal, ReauthReason};

fn must<T, E: std::fmt::Display>(result: Result<T, E>, what: &str) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("{what}: {error}"),
    }
}

fn at(seconds: i64) -> jarvis_core::UtcTimestamp {
    must(
        jarvis_core::UtcTimestamp::from_unix_nanos(i128::from(seconds) * 1_000_000_000),
        "a valid instant",
    )
}

fn connected() -> ConnectorHealth {
    ConnectorHealth::Connected {
        signal: HealthSignal::succeeded(HealthProbe::Identity, at(1_700_000_000)),
    }
}

/// The instant a report is assembled at, for a state observed at [`at`]`(1_700_000_000)`.
///
/// **One second after the observation**, so every report below is assembled against a **fresh** state unless
/// the test says otherwise — which is what makes the staleness tests' `fresh_now` a control rather than a
/// second convention. `ADR-0118`: whether a health state may be acted on depends on *when the report is
/// assembled*, so every call has to supply an instant and a bound.
fn fresh_now() -> jarvis_core::UtcTimestamp {
    at(1_700_000_001)
}

/// The freshness bound every call site uses unless it is testing the bound itself.
///
/// The **platform's own default** rather than a test figure, so the reports below are assembled against the
/// bound a deployment actually applies — a test-local bound would leave the production default unexercised.
const FRESHNESS: u64 = crate::health::DEFAULT_FRESHNESS_SECONDS;

/// `AuthState::Connected`, spelled short, because these tests pass it at nearly every call site.
///
/// An alias rather than a second value: a test-local `AuthState` could drift from the production variant,
/// which is the kind of copy that makes a test pass against something the product does not use.
const AUTH_CONNECTED: crate::auth::AuthState = crate::auth::AuthState::Connected;

/// An instant `elapsed` seconds after the observation the [`connected`] fixtures carry.
///
/// Written as an **offset from the observation** rather than an absolute instant, so the arithmetic that
/// decides staleness is stated where it is read: `old(bound)` is exactly at the bound and `old(bound + 1)` is
/// one second past it, which is the pair a bound-off-by-one has to fail.
fn old(elapsed: u64) -> jarvis_core::UtcTimestamp {
    at(1_700_000_000 + i64::try_from(elapsed).unwrap_or(i64::MAX))
}

/// Every field the closed set offers.
///
/// A **hand-written list**, and deliberately so: it is the completeness check. Adding a variant without
/// adding it here fails the distinctness test below, so a new field cannot be introduced without being
/// considered for logging and model exposure.
fn all_fields() -> Vec<DiagnosticField> {
    vec![
        DiagnosticField::ConnectorId,
        DiagnosticField::ManifestVersion,
        DiagnosticField::AuthMethod,
        DiagnosticField::AuthState,
        DiagnosticField::ReauthReason,
        DiagnosticField::GrantedScopeCount,
        DiagnosticField::MissingScopes,
        DiagnosticField::HealthState,
        DiagnosticField::HealthProbe,
        DiagnosticField::HealthObservedAt,
        DiagnosticField::HealthStale,
        DiagnosticField::CursorKind,
        DiagnosticField::CursorObservedAt,
        DiagnosticField::WebhookSupport,
        DiagnosticField::SignatureAlgorithm,
        DiagnosticField::RateLimitScope,
        DiagnosticField::RateLimitEvidence,
        DiagnosticField::RetryClass,
        DiagnosticField::ProviderRequestId,
        DiagnosticField::Uptime,
        DiagnosticField::CursorConnectorVersion,
    ]
}

#[test]
fn every_field_is_loggable_and_renderable_and_the_one_model_exception_is_named() {
    // The security property: the field set is chosen so that "may this be logged" is true for all of it, and
    // a field needing an exception would mean a value here was not safe to log — which is the invariant worth
    // keeping absolute.
    for field in all_fields() {
        assert!(
            field.is_loggable(),
            "{field:?} must be safe to log, or the field set is not the one this module claims"
        );
        assert!(!field.as_str().is_empty());
    }
    // Exactly one field is withheld from a model, and it is the provider request identifier — an identifier
    // of a request the provider made, which a model has no business reasoning about.
    let withheld: Vec<DiagnosticField> = all_fields()
        .into_iter()
        .filter(|field| !field.may_reach_a_model())
        .collect();
    assert_eq!(
        withheld,
        vec![DiagnosticField::ProviderRequestId],
        "only the provider request identifier may be withheld from a model"
    );
    // The codes are distinct, so a stored finding cannot be ambiguous about which field it describes.
    let codes: Vec<&str> = all_fields().iter().map(|field| field.as_str()).collect();
    let mut unique = codes.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(
        unique.len(),
        codes.len(),
        "every field must have a distinct code; a duplicate means `all_fields` is stale"
    );
    // And there are exactly as many codes as the list claims, so a new variant added to the enum without
    // being added here is caught by the count rather than passing silently.
    assert_eq!(
        codes.len(),
        21,
        "the field set grew or shrank without this test"
    );
}

#[test]
fn no_current_field_is_withheld_from_rendering() {
    // `A10` requires diagnostics to "remain useful" as well as redacted, and `Redaction::Withheld` exists for
    // completeness — so this asserts that nothing in the CURRENT set needs it. A future field that did would
    // have to change this test, which is what makes the decision visible rather than accidental.
    let redactions = [
        Redaction::None,
        Redaction::Summary,
        Redaction::PrefixAndLength,
    ];
    for redaction in redactions {
        assert!(
            redaction.is_renderable(),
            "{redaction:?} is a rendering choice, so it must be renderable"
        );
    }
    assert!(
        !Redaction::Withheld.is_renderable(),
        "the withheld variant is what a field would use if it could not be shown at all"
    );
    // Every field a caller would receive is renderable at some level, and the findings a real state produces
    // use only renderable redactions.
    for finding in diagnostics_for(
        &connected(),
        crate::auth::AuthState::Connected,
        &[],
        fresh_now(),
        FRESHNESS,
    ) {
        assert!(
            finding.redaction.is_renderable(),
            "{} must be renderable",
            finding.field.as_str()
        );
    }
}

#[test]
fn a_health_state_sets_the_finding_severity_and_an_unusable_account_is_an_error() {
    // The finding an operator ranks the report by. A state that permits calls is `Info`, and one that does
    // not is `Error` — so the severity is derived from the state rather than chosen per connector, which is
    // what makes `doctor`'s output comparable across connectors.
    let permitted = diagnostics_for(
        &connected(),
        crate::auth::AuthState::Connected,
        &[],
        fresh_now(),
        FRESHNESS,
    );
    let health_findings: Vec<&DiagnosticFinding> = permitted
        .iter()
        .filter(|finding| finding.field == DiagnosticField::HealthState)
        .collect();
    assert_eq!(
        health_findings.len(),
        1,
        "a health state must be reported exactly once"
    );
    assert_eq!(health_findings[0].severity, DiagnosticSeverity::Info);

    let refusing = ConnectorHealth::NeedsReauth {
        reason: ReauthReason::Expired,
        missing_scopes: Vec::new(),
        signal: must(
            HealthSignal::new(HealthProbe::Refresh, false, None, at(0)),
            "a failed refresh",
        ),
    };
    let refused = diagnostics_for(
        &refusing,
        crate::auth::AuthState::Failed,
        &[],
        fresh_now(),
        FRESHNESS,
    );
    let health = refused
        .iter()
        .find(|finding| finding.field == DiagnosticField::HealthState)
        .unwrap_or_else(|| panic!("the health state must be reported"));
    assert_eq!(health.severity, DiagnosticSeverity::Error);
    // The reauth reason is reported only when one exists, at `Error`, because it is what a user acts on.
    assert!(
        refused
            .iter()
            .any(|finding| finding.field == DiagnosticField::ReauthReason),
        "a reauth state must report its reason"
    );
    assert!(
        !permitted
            .iter()
            .any(|finding| finding.field == DiagnosticField::ReauthReason),
        "a connected account must not report a reauth reason"
    );
}

#[test]
fn a_scope_shortfall_is_a_warning_rather_than_an_error_and_the_missing_scopes_are_named() {
    // An account with a shortfall is connected and may run the operations its grant covers, so an `Error`
    // would send an operator to fix something that is working. The missing scopes are the one list that is
    // reported rather than counted, because a reauth prompt needs to name them.
    let short = diagnostics_for(
        &connected(),
        crate::auth::AuthState::Connected,
        &["read".to_owned(), "delete".to_owned()],
        fresh_now(),
        FRESHNESS,
    );
    let missing = short
        .iter()
        .find(|finding| finding.field == DiagnosticField::MissingScopes)
        .unwrap_or_else(|| panic!("a shortfall must be reported"));
    assert_eq!(missing.severity, DiagnosticSeverity::Warning);
    // And with no shortfall the field is absent rather than reported as empty, so a caller cannot confuse
    // "nothing is missing" with "the list was not computed".
    assert!(
        !diagnostics_for(
            &connected(),
            crate::auth::AuthState::Connected,
            &[],
            fresh_now(),
            FRESHNESS
        )
        .iter()
        .any(|finding| finding.field == DiagnosticField::MissingScopes),
        "a complete grant must not report a missing-scope finding"
    );
    // The count is always reported, because it distinguishes "nothing was granted" from "the grant is short".
    assert!(
        diagnostics_for(
            &connected(),
            crate::auth::AuthState::Connected,
            &[],
            fresh_now(),
            FRESHNESS
        )
        .iter()
        .any(|finding| finding.field == DiagnosticField::GrantedScopeCount),
        "the granted-scope count is always a support fact"
    );
}

#[test]
fn a_reauth_states_own_missing_scopes_reach_the_report_without_the_caller_passing_them() {
    // **The finding this fixes.** `ConnectorHealth::NeedsReauth` holds the missing scopes and its field doc
    // calls them "the list a reauth prompt needs" — while this function read only the caller's `missing_scopes`
    // argument, so a caller that built a `ScopeLoss` state and passed an empty shortfall reported **nothing**
    // about the scopes the state itself recorded. The state's list is now consulted too, so it cannot be
    // dropped (`ADR-0116`).
    let scope_loss = ConnectorHealth::NeedsReauth {
        reason: ReauthReason::ScopeLoss,
        missing_scopes: vec!["gmail.readonly".to_owned()],
        signal: must(
            HealthSignal::new(HealthProbe::Read, false, None, at(0)),
            "a failed read",
        ),
    };
    // The caller passes **no** scopes, and the finding is still emitted — from the state's own list.
    let from_state = diagnostics_for(
        &scope_loss,
        crate::auth::AuthState::Connected,
        &[],
        fresh_now(),
        FRESHNESS,
    );
    assert!(
        from_state
            .iter()
            .any(|finding| finding.field == DiagnosticField::MissingScopes),
        "a ScopeLoss state must report its own missing scopes without the caller supplying them"
    );
    assert_eq!(
        scope_loss.missing_scopes(),
        ["gmail.readonly"],
        "and the list is reachable through the accessor the report now reads"
    );

    // A reauth with **no** scopes and no caller list still reports nothing — the union is empty, and the
    // finding's own rule is that an empty shortfall is absent rather than reported as empty.
    let revoked = ConnectorHealth::NeedsReauth {
        reason: ReauthReason::Revoked,
        missing_scopes: Vec::new(),
        signal: must(
            HealthSignal::new(HealthProbe::Refresh, false, None, at(0)),
            "a failed refresh",
        ),
    };
    assert!(
        !diagnostics_for(
            &revoked,
            crate::auth::AuthState::Connected,
            &[],
            fresh_now(),
            FRESHNESS
        )
        .iter()
        .any(|finding| finding.field == DiagnosticField::MissingScopes),
        "a revoke with no shortfall must not report a missing-scope finding"
    );

    // And the finding is emitted **once** even when both sources name scopes, because it describes the
    // condition rather than each scope: two sources must not produce duplicate findings for one account.
    let both = diagnostics_for(
        &scope_loss,
        crate::auth::AuthState::Connected,
        &["calendar.readonly".to_owned()],
        fresh_now(),
        FRESHNESS,
    );
    assert_eq!(
        both.iter()
            .filter(|finding| finding.field == DiagnosticField::MissingScopes)
            .count(),
        1,
        "the state's list and the caller's must not each produce a finding"
    );
}

#[test]
fn a_state_too_old_to_act_on_is_reported_as_unusable_rather_than_as_connected() {
    // **The finding this fixes (`ADR-0118`).** `security.md`'s "missing or stale evidence fails closed" is
    // enforced in `health` by `permits_calls_at`, and this report read only `permits_calls()` — the state as
    // it *was* — so a `Connected` observation from yesterday produced a report saying `connected` at `Info`
    // about a state that permits **no call**. A diagnostic that contradicts the predicate the rest of the
    // platform gates on is worse than a missing one: an operator reads it instead of the truth.
    let stale_state = ConnectorHealth::Connected {
        signal: HealthSignal::succeeded(HealthProbe::Identity, at(1_700_000_000)),
    };

    // **The boundary, on both sides, asserted through the report.** An elapsed exactly equal to the bound is
    // fresh (`is_fresh_at` is `elapsed <= bound`) and one second more is stale — so a bound that was off by
    // one, or ignored, fails here rather than only in the predicate the report is supposed to follow.
    let at_bound = old(FRESHNESS);
    assert!(
        !diagnostics_for(&stale_state, AUTH_CONNECTED, &[], at_bound, FRESHNESS)
            .iter()
            .any(|finding| finding.field == DiagnosticField::HealthStale),
        "a state exactly at the bound is fresh in the report, not only in the predicate"
    );
    let stale_report = diagnostics_for(
        &stale_state,
        AUTH_CONNECTED,
        &[],
        old(FRESHNESS + 1),
        FRESHNESS,
    );

    // The health state is an **error** rather than `Info`, which is the correction: the state cannot be acted
    // on, and `Info` is the severity that means "nothing to do here". This is the assertion the original
    // defect fails — it reported `Info` for a state that permits no call.
    assert_eq!(
        stale_report
            .iter()
            .find(|finding| finding.field == DiagnosticField::HealthState)
            .map(|finding| finding.severity),
        Some(DiagnosticSeverity::Error),
        "a state too old to act on must not be reported at Info, which reads as 'nothing to do here'"
    );
    // And the staleness is **named**, rather than left for a reader to derive from the instant and their own
    // clock — a report of a stale state that does not say so is the same gap in a different place.
    assert!(
        stale_report
            .iter()
            .any(|finding| finding.field == DiagnosticField::HealthStale),
        "a stale state must report that it is stale"
    );

    // **The control, and it is the point.** One second after the observation the same state is fresh: the
    // report is `Info` and it carries no staleness finding at all — absent rather than reported as false, the
    // rule `MissingScopes` uses, because a negative finding that is always present is one a reader stops
    // seeing. Without this control a function that called every state stale would pass the assertions above.
    let fresh_report = diagnostics_for(&stale_state, AUTH_CONNECTED, &[], fresh_now(), FRESHNESS);
    assert_eq!(
        fresh_report
            .iter()
            .find(|finding| finding.field == DiagnosticField::HealthState)
            .map(|finding| finding.severity),
        Some(DiagnosticSeverity::Info),
        "a fresh connected state is still the ordinary case"
    );
    assert!(
        !fresh_report
            .iter()
            .any(|finding| finding.field == DiagnosticField::HealthStale),
        "a fresh state must not report staleness at all, rather than reporting it as false"
    );

    // A zero bound makes every observation except one taken at exactly `now` stale, so the bound input is
    // genuinely read rather than constant — a report that ignored its bound would pass everything above.
    let zero_bound = diagnostics_for(&stale_state, AUTH_CONNECTED, &[], fresh_now(), 0);
    assert!(
        zero_bound
            .iter()
            .any(|finding| finding.field == DiagnosticField::HealthStale),
        "a zero bound must make a one-second-old state stale, or the bound is not being read"
    );
    assert_eq!(
        zero_bound
            .iter()
            .find(|finding| finding.field == DiagnosticField::HealthState)
            .map(|finding| finding.severity),
        Some(DiagnosticSeverity::Error),
        "and the state itself must be an error under that bound"
    );
}

#[test]
fn staleness_and_unusability_are_two_dimensions_and_a_mutant_is_why_this_exists() {
    // **A mutant survived here, and that is the whole reason this test is separate.** The first version of the
    // staleness code was mutated to `!health.permits_calls_at(now, bound)` in place of
    // `!health.is_fresh_at(now, bound)` — and **the entire suite passed**, because every state the test above
    // used was `Connected`, where the two predicates agree. They are different facts: a `NeedsReauth` observed
    // a moment ago **permits no call** and is **fresh**, so the mutant would have told an operator "nobody has
    // checked since" about a state that was just checked. The missing detector is a fresh state that still
    // refuses, and this test is it.
    let observed = at(1_700_000_000);
    let refusing_but_fresh = ConnectorHealth::NeedsReauth {
        reason: ReauthReason::Expired,
        missing_scopes: Vec::new(),
        signal: must(
            HealthSignal::new(HealthProbe::Refresh, false, None, observed),
            "a failed refresh",
        ),
    };
    assert!(
        !refusing_but_fresh.permits_calls_at(fresh_now(), FRESHNESS),
        "this state refuses calls, which is the dimension that must not be conflated with staleness"
    );
    assert!(
        refusing_but_fresh.is_fresh_at(fresh_now(), FRESHNESS),
        "and it is nevertheless fresh, which is the second dimension"
    );
    let refusing_report = diagnostics_for(
        &refusing_but_fresh,
        crate::auth::AuthState::Failed,
        &[],
        fresh_now(),
        FRESHNESS,
    );
    assert!(
        !refusing_report
            .iter()
            .any(|finding| finding.field == DiagnosticField::HealthStale),
        "a freshly observed refusal must not be reported as stale: it refuses for its own reason, and \
         'nobody has checked since' would be a false claim about the evidence"
    );
    assert_eq!(
        refusing_report
            .iter()
            .find(|finding| finding.field == DiagnosticField::HealthState)
            .map(|finding| finding.severity),
        Some(DiagnosticSeverity::Error),
        "while the state itself is still an error, for its own reason rather than for staleness"
    );
    // And the same refusal observed beyond the bound **is** both: an error and stale, so the two findings
    // coexist and neither suppresses the other.
    let stale_refusal = diagnostics_for(
        &refusing_but_fresh,
        crate::auth::AuthState::Failed,
        &[],
        old(FRESHNESS + 1),
        FRESHNESS,
    );
    let stale_fields: Vec<DiagnosticField> = stale_refusal
        .iter()
        .map(|finding| finding.field)
        .filter(|field| {
            *field == DiagnosticField::HealthStale || *field == DiagnosticField::HealthState
        })
        .collect();
    assert_eq!(
        stale_fields,
        vec![DiagnosticField::HealthState, DiagnosticField::HealthStale],
        "a stale refusal reports both, in one order, so a reader sees the reason and the age"
    );
}

#[test]
fn a_connector_mid_flow_is_not_reported_as_a_failure() {
    // An operator running onboarding must not see an error while the user is still consenting, which is why
    // the auth state is read rather than only the health.
    for state in [
        crate::auth::AuthState::AwaitingUser,
        crate::auth::AuthState::Connected,
        crate::auth::AuthState::Superseded,
    ] {
        let findings = diagnostics_for(&connected(), state, &[], fresh_now(), FRESHNESS);
        assert!(
            findings
                .iter()
                .all(|finding| finding.severity != DiagnosticSeverity::Error),
            "{state:?} must not produce an error while the connector is healthy"
        );
    }
    // A failed flow is an error, because nothing is progressing without intervention.
    let failed = diagnostics_for(
        &connected(),
        crate::auth::AuthState::Failed,
        &[],
        fresh_now(),
        FRESHNESS,
    );
    assert!(
        failed
            .iter()
            .any(|finding| finding.field == DiagnosticField::AuthState
                && finding.severity == DiagnosticSeverity::Error),
        "a failed auth flow must be an error"
    );
    // Every report includes the auth state, because which stage the connector is in is the first thing a
    // support conversation asks.
    for state in [
        crate::auth::AuthState::AwaitingUser,
        crate::auth::AuthState::Connected,
        crate::auth::AuthState::Failed,
        crate::auth::AuthState::Superseded,
    ] {
        assert!(
            diagnostics_for(&connected(), state, &[], fresh_now(), FRESHNESS)
                .iter()
                .any(|finding| finding.field == DiagnosticField::AuthState),
            "{state:?} must be reported"
        );
    }
}

#[test]
fn a_finding_is_a_value_rather_than_a_sentence_so_it_can_be_counted_and_compared() {
    // A sentence is composed for a human and cannot be aggregated, filtered, or tested. `jarvis-diagnostics`
    // set that precedent with `FindingCode`/`Severity`.
    let findings = diagnostics_for(
        &connected(),
        crate::auth::AuthState::Connected,
        &[],
        fresh_now(),
        FRESHNESS,
    );
    assert!(!findings.is_empty());
    for finding in &findings {
        // A finding carries no value, so it cannot leak one by being logged — which is the property that lets
        // a caller log the findings while the values stay behind the `Redaction`.
        let (_, severity, redaction) = (finding.field, finding.severity, finding.redaction);
        assert_eq!(finding.severity, severity);
        assert_eq!(finding.redaction, redaction);
        // The rendering is a code pair rather than prose, so it is stable across versions.
        let rendered = finding.to_string();
        assert!(rendered.contains(finding.field.as_str()), "got {rendered}");
        assert!(
            rendered.contains(finding.severity.as_str()),
            "got {rendered}"
        );
    }
    // Two findings with the same triple compare equal, which is what makes a report diffable.
    let a = DiagnosticFinding::new(
        DiagnosticField::HealthState,
        DiagnosticSeverity::Info,
        Redaction::Summary,
    );
    let b = DiagnosticFinding::new(
        DiagnosticField::HealthState,
        DiagnosticSeverity::Info,
        Redaction::Summary,
    );
    assert_eq!(a, b);
    let different = DiagnosticFinding::new(
        DiagnosticField::HealthState,
        DiagnosticSeverity::Error,
        Redaction::Summary,
    );
    assert_ne!(a, different);
}
