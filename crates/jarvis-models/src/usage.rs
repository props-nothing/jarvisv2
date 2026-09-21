use serde::{Deserialize, Serialize};

/// Token accounting for one model call.
///
/// Cache-read and cache-write counts are recorded separately from fresh prompt
/// tokens because providers bill them at different rates; collapsing them would
/// make a cost record wrong without making it look wrong.
// `_tokens` is the domain's own unit name, so the shared suffix is intentional
// rather than accidental repetition.
#[allow(clippy::struct_field_names)]
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct TokenUsage {
    input_tokens: u64,
    output_tokens: u64,
    cached_input_tokens: u64,
}

impl TokenUsage {
    /// Creates usage from fresh input and output counts.
    #[must_use]
    pub const fn new(input_tokens: u64, output_tokens: u64) -> Self {
        Self {
            input_tokens,
            output_tokens,
            cached_input_tokens: 0,
        }
    }

    /// Records how many input tokens were served from a provider cache.
    #[must_use]
    pub const fn with_cached_input_tokens(mut self, value: u64) -> Self {
        self.cached_input_tokens = value;
        self
    }

    /// Returns the count of input tokens billed at the fresh rate.
    #[must_use]
    pub const fn input_tokens(self) -> u64 {
        self.input_tokens
    }

    /// Returns the count of generated tokens.
    #[must_use]
    pub const fn output_tokens(self) -> u64 {
        self.output_tokens
    }

    /// Returns the count of input tokens served from a provider cache.
    #[must_use]
    pub const fn cached_input_tokens(self) -> u64 {
        self.cached_input_tokens
    }

    /// Returns the total billable token count.
    ///
    /// Checked, not wrapping: a saturating total would hide a counter that had
    /// already overflowed, and a wrapped total would report a large call as a small
    /// one.
    #[must_use]
    pub fn total_tokens(self) -> Option<u64> {
        let combined = self.input_tokens.checked_add(self.output_tokens)?;
        combined.checked_add(self.cached_input_tokens)
    }

    /// Returns whether every counter is still zero.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.input_tokens == 0 && self.output_tokens == 0 && self.cached_input_tokens == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_reads_are_preserved_separately_from_fresh_input() {
        let usage = TokenUsage::new(100, 20).with_cached_input_tokens(900);
        assert_eq!(usage.input_tokens(), 100);
        assert_eq!(usage.cached_input_tokens(), 900);
        assert_eq!(
            usage.total_tokens(),
            Some(1020),
            "a cached token is still a token and must appear in the total"
        );
    }

    #[test]
    fn an_overflowing_total_is_reported_as_absent_rather_than_wrapped() {
        let usage = TokenUsage::new(u64::MAX, 1);
        assert_eq!(
            usage.total_tokens(),
            None,
            "a wrapped total would report a huge call as a tiny one"
        );
    }

    #[test]
    fn absent_usage_is_distinguishable_from_zero_usage() {
        assert!(TokenUsage::default().is_empty());
        assert!(!TokenUsage::new(1, 0).is_empty());
    }
}
