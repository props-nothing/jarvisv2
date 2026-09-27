//! Diagnostics: what an operator can see about a connector without seeing a credential.
//!
//! `docs/quality/acceptance-tests.md`'s `A10` requires that "Diagnostics remain useful and redact all
//! credentials/content not needed for support", and that sentence is the whole specification for this
//! module. Two obligations that pull against each other, and the resolution is not "redact more": a
//! diagnostic that redacts so much it cannot answer "which stage failed" is one an operator works around by
//! pasting a token into a bug report.
//!
//! So the module's shape is a **closed set of findings**, each of which is known in advance to be
//! support-relevant and known to carry no secret.
//!
//! # Why a closed set rather than a free-form message
//!
//! `security.md`: "no token material in model context, URLs/logs, diagnostics, or normal database columns".
//! A free-form `String` is a place that rule can be violated **by accident** — a provider's error body
//! echoed into a diagnostic is the standard way a refresh token reaches a log, because providers do put
//! request material in their error text. A closed set makes that unrepresentable: [`DiagnosticField`] says
//! *which* facts may appear, and every one of them is a value this crate produced.
//!
//! The cost is real and worth naming: a diagnostic cannot quote a provider's message, so an unfamiliar
//! failure is diagnosed by class rather than by text. That is the correct trade for a platform whose secrets
//! are bearer tokens for a user's mail, and it is the same trade `P3-008i` made when it refused to forward a
//! parser's text.
//!
//! # Why every finding is a `(field, severity)` pair and not a sentence
//!
//! A sentence is composed for a human and cannot be aggregated, filtered, or tested. A finding is a
//! **value**, so `doctor` can count them, a test can assert one, and a UI can render it in the operator's
//! language. `jarvis-diagnostics` set that precedent with `FindingCode`/`Severity`.

use std::fmt;

/// A fact a diagnostic may report.
///
/// Every variant is a value this crate produced, so **none can carry provider-supplied text**. That is the
/// module's security property rather than a convention: adding a variant that quoted a response body would
/// be adding a possible secret leak, and the type is the place that decision is visible.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum DiagnosticField {
    /// Which connector, by its own identifier.
    ConnectorId,
    /// The manifest version in use.
    ManifestVersion,
    /// Which auth method the account connected with.
    AuthMethod,
    /// Which stage of the auth flow the connector is in.
    AuthState,
    /// Why a reauth is needed, as a reason code and never a provider message.
    ReauthReason,
    /// The number of scopes granted.
    ///
    /// A **count**, not the list. A scope name can disclose what a deployment can do — a
    /// `https://www.googleapis.com/auth/gmail.readonly` scope names a mailbox this account reads — so the
    /// list is not a support fact, while the count is what distinguishes "nothing was granted" from "the
    /// grant is short".
    GrantedScopeCount,
    /// The scopes the account is **missing**, which is the list a reauth prompt needs.
    ///
    /// The exception to [`Self::GrantedScopeCount`]'s rule, and it is deliberate: a missing scope is what the
    /// user must act on, so hiding it makes the diagnostic useless in exactly the case it is most needed. It
    /// names what the connector *wants* rather than what the account *has*, which is not a disclosure about
    /// the account.
    MissingScopes,
    /// The health state, as a state code.
    HealthState,
    /// Which probe produced the health state.
    HealthProbe,
    /// When the health state was observed.
    HealthObservedAt,
    /// Whether the cursor is a start cursor, which decides whether a run is a full resync.
    CursorKind,
    /// When the cursor was observed.
    ///
    /// The **age** is what matters and an instant answers it; the token itself is never a field, because a
    /// cursor is provider-issued text that can address another account's data.
    CursorObservedAt,
    /// Whether webhooks are supported, and by which mechanism.
    WebhookSupport,
    /// The signature algorithm in use, or that none is.
    SignatureAlgorithm,
    /// The rate limit's scope, which decides whose budget a refusal belongs to.
    RateLimitScope,
    /// Whether the declared rate limit is documented or observed.
    RateLimitEvidence,
    /// What the last retry classification was.
    RetryClass,
    /// The provider's request identifier, when a response supplied one.
    ///
    /// Included because it is the **only** value that lets a support conversation proceed with the provider,
    /// and it is not a credential: it identifies a request the provider already logged. It is rendered
    /// through [`crate::ratelimit::ProviderRequestId`], which refuses a control character.
    ProviderRequestId,
    /// How long the connector has been running.
    Uptime,
    /// Which connector version issued the stored cursor, which explains a resync after an upgrade.
    CursorConnectorVersion,
}

impl DiagnosticField {
    /// Returns the stable snake-case code.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ConnectorId => "connector_id",
            Self::ManifestVersion => "manifest_version",
            Self::AuthMethod => "auth_method",
            Self::AuthState => "auth_state",
            Self::ReauthReason => "reauth_reason",
            Self::GrantedScopeCount => "granted_scope_count",
            Self::MissingScopes => "missing_scopes",
            Self::HealthState => "health_state",
            Self::HealthProbe => "health_probe",
            Self::HealthObservedAt => "health_observed_at",
            Self::CursorKind => "cursor_kind",
            Self::CursorObservedAt => "cursor_observed_at",
            Self::WebhookSupport => "webhook_support",
            Self::SignatureAlgorithm => "signature_algorithm",
            Self::RateLimitScope => "rate_limit_scope",
            Self::RateLimitEvidence => "rate_limit_evidence",
            Self::RetryClass => "retry_class",
            Self::ProviderRequestId => "provider_request_id",
            Self::Uptime => "uptime",
            Self::CursorConnectorVersion => "cursor_connector_version",
        }
    }

    /// Returns whether a value for this field may be shown to a **model**.
    ///
    /// Only false for [`Self::ProviderRequestId`], and that is not because it is a secret but because it is
    /// an identifier of a request the provider made: putting it in a prompt invites a model to reason about
    /// it, and `security.md` minimizes what reaches a model rather than deciding case by case at each call
    /// site. Every other field describes this platform's own state.
    ///
    /// This is the predicate a presentation layer asks, so "may this be shown where" is a property of the
    /// **field** rather than of every call site — the same arrangement as `ContextSourceKind::allowed_trusts`.
    #[must_use]
    pub const fn may_reach_a_model(self) -> bool {
        !matches!(self, Self::ProviderRequestId)
    }

    /// Returns whether a value for this field may be written to a log.
    ///
    /// True for every field, and the constancy is the point: the type is a **closed set chosen so that this
    /// can be true**, rather than a set with a `may_log` exception list. A field that needed an exception
    /// would mean a value in this type was not safe to log, which is the invariant worth keeping absolute.
    #[must_use]
    pub const fn is_loggable(self) -> bool {
        true
    }
}

/// How serious a finding is.
///
/// The same three levels `jarvis-diagnostics` uses, and for the same reason: a diagnostic an operator cannot
/// rank is one they must read entirely, so severity is what makes `doctor`'s output scannable.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum DiagnosticSeverity {
    /// Ordinary state, reported for completeness.
    Info,
    /// Something needs attention but the connector still works.
    Warning,
    /// The connector cannot work as configured.
    Error,
}

impl DiagnosticSeverity {
    /// Returns the stable lowercase code.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

/// How a value is rendered when it is shown.
///
/// Recorded **with the finding** rather than applied by a presentation layer, because whether a value is a
/// credential is a fact only the field knows. `jarvis-storage`'s `redact_key` handles the URL forms a
/// credential appears in, and `endpoint::ApiKey`'s `Debug` prints a prefix and a length — this enum is the
/// same idea carried as data, so a UI cannot render a field without being told what it is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Redaction {
    /// Safe to render verbatim.
    None,
    /// Safe to render as a count or a state code.
    ///
    /// The default for most fields: a count of scopes and a health state code disclose nothing about content,
    /// and rendering them verbatim is what makes the diagnostic useful.
    Summary,
    /// Render as a prefix and a length, so an operator can confirm *which* value they pasted without the
    /// value being readable. `ApiKey`'s own `Display` uses the `abc... (32 chars)` form.
    PrefixAndLength,
    /// Never render.
    ///
    /// Representable for completeness, and **no current field uses it**. Kept because a future field that
    /// needed it would otherwise have to be added along with a new variant, and because the tests assert that
    /// no field in the current set is unrenderable — a `Withheld` field would make the diagnostic less useful
    /// and that should be a visible decision rather than an accident.
    Withheld,
}

impl Redaction {
    /// Returns the stable snake-case code.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Summary => "summary",
            Self::PrefixAndLength => "prefix_and_length",
            Self::Withheld => "withheld",
        }
    }

    /// Returns whether a value may be shown at all.
    #[must_use]
    pub const fn is_renderable(self) -> bool {
        !matches!(self, Self::Withheld)
    }
}

/// One diagnostic finding: which field, how serious, and how to render it.
///
/// A `(field, severity, redaction)` triple rather than a message, so it is aggregable and testable. The
/// **value** is deliberately absent: a finding says what may be known about, and a rendering step supplies
/// the value while the `Redaction` tells it how. Keeping the value out of the finding means a finding cannot
/// be logged by accident in a place that only wanted to count them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiagnosticFinding {
    /// The field.
    pub field: DiagnosticField,
    /// How serious.
    pub severity: DiagnosticSeverity,
    /// How to render a value for it.
    pub redaction: Redaction,
}

impl DiagnosticFinding {
    /// Records a finding.
    #[must_use]
    pub const fn new(
        field: DiagnosticField,
        severity: DiagnosticSeverity,
        redaction: Redaction,
    ) -> Self {
        Self {
            field,
            severity,
            redaction,
        }
    }
}

impl fmt::Display for DiagnosticFinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}: {} ({})",
            self.severity.as_str(),
            self.field.as_str(),
            self.redaction.as_str()
        )
    }
}

/// The findings a connector in a given state reports.
///
/// Derived from the state rather than assembled by each connector, so two connectors with the same problem
/// report the same findings — which is what makes `doctor`'s output comparable across connectors, and what
/// `A10`'s "diagnostics remain useful" means in practice.
///
/// # Why this takes the values rather than a `&Connector`
///
/// There is no `Connector` trait in this crate (the api contract's trait needs a provider client, which is
/// `P5-002`'s). Taking the values keeps the function a pure function of its arguments, so every branch is
/// testable without a connector — the same reasoning `jarvis-sandbox`'s `sandbox_finding(support, facility)`
/// records: an inline branch in a larger function leaves half of itself unmeasured.
#[must_use]
pub fn diagnostics_for(
    health: &crate::health::ConnectorHealth,
    auth_state: crate::auth::AuthState,
    missing_scopes: &[String],
) -> Vec<DiagnosticFinding> {
    use crate::auth::AuthState;

    let mut findings = vec![
        DiagnosticFinding::new(
            DiagnosticField::HealthProbe,
            DiagnosticSeverity::Info,
            Redaction::Summary,
        ),
        DiagnosticFinding::new(
            DiagnosticField::HealthObservedAt,
            DiagnosticSeverity::Info,
            Redaction::Summary,
        ),
        DiagnosticFinding::new(
            DiagnosticField::AuthState,
            DiagnosticSeverity::Info,
            Redaction::Summary,
        ),
        DiagnosticFinding::new(
            DiagnosticField::GrantedScopeCount,
            DiagnosticSeverity::Info,
            Redaction::Summary,
        ),
    ];

    // The health state's own severity, which is the finding an operator ranks the report by.
    let health_severity = if health.permits_calls() {
        DiagnosticSeverity::Info
    } else {
        DiagnosticSeverity::Error
    };
    findings.push(DiagnosticFinding::new(
        DiagnosticField::HealthState,
        health_severity,
        Redaction::Summary,
    ));

    // A reauth reason is reported **only** when one exists, and at `Error` severity: the reason is what a
    // user acts on, so a report that omitted it would be one the operator has to reproduce the failure to
    // understand.
    if health.reauth_reason().is_some() {
        findings.push(DiagnosticFinding::new(
            DiagnosticField::ReauthReason,
            DiagnosticSeverity::Error,
            Redaction::Summary,
        ));
    }

    // Missing scopes are reported at `Warning` rather than `Error`: an account with a shortfall is connected
    // and may run the operations its grant covers, so an `Error` would send an operator to fix something that
    // is working. This is the same distinction `AccountStatus::ScopeShortfall` draws.
    if !missing_scopes.is_empty() {
        findings.push(DiagnosticFinding::new(
            DiagnosticField::MissingScopes,
            DiagnosticSeverity::Warning,
            Redaction::Summary,
        ));
    }

    // A connector mid-flow is `Info` and not a problem, which is why the state is read rather than only the
    // health: an operator running onboarding must not see an error while the user is still consenting.
    if matches!(auth_state, AuthState::Failed) {
        findings.push(DiagnosticFinding::new(
            DiagnosticField::AuthState,
            DiagnosticSeverity::Error,
            Redaction::Summary,
        ));
    }

    let _ = missing_scopes;
    findings
}

#[cfg(test)]
#[path = "diagnostics_tests.rs"]
mod tests;
