use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, de};
use thiserror::Error;

use crate::CorrelationId;

/// Stable provider-neutral error categories.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// Client authentication failed or is missing.
    Authentication,
    /// The authenticated actor lacks permission.
    Authorization,
    /// Input or configuration is invalid.
    Validation,
    /// State or protocol versions conflict.
    Conflict,
    /// A durable human approval is required.
    ApprovalRequired,
    /// A requested capability is not currently available.
    UnavailableCapability,
    /// The requested operation is unsupported by this build or platform.
    Unsupported,
    /// A bounded quota or rate limit was reached.
    RateLimited,
    /// Work exceeded its deadline.
    Timeout,
    /// Work was cancelled.
    Cancelled,
    /// An upstream failure may succeed on a bounded retry.
    TransientUpstream,
    /// An upstream failure will not succeed without a change.
    PermanentUpstream,
    /// An external effect may have happened and must not be blindly retried.
    AmbiguousEffect,
    /// An unexpected internal invariant or adapter failure occurred.
    Internal,
}

impl ErrorCode {
    /// Returns the stable snake-case wire/log code.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Authentication => "authentication",
            Self::Authorization => "authorization",
            Self::Validation => "validation",
            Self::Conflict => "conflict",
            Self::ApprovalRequired => "approval_required",
            Self::UnavailableCapability => "unavailable_capability",
            Self::Unsupported => "unsupported",
            Self::RateLimited => "rate_limited",
            Self::Timeout => "timeout",
            Self::Cancelled => "cancelled",
            Self::TransientUpstream => "transient_upstream",
            Self::PermanentUpstream => "permanent_upstream",
            Self::AmbiguousEffect => "ambiguous_effect",
            Self::Internal => "internal",
        }
    }

    /// Returns whether a caller may attempt a bounded retry by default.
    #[must_use]
    pub const fn is_retryable(self) -> bool {
        matches!(
            self,
            Self::UnavailableCapability
                | Self::RateLimited
                | Self::Timeout
                | Self::TransientUpstream
        )
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Explains why a candidate safe message was rejected.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum UnsafeMessageReason {
    /// The message was empty.
    #[error("message is empty")]
    Empty,
    /// The message exceeded the bounded diagnostic length.
    #[error("message exceeds 512 bytes")]
    TooLong,
    /// The message contained a control character that can forge log lines.
    #[error("message contains control characters")]
    ControlCharacter,
}

/// Indicates that text was unsuitable for a client-visible or normal-log message.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[error("unsafe diagnostic message: {reason}")]
pub struct UnsafeMessage {
    reason: UnsafeMessageReason,
}

impl UnsafeMessage {
    /// Returns the message validation failure.
    #[must_use]
    pub const fn reason(&self) -> UnsafeMessageReason {
        self.reason
    }
}

/// A bounded single-line message suitable for normal diagnostics.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct SafeMessage(String);

impl SafeMessage {
    /// Validates and wraps diagnostic text.
    ///
    /// # Errors
    ///
    /// Returns [`UnsafeMessage`] when the text is empty, longer than 512 bytes,
    /// or contains a control character.
    pub fn new(value: impl Into<String>) -> Result<Self, UnsafeMessage> {
        let value = value.into();
        let reason = if value.is_empty() {
            Some(UnsafeMessageReason::Empty)
        } else if value.len() > 512 {
            Some(UnsafeMessageReason::TooLong)
        } else if value.chars().any(char::is_control) {
            Some(UnsafeMessageReason::ControlCharacter)
        } else {
            None
        };

        match reason {
            Some(reason) => Err(UnsafeMessage { reason }),
            None => Ok(Self(value)),
        }
    }

    /// Borrows the validated text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SafeMessage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Default for SafeMessage {
    /// Returns a valid placeholder, so callers never need a panic to recover
    /// from a rejected message.
    fn default() -> Self {
        Self("unspecified".to_owned())
    }
}

impl<'de> Deserialize<'de> for SafeMessage {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

/// A stable domain failure with a safe message and optional correlation identity.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[error("{code}: {message}")]
pub struct DomainError {
    code: ErrorCode,
    message: SafeMessage,
    correlation_id: Option<CorrelationId>,
}

impl DomainError {
    /// Creates a domain error from validated diagnostic text.
    #[must_use]
    pub const fn new(code: ErrorCode, message: SafeMessage) -> Self {
        Self {
            code,
            message,
            correlation_id: None,
        }
    }

    /// Attaches a correlation identity without changing the safe message.
    #[must_use]
    pub const fn with_correlation_id(mut self, correlation_id: CorrelationId) -> Self {
        self.correlation_id = Some(correlation_id);
        self
    }

    /// Returns the stable error category.
    #[must_use]
    pub const fn code(&self) -> ErrorCode {
        self.code
    }

    /// Returns the safe diagnostic message.
    #[must_use]
    pub const fn message(&self) -> &SafeMessage {
        &self.message
    }

    /// Returns the correlation identity when one is available.
    #[must_use]
    pub const fn correlation_id(&self) -> Option<CorrelationId> {
        self.correlation_id
    }

    /// Returns the default retryability for this error category.
    #[must_use]
    pub const fn is_retryable(&self) -> bool {
        self.code.is_retryable()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retryability_is_deterministic_and_ambiguous_effects_fail_closed() {
        assert!(ErrorCode::TransientUpstream.is_retryable());
        assert!(ErrorCode::RateLimited.is_retryable());
        assert!(!ErrorCode::Validation.is_retryable());
        assert!(!ErrorCode::AmbiguousEffect.is_retryable());
    }

    #[test]
    fn safe_messages_reject_log_forging_and_unbounded_text() {
        let newline = SafeMessage::new("safe\nforged");
        assert_eq!(
            newline.map_err(|error| error.reason()),
            Err(UnsafeMessageReason::ControlCharacter)
        );

        let oversized = SafeMessage::new("x".repeat(513));
        assert_eq!(
            oversized.map_err(|error| error.reason()),
            Err(UnsafeMessageReason::TooLong)
        );
    }
}
