//! Who may call JARVIS as an MCP client: an allowlist whose subject is a **credential**, never a label.
//!
//! # The finding this module is built around
//!
//! A per-client allowlist needs a **subject** — something an entry names. The obvious candidate is the
//! `clientInfo` a caller sends in each request's `_meta`, and reading the protocol's shape shows why that is
//! the wrong one: `Implementation` is `{name, title, version, description, icons, website_url}` (verified in
//! `rmcp-3.4.0`'s `model.rs`), every field of which the **client chooses**. Nothing in it is a credential, and
//! nothing verifies it.
//!
//! So an allowlist keyed on `clientInfo` would be a control a client names itself into. That is `ADR-0024`'s
//! defect — *a server does not name itself* — arriving on the **inbound** side, where it is worse: the label is
//! now deciding whether a caller is permitted, so a client that sends `name = "vscode"` is admitted as VS Code.
//! The rule is the same in both directions and is stated here as [`CallerLabel`]'s doc:
//!
//! > **A self-reported name is evidence or nothing. It is never an identity, and never a permit.**
//!
//! # What the subject is instead
//!
//! MCP's own answer for a remote server is an OAuth token, and the specification's rules are specific: a server
//! **MUST** validate that a token was issued **for it** as the intended audience (RFC 8707) and **MUST NOT**
//! accept a token meant for another resource. That is a slice, not a value. So this module models the
//! **decision** in the terms the token slice will supply, and the default admits **nobody** remotely:
//!
//! - [`CallerAdmission::local_only`] is what a deployment that says nothing gets. A remote caller is refused,
//!   which is the same shape `ServingConfig` already enforces for the bind (ADR-0034) — the endpoint is
//!   loopback-only, so an off-host caller cannot arrive at all, and this policy is the second statement of the
//!   same decision rather than a substitute for it.
//! - [`CallerAdmission::new`] takes entries naming a **credential fingerprint** and a label it may carry. The
//!   label is recorded and reported; the fingerprint is what admits.
//!
//! # Why the entries are not a token store
//!
//! An entry holds a **fingerprint** of a credential, never the credential, and the same reasoning as
//! `jarvis-tools`'s approval nonces applies: this value is compared on every request, formatted into
//! diagnostics, and held for the process's life, so a bearer token here would be a secret in the one place
//! that is least able to protect it. A fingerprint is enough to answer "is this caller one we allow", which is
//! the only question the allowlist asks.
//!
//! # Rate limits, and which layer owns them
//!
//! Rate limiting is here because it is a **per-caller** decision and this is where callers are identified, but
//! it is deliberately a *bound* rather than a scheduler: this module decides whether a caller has room for one
//! more request, and the daemon's request layer counts. Putting the counting here would mean this value held
//! mutable state, and a policy that mutates on every request cannot be compared, logged, or reused — which are
//! the properties that make it reviewable.

use std::collections::BTreeSet;
use std::fmt;

/// The longest a credential fingerprint may be.
///
/// A fingerprint is a digest rendered as text, so 128 characters is generous for a hex or base64 form and far
/// below anything that could carry a secret. The bound exists so a configuration file cannot make the
/// per-request comparison unbounded, the same reasoning as `MAX_ALLOWED_ORIGINS`.
pub const MAX_FINGERPRINT_CHARS: usize = 128;

/// The longest a caller's recorded label may be.
///
/// Bounded for the reason `MAX_REPORTED_TEXT_CHARS` is in `jarvis-mcp`: the label reaches a log line and an
/// audit record, and a client-supplied string in a log line is where an injection arrives.
pub const MAX_CALLER_LABEL_CHARS: usize = 64;

/// The most callers one daemon may admit.
///
/// The same bound rationale as `MAX_ALLOWED_ORIGINS` and `MAX_MCP_SERVERS`: a configuration file must not be
/// able to make a per-request check unbounded. Sixteen is far above a plausible set of clients for a
/// single-user assistant.
pub const MAX_ADMITTED_CALLERS: usize = 16;

/// The default per-caller request budget, as a rate rather than a total.
///
/// Twelve requests per sixty seconds. Chosen against what a real MCP client does — an agent turn is a handful
/// of `tools/list` and `tools/call` round trips — so the bound is far above interactive use and far below what
/// a runaway or hostile caller would want. It is a **rate**, so a client that bursts then idles is not
/// punished for the burst.
pub const DEFAULT_REQUESTS_PER_MINUTE: u32 = 12;

/// A credential fingerprint: what an allowlist entry actually admits on.
///
/// A **digest**, never the credential. See the module docs on why: this value is compared per request, put in
/// diagnostics, and held for the process's life, so it must be something that is safe to handle.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Fingerprint(String);

impl Fingerprint {
    /// Parses a fingerprint.
    ///
    /// # Errors
    ///
    /// Returns [`AdmissionError::FingerprintInvalid`] when the value is empty, over
    /// [`MAX_FINGERPRINT_CHARS`], or contains a character outside the digest alphabet. A fingerprint is
    /// **not** free text: a value with a space or a quote in it is either a pasted something-else or a
    /// malformed digest, and refusing it is how "the operator pasted a token instead of its digest" is caught
    /// locally rather than by admitting a caller or by putting a secret in a config file that then gets
    /// committed.
    pub fn parse(value: &str) -> Result<Self, AdmissionError> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(AdmissionError::FingerprintInvalid {
                reason: "it is empty".to_owned(),
            });
        }
        if trimmed.chars().count() > MAX_FINGERPRINT_CHARS {
            return Err(AdmissionError::FingerprintInvalid {
                reason: format!("it exceeds {MAX_FINGERPRINT_CHARS} characters"),
            });
        }
        // The digest alphabet. A space, a colon, a quote, or a `-` is refused, which is what makes a pasted
        // JWT (`eyJ…eyJ…`) or a `Bearer …` prefix fail here rather than later.
        if !trimmed.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '+' | '/' | '=')
        }) {
            return Err(AdmissionError::FingerprintInvalid {
                reason:
                    "it contains a character a digest does not, so it is probably not a fingerprint"
                        .to_owned(),
            });
        }
        Ok(Self(trimmed.to_owned()))
    }

    /// Returns the fingerprint as stored.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Fingerprint {
    /// Prints a **prefix** rather than the whole value.
    ///
    /// So a log line or an error names which caller was refused without putting the full digest where anything
    /// that reads logs can collect them. The same reasoning as the API key's `abc… (32 chars)` form recorded in
    /// this project's endpoint config.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let prefix: String = self.0.chars().take(8).collect();
        write!(formatter, "{prefix}… ({} chars)", self.0.chars().count())
    }
}

/// What a caller asserted about itself, kept as **evidence**.
///
/// **Never an identity and never a permit.** This is the inbound mirror of
/// [`jarvis_mcp::ReportedIdentity`], and it exists so an operator can answer "which client asked" and so a
/// caller that changes what it claims is visible. A caller with no label is recorded as one rather than
/// refused, because the field is evidence and discarding a malformed self-assertion would discard the fact
/// that it was malformed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CallerLabel {
    /// The `name` the caller sent, bounded and trimmed.
    pub name: String,
    /// The `version` the caller sent, bounded and trimmed.
    pub version: String,
}

impl CallerLabel {
    /// Records what a caller reported, bounding both fields.
    #[must_use]
    pub fn new(name: &str, version: &str) -> Self {
        Self {
            name: bounded(name),
            version: bounded(version),
        }
    }

    /// Returns the label a caller with no `clientInfo` at all gets.
    ///
    /// An empty name rather than a placeholder like `"unknown"`, because a placeholder is a **value** a caller
    /// could also send — so `name = "unknown"` would be indistinguishable from silence, and the whole point of
    /// recording a label is being able to tell those apart.
    #[must_use]
    pub fn absent() -> Self {
        Self {
            name: String::new(),
            version: String::new(),
        }
    }

    /// Returns whether the caller supplied any label at all.
    #[must_use]
    pub fn is_absent(&self) -> bool {
        self.name.is_empty() && self.version.is_empty()
    }
}

/// Truncates client-supplied text to the label bound, on a character boundary.
///
/// Char-aware, because a byte slice of an untrusted string can split a multi-byte character — the same reason
/// `jarvis_mcp`'s `bounded` is char-aware.
fn bounded(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.chars().count() <= MAX_CALLER_LABEL_CHARS {
        return trimmed.to_owned();
    }
    trimmed.chars().take(MAX_CALLER_LABEL_CHARS).collect()
}

/// Why a caller was refused, or that it was admitted.
///
/// Distinct variants rather than a boolean, because the remedies differ and a log that cannot name the reason
/// cannot explain a wave of refusals. The **wire** answer is uniform (see [`Self::refusal_status`]), so the
/// distinction is for the operator and the audit record, never for the caller.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AdmissionVerdict {
    /// The caller's fingerprint is in the allowlist.
    Admitted,
    /// No fingerprint was presented.
    ///
    /// The only variant a **local** caller can produce, since a local one has no token to present. See
    /// [`CallerAdmission::local_only`].
    NoCredential,
    /// A fingerprint was presented and is not in the allowlist.
    NotAllowed,
    /// The fingerprint is in the allowlist but the caller has used its budget.
    RateLimited {
        /// The budget, so the message can state what was exceeded.
        requests_per_minute: u32,
    },
}

impl AdmissionVerdict {
    /// Returns whether the request may be served.
    #[must_use]
    pub const fn permits(&self) -> bool {
        matches!(self, Self::Admitted)
    }

    /// Returns the HTTP status this verdict obliges, when it refuses.
    ///
    /// `401` for a missing or unknown credential and `429` for a spent budget, which is what the
    /// specification's own error table requires: `401` is "authorization required or token invalid".
    /// [`Self::NotAllowed`] answers `401` rather than `403` because from the caller's side an unrecognized
    /// credential and an absent one are the same situation — present a valid one — and distinguishing them
    /// would tell a prober which fingerprints exist.
    #[must_use]
    pub const fn refusal_status(&self) -> Option<u16> {
        match self {
            Self::Admitted => None,
            Self::NoCredential | Self::NotAllowed => Some(401),
            Self::RateLimited { .. } => Some(429),
        }
    }

    /// Returns a bounded, operator-facing reason.
    #[must_use]
    pub fn reason(&self) -> String {
        match self {
            Self::Admitted => "admitted".to_owned(),
            Self::NoCredential => {
                "no credential was presented, and this daemon does not admit an anonymous remote caller"
                    .to_owned()
            }
            Self::NotAllowed => "the presented credential is not in the caller allowlist".to_owned(),
            Self::RateLimited {
                requests_per_minute,
            } => format!("the caller exceeded {requests_per_minute} requests per minute"),
        }
    }
}

/// One admitted caller: the credential that admits it, and the label it may carry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdmittedCaller {
    fingerprint: Fingerprint,
    label: CallerLabel,
}

impl AdmittedCaller {
    /// Builds an entry, recording the label it may carry.
    ///
    /// # Errors
    ///
    /// Returns [`AdmissionError::LabelInvalid`] when the label has a control character, which would reach a log
    /// line and an audit record.
    pub fn new(fingerprint: Fingerprint, label: CallerLabel) -> Result<Self, AdmissionError> {
        if !is_loggable(&label.name) || !is_loggable(&label.version) {
            return Err(AdmissionError::LabelInvalid);
        }
        Ok(Self { fingerprint, label })
    }

    /// Returns the credential fingerprint this entry admits.
    #[must_use]
    pub const fn fingerprint(&self) -> &Fingerprint {
        &self.fingerprint
    }

    /// Returns the label recorded for this caller.
    ///
    /// **Evidence, not a permit.** An entry could record a label and then be presented with a different one; the
    /// fingerprint is what admits, and the label is compared to **report a mismatch** rather than to decide.
    #[must_use]
    pub const fn label(&self) -> &CallerLabel {
        &self.label
    }
}

/// Returns whether a string is safe to put in a log line.
///
/// Refuses control characters, which is where a log-injection arrives: a newline in a label lets a caller forge
/// a subsequent record.
fn is_loggable(value: &str) -> bool {
    !value.chars().any(char::is_control)
}

/// Who may call this daemon, and how often.
///
/// Built from an operator's configuration; [`Self::local_only`] is what a deployment that says nothing gets.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CallerAdmission {
    admitted: Vec<AdmittedCaller>,
    requests_per_minute: u32,
}

impl CallerAdmission {
    /// Builds a policy from an operator's admitted callers.
    ///
    /// # Errors
    ///
    /// Returns [`AdmissionError::TooManyCallers`] past [`MAX_ADMITTED_CALLERS`], and
    /// [`AdmissionError::DuplicateFingerprint`] for two entries naming one credential — which is refused rather
    /// than tolerated, because the second entry's label would never be the one reported and an operator would
    /// have no way to tell which of their two entries was in force.
    pub fn new(
        callers: impl IntoIterator<Item = AdmittedCaller>,
        requests_per_minute: u32,
    ) -> Result<Self, AdmissionError> {
        let mut admitted: Vec<AdmittedCaller> = Vec::new();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        for caller in callers {
            if admitted.len() >= MAX_ADMITTED_CALLERS {
                return Err(AdmissionError::TooManyCallers);
            }
            if !seen.insert(caller.fingerprint.as_str().to_owned()) {
                return Err(AdmissionError::DuplicateFingerprint {
                    fingerprint: caller.fingerprint.clone(),
                });
            }
            admitted.push(caller);
        }
        if requests_per_minute == 0 {
            return Err(AdmissionError::EmptyBudget);
        }
        // Sorted so two policies built from the same callers in a different order compare equal and a stored
        // policy is reproducible — the same reasoning as `ServerExposure`.
        admitted.sort_by(|left, right| left.fingerprint.cmp(&right.fingerprint));
        Ok(Self {
            admitted,
            requests_per_minute,
        })
    }

    /// The policy a deployment that says nothing gets: **no remote caller is admitted.**
    ///
    /// This is the same decision `ServingConfig` already makes for the bind, stated again where a request would
    /// be decided. Two statements rather than one is deliberate: the bind is what makes an off-host caller
    /// unable to arrive, and this is what refuses one that somehow does — a reverse proxy, a forwarded socket,
    /// or a misconfiguration that changed the bind. A control that depends on a second control having worked is
    /// not a control.
    #[must_use]
    pub const fn local_only() -> Self {
        Self {
            admitted: Vec::new(),
            requests_per_minute: DEFAULT_REQUESTS_PER_MINUTE,
        }
    }

    /// Returns whether no caller is admitted.
    ///
    /// **The stronger of the two empty states, and named for it** — the same reasoning as
    /// `ServerExposure::is_loopback_only`. An empty allowlist here is *enforced*, so the only admitted caller is
    /// a local one, which presents no credential.
    #[must_use]
    pub fn is_local_only(&self) -> bool {
        self.admitted.is_empty()
    }

    /// Returns the admitted callers in a stable order.
    #[must_use]
    pub fn admitted(&self) -> &[AdmittedCaller] {
        &self.admitted
    }

    /// Returns the per-caller request budget.
    #[must_use]
    pub const fn requests_per_minute(&self) -> u32 {
        self.requests_per_minute
    }

    /// Decides whether a caller may be served.
    ///
    /// `presented` is the **fingerprint** of whatever credential the request carried, or `None` when it carried
    /// none. Taking a fingerprint rather than a token is deliberate: the token's own validation is the token
    /// slice's, and this decision is "is this caller one we allow", which a digest answers.
    #[must_use]
    pub fn decide(&self, presented: Option<&Fingerprint>, spent_budget: bool) -> AdmissionVerdict {
        let Some(presented) = presented else {
            return AdmissionVerdict::NoCredential;
        };
        if !self
            .admitted
            .iter()
            .any(|caller| caller.fingerprint() == presented)
        {
            return AdmissionVerdict::NotAllowed;
        }
        if spent_budget {
            return AdmissionVerdict::RateLimited {
                requests_per_minute: self.requests_per_minute,
            };
        }
        AdmissionVerdict::Admitted
    }

    /// Returns the entry an admitted fingerprint belongs to.
    ///
    /// `None` for a fingerprint that is not admitted, so a caller **cannot** use a lookup to learn which labels
    /// exist — the same refusal-by-absence shape `serves` uses for a tool name.
    #[must_use]
    pub fn entry_for(&self, presented: &Fingerprint) -> Option<&AdmittedCaller> {
        self.admitted
            .iter()
            .find(|caller| caller.fingerprint() == presented)
    }

    /// Reports whether a caller's presented label matches the label its entry records.
    ///
    /// **A report, never a refusal**, which is the decision worth stating. A caller presenting a different label
    /// than its entry records is worth **seeing** — it may be an upgrade, or a client reused with a different
    /// identity — but the fingerprint already admitted it, and refusing on a mismatch would make the label a
    /// second permit after the module has said it is not one. Reported as a mismatch so the operator can decide.
    ///
    /// `None` when the fingerprint is not admitted, because there is no recorded label to compare against.
    #[must_use]
    pub fn label_matches(&self, presented: &Fingerprint, claimed: &CallerLabel) -> Option<bool> {
        self.entry_for(presented)
            .map(|entry| entry.label() == claimed)
    }
}

/// Explains why a caller-admission policy could not be built.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AdmissionError {
    /// A fingerprint was not a usable digest.
    FingerprintInvalid {
        /// Why it was refused.
        reason: String,
    },
    /// A label contained a control character.
    LabelInvalid,
    /// Two entries named one credential.
    DuplicateFingerprint {
        /// The fingerprint both entries named, rendered as a prefix.
        fingerprint: Fingerprint,
    },
    /// The allowlist exceeded [`MAX_ADMITTED_CALLERS`].
    TooManyCallers,
    /// The request budget was zero, which would refuse every admitted caller.
    EmptyBudget,
}

impl fmt::Display for AdmissionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FingerprintInvalid { reason } => {
                write!(
                    formatter,
                    "the credential fingerprint is unusable: {reason}"
                )
            }
            Self::LabelInvalid => write!(
                formatter,
                "a caller label contains a control character, which would let it forge a log line"
            ),
            Self::DuplicateFingerprint { fingerprint } => write!(
                formatter,
                "the fingerprint {fingerprint} is listed twice, so which entry's label applies would be \
                 a function of ordering"
            ),
            Self::TooManyCallers => {
                write!(
                    formatter,
                    "more than {MAX_ADMITTED_CALLERS} callers are admitted"
                )
            }
            Self::EmptyBudget => write!(
                formatter,
                "a request budget of zero would refuse every admitted caller, which is not a rate limit"
            ),
        }
    }
}

impl std::error::Error for AdmissionError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn fingerprint(value: &str) -> Fingerprint {
        Fingerprint::parse(value).unwrap_or_else(|error| panic!("{value}: {error}"))
    }

    fn caller(fingerprint_value: &str, name: &str, version: &str) -> AdmittedCaller {
        AdmittedCaller::new(
            fingerprint(fingerprint_value),
            CallerLabel::new(name, version),
        )
        .unwrap_or_else(|error| panic!("{fingerprint_value}: {error}"))
    }

    fn policy(callers: Vec<AdmittedCaller>) -> CallerAdmission {
        CallerAdmission::new(callers, DEFAULT_REQUESTS_PER_MINUTE)
            .unwrap_or_else(|error| panic!("{error}"))
    }

    /// **A caller-chosen label is never a permit.**
    ///
    /// Falsified by keying the allowlist on `CallerLabel` instead of `Fingerprint`: an arbitrary caller that
    /// simply sends `name = "vscode"` is then admitted as VS Code, with no credential at all. That is
    /// `ADR-0024`'s defect on the inbound side, where the label is deciding permission rather than identity.
    #[test]
    fn a_claimed_label_does_not_admit_an_unlisted_credential() {
        let admission = policy(vec![caller("aaaa", "vscode", "1.97.0")]);
        // The same label the admitted caller uses, presented with a different credential.
        let impostor = fingerprint("bbbb");
        assert_eq!(
            admission.decide(Some(&impostor), false),
            AdmissionVerdict::NotAllowed
        );
        // And with no credential at all, which is the case a label-based control would admit.
        assert_eq!(
            admission.decide(None, false),
            AdmissionVerdict::NoCredential
        );
        // The positive control, so neither assertion passes because everything is refused.
        assert_eq!(
            admission.decide(Some(&fingerprint("aaaa")), false),
            AdmissionVerdict::Admitted
        );
    }

    /// **`local_only` admits no remote caller, and the two empty states are named for the difference.**
    ///
    /// Falsified by making an empty allowlist mean "allow all" — the SDK-default shape `P3-009a` recorded for
    /// origins — which fails the first assertion.
    #[test]
    fn the_default_admits_no_remote_caller() {
        let admission = CallerAdmission::local_only();
        assert!(admission.is_local_only());
        assert_eq!(
            admission.decide(None, false),
            AdmissionVerdict::NoCredential
        );
        // Every credential is refused, because admitting one would require an entry.
        for presented in ["aaaa", "bbbb"] {
            assert_eq!(
                admission.decide(Some(&fingerprint(presented)), false),
                AdmissionVerdict::NotAllowed
            );
        }
        assert!(admission.entry_for(&fingerprint("aaaa")).is_none());
    }

    /// **A pasted credential is refused as a fingerprint**, which is the check that catches the operator error
    /// that would otherwise put a secret in a configuration file.
    ///
    /// Falsified by accepting any non-empty string: a JWT or a `Bearer …` value is then stored, compared,
    /// formatted into diagnostics, and committed.
    #[test]
    fn a_pasted_credential_is_refused_as_a_fingerprint() {
        // A JWT's shape: base64url with dots, which the digest alphabet excludes.
        assert!(matches!(
            Fingerprint::parse("eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.signature"),
            Err(AdmissionError::FingerprintInvalid { .. })
        ));
        assert!(matches!(
            Fingerprint::parse("Bearer abcdef"),
            Err(AdmissionError::FingerprintInvalid { .. })
        ));
        assert!(matches!(
            Fingerprint::parse("not a fingerprint"),
            Err(AdmissionError::FingerprintInvalid { .. })
        ));
        assert!(matches!(
            Fingerprint::parse(""),
            Err(AdmissionError::FingerprintInvalid { .. })
        ));
        assert!(matches!(
            Fingerprint::parse(&"a".repeat(MAX_FINGERPRINT_CHARS + 1)),
            Err(AdmissionError::FingerprintInvalid { .. })
        ));
        // Hex and base64 forms are accepted, which is what a real digest looks like.
        assert!(Fingerprint::parse(&"a".repeat(MAX_FINGERPRINT_CHARS)).is_ok());
        assert!(Fingerprint::parse("Zm9vYmFy+/==").is_ok());
        assert!(Fingerprint::parse("  deadbeef00  ").is_ok());
    }

    /// The fingerprint's `Display` is a **prefix**, so a log line cannot be used to collect full digests.
    ///
    /// Falsified by printing the whole value: the assertion on the prefix length fails and the full digest is in
    /// every message.
    #[test]
    fn a_fingerprint_is_printed_as_a_prefix() {
        let rendered = fingerprint("deadbeefcafebabe0011223344556677").to_string();
        assert!(rendered.starts_with("deadbeef"), "got: {rendered}");
        assert!(
            !rendered.contains("cafebabe"),
            "the rest must not appear: {rendered}"
        );
        assert!(
            rendered.contains("32 chars"),
            "the length must be visible: {rendered}"
        );
    }

    /// **A rate-limited caller is refused, and the two refusals answer different statuses.**
    ///
    /// Falsified by ignoring `spent_budget`: the caller is admitted with its budget exhausted.
    #[test]
    fn an_exhausted_budget_is_refused_with_its_own_status() {
        let admission = policy(vec![caller("aaaa", "vscode", "1.97.0")]);
        let allowed = fingerprint("aaaa");
        assert_eq!(
            admission.decide(Some(&allowed), true),
            AdmissionVerdict::RateLimited {
                requests_per_minute: DEFAULT_REQUESTS_PER_MINUTE
            }
        );
        assert_eq!(
            admission.decide(Some(&allowed), true).refusal_status(),
            Some(429)
        );
        // A missing or unknown credential answers 401 rather than 403: from the caller's side both mean
        // "present a valid credential", and distinguishing them would reveal which fingerprints exist.
        assert_eq!(admission.decide(None, false).refusal_status(), Some(401));
        assert_eq!(
            admission
                .decide(Some(&fingerprint("cccc")), false)
                .refusal_status(),
            Some(401)
        );
        // The positive control.
        assert_eq!(
            admission.decide(Some(&allowed), false).refusal_status(),
            None
        );
    }

    /// **A label mismatch is reported, never enforced.**
    ///
    /// Falsified by refusing on a mismatch: the label becomes a second permit after the module has said it is
    /// not one, and a client upgrade would break every call from a credential that is still valid.
    #[test]
    fn a_label_mismatch_is_reported_rather_than_refused() {
        let admission = policy(vec![caller("aaaa", "vscode", "1.97.0")]);
        let allowed = fingerprint("aaaa");

        // A caller presenting the recorded label reports a match...
        assert_eq!(
            admission.label_matches(&allowed, &CallerLabel::new("vscode", "1.97.0")),
            Some(true)
        );
        // ...a different one reports a mismatch...
        assert_eq!(
            admission.label_matches(&allowed, &CallerLabel::new("vscode", "2.0.0")),
            Some(false)
        );
        // ...and it is **still admitted**, because the credential is what admits.
        assert_eq!(
            admission.decide(Some(&allowed), false),
            AdmissionVerdict::Admitted
        );
        // An unadmitted credential has no recorded label, so there is nothing to compare.
        assert_eq!(
            admission.label_matches(&fingerprint("bbbb"), &CallerLabel::absent()),
            None
        );
    }

    /// **An absent label is recorded as absent, not as a placeholder.**
    ///
    /// Falsified by using a placeholder like `"unknown"`: a caller sending `name = "unknown"` then becomes
    /// indistinguishable from silence, and telling those apart is the only reason to record a label.
    #[test]
    fn an_absent_label_is_not_a_placeholder() {
        let absent = CallerLabel::absent();
        assert!(absent.is_absent());
        let claimed = CallerLabel::new("unknown", "unknown");
        assert!(
            !claimed.is_absent(),
            "a claimed placeholder is a claim, not silence"
        );
        assert_ne!(absent, claimed);
    }

    /// A label containing a control character is refused, because it would let a caller forge a log line.
    #[test]
    fn a_label_with_a_control_character_is_refused() {
        assert_eq!(
            AdmittedCaller::new(fingerprint("aaaa"), CallerLabel::new("vs\ncode", "1.0")),
            Err(AdmissionError::LabelInvalid)
        );
        assert_eq!(
            AdmittedCaller::new(fingerprint("aaaa"), CallerLabel::new("vscode", "1\r0")),
            Err(AdmissionError::LabelInvalid)
        );
        assert!(
            AdmittedCaller::new(fingerprint("aaaa"), CallerLabel::new("vscode", "1.0")).is_ok()
        );
    }

    /// An over-long label is **truncated on a character boundary** rather than refused, because it is evidence
    /// rather than a permit.
    ///
    /// Char-aware, so a multi-byte character is never split — the same reason `jarvis_mcp`'s `bounded` is.
    #[test]
    fn an_over_long_label_is_bounded_without_splitting_a_character() {
        let long = "é".repeat(MAX_CALLER_LABEL_CHARS + 10);
        let label = CallerLabel::new(&long, "1");
        assert_eq!(label.name.chars().count(), MAX_CALLER_LABEL_CHARS);
        // The result is valid UTF-8, which a byte slice would not have guaranteed.
        assert!(label.name.is_char_boundary(label.name.len()));
    }

    /// Two entries naming one credential are refused, because which label applied would depend on ordering.
    ///
    /// Falsified by tolerating the duplicate: the operator has two entries and no way to tell which is in force.
    #[test]
    fn two_entries_for_one_credential_are_refused() {
        assert!(matches!(
            CallerAdmission::new(
                vec![
                    caller("aaaa", "vscode", "1.0"),
                    caller("aaaa", "a different label", "2.0")
                ],
                DEFAULT_REQUESTS_PER_MINUTE
            ),
            Err(AdmissionError::DuplicateFingerprint { .. })
        ));
    }

    /// A zero budget is refused, because it is not a rate limit — it is a refusal to serve everyone.
    #[test]
    fn a_zero_budget_is_refused_rather_than_meaning_never_serve() {
        assert_eq!(
            CallerAdmission::new(vec![caller("aaaa", "vscode", "1.0")], 0),
            Err(AdmissionError::EmptyBudget)
        );
    }

    /// The bound on the allowlist is enforced, so a configuration file cannot make the check unbounded.
    #[test]
    fn more_callers_than_the_bound_are_refused() {
        let callers: Vec<AdmittedCaller> = (0..=MAX_ADMITTED_CALLERS)
            .map(|index| caller(&format!("{index:08}"), "client", "1.0"))
            .collect();
        assert_eq!(
            CallerAdmission::new(callers, DEFAULT_REQUESTS_PER_MINUTE),
            Err(AdmissionError::TooManyCallers)
        );
    }

    /// Order is not part of a policy's identity, so two of the same list compare equal.
    #[test]
    fn the_order_of_the_allowlist_does_not_change_the_policy() {
        let first = policy(vec![caller("bbbb", "b", "1"), caller("aaaa", "a", "1")]);
        let second = policy(vec![caller("aaaa", "a", "1"), caller("bbbb", "b", "1")]);
        assert_eq!(first, second);
    }
}
