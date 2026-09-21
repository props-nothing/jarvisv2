use std::fmt;

use jarvis_core::ErrorCode;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::identity::{IdentityError, MAX_IDENTIFIER_BYTES};

/// An opaque provider-assigned request identifier.
///
/// Providers let you supply your own correlation value and return their own
/// identifier. JARVIS stores the provider's value only as an external reference
/// for support escalation; it is never used as a JARVIS identifier.
///
/// Debug output deliberately shows the length rather than the value, because this
/// type travels inside diagnostics that may be attached to a bug report.
#[derive(Clone, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderRequestId(String);

impl ProviderRequestId {
    /// Creates a provider request identifier.
    ///
    /// # Errors
    ///
    /// Returns [`IdentityError`] when the value is empty, oversized, or contains
    /// characters that could forge a log line.
    pub fn new(value: impl Into<String>) -> Result<Self, IdentityError> {
        let value = value.into();
        if value.is_empty() {
            return Err(IdentityError::Empty);
        }
        if value.len() > MAX_IDENTIFIER_BYTES {
            return Err(IdentityError::TooLong);
        }
        if value.chars().any(char::is_control) {
            return Err(IdentityError::InvalidCharacter);
        }
        Ok(Self(value))
    }

    /// Borrows the opaque identifier text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ProviderRequestId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "ProviderRequestId({} bytes)", self.0.len())
    }
}

/// The normalized category of a model failure.
///
/// Each variant maps onto exactly one [`ErrorCode`], so retry and surfacing policy
/// is decided by a single stable category rather than by provider-specific text.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelErrorKind {
    /// The request was malformed or failed provider validation.
    InvalidRequest,
    /// The credential is missing, revoked, or malformed.
    Authentication,
    /// The credential is valid but lacks access to the model or resource.
    Authorization,
    /// The provider refused the model's output on safety grounds.
    ContentRefusal,
    /// A bounded rate limit was reached; the provider may supply a retry hint.
    RateLimited,
    /// The model is temporarily unavailable or overloaded.
    Overloaded,
    /// A provider-side failure that may succeed on a bounded retry.
    Transient,
    /// Prepaid credit, spend, or usage limits block access until they are changed.
    QuotaExhausted,
    /// The request exceeded the model's context window.
    ContextOverflow,
    /// The configured model does not exist or is not served here.
    ModelNotFound,
    /// The work exceeded the client-side deadline.
    Timeout,
    /// The caller's cancellation token fired.
    Cancelled,
    /// The stream ended before a terminal event.
    Incomplete,
    /// A 2xx response could not be interpreted.
    MalformedResponse,
    /// An unexpected adapter invariant failed.
    Internal,
}

impl ModelErrorKind {
    /// Returns the stable snake-case wire/log code.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::Authentication => "authentication",
            Self::Authorization => "authorization",
            Self::ContentRefusal => "content_refusal",
            Self::RateLimited => "rate_limited",
            Self::Overloaded => "overloaded",
            Self::Transient => "transient",
            Self::QuotaExhausted => "quota_exhausted",
            Self::ContextOverflow => "context_overflow",
            Self::ModelNotFound => "model_not_found",
            Self::Timeout => "timeout",
            Self::Cancelled => "cancelled",
            Self::Incomplete => "incomplete",
            Self::MalformedResponse => "malformed_response",
            Self::Internal => "internal",
        }
    }

    /// Returns the domain error category surfaced to the rest of JARVIS.
    ///
    /// `QuotaExhausted` maps to [`ErrorCode::PermanentUpstream`] rather than
    /// [`ErrorCode::UnavailableCapability`]. Official `OpenAI` documentation states
    /// that retrying billing, spend, or quota errors does not restore access and
    /// that the limits must be updated first. Because `UnavailableCapability` is a
    /// **retryable** category in `jarvis-core`, routing quota through it would make
    /// the default policy retry against a wall.
    #[must_use]
    pub const fn domain_code(self) -> ErrorCode {
        match self {
            Self::InvalidRequest | Self::ContextOverflow => ErrorCode::Validation,
            Self::Authentication => ErrorCode::Authentication,
            Self::Authorization => ErrorCode::Authorization,
            Self::ContentRefusal => ErrorCode::Unsupported,
            Self::RateLimited | Self::Overloaded => ErrorCode::RateLimited,
            Self::Transient => ErrorCode::TransientUpstream,
            Self::QuotaExhausted => ErrorCode::PermanentUpstream,
            Self::ModelNotFound => ErrorCode::UnavailableCapability,
            Self::Timeout => ErrorCode::Timeout,
            Self::Cancelled => ErrorCode::Cancelled,
            Self::Incomplete => ErrorCode::AmbiguousEffect,
            Self::MalformedResponse | Self::Internal => ErrorCode::Internal,
        }
    }

    /// Returns whether a bounded caller-side retry may help.
    ///
    /// This is computed from [`Self::domain_code`], so the retry decision, the
    /// persisted error code, and the client-visible code cannot disagree.
    #[must_use]
    pub const fn is_retryable(self) -> bool {
        self.domain_code().is_retryable()
    }
}

impl fmt::Display for ModelErrorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A normalized model failure.
///
/// The message is a fixed, non-secret explanation owned by JARVIS; a provider's own
/// message, which may echo request content, is never carried here.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[error("{kind}: {message}")]
pub struct ModelError {
    kind: ModelErrorKind,
    message: jarvis_core::SafeMessage,
    retry_after_seconds: Option<u64>,
    provider_request_id: Option<ProviderRequestId>,
    provider_status: Option<u16>,
}

impl ModelError {
    /// Creates a normalized failure with a bounded, non-secret explanation.
    #[must_use]
    pub const fn new(kind: ModelErrorKind, message: jarvis_core::SafeMessage) -> Self {
        Self {
            kind,
            message,
            retry_after_seconds: None,
            provider_request_id: None,
            provider_status: None,
        }
    }

    /// Creates a normalized failure from static text known not to contain a secret.
    ///
    /// # Panics
    ///
    /// Panics when the text is empty, longer than 512 bytes, or contains a control
    /// character, which would forge a log line. Callers pass constants only.
    #[must_use]
    pub fn from_static(kind: ModelErrorKind, message: &'static str) -> Self {
        let message = jarvis_core::SafeMessage::new(message)
            .unwrap_or_else(|error| panic!("static model error text must be safe: {error}"));
        Self::new(kind, message)
    }

    /// Attaches the provider's requested retry delay.
    ///
    /// Only meaningful for retryable kinds; a provider hint on a permanent failure
    /// is preserved for diagnostics but never acted on.
    #[must_use]
    pub const fn with_retry_after_seconds(mut self, seconds: Option<u64>) -> Self {
        self.retry_after_seconds = seconds;
        self
    }

    /// Attaches the provider-assigned request identifier for support escalation.
    #[must_use]
    pub fn with_provider_request_id(mut self, id: Option<ProviderRequestId>) -> Self {
        self.provider_request_id = id;
        self
    }

    /// Attaches the provider's HTTP status for diagnostics.
    #[must_use]
    pub const fn with_provider_status(mut self, status: Option<u16>) -> Self {
        self.provider_status = status;
        self
    }

    /// Returns the normalized failure category.
    #[must_use]
    pub const fn kind(&self) -> ModelErrorKind {
        self.kind
    }

    /// Returns the bounded, non-secret explanation.
    #[must_use]
    pub const fn message(&self) -> &jarvis_core::SafeMessage {
        &self.message
    }

    /// Returns the domain error category for persistence and client surfacing.
    #[must_use]
    pub const fn domain_code(&self) -> ErrorCode {
        self.kind.domain_code()
    }

    /// Returns whether a bounded retry may help.
    #[must_use]
    pub const fn is_retryable(&self) -> bool {
        self.kind.is_retryable()
    }

    /// Returns the provider's requested retry delay, when the provider supplied one.
    ///
    /// A present value is authoritative and must override a computed backoff, per
    /// official guidance to follow `Retry-After` when it appears.
    #[must_use]
    pub const fn retry_after_seconds(&self) -> Option<u64> {
        self.retry_after_seconds
    }

    /// Returns the provider-assigned request identifier, when captured.
    #[must_use]
    pub const fn provider_request_id(&self) -> Option<&ProviderRequestId> {
        self.provider_request_id.as_ref()
    }

    /// Returns the provider HTTP status, when the failure came from a response.
    #[must_use]
    pub const fn provider_status(&self) -> Option<u16> {
        self.provider_status
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quota_is_not_reported_as_retryable() {
        // Official documentation: retrying billing, spend, or quota errors does not
        // restore API access. If this regresses, the default policy starts a retry
        // loop that cannot succeed.
        let quota = ModelError::from_static(ModelErrorKind::QuotaExhausted, "provider quota limit");
        assert!(!quota.is_retryable());
        assert_eq!(quota.domain_code(), ErrorCode::PermanentUpstream);

        // The retryable categories must still be retryable, or the fail-closed fix
        // would have disabled legitimate retries.
        for kind in [
            ModelErrorKind::RateLimited,
            ModelErrorKind::Overloaded,
            ModelErrorKind::Transient,
        ] {
            let error = ModelError::from_static(kind, "temporary provider failure");
            assert!(error.is_retryable(), "{kind} must remain retryable");
        }
    }

    #[test]
    fn a_truncated_stream_is_not_silently_successful() {
        // A partial answer must never be reported as a completed answer.
        let error = ModelError::from_static(ModelErrorKind::Incomplete, "stream ended early");
        assert_eq!(error.domain_code(), ErrorCode::AmbiguousEffect);
        assert!(!error.is_retryable());
    }

    #[test]
    fn every_kind_displays_its_stable_code() {
        for kind in [
            ModelErrorKind::InvalidRequest,
            ModelErrorKind::Authentication,
            ModelErrorKind::Authorization,
            ModelErrorKind::ContentRefusal,
            ModelErrorKind::RateLimited,
            ModelErrorKind::Overloaded,
            ModelErrorKind::Transient,
            ModelErrorKind::QuotaExhausted,
            ModelErrorKind::ContextOverflow,
            ModelErrorKind::ModelNotFound,
            ModelErrorKind::Timeout,
            ModelErrorKind::Cancelled,
            ModelErrorKind::Incomplete,
            ModelErrorKind::MalformedResponse,
            ModelErrorKind::Internal,
        ] {
            assert_eq!(kind.to_string(), kind.as_str());
        }
    }

    #[test]
    fn provider_request_ids_do_not_leak_their_value_into_debug_output() {
        let id = ProviderRequestId::new("req_abc123secret")
            .unwrap_or_else(|error| panic!("valid fixture identifier: {error}"));
        let rendered = format!("{id:?}");
        assert!(
            !rendered.contains("abc123secret"),
            "provider request IDs are opaque and must not be printed verbatim"
        );
    }

    #[test]
    fn provider_request_ids_reject_line_forging_text() {
        assert!(ProviderRequestId::new("req\nmalformed").is_err());
        assert!(ProviderRequestId::new("").is_err());
    }
}
