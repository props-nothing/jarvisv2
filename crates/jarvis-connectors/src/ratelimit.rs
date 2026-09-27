//! Rate limits and retry classification: what the connector knows about the provider's budget.
//!
//! `tools-and-connectors.md` gives a connector "provider client, pagination, rate limits, retry
//! classification, and request IDs", and `security.md` lists "Cost/resource abuse" with "rate/concurrency
//! limits" as the control. This module is the vocabulary for both, and the two are together because they are
//! the same decision: **what to do when the provider says no**.
//!
//! # Why the rate limit is declared rather than discovered
//!
//! A connector could learn a provider's limits by being throttled. Declaring them is better for a specific
//! reason: a limit discovered by exhaustion has already **spent** the budget, and the first thing that
//! happens in a fresh deployment is a burst of work — onboarding, a first sync, a webhook backlog — which is
//! exactly when burning the daily quota is most damaging. `P3-003`'s escalation reasoning applies: a limit
//! the connector can *state* before it runs is a limit policy can schedule against.
//!
//! # Why retry classification is not just a duration
//!
//! `docs/architecture/tools-and-connectors.md`: "Automatic retry of `unknown` is forbidden for non-idempotent
//! effects", and `P3-005`'s outcome mapping is where that is enforced. What a connector adds is the
//! **provider-side** classification: which provider responses mean "try again" and which mean "this will
//! never work". Getting that wrong in the permissive direction sends a second effect; getting it wrong in the
//! strict direction abandons work that would have succeeded. So [`RetryClass`] is a closed set of provider
//! response shapes, and [`RetryDecision`] is the classification of one response.

use std::fmt;

use serde::{Deserialize, Serialize};

/// The most requests a rate limit may allow in one window.
pub const MAX_RATE_LIMIT_PER_WINDOW: u32 = 1_000_000;

/// The most requests a rate limit may allow in a burst.
pub const MAX_RATE_LIMIT_BURST: u32 = 100_000;

/// The longest a provider may ask a caller to wait, in seconds.
///
/// One hour. A provider asking for longer is describing a quota window rather than a transient limit, and
/// honouring it inside a retry loop would hold a worker for the whole window — which is why the bound exists
/// and why a longer value is refused rather than clamped: a clamp would silently retry sooner than the
/// provider asked.
pub const MAX_RETRY_AFTER_SECONDS: u32 = 3_600;

/// What a rate limit applies to.
///
/// `security.md`'s control is "per-actor/client/workspace budgets, rate/concurrency limits", and the scope
/// decides **whose** budget is spent. This matters because a provider limit is usually per account while a
/// JARVIS budget is per workspace: a connector that conflated them would let one workspace exhaust a
/// shared account's quota and report the refusal as that workspace's own limit.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RateLimitScope {
    /// The provider applies it per connected account.
    PerAccount,
    /// The provider applies it per user.
    PerUser,
    /// The provider applies it per application or client identifier.
    PerClient,
    /// The provider applies it globally to the application.
    Global,
}

impl RateLimitScope {
    /// Returns the stable snake-case code.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PerAccount => "per_account",
            Self::PerUser => "per_user",
            Self::PerClient => "per_client",
            Self::Global => "global",
        }
    }

    /// Returns whether spending from one account affects another.
    ///
    /// `Global` and `PerClient` do, which is the property a scheduler needs: exhausting a global budget makes
    /// every account unavailable, so a caller must not report the failure against one account. The same
    /// reasoning as pgvector's multitenancy note (`ADR-0053`), where one tenant's vectors affect another's
    /// recall — a shared resource makes per-account reasoning wrong.
    #[must_use]
    pub const fn is_shared_between_accounts(self) -> bool {
        matches!(self, Self::Global | Self::PerClient)
    }
}

/// A provider rate limit, as the connector declared it.
///
/// Both the sustained rate and the burst are required, because a limit with only one of them is unusable: a
/// rate with no burst allowance throttles a caller that has been idle, and a burst with no rate allows an
/// unbounded average. This is the same reasoning as a token bucket needing both a refill rate and a capacity.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RateLimit {
    /// The requests allowed per window, sustained.
    pub per_window: u32,
    /// The length of that window, in seconds.
    pub window_seconds: u32,
    /// The most requests allowed in one burst.
    pub burst: u32,
    /// Whose budget is spent.
    pub scope: RateLimitScope,
    /// Whether the limit is documented by the provider or inferred.
    ///
    /// Three-valued like [`crate::health::ProbeOutcome`]'s shape, because "the vendor documents 250
    /// quota units per user per second" and "we observed a 429 after about this many requests" require
    /// different confidence. A limit that an operator reads as documented when it was guessed is a budget the
    /// deployment may plan around incorrectly.
    pub evidence: RateLimitEvidence,
}

/// Where a declared rate limit came from.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RateLimitEvidence {
    /// The provider's own documentation states it, and the manifest links that page.
    Documented,
    /// Observed from provider responses, with no documented figure.
    Observed,
    /// Nobody has established it.
    Unknown,
}

impl RateLimitEvidence {
    /// Returns the stable snake-case code.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Documented => "documented",
            Self::Observed => "observed",
            Self::Unknown => "unknown",
        }
    }

    /// Returns whether a scheduler may plan against this figure.
    ///
    /// Only a documented limit, because planning against an observed one means the plan is derived from the
    /// limits of the workload that produced the observation rather than from the provider's rules.
    #[must_use]
    pub const fn is_plannable(self) -> bool {
        matches!(self, Self::Documented)
    }
}

impl RateLimit {
    /// Validates a rate limit.
    ///
    /// # Errors
    ///
    /// Returns [`RateLimitError::Shape`] for a zero window, or a zero count, or a count or burst above the
    /// module's ceilings. Zero is refused rather than read as "unlimited", which is the reasoning every other
    /// bound in this workspace records: a zero read as unlimited is an unbounded budget.
    pub fn new(
        per_window: u32,
        window_seconds: u32,
        burst: u32,
        scope: RateLimitScope,
        evidence: RateLimitEvidence,
    ) -> Result<Self, RateLimitError> {
        if per_window == 0 || per_window > MAX_RATE_LIMIT_PER_WINDOW {
            return Err(RateLimitError::Shape {
                reason: "a rate limit must allow 1 to 1000000 requests per window; zero would read as \
                         `unlimited`",
            });
        }
        if window_seconds == 0 {
            return Err(RateLimitError::Shape {
                reason: "a rate limit's window must be at least one second",
            });
        }
        if burst == 0 || burst > MAX_RATE_LIMIT_BURST {
            return Err(RateLimitError::Shape {
                reason: "a rate limit's burst must be 1 to 100000; a limit with no burst allowance would \
                         throttle a caller that has been idle",
            });
        }
        Ok(Self {
            per_window,
            window_seconds,
            burst,
            scope,
            evidence,
        })
    }

    /// Returns the sustained requests per second, rounded down.
    ///
    /// Used by a scheduler, and the rounding direction is deliberate: a rate that **under**-estimates keeps
    /// the caller inside the provider's limit, while rounding up would exceed it by a fraction on every
    /// window. `P3-002`'s `MAX_SCOPE_CHARS` lesson applies — a bound that cannot be reached enforces
    /// nothing, so the arithmetic has to be the one that binds.
    #[must_use]
    pub const fn sustained_per_second(&self) -> u32 {
        self.per_window / self.window_seconds
    }

    /// Returns whether a burst of `count` requests is within the limit.
    #[must_use]
    pub const fn admits_burst(&self, count: u32) -> bool {
        count <= self.burst
    }
}

/// What a provider response means for a retry.
///
/// A closed set of **provider response shapes**, each of which a connector classifies one of. The set is
/// closed rather than free text because a retry decision is a policy input and a typo in a free-text class
/// would silently change it — `P3-008c`'s rule: "DO NOT DERIVE A SAFETY FLAG FROM MESSAGE TEXT", where the
/// first version matched a `Display` string and a reworded error would have stopped a refusal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetryClass {
    /// The provider refused transiently; retrying after the stated delay is expected to succeed.
    Transient,
    /// The provider throttled the caller; retrying before the stated delay is counterproductive.
    Throttled,
    /// The provider rejected the request itself, which will not change on a retry.
    Permanent,
    /// The request needs new authorization.
    Authentication,
    /// The provider had an internal failure; retrying may succeed.
    ProviderFault,
    /// The result is unknown, so the effect may or may not have happened.
    ///
    /// The dangerous class, and the reason `tools-and-connectors.md` says automatic retry of `unknown` is
    /// forbidden for non-idempotent effects. Representable because it is a real outcome a connector observes,
    /// and its `permits_automatic_retry` answer is `false` in every case — an unanswered question must not be
    /// read as a yes, the same reasoning as [`crate::manifest::ProviderIdempotency::Unknown`].
    Unknown,
}

impl RetryClass {
    /// Returns the stable snake-case code.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Transient => "transient",
            Self::Throttled => "throttled",
            Self::Permanent => "permanent",
            Self::Authentication => "authentication",
            Self::ProviderFault => "provider_fault",
            Self::Unknown => "unknown",
        }
    }

    /// Returns whether the class permits **any** automatic retry, given the effect's idempotency.
    ///
    /// Takes the idempotency rather than assuming one, because the two together are the decision:
    /// `Transient` on an idempotent operation retries, `Transient` on a non-idempotent one does not. `Unknown`
    /// never retries whatever the idempotency, because the question it asks — "did the effect happen" — has
    /// not been answered and a retry answers it by making it happen twice.
    #[must_use]
    pub const fn permits_automatic_retry(
        self,
        idempotency: crate::manifest::ProviderIdempotency,
    ) -> bool {
        match self {
            // The dangerous class is named FIRST and separately, because it is the one case where the
            // idempotency argument does not matter at all — merging it with the refusal arm would still behave
            // correctly but would lose the reason. `clippy::match_same_arms` fires on the two `false` arms and
            // is satisfied by merging them; the comment is what carries the distinction.
            Self::Unknown | Self::Permanent | Self::Authentication => false,
            Self::Transient | Self::Throttled | Self::ProviderFault => {
                idempotency.permits_automatic_retry()
            }
        }
    }

    /// Returns whether a user must act before the request can succeed.
    #[must_use]
    pub const fn needs_user(self) -> bool {
        matches!(self, Self::Authentication)
    }
}

/// The classification of one provider response, with what to do about it.
///
/// Carries the **class and the guidance together**, so a caller cannot read one without the other. `P3-006a`
/// established this shape for exactly this reason: a class that a caller could act on without reading the
/// delay would retry immediately, which for a `Throttled` response is the one thing the provider asked it not
/// to do.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RetryDecision {
    /// What the response meant.
    pub class: RetryClass,
    /// The guidance.
    pub guidance: RetryGuidance,
    /// The provider's own request identifier, when it supplied one.
    ///
    /// Required by `tools-and-connectors.md`'s "preserve provider IDs and receipts separately from
    /// user-facing text", and carried here rather than derived from the class because it is the only value
    /// that lets a support conversation proceed with the provider.
    pub provider_request_id: Option<ProviderRequestId>,
}

/// What to do after a classified response.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetryGuidance {
    /// Retry after the stated number of seconds.
    RetryAfterSeconds(u32),
    /// Retry with exponential backoff, starting from the stated number of seconds.
    BackoffSeconds(u32),
    /// Do not retry; the request is permanently unusable as sent.
    DoNotRetry,
    /// Do not retry; a user must reconnect.
    Reauthenticate,
    /// Do not retry; establish what happened before doing anything else.
    ///
    /// The only guidance for a [`RetryClass::Unknown`], and its name is the instruction: the caller must
    /// reconcile rather than retry. `tools-and-connectors.md`'s "`unknown` reconciliation" is this, and a
    /// caller that mapped it to `Transient` would send a second effect.
    Reconcile,
}

impl RetryGuidance {
    /// Returns whether the guidance permits a retry at all.
    #[must_use]
    pub const fn permits_retry(self) -> bool {
        matches!(self, Self::RetryAfterSeconds(_) | Self::BackoffSeconds(_))
    }

    /// Returns the delay, when the guidance states one.
    #[must_use]
    pub const fn delay_seconds(self) -> Option<u32> {
        match self {
            Self::RetryAfterSeconds(seconds) | Self::BackoffSeconds(seconds) => Some(seconds),
            Self::DoNotRetry | Self::Reauthenticate | Self::Reconcile => None,
        }
    }
}

/// A provider's own request identifier.
///
/// A validated value rather than a `String`, because it is the string an operator reads back to a vendor's
/// support, so a value containing a newline would forge a log line and one containing a control character
/// would rewrite a terminal. `tools-and-connectors.md`'s "provider IDs preserved separately from user-facing
/// text" is why this is its own type rather than a field on the output.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderRequestId(String);

impl ProviderRequestId {
    /// The longest accepted identifier.
    pub const MAX_BYTES: usize = 256;

    /// Validates a provider request identifier.
    ///
    /// # Errors
    ///
    /// Returns [`RateLimitError::RequestId`] when the value is empty, oversized, or holds a control
    /// character.
    pub fn new(value: impl Into<String>) -> Result<Self, RateLimitError> {
        let value = value.into();
        if value.is_empty() || value.len() > Self::MAX_BYTES || value.chars().any(char::is_control)
        {
            return Err(RateLimitError::RequestId);
        }
        Ok(Self(value))
    }

    /// Returns the identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProviderRequestId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// The outcome of asking a budget whether a request may proceed.
///
/// Named rather than a `bool`, because "you may proceed", "wait this long", and "this scope is exhausted for
/// the window" lead to three different actions and a boolean would make two of them read as refusals.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BudgetOutcome {
    /// The request is within budget.
    Allowed,
    /// The request would exceed the sustained rate; wait this long.
    WaitForSeconds(u32),
    /// The scope's window is exhausted.
    ///
    /// Distinct from [`Self::WaitForSeconds`], which is a **short** delay for smoothing a burst. An exhausted
    /// window means the work should be deferred rather than retried in a loop, because the delay is minutes
    /// or hours — and a caller that treated it as a short wait would spin for the whole window.
    Exhausted,
    /// The budget is shared and another account has spent it.
    ///
    /// Reported separately because the remedy is not this account's: a caller must not tell an operator that
    /// *their* account exceeded a limit that another account consumed. This is
    /// [`RateLimitScope::is_shared_between_accounts`]'s consequence.
    SharedBudgetSpent,
}

impl BudgetOutcome {
    /// Returns whether the request may proceed.
    #[must_use]
    pub const fn is_allowed(self) -> bool {
        matches!(self, Self::Allowed)
    }

    /// Returns whether waiting and retrying is the right response.
    ///
    /// Only for a **short** stated delay. [`Self::Exhausted`] and [`Self::SharedBudgetSpent`] are both
    /// false, which is the whole reason they are separate variants: a scheduler that retried them in a loop
    /// would spin for the length of a quota window.
    #[must_use]
    pub const fn should_wait_and_retry(self) -> bool {
        matches!(self, Self::WaitForSeconds(_))
    }
}

/// Why a rate limit or retry declaration is unusable.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum RateLimitError {
    /// The rate limit's shape is unusable.
    #[error("the rate limit is unusable: {reason}")]
    Shape {
        /// What is wrong.
        reason: &'static str,
    },
    /// A provider request identifier is unusable.
    #[error("a provider request identifier must be 1 to 256 characters with no control characters")]
    RequestId,
    /// A retry-after value is longer than the caller may hold.
    #[error("a provider asked to wait {requested} seconds, above the {maximum} a caller may hold")]
    RetryAfterTooLong {
        /// What the provider asked for.
        requested: u32,
        /// [`MAX_RETRY_AFTER_SECONDS`].
        maximum: u32,
    },
}

#[cfg(test)]
#[path = "ratelimit_tests.rs"]
mod tests;
