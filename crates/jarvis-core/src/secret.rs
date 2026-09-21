use std::fmt;

use thiserror::Error;

/// Indicates that secret-reference metadata is empty, oversized, or structurally unsafe.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum SecretRefValidationError {
    /// A required component was empty.
    #[error("secret reference components cannot be empty")]
    Empty,
    /// A component exceeded its documented bound.
    #[error("secret reference component is too long")]
    TooLong,
    /// A component contained whitespace, controls, or unsupported punctuation.
    #[error("secret reference component contains unsupported characters")]
    InvalidCharacter,
}

/// Metadata that locates a secret without containing the secret value.
///
/// Generic serialization is intentionally unavailable because it could expose
/// the locator through diagnostics or an unrelated wire contract. Storage
/// adapters must map the fields explicitly.
///
/// ```compile_fail
/// use jarvis_core::SecretRef;
/// let secret_ref = SecretRef::new("os-keychain", "item-id", "local-ipc")?;
/// let _json = serde_json::to_string(&secret_ref)?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct SecretRef {
    provider: String,
    locator: String,
    purpose: String,
}

impl SecretRef {
    /// Creates a validated secret reference.
    ///
    /// # Errors
    ///
    /// Returns [`SecretRefValidationError`] when a component is empty, exceeds
    /// its bound, or contains whitespace, control characters, or unsupported punctuation.
    pub fn new(
        provider: impl Into<String>,
        locator: impl Into<String>,
        purpose: impl Into<String>,
    ) -> Result<Self, SecretRefValidationError> {
        let provider = provider.into();
        let locator = locator.into();
        let purpose = purpose.into();

        validate_component(&provider, 32)?;
        validate_component(&locator, 256)?;
        validate_component(&purpose, 64)?;

        Ok(Self {
            provider,
            locator,
            purpose,
        })
    }

    /// Returns the secret-provider identifier.
    #[must_use]
    pub fn provider(&self) -> &str {
        &self.provider
    }

    /// Returns the non-secret purpose label.
    #[must_use]
    pub fn purpose(&self) -> &str {
        &self.purpose
    }

    /// Explicitly exposes the locator to the final secret-store adapter.
    ///
    /// Callers must not place this value into normal logs, prompts, URLs, or errors.
    #[must_use]
    pub fn expose_locator(&self) -> &str {
        &self.locator
    }
}

impl fmt::Debug for SecretRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SecretRef")
            .field("provider", &self.provider)
            .field("locator", &"[REDACTED]")
            .field("purpose", &self.purpose)
            .finish()
    }
}

impl fmt::Display for SecretRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}://[REDACTED]", self.provider)
    }
}

fn validate_component(value: &str, max_len: usize) -> Result<(), SecretRefValidationError> {
    if value.is_empty() {
        return Err(SecretRefValidationError::Empty);
    }
    if value.len() > max_len {
        return Err(SecretRefValidationError::TooLong);
    }
    if value
        .chars()
        .any(|character| !(character.is_ascii_alphanumeric() || "._:/-".contains(character)))
    {
        return Err(SecretRefValidationError::InvalidCharacter);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_formatting_redacts_the_locator() {
        let canary = "canary-secret-reference-123";
        let secret_ref = SecretRef::new("os-keychain", canary, "local-ipc")
            .unwrap_or_else(|error| panic!("valid secret reference fixture: {error}"));

        let debug = format!("{secret_ref:?}");
        let display = secret_ref.to_string();
        assert!(!debug.contains(canary));
        assert!(!display.contains(canary));
        assert_eq!(secret_ref.expose_locator(), canary);
    }

    #[test]
    fn unsafe_reference_components_fail_closed() {
        assert_eq!(
            SecretRef::new("os keychain", "item", "purpose"),
            Err(SecretRefValidationError::InvalidCharacter)
        );
    }
}
