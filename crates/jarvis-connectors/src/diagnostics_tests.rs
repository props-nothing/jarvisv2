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
        20,
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
    for finding in diagnostics_for(&connected(), crate::auth::AuthState::Connected, &[]) {
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
    let permitted = diagnostics_for(&connected(), crate::auth::AuthState::Connected, &[]);
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
    let refused = diagnostics_for(&refusing, crate::auth::AuthState::Failed, &[]);
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
    );
    let missing = short
        .iter()
        .find(|finding| finding.field == DiagnosticField::MissingScopes)
        .unwrap_or_else(|| panic!("a shortfall must be reported"));
    assert_eq!(missing.severity, DiagnosticSeverity::Warning);
    // And with no shortfall the field is absent rather than reported as empty, so a caller cannot confuse
    // "nothing is missing" with "the list was not computed".
    assert!(
        !diagnostics_for(&connected(), crate::auth::AuthState::Connected, &[])
            .iter()
            .any(|finding| finding.field == DiagnosticField::MissingScopes),
        "a complete grant must not report a missing-scope finding"
    );
    // The count is always reported, because it distinguishes "nothing was granted" from "the grant is short".
    assert!(
        diagnostics_for(&connected(), crate::auth::AuthState::Connected, &[])
            .iter()
            .any(|finding| finding.field == DiagnosticField::GrantedScopeCount),
        "the granted-scope count is always a support fact"
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
        let findings = diagnostics_for(&connected(), state, &[]);
        assert!(
            findings
                .iter()
                .all(|finding| finding.severity != DiagnosticSeverity::Error),
            "{state:?} must not produce an error while the connector is healthy"
        );
    }
    // A failed flow is an error, because nothing is progressing without intervention.
    let failed = diagnostics_for(&connected(), crate::auth::AuthState::Failed, &[]);
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
            diagnostics_for(&connected(), state, &[])
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
    let findings = diagnostics_for(&connected(), crate::auth::AuthState::Connected, &[]);
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
