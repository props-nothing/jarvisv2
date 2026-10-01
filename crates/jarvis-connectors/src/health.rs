//! Connector health: what is true about an account **now**, and what evidence says so.
//!
//! `tools-and-connectors.md` lists "account setup and connectivity verification" and "health and
//! secret-redacted diagnostics" as connector ownership, and `security.md`'s rule for the whole platform is
//! "missing or stale evidence fails closed". This module is where those two meet.
//!
//! # Why health is not a boolean
//!
//! The obvious shape is `healthy: bool`, and it is wrong in the way that matters most: **`false` cannot say
//! why**. An account that needs a user to reauthorize, one the provider has rate-limited, one whose token
//! expired, and one nobody has ever tested require four different responses — and three of them are not
//! failures. `P3-003`'s `DenyReason::is_refusal()` separated a hold from a refusal for exactly this reason
//! ("a refusal would make that documented resume path unreachable"), and `jarvis-storage`'s
//! `AccountStatus`-shaped problems record the same lesson.
//!
//! So [`ConnectorHealth`] is an enum of states, and the two that permit work are narrow.
//!
//! # Why every state carries a probe
//!
//! A state on its own is a claim. `docs/quality/acceptance-tests.md`'s `A10` requires that "onboarding
//! verifies provider identity and connectivity before saving", which is a statement about **evidence**: the
//! check happened and it said something. [`HealthProbe`] records what was checked, when, and what came back,
//! so a stored health state without a probe is not constructible — the same reasoning `jarvis-tools`'
//! `ToolOutcomeRecord` uses when it refuses a `confirmed` outcome with no evidence.
//!
//! # Why staleness is a first-class concept
//!
//! A health state is a fact about a moment. A connector that recorded `Connected` yesterday and reports it
//! today is reporting something it does not know, and the requirement it violates is `security.md`'s: stale
//! evidence must fail closed. [`ConnectorHealth::is_fresh_at`] therefore takes the current instant and the
//! freshness bound, rather than the state carrying an opinion about its own age.

use std::fmt;

use jarvis_core::UtcTimestamp;

/// The longest accepted health detail or reauth reason.
pub const MAX_HEALTH_DETAIL_CHARS: usize = 500;

/// The default age at which a health observation must be refreshed.
///
/// Five minutes: long enough that a healthy account is not re-probed on every call, short enough that a
/// revocation is noticed within one working interaction. It is a **named default a caller passes to
/// [`ConnectorHealth::permits_calls_at`]** rather than something this crate reads internally, because the
/// freshness bound is a policy decision whose owner is the daemon — so the constant documents the value to
/// reach for rather than deciding it here.
pub const DEFAULT_FRESHNESS_SECONDS: u64 = 300;

/// What a health probe checked.
///
/// Named per check because "the account is unhealthy" is not actionable while "the token endpoint refused"
/// is. `docs/operations/observability.md` requires content-free spans by default, so these are **classes of
/// check** rather than descriptions of a response body.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HealthProbe {
    /// The provider's identity endpoint confirmed which account this is.
    Identity,
    /// A token refresh succeeded.
    Refresh,
    /// A representative read returned successfully.
    Read,
    /// The provider's push subscription is still registered.
    Subscription,
    /// The provider reported a rate limit or quota refusal.
    RateLimit,
    /// Nothing was checked.
    ///
    /// Representable because "we have not checked" is a real state and must not be reported as one of the
    /// definite checks — the same reasoning `jarvis_models::Normalization::Unknown` records. A
    /// [`ConnectorHealth::Unknown`] state is the only one that carries it.
    None,
}

impl HealthProbe {
    /// Returns the stable snake-case code.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Identity => "identity",
            Self::Refresh => "refresh",
            Self::Read => "read",
            Self::Subscription => "subscription",
            Self::RateLimit => "rate_limit",
            Self::None => "none",
        }
    }
}

/// What a probe observed, recorded with its time.
///
/// The pairing of a check with its outcome and instant is what makes a health state **evidence** rather than
/// an assertion. It is a struct with a private constructor so an outcome cannot be recorded without the
/// check and the time that produced it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HealthSignal {
    probe: HealthProbe,
    succeeded: bool,
    detail: Option<String>,
    observed_at: UtcTimestamp,
}

impl HealthSignal {
    /// Records what a check observed.
    ///
    /// # Errors
    ///
    /// Returns [`crate::ConnectorError::Identifier`] when a detail is empty, oversized, or holds a control
    /// character. A detail is **optional** because a success needs none, and refused when supplied but
    /// unusable — and the control-character refusal matters because a detail is rendered by `doctor` into a
    /// terminal, where an escape sequence can rewrite the surrounding lines.
    pub fn new(
        probe: HealthProbe,
        succeeded: bool,
        detail: Option<String>,
        observed_at: UtcTimestamp,
    ) -> Result<Self, crate::ConnectorError> {
        if let Some(detail) = detail.as_deref()
            && (detail.trim().is_empty()
                || detail.chars().count() > MAX_HEALTH_DETAIL_CHARS
                || detail.chars().any(char::is_control))
        {
            return Err(crate::ConnectorError::Identifier {
                value: detail.to_owned(),
                reason: "a health detail, when present, must be 1 to 500 characters with no control \
                         characters, because an operator's terminal renders it",
            });
        }
        Ok(Self {
            probe,
            succeeded,
            detail,
            observed_at,
        })
    }

    /// Returns a successful observation with no detail.
    #[must_use]
    pub const fn succeeded(probe: HealthProbe, observed_at: UtcTimestamp) -> Self {
        Self {
            probe,
            succeeded: true,
            detail: None,
            observed_at,
        }
    }

    /// Returns the check that produced this.
    #[must_use]
    pub const fn probe(&self) -> HealthProbe {
        self.probe
    }

    /// Returns whether the check succeeded.
    #[must_use]
    pub const fn is_success(&self) -> bool {
        self.succeeded
    }

    /// Returns the detail, when there is one.
    #[must_use]
    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }

    /// Returns when the check ran.
    #[must_use]
    pub const fn observed_at(&self) -> UtcTimestamp {
        self.observed_at
    }
}

/// Why an account needs reauthorization.
///
/// `tools-and-connectors.md` requires a "reauth path that preserves account references without hiding lost
/// scopes", and the reasons below are what that path displays. They are separated because the **remedy**
/// differs: a `Revoked` account needs the user to grant access again, an `Expired` one needs the same, but a
/// `ScopeLoss` may be fixable by requesting only the incremental scope it lost — and telling a user to
/// "reconnect" for a scope shortfall sends them through a consent screen that will not say which scope
/// mattered.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReauthReason {
    /// The provider revoked the grant, or the user did.
    Revoked,
    /// The refresh token expired.
    Expired,
    /// The grant no longer includes a scope the connector needs.
    ScopeLoss,
    /// The provider refused the refresh for a reason that indicates the grant is gone.
    ///
    /// Distinct from [`Self::Revoked`] because nothing in the response said the user acted; a provider change
    /// that broke the client identifier lands here, and its remedy is an operator's rather than a user's.
    ProviderRefused,
}

impl ReauthReason {
    /// Returns the stable snake-case code.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Revoked => "revoked",
            Self::Expired => "expired",
            Self::ScopeLoss => "scope_loss",
            Self::ProviderRefused => "provider_refused",
        }
    }

    /// Returns whether a user can resolve this by reconnecting.
    ///
    /// `ProviderRefused` is the exception and it is the interesting one: if the provider rejected the client
    /// itself, then a user reconnecting hits the same refusal, and a message telling them to reconnect would
    /// be a loop. `P3-008i` records this shape of defect — a message must not assert a cause it cannot know.
    #[must_use]
    pub const fn is_user_resolvable(self) -> bool {
        matches!(self, Self::Revoked | Self::Expired | Self::ScopeLoss)
    }
}

/// What is true about a connected account now.
///
/// The two variants that permit calls are [`Self::Connected`] and [`Self::Degraded`], and everything else
/// refuses — the `security.md` rule that missing or stale evidence fails closed, expressed as the type's
/// default direction. [`Self::Unknown`] exists so "nobody has checked" is representable, because a state
/// machine whose only options are healthy and unhealthy forces an unchecked account into one of them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConnectorHealth {
    /// The last probe succeeded and the account can serve every declared operation.
    Connected {
        /// The observation that says so.
        signal: HealthSignal,
    },
    /// The provider is refusing requests, typically a rate limit or an outage.
    ///
    /// **Still permits the calls the provider will accept**, because a rate limit is not a revocation: an
    /// account in this state may succeed on a read the provider has budget for. That is why it carries a
    /// retry hint rather than being a refusal.
    Degraded {
        /// The observation that says so.
        signal: HealthSignal,
        /// When the connector believes the condition will clear, when the provider said.
        retry_after: Option<UtcTimestamp>,
    },
    /// A user must reconnect before anything works.
    NeedsReauth {
        /// Why.
        reason: ReauthReason,
        /// The scopes that are missing, when the reason is a scope loss.
        missing_scopes: Vec<String>,
        /// The observation that says so.
        signal: HealthSignal,
    },
    /// The account was disconnected deliberately.
    Disconnected {
        /// The observation that says so.
        signal: HealthSignal,
    },
    /// Nothing has been checked.
    ///
    /// Carries a probe of [`HealthProbe::None`], so the value says **what** was not checked rather than only
    /// that something was not.
    Unknown {
        /// The observation that says so.
        signal: HealthSignal,
    },
}

impl ConnectorHealth {
    /// Returns whether a call may be attempted.
    ///
    /// `Connected` and `Degraded` only. `Degraded` is included deliberately: a provider that rate-limited one
    /// request has not said the account is unusable, and refusing everything would make an ordinary throttle
    /// look like a disconnect — the over-blocking direction `P4-008` recorded as safe to *fail* in, but wrong
    /// here because the account is genuinely still connected and the caller can be told to retry.
    #[must_use]
    pub const fn permits_calls(&self) -> bool {
        matches!(self, Self::Connected { .. } | Self::Degraded { .. })
    }

    /// Returns whether a user must act.
    #[must_use]
    pub const fn needs_user(&self) -> bool {
        matches!(self, Self::NeedsReauth { .. })
    }

    /// Returns the observation behind this state.
    #[must_use]
    pub const fn signal(&self) -> &HealthSignal {
        match self {
            Self::Connected { signal }
            | Self::Degraded { signal, .. }
            | Self::NeedsReauth { signal, .. }
            | Self::Disconnected { signal }
            | Self::Unknown { signal } => signal,
        }
    }

    /// Returns the reauth reason, when the account needs reconnecting.
    #[must_use]
    pub fn reauth_reason(&self) -> Option<ReauthReason> {
        match self {
            Self::NeedsReauth { reason, .. } => Some(*reason),
            _ => None,
        }
    }

    /// Returns the scopes the account is missing, when a reauth state carries them.
    ///
    /// # Why this exists: the field had a producer and no reader
    ///
    /// [`Self::NeedsReauth`]'s `missing_scopes` is documented as *"the scopes that are missing, when the
    /// reason is a scope loss"*, and `crate::diagnostics::diagnostics_for` emits
    /// [`DiagnosticField::MissingScopes`](crate::diagnostics::DiagnosticField::MissingScopes) — described as
    /// *"the list a reauth prompt needs"* — from a **separate** argument it takes from its caller. So the
    /// state's own list was read by **nothing**: a caller that built a `ScopeLoss` reauth state and passed an
    /// empty shortfall reported no missing scopes at all, which is the one list the reauth prompt exists to
    /// name. That is `ADR-0092`'s "a value with a producer and no reader" and `ADR-0021`'s "two values that
    /// must agree, with nothing making them" in one field (`ADR-0116`).
    ///
    /// The accessor is the reader, and `diagnostics_for` uses it, so the list the state carries reaches the
    /// report rather than being droppable by a caller. **Empty for every state that carries none**, so a
    /// caller branches on `is_empty` rather than matching the enum — the same accessor shape
    /// [`Self::reauth_reason`] uses, and the reason the return is a slice rather than an `Option<&Vec>`: an
    /// absent list and an empty one call for the same action here.
    #[must_use]
    pub fn missing_scopes(&self) -> &[String] {
        match self {
            Self::NeedsReauth { missing_scopes, .. } => missing_scopes,
            Self::Connected { .. }
            | Self::Degraded { .. }
            | Self::Disconnected { .. }
            | Self::Unknown { .. } => &[],
        }
    }

    /// Returns whether this state was observed within `freshness_seconds` of `now`.
    ///
    /// The stale-evidence rule, and it takes the current instant rather than consulting a clock internally:
    /// a value that asked the system clock about its own age could not be checked against a supplied instant,
    /// and `jarvis_core::Clock` exists precisely so time is injectable. A state older than the bound is
    /// **stale**, and a caller must treat stale as [`Self::Unknown`] rather than as whatever it says.
    #[must_use]
    pub fn is_fresh_at(&self, now: UtcTimestamp, freshness_seconds: u64) -> bool {
        let observed = self.signal().observed_at();
        // `unix_nanos` rather than a comparison of the RFC3339 text: `jarvis_core::UtcTimestamp`'s rendered
        // form omits a zero fraction, so two instants in one second do not sort lexicographically
        // (`P3-004`'s recorded trap). Comparing integers is the only ordering that is correct for all values.
        let Some(elapsed) = now.unix_nanos().checked_sub(observed.unix_nanos()) else {
            return false;
        };
        // A NEGATIVE elapsed means the observation is in the future, and it is not fresh. This was a real
        // defect the test caught: the first version only rejected the overflow case, so `checked_sub`'s
        // `Some(negative)` fell through to the bound comparison and **a future observation was reported as
        // fresh**. The consequences are not symmetric — treating a future observation as fresh lets a record
        // whose clock moved backwards look current, which is the failure direction that admits stale
        // evidence. The documented intent and the implementation disagreed and the test is what found it.
        if elapsed < 0 {
            return false;
        }
        elapsed <= i128::from(freshness_seconds) * 1_000_000_000
    }

    /// Returns whether a call may be attempted **given the current instant**.
    ///
    /// The method a caller should use, because it combines the state with the staleness rule. Calling
    /// [`Self::permits_calls`] on an unfresh state is the defect this exists to prevent: a `Connected`
    /// observation from yesterday permits nothing today, and only the pair of methods makes that sayable.
    #[must_use]
    pub fn permits_calls_at(&self, now: UtcTimestamp, freshness_seconds: u64) -> bool {
        self.permits_calls() && self.is_fresh_at(now, freshness_seconds)
    }
}

impl fmt::Display for ConnectorHealth {
    /// Renders the state for an operator, using the probe's own vocabulary.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connected { signal } => {
                write!(
                    formatter,
                    "connected (verified by {})",
                    signal.probe().as_str()
                )
            }
            Self::Degraded { signal, .. } => {
                write!(
                    formatter,
                    "degraded (observed by {})",
                    signal.probe().as_str()
                )
            }
            Self::NeedsReauth { reason, .. } => {
                write!(formatter, "needs reauthorization ({})", reason.as_str())
            }
            Self::Disconnected { .. } => formatter.write_str("disconnected"),
            Self::Unknown { .. } => formatter.write_str("unknown: nothing has been checked"),
        }
    }
}

/// A probe outcome, separated from the health state it produces.
///
/// The distinction between "what a check found" and "what that means for the account" is the module's
/// central separation: a failed refresh means reauth, a failed read means degraded, and a failed identity
/// check means the account is not what was stored. A single `Result` would flatten those into one answer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProbeOutcome {
    /// The check succeeded.
    Succeeded,
    /// The check failed with a reason the caller can act on.
    Failed {
        /// What the check found, in the connector's words.
        detail: String,
    },
    /// The check could not run.
    ///
    /// Distinct from [`Self::Failed`] because "we could not reach the provider" is not evidence that
    /// anything is wrong with the **account**, and treating it as a failure would enter reauth for a network
    /// problem.
    Inconclusive {
        /// Why it could not run.
        detail: String,
    },
}

impl ProbeOutcome {
    /// Returns whether the check produced evidence.
    ///
    /// True for both definite outcomes, false for [`Self::Inconclusive`]. This is the predicate that decides
    /// whether a health state may be updated at all: an inconclusive probe must leave the previous state
    /// alone rather than replacing a `Connected` account with an `Unknown` one, because the previous
    /// observation is still the best evidence available and its **age** is what the staleness rule reports.
    #[must_use]
    pub const fn is_evidence(&self) -> bool {
        !matches!(self, Self::Inconclusive { .. })
    }
}

#[cfg(test)]
#[path = "health_tests.rs"]
mod tests;
