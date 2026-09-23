//! Applying both caller decisions to one request, in one place, in one order.
//!
//! # The limit this closes
//!
//! `P3-009a` decided which browser origins may reach JARVIS and `P3-009g` decided which callers may. Both are
//! `pub fn` on a value, so a daemon *could* call one and forget the other — and the honest report at the end of
//! each slice said exactly that: **nothing enforces them**. A policy is only a control where it is consulted,
//! and "consulted somewhere in the daemon" is how a pair becomes a single check nobody notices is missing.
//!
//! So this module is the one place a request is admitted, and its output is the thing that proves it was.
//!
//! # Why an admitted request is a type rather than a `bool`
//!
//! [`RequestAdmission`] has **private fields and no public constructor**, so the only way to obtain one is
//! [`RequestGate::decide`] returning `Ok`. That makes "this request passed the gate" a property of the **type**
//! rather than of a caller's discipline: a function that takes a `RequestAdmission` cannot be handed one that
//! skipped a check, because there is no way to build one.
//!
//! A `bool` would have been the same information and none of the guarantee — the caller decides what `true`
//! means, and `true` is also what a default-initialized field holds.
//!
//! # Why the order is stated rather than left to the caller
//!
//! **`Origin` is checked before admission**, and that is a decision worth pinning. The specification makes
//! `Origin` validation unconditional on every incoming connection — "servers **MUST** validate the `Origin`
//! header on all incoming connections" — so a hostile origin must be refused whether or not the caller would
//! otherwise have been admitted. Reversing the order would mean a request with a bad credential is answered
//! `401` and its hostile origin is never examined, which is a request served to a state the spec says to refuse
//! outright.
//!
//! The second consequence is which refusal wins when **both** are wrong: the origin refusal, because it was
//! checked first and is the stricter statement.
//!
//! # What is deliberately not here
//!
//! The gate takes **values** — an `Origin` string, a caller origin, a fingerprint — and never a request. Deriving
//! those from headers is the daemon's, for two reasons: the HTTP layer is the composition root's (it owns the
//! listener), and a gate that read a `HeaderMap` could not be tested without one. So this stays a function of its
//! arguments, which is what makes the order and the pair testable at all.

use std::fmt;

use crate::admission::{AdmissionVerdict, CallerAdmission, CallerLabel, CallerOrigin, Fingerprint};
use crate::serving::ServingConfig;
use jarvis_mcp::OriginVerdict;

/// The two policies a request must satisfy, applied in a stated order.
///
/// Built from a [`ServingConfig`] (which owns the origin policy) and a [`CallerAdmission`]. Holding both in one
/// value is the point: there is no way to apply one and forget the other, because
/// [`RequestGate::decide`] consults both and a partial result cannot be expressed.
#[derive(Clone, Debug)]
pub struct RequestGate {
    serving: ServingConfig,
    admission: CallerAdmission,
}

impl RequestGate {
    /// Builds a gate from the two policies.
    ///
    /// **No consistency check between them, and that is deliberate.** A gate over a loopback-only
    /// `ServingConfig` and a `CallerAdmission` that admits remote callers looks contradictory and is not: the
    /// bind is what makes a remote caller unable to arrive, and the admission policy is what refuses one that
    /// somehow does. Refusing that combination would remove exactly the defence in depth `P3-009g` argues for —
    /// *a control that depends on another control having worked is not a control*. A reverse proxy, a forwarded
    /// socket, or a changed bind are each enough for the pair to be doing different work.
    #[must_use]
    pub const fn new(serving: ServingConfig, admission: CallerAdmission) -> Self {
        Self { serving, admission }
    }

    /// Returns the origin policy in force.
    #[must_use]
    pub const fn serving(&self) -> &ServingConfig {
        &self.serving
    }

    /// Returns the caller policy in force.
    #[must_use]
    pub const fn admission(&self) -> &CallerAdmission {
        &self.admission
    }

    /// Decides one request, or reports which policy refused it.
    ///
    /// `origin` is the raw `Origin` header value, or `None` when the header was absent. `caller` is the origin
    /// **the request layer** established — never anything a caller sent (see [`CallerOrigin`]). `credential` is
    /// the fingerprint of whatever credential the request carried, and `spent_budget` is the rate-limit state the
    /// counting layer holds.
    ///
    /// # Errors
    ///
    /// Returns [`RequestRefusal::Origin`] for a refused `Origin` and [`RequestRefusal::Admission`] for a refused
    /// caller. The origin is checked first, so **a request that fails both is reported as an origin refusal** —
    /// see the module docs on why that order is the specification's rather than a preference.
    pub fn decide(
        &self,
        origin: Option<&str>,
        caller: CallerOrigin,
        credential: Option<&Fingerprint>,
        spent_budget: bool,
    ) -> Result<RequestAdmission, RequestRefusal> {
        let origin_verdict = self.serving.origin_check(origin);
        if !origin_verdict.permits() {
            return Err(RequestRefusal::Origin {
                verdict: origin_verdict,
            });
        }

        let admission_verdict = self.admission.decide(caller, credential, spent_budget);
        if !admission_verdict.permits() {
            return Err(RequestRefusal::Admission {
                verdict: admission_verdict,
            });
        }

        // The recorded label, when the caller's credential names one. Reported so an audit record can say
        // *which* admitted caller made a call, and deliberately not used for the decision — see `CallerLabel`.
        let recorded_label = credential
            .and_then(|presented| self.admission.entry_for(presented))
            .map(|entry| entry.label().clone());

        Ok(RequestAdmission {
            caller,
            recorded_label,
            origin: origin_verdict,
        })
    }
}

/// An admitted request: proof that both policies permitted it.
///
/// **Private fields and no public constructor**, so the only way to hold one is a gate returning `Ok`. A
/// function that takes a `RequestAdmission` therefore cannot receive one that skipped a check — the guarantee is
/// in the type rather than in a caller's care.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestAdmission {
    caller: CallerOrigin,
    recorded_label: Option<CallerLabel>,
    origin: OriginVerdict,
}

impl RequestAdmission {
    /// Returns whether the caller was local or remote.
    #[must_use]
    pub const fn caller(&self) -> CallerOrigin {
        self.caller
    }

    /// Returns the label the admitted caller's allowlist entry records, when it has one.
    ///
    /// `None` for a local caller, which needs no entry — and `None` is distinct from a caller whose entry records
    /// an **absent** label ([`CallerLabel::absent`]), so an audit record can tell "no entry" from "an entry whose
    /// caller sends no `clientInfo`".
    #[must_use]
    pub const fn recorded_label(&self) -> Option<&CallerLabel> {
        self.recorded_label.as_ref()
    }

    /// Returns how the `Origin` header was decided.
    ///
    /// Carried rather than discarded because `Absent` and `Allowed` are different justifications for the same
    /// outcome, and an audit record that could not tell them apart could not answer "was this a browser request".
    #[must_use]
    pub const fn origin(&self) -> OriginVerdict {
        self.origin
    }
}

/// Which policy refused a request.
///
/// The variants are for the **log and the audit record**; the wire answer comes from
/// [`RequestRefusal::refusal_status`], which is uniform per policy because the two policies answer different
/// statuses for reasons the caller can act on.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RequestRefusal {
    /// The `Origin` header was refused.
    Origin {
        /// The verdict, which distinguishes a hostile origin from an unparseable one.
        verdict: OriginVerdict,
    },
    /// The caller was refused.
    Admission {
        /// The verdict, which distinguishes an absent credential from an unknown one and a spent budget.
        verdict: AdmissionVerdict,
    },
}

impl RequestRefusal {
    /// Returns the HTTP status this refusal obliges.
    ///
    /// The statuses differ because the remedies do: `403` for an `Origin` is what the specification requires and
    /// tells a browser to stop, while `401`/`429` describe something about the caller that a client can act on.
    /// Folding them into one status would lose whichever remedy the refused caller needs.
    #[must_use]
    pub const fn refusal_status(&self) -> u16 {
        match self {
            Self::Origin { verdict } => match verdict.refusal_status() {
                Some(status) => status,
                // Unreachable: a refusal is only constructed from a verdict that did not permit, and every such
                // verdict obliges a status. `403` rather than a panic, because a status is a wire answer and
                // panicking while answering a request takes the server down for a policy bug.
                None => 403,
            },
            Self::Admission { verdict } => match verdict.refusal_status() {
                Some(status) => status,
                None => 500,
            },
        }
    }

    /// Returns a bounded, operator-facing reason.
    #[must_use]
    pub fn reason(&self) -> String {
        match self {
            Self::Origin { verdict } => format!("the request origin was refused: {verdict:?}"),
            Self::Admission { verdict } => verdict.reason(),
        }
    }

    /// Returns whether the request would **also** have failed the caller policy.
    ///
    /// Offered so a caller that wants the full picture for an audit record can get it, while `decide` still
    /// reports the origin refusal as the reason. Recomputing both costs one extra comparison and answers a
    /// question a reviewer will otherwise ask: "was the credential checked at all for a hostile origin?"
    ///
    /// It takes the admission verdict rather than recomputing it, because recomputing would need the same four
    /// arguments `decide` took and would be a second place the policy is consulted, with values that could differ.
    ///
    /// **The first version had a bug and its own test found it.** It returned `false` whenever the allowlist was
    /// `local_only`, reasoning that a local-only policy has no credential to examine — which conflates "the
    /// allowlist is empty" with "this request carried nothing to check". That is the same confusion `decide` had
    /// before [`CallerOrigin`] existed, and an empty allowlist does refuse a **remote** caller with a credential,
    /// so the diagnostic must report `true` for one.
    #[must_use]
    pub fn also_refused_admission(&self, admission_verdict: &AdmissionVerdict) -> bool {
        matches!(self, Self::Origin { .. }) && !admission_verdict.permits()
    }
}

impl fmt::Display for RequestRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.reason())
    }
}

impl std::error::Error for RequestRefusal {}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::admission::{AdmittedCaller, DEFAULT_REQUESTS_PER_MINUTE};
    use jarvis_mcp::ServerExposure;

    fn fingerprint(value: &str) -> Fingerprint {
        Fingerprint::parse(value).unwrap_or_else(|error| panic!("{value}: {error}"))
    }

    fn gate(origins: &[&str], callers: Vec<AdmittedCaller>) -> RequestGate {
        let exposure = ServerExposure::new(origins.iter().copied())
            .unwrap_or_else(|error| panic!("{error:?}"));
        let serving = ServingConfig::new(exposure).unwrap_or_else(|error| panic!("{error}"));
        let admission = CallerAdmission::new(callers, DEFAULT_REQUESTS_PER_MINUTE)
            .unwrap_or_else(|error| panic!("{error}"));
        RequestGate::new(serving, admission)
    }

    fn caller(fingerprint_value: &str, name: &str) -> AdmittedCaller {
        AdmittedCaller::new(
            fingerprint(fingerprint_value),
            CallerLabel::new(name, "1.0"),
        )
        .unwrap_or_else(|error| panic!("{error}"))
    }

    /// **A local caller on a loopback-only gate is admitted, which is the case that must work.**
    ///
    /// The positive control for everything below: without it, every refusal test would pass on a gate that
    /// refuses all requests.
    #[test]
    fn a_local_caller_is_admitted_by_the_default_gate() {
        let gate = gate(&[], Vec::new());
        let admitted = gate
            .decide(None, CallerOrigin::Local, None, false)
            .unwrap_or_else(|error| panic!("a local caller must be admitted: {error}"));
        assert_eq!(admitted.caller(), CallerOrigin::Local);
        assert_eq!(admitted.origin(), OriginVerdict::Absent);
        // A local caller has no allowlist entry, so there is no recorded label — which is distinct from an
        // entry whose caller sends no clientInfo.
        assert!(admitted.recorded_label().is_none());
    }

    /// **A remote caller is refused by the default gate, even with an `Origin` the policy allows.**
    ///
    /// Falsified by ignoring the admission decision: the request is admitted and a remote caller reaches the
    /// handler.
    #[test]
    fn a_remote_caller_is_refused_by_the_default_gate() {
        let gate = gate(&[], Vec::new());
        let refusal = gate
            .decide(
                None,
                CallerOrigin::Remote,
                Some(&fingerprint("aaaa")),
                false,
            )
            .err()
            .unwrap_or_else(|| panic!("a remote caller must be refused"));
        assert!(matches!(
            refusal,
            RequestRefusal::Admission {
                verdict: AdmissionVerdict::NotAllowed
            }
        ));
        assert_eq!(refusal.refusal_status(), 401);
    }

    /// **`Origin` is checked before admission, so a request that fails both is reported as an origin refusal.**
    ///
    /// This is the order the specification requires, and the falsification is reversing it: the refusal then
    /// becomes `NotAllowed`, and a request with a hostile origin is answered `401` — telling the caller about its
    /// credential while the origin was never examined.
    ///
    /// The allowed origin is a **loopback** one, because `ServingConfig::new` refuses a policy whose origins
    /// require a remote bind — so a fixture using a public origin would fail one layer earlier and prove nothing
    /// about this gate. That the fixture had to change is itself the check working.
    #[test]
    fn a_hostile_origin_is_refused_before_the_credential_is_considered() {
        let gate = gate(&["http://localhost:3000"], Vec::new());
        // A hostile origin **and** a credential that is not admitted.
        let refusal = gate
            .decide(
                Some("https://evil.example.com"),
                CallerOrigin::Remote,
                Some(&fingerprint("aaaa")),
                false,
            )
            .err()
            .unwrap_or_else(|| panic!("a hostile origin must be refused"));
        assert!(
            matches!(refusal, RequestRefusal::Origin { .. }),
            "the origin must be the reported reason, got {refusal:?}"
        );
        assert_eq!(refusal.refusal_status(), 403);
    }

    /// An allowed origin with an unadmitted credential is refused **as an admission problem**, so the order
    /// does not hide the second policy's answer when the first one passes.
    #[test]
    fn an_allowed_origin_still_requires_an_admitted_credential() {
        let gate = gate(&["http://localhost:3000"], Vec::new());
        let refusal = gate
            .decide(
                Some("http://localhost:3000"),
                CallerOrigin::Remote,
                Some(&fingerprint("aaaa")),
                false,
            )
            .err()
            .unwrap_or_else(|| panic!("an unadmitted credential must be refused"));
        assert!(matches!(refusal, RequestRefusal::Admission { .. }));
        assert_eq!(refusal.refusal_status(), 401);
    }

    /// **A remote caller whose credential is admitted passes both policies**, which is the case the whole
    /// apparatus exists for.
    ///
    /// Without it, "every remote caller is refused" would satisfy the tests above.
    #[test]
    fn an_admitted_remote_caller_passes_both_policies() {
        let gate = gate(&["http://localhost:3000"], vec![caller("aaaa", "vscode")]);
        let admitted = gate
            .decide(
                Some("http://localhost:3000"),
                CallerOrigin::Remote,
                Some(&fingerprint("aaaa")),
                false,
            )
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(admitted.caller(), CallerOrigin::Remote);
        assert_eq!(admitted.origin(), OriginVerdict::Allowed);
        assert_eq!(
            admitted.recorded_label().map(|label| label.name.as_str()),
            Some("vscode"),
            "the audit record must be able to name which admitted caller this was"
        );
    }

    /// **An admitted remote caller with an unlisted origin is refused as an origin problem — the opposite order
    /// of the both-fail case.**
    ///
    /// Together with `a_hostile_origin_is_refused_before_the_credential_is_considered` this pins that both
    /// policies are genuinely consulted rather than one shadowing the other.
    #[test]
    fn an_admitted_credential_does_not_excuse_an_unlisted_origin() {
        let gate = gate(&["http://localhost:3000"], vec![caller("aaaa", "vscode")]);
        let refusal = gate
            .decide(
                Some("https://evil.example.com"),
                CallerOrigin::Remote,
                Some(&fingerprint("aaaa")),
                false,
            )
            .err()
            .unwrap_or_else(|| panic!("the origin must be refused"));
        assert!(
            matches!(refusal, RequestRefusal::Origin { .. }),
            "a valid credential must not excuse an origin, got {refusal:?}"
        );
    }

    /// **A spent budget is refused even though both other policies pass.**
    ///
    /// Falsified by dropping `spent_budget` from the admission call: the request is admitted and the bound is
    /// decorative.
    #[test]
    fn a_spent_budget_is_refused_after_both_policies_pass() {
        let gate = gate(&[], vec![caller("aaaa", "vscode")]);
        let refusal = gate
            .decide(None, CallerOrigin::Remote, Some(&fingerprint("aaaa")), true)
            .err()
            .unwrap_or_else(|| panic!("a spent budget must be refused"));
        assert!(matches!(
            refusal,
            RequestRefusal::Admission {
                verdict: AdmissionVerdict::RateLimited { .. }
            }
        ));
        assert_eq!(refusal.refusal_status(), 429);
    }

    /// **An admitted request cannot be fabricated**, which is the property the type carries.
    ///
    /// Asserted by construction: there is no public constructor and the fields are private, so this test's own
    /// body is the evidence — it can only obtain a `RequestAdmission` from `decide`. A `bool` return would let a
    /// caller decide what "admitted" means.
    #[test]
    fn an_admitted_request_only_comes_from_the_gate() {
        let gate = gate(&[], Vec::new());
        // The only expression that produces the value under test.
        let admitted: RequestAdmission = gate
            .decide(None, CallerOrigin::Local, None, false)
            .unwrap_or_else(|error| panic!("{error}"));
        // And it is comparable, so a daemon can log or assert one without reaching into it.
        let again = gate
            .decide(None, CallerOrigin::Local, None, false)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(admitted, again);
    }

    /// The two policies are readable from the gate, so a daemon can report what is in force without holding them
    /// separately — which is how one of them comes to be forgotten.
    #[test]
    fn the_gate_exposes_both_policies() {
        let gate = gate(&["http://localhost:3000"], vec![caller("aaaa", "vscode")]);
        assert!(!gate.serving().exposure().is_loopback_only());
        assert!(!gate.admission().is_local_only());
        assert_eq!(gate.admission().admitted().len(), 1);
    }

    /// A refusal's `reason` names the policy, so a log line answers "which control refused this" without the
    /// reader having to know the status mapping.
    #[test]
    fn a_refusal_reason_names_the_policy() {
        let gate = gate(&[], Vec::new());
        let origin_refusal = gate
            .decide(
                Some("https://evil.example.com"),
                CallerOrigin::Local,
                None,
                false,
            )
            .err()
            .unwrap_or_else(|| panic!("the origin must be refused"));
        assert!(
            origin_refusal.reason().contains("origin"),
            "got: {}",
            origin_refusal.reason()
        );

        let admission_refusal = gate
            .decide(None, CallerOrigin::Remote, None, false)
            .err()
            .unwrap_or_else(|| panic!("the caller must be refused"));
        assert!(
            admission_refusal.reason().contains("credential"),
            "got: {}",
            admission_refusal.reason()
        );
    }

    /// **The diagnostic answers "was the credential checked at all for a hostile origin?"**
    ///
    /// `decide` reports the origin refusal and stops, which is correct — but a reviewer will ask whether the
    /// credential was examined, and an audit record that cannot answer is one an operator cannot use to tell a
    /// probing caller from a misconfigured one. `also_refused_admission` recomputes the second verdict so the
    /// answer is available without changing which refusal is reported.
    #[test]
    fn the_diagnostic_reports_when_both_policies_would_have_refused() {
        let gate = gate(&["http://localhost:3000"], Vec::new());
        // A hostile origin **and** an unadmitted credential would fail both. The gate over an empty allowlist is
        // `local_only`, so the helper reports `false` for a *local* caller by design — there is no credential to
        // examine. A remote caller is the case it answers for.
        let remote_fails =
            gate.admission()
                .decide(CallerOrigin::Remote, Some(&fingerprint("aaaa")), false);
        assert!(!remote_fails.permits());

        let refusal = gate
            .decide(
                Some("https://evil.example.com"),
                CallerOrigin::Remote,
                Some(&fingerprint("aaaa")),
                false,
            )
            .err()
            .unwrap_or_else(|| panic!("the origin must be refused"));
        // The reported refusal is the origin's, and the diagnostic confirms the credential would have failed too.
        assert!(matches!(refusal, RequestRefusal::Origin { .. }));
        assert!(refusal.also_refused_admission(&remote_fails));
    }
}
