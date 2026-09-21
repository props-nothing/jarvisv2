use std::time::Duration;

/// Bounded retry policy for transient model failures.
///
/// Retry policy lives in JARVIS rather than in the HTTP client because it must
/// honour a provider's `Retry-After` hint, refuse to retry failures that cannot
/// succeed, and let every attempt be recorded. A client-side auto-retry would hide
/// attempts from the run record.
///
/// The policy is intentionally tiny and `Copy` so it can be asserted in tests
/// without a clock.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetryPolicy {
    max_attempts: u32,
    base_delay: Duration,
    max_delay: Duration,
    jitter_source: JitterSource,
}

/// How retry jitter is produced.
///
/// Jitter is injectable so a test can assert a retry count without asserting a
/// random value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum JitterSource {
    /// Derive jitter deterministically from the attempt number.
    Deterministic,
    /// Use the operating-system random source.
    Random,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(30),
            jitter_source: JitterSource::Random,
        }
    }
}

impl RetryPolicy {
    /// Creates a policy with an explicit attempt ceiling.
    ///
    /// `max_attempts` counts the first attempt, so a value of 1 disables retry.
    #[must_use]
    pub const fn new(max_attempts: u32) -> Self {
        Self {
            max_attempts,
            base_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(30),
            jitter_source: JitterSource::Deterministic,
        }
    }

    /// Creates a policy that never retries.
    #[must_use]
    pub const fn none() -> Self {
        Self::new(1)
    }

    /// Replaces the backoff base delay.
    #[must_use]
    pub const fn with_base_delay(mut self, value: Duration) -> Self {
        self.base_delay = value;
        self
    }

    /// Replaces the backoff ceiling.
    #[must_use]
    pub const fn with_max_delay(mut self, value: Duration) -> Self {
        self.max_delay = value;
        self
    }

    /// Uses the operating-system random source for jitter.
    #[must_use]
    pub const fn with_random_jitter(mut self) -> Self {
        self.jitter_source = JitterSource::Random;
        self
    }

    /// Returns the maximum number of attempts, counting the first.
    #[must_use]
    pub const fn max_attempts(self) -> u32 {
        self.max_attempts
    }

    /// Returns whether another attempt is permitted after `attempts_so_far`.
    #[must_use]
    pub const fn allows_attempt(self, attempts_so_far: u32) -> bool {
        attempts_so_far < self.max_attempts
    }

    /// Returns the delay before the attempt following `attempts_so_far`.
    ///
    /// A provider hint always wins over the computed value, because official
    /// guidance is to honour `Retry-After` when it is present. The hint is not
    /// clamped to [`Self::max_delay`] either: the provider is stating how long *it*
    /// needs, and capping that produces another rejection.
    #[must_use]
    pub fn delay_before_attempt(self, attempts_so_far: u32, retry_after: Option<u64>) -> Duration {
        if let Some(seconds) = retry_after {
            return Duration::from_secs(seconds);
        }

        let exponent = attempts_so_far.saturating_sub(1).min(16);
        let factor = 1_u32 << exponent;
        let base = self.base_delay.saturating_mul(factor);
        let capped = base.min(self.max_delay);
        capped.saturating_add(self.jitter(attempts_so_far, capped))
    }

    /// Returns a bounded jitter fraction of the computed delay.
    fn jitter(self, attempt: u32, delay: Duration) -> Duration {
        let quarter = delay / 4;
        if quarter.is_zero() {
            return Duration::ZERO;
        }
        let span = quarter.as_millis().max(1);
        let value = match self.jitter_source {
            JitterSource::Deterministic => {
                // A stable per-attempt offset. Determinism here is a testability
                // choice, not a security property.
                u128::from(attempt).wrapping_mul(2_654_435_761) % span
            }
            JitterSource::Random => {
                let mut bytes = [0_u8; 8];
                if getrandom::fill(&mut bytes).is_err() {
                    // Falling back to no jitter is safe: jitter reduces collisions,
                    // it is not a correctness requirement.
                    return Duration::ZERO;
                }
                u128::from(u64::from_le_bytes(bytes)) % span
            }
        };
        Duration::from_millis(u64::try_from(value).unwrap_or_default())
    }
}

/// Returns whether an HTTP status is worth another attempt.
///
/// `429` and `503` are the documented transient statuses. `5xx` beyond those are
/// treated as transient because a provider-side fault may clear; `4xx` never is,
/// because the request itself must change. Billing, spend, and quota limits arrive
/// as `429` and are **not** retryable, but that distinction lives in the error body,
/// not the status, so the adapter excludes them before this is consulted.
#[must_use]
pub const fn is_retryable_status(status: u16) -> bool {
    matches!(status, 429 | 500 | 502 | 503 | 504)
}

/// Parses a `Retry-After` header value as a delay in seconds.
///
/// Accepts the delay-seconds form only. The HTTP-date form is not supported because
/// it requires trusting the local clock against a remote date, and a skewed clock
/// would produce a negative or wildly large delay. Returning `None` for a date
/// falls back to the computed backoff, which is safe.
#[must_use]
pub fn parse_retry_after(value: &str) -> Option<u64> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    if !trimmed.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    trimmed.parse::<u64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_provider_retry_hint_overrides_the_computed_backoff() {
        let policy = RetryPolicy::new(3).with_base_delay(Duration::from_millis(1));
        let computed = policy.delay_before_attempt(2, None);
        let hinted = policy.delay_before_attempt(2, Some(7));

        assert_eq!(
            hinted,
            Duration::from_secs(7),
            "the provider is stating how long it needs; the hint must win"
        );
        assert!(
            hinted > computed,
            "if the hint did not override, this test proves nothing"
        );
    }

    #[test]
    fn a_provider_hint_is_not_clamped_to_the_local_ceiling() {
        // Capping the provider's stated delay would produce another rejection.
        let policy = RetryPolicy::new(3).with_max_delay(Duration::from_secs(1));
        assert_eq!(
            policy.delay_before_attempt(1, Some(120)),
            Duration::from_secs(120)
        );
    }

    #[test]
    fn backoff_is_capped_and_attempts_are_bounded() {
        let policy = RetryPolicy::new(4)
            .with_base_delay(Duration::from_secs(1))
            .with_max_delay(Duration::from_secs(8));

        for attempt in 1..=8 {
            assert!(
                policy.delay_before_attempt(attempt, None) <= Duration::from_secs(10),
                "backoff must stay near its ceiling"
            );
        }

        assert!(policy.allows_attempt(0));
        assert!(policy.allows_attempt(3));
        assert!(!policy.allows_attempt(4), "attempts must be bounded");
        assert!(!RetryPolicy::none().allows_attempt(1));
    }

    #[test]
    fn a_billing_style_429_status_is_retryable_but_the_adapter_excludes_it_by_code() {
        // The status alone cannot distinguish "slow down" from "out of credit", so
        // this documents why the adapter must classify the body before retrying.
        assert!(is_retryable_status(429));
        assert!(is_retryable_status(503));
        assert!(!is_retryable_status(400));
        assert!(!is_retryable_status(401));
        assert!(!is_retryable_status(404));
        assert!(!is_retryable_status(200));
    }

    #[test]
    fn retry_after_parsing_accepts_seconds_and_rejects_an_http_date() {
        assert_eq!(parse_retry_after("7"), Some(7));
        assert_eq!(parse_retry_after(" 12 "), Some(12));
        assert_eq!(
            parse_retry_after("Wed, 21 Oct 2026 07:28:00 GMT"),
            None,
            "a date would require trusting the local clock"
        );
        assert_eq!(parse_retry_after(""), None);
        assert_eq!(parse_retry_after("-1"), None);
        assert_eq!(parse_retry_after("1e3"), None);
    }
}
