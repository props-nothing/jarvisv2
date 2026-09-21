use std::{fmt, str};

use thiserror::Error;

/// Entropy bytes in a generated client credential.
pub const CREDENTIAL_BYTES: usize = 32;
/// Encoded credential length in ASCII hexadecimal characters.
pub const CREDENTIAL_CHARS: usize = CREDENTIAL_BYTES * 2;

const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

/// Explains why a client credential was rejected.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum CredentialError {
    /// The text was not the fixed encoded credential length.
    #[error("client credential must be exactly {CREDENTIAL_CHARS} hexadecimal characters")]
    InvalidLength,
    /// The text contained a non-hexadecimal character.
    #[error("client credential contains a non-hexadecimal character")]
    InvalidEncoding,
    /// The operating-system random source was unavailable.
    #[error("the operating-system random source is unavailable")]
    RandomUnavailable,
}

/// A profile-bound secret shared between the daemon and its local clients.
///
/// The value is generated once per profile, stored through an OS-protected
/// path, presented only inside the local handshake, and never included in
/// diagnostics. The buffer is zeroed when the value is dropped.
#[derive(Clone)]
pub struct ClientCredential {
    encoded: Vec<u8>,
}

impl ClientCredential {
    /// Generates a new random credential from the operating-system source.
    ///
    /// # Errors
    ///
    /// Returns [`CredentialError::RandomUnavailable`] when the OS random source
    /// fails, so an unauthenticated daemon is never started by accident.
    pub fn generate() -> Result<Self, CredentialError> {
        let mut bytes = [0_u8; CREDENTIAL_BYTES];
        getrandom::fill(&mut bytes).map_err(|_| CredentialError::RandomUnavailable)?;
        Ok(Self {
            encoded: encode_hex(&bytes).into_bytes(),
        })
    }

    /// Parses a credential from hexadecimal text, normalizing case and trimming.
    ///
    /// # Errors
    ///
    /// Returns [`CredentialError::InvalidLength`] or
    /// [`CredentialError::InvalidEncoding`] when the text cannot be a credential.
    pub fn parse(text: &str) -> Result<Self, CredentialError> {
        let text = text.trim();
        if text.len() != CREDENTIAL_CHARS {
            return Err(CredentialError::InvalidLength);
        }
        if !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(CredentialError::InvalidEncoding);
        }
        Ok(Self {
            encoded: text.to_ascii_lowercase().into_bytes(),
        })
    }

    /// Borrows the credential to place it inside the local protocol handshake.
    ///
    /// Callers must not write this value to logs, errors, or durable files.
    #[must_use]
    pub fn expose(&self) -> &str {
        // The buffer only ever holds validated ASCII hexadecimal bytes.
        str::from_utf8(&self.encoded).unwrap_or_default()
    }

    /// Compares a presented value in time that does not depend on its content.
    #[must_use]
    pub fn matches(&self, presented: &str) -> bool {
        constant_time_eq(&self.encoded, presented.as_bytes())
    }
}

impl PartialEq for ClientCredential {
    fn eq(&self, other: &Self) -> bool {
        constant_time_eq(&self.encoded, &other.encoded)
    }
}

impl Eq for ClientCredential {}

impl fmt::Debug for ClientCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ClientCredential([REDACTED])")
    }
}

impl fmt::Display for ClientCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("local client credential [REDACTED]")
    }
}

impl Drop for ClientCredential {
    fn drop(&mut self) {
        self.encoded.fill(0);
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(CREDENTIAL_CHARS);
    for byte in bytes {
        text.push(char::from(HEX_DIGITS[usize::from(byte >> 4)]));
        text.push(char::from(HEX_DIGITS[usize::from(byte & 0x0f)]));
    }
    text
}

/// Compares two byte strings without an early exit that reveals the first mismatch.
fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut difference = left.len() ^ right.len();
    let width = left.len().max(right.len());
    for index in 0..width {
        let left_byte = left.get(index).copied().unwrap_or_default();
        let right_byte = right.get(index).copied().unwrap_or_default();
        difference |= usize::from(left_byte ^ right_byte);
    }
    difference == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_credentials_are_hex_and_unique() {
        let first =
            ClientCredential::generate().unwrap_or_else(|error| panic!("generate: {error}"));
        let second =
            ClientCredential::generate().unwrap_or_else(|error| panic!("generate: {error}"));

        assert_eq!(first.expose().len(), CREDENTIAL_CHARS);
        assert!(first.expose().bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_ne!(first.expose(), second.expose());
        assert!(!first.matches(second.expose()));
    }

    #[test]
    fn parsing_normalizes_case_and_rejects_unsafe_text() {
        let generated =
            ClientCredential::generate().unwrap_or_else(|error| panic!("generate: {error}"));
        let uppercase = generated.expose().to_ascii_uppercase();
        let parsed =
            ClientCredential::parse(&uppercase).unwrap_or_else(|error| panic!("parse: {error}"));

        assert_eq!(parsed.expose(), generated.expose());
        assert!(parsed.matches(generated.expose()));

        assert_eq!(
            ClientCredential::parse("short"),
            Err(CredentialError::InvalidLength)
        );
        assert_eq!(
            ClientCredential::parse(&"z".repeat(CREDENTIAL_CHARS)),
            Err(CredentialError::InvalidEncoding)
        );
    }

    #[test]
    fn credential_comparison_rejects_length_and_content_mismatches() {
        let credential = ClientCredential::parse(&"ab".repeat(CREDENTIAL_BYTES))
            .unwrap_or_else(|error| panic!("parse: {error}"));

        assert!(credential.matches(&"ab".repeat(CREDENTIAL_BYTES)));
        assert!(!credential.matches(&"ac".repeat(CREDENTIAL_BYTES)));
        assert!(!credential.matches("ab"));
        assert!(!credential.matches(""));
    }

    #[test]
    fn ordinary_formatting_never_reveals_the_value() {
        let credential =
            ClientCredential::generate().unwrap_or_else(|error| panic!("generate: {error}"));
        let secret = credential.expose().to_owned();

        let debug = format!("{credential:?}");
        let display = format!("{credential}");

        assert_eq!(debug, "ClientCredential([REDACTED])");
        assert!(!display.contains(&secret));
        assert!(!debug.contains(&secret));
        assert!(format!("{credential:?}").contains("REDACTED"));
    }
}
