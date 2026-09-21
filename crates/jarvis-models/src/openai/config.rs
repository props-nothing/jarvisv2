//! Provider credential and base-URL handling.
//!
//! Both types validate on construction, because the two mistakes this project has
//! already had to fix are "a URL was pasted where a key was expected" and "a key was
//! placed in a URL". A key in a URL is a substring of every log line that mentions
//! the endpoint, so it is rejected structurally rather than by convention.

use std::{fmt, str};

use thiserror::Error;

/// Maximum byte length accepted for an API key.
pub const MAX_API_KEY_BYTES: usize = 512;

/// Maximum byte length accepted for a base URL.
pub const MAX_BASE_URL_BYTES: usize = 2048;

/// Explains why an API key was rejected.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ApiKeyError {
    /// The value was empty or whitespace only.
    #[error("api key is empty")]
    Empty,
    /// The value exceeded [`MAX_API_KEY_BYTES`].
    #[error("api key is too long")]
    TooLong,
    /// The value contained a control character that could forge a header or log line.
    #[error("api key contains a control character")]
    ControlCharacter,
    /// The value contained whitespace, which no provider issues inside a key.
    #[error("api key contains whitespace")]
    Whitespace,
    /// The value looked like a URL rather than a key.
    #[error("api key looks like a URL; supply only the key value")]
    LooksLikeUrl,
    /// The value still carried the `Bearer` scheme from a copied header.
    #[error("api key includes the Bearer scheme; supply only the key value")]
    IncludesScheme,
}

/// A provider credential held in memory and never rendered.
///
/// The bytes are zeroed on drop, `Debug` and `Display` are redacted, and the value
/// is reachable only through [`Self::header_value`]. That accessor's name states
/// where the secret is going, so a reviewer can see that a credential reached a
/// header and not a log line.
#[derive(Clone)]
pub struct ApiKey {
    /// Validated bytes rather than a `String`, so the buffer can be zeroed on drop.
    value: Vec<u8>,
}

impl ApiKey {
    /// Validates and wraps a provider credential.
    ///
    /// # Errors
    ///
    /// Returns [`ApiKeyError`] when the value is empty, oversized, contains
    /// whitespace or a control character, or is visibly a URL or a copied header.
    pub fn new(value: impl Into<String>) -> Result<Self, ApiKeyError> {
        let value = value.into();
        let trimmed = value.trim();

        if trimmed.is_empty() {
            return Err(ApiKeyError::Empty);
        }
        if trimmed.len() > MAX_API_KEY_BYTES {
            return Err(ApiKeyError::TooLong);
        }
        if trimmed.chars().any(char::is_control) {
            return Err(ApiKeyError::ControlCharacter);
        }
        if trimmed.contains("://") {
            return Err(ApiKeyError::LooksLikeUrl);
        }
        // Checked before whitespace so the diagnosis names the actual mistake. A
        // copied `Bearer ...` header contains a space, and reporting "whitespace"
        // would send the reader looking for an invisible character instead of telling
        // them to remove the scheme prefix.
        if trimmed.len() > 7 && trimmed[..7].eq_ignore_ascii_case("bearer ") {
            return Err(ApiKeyError::IncludesScheme);
        }
        if trimmed.chars().any(char::is_whitespace) {
            // Providers never issue a key containing whitespace, and a pasted
            // multi-word value is a sign the wrong field was copied.
            return Err(ApiKeyError::Whitespace);
        }

        Ok(Self {
            value: trimmed.as_bytes().to_vec(),
        })
    }

    /// Returns the `Authorization` header value.
    ///
    /// The validated value is non-empty with no control characters and no
    /// whitespace, so it cannot split or extend the header it is placed in.
    #[must_use]
    pub fn header_value(&self) -> String {
        let text = str::from_utf8(&self.value).unwrap_or_default();
        format!("Bearer {text}")
    }

    /// Returns the byte length, which is safe to record for diagnostics.
    #[must_use]
    pub fn len(&self) -> usize {
        self.value.len()
    }

    /// Returns whether the credential is zero length.
    ///
    /// Always false for a constructed value; present because a length accessor
    /// without one is a lint finding.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.value.is_empty()
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "ApiKey({} bytes, [REDACTED])", self.value.len())
    }
}

impl fmt::Display for ApiKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("provider api key [REDACTED]")
    }
}

impl Drop for ApiKey {
    fn drop(&mut self) {
        // Clear the buffer while it is still owned here. This does not defend
        // against a process-wide memory dump; it removes the value from this
        // allocation as soon as the credential is no longer needed.
        self.value.fill(0);
    }
}

/// Explains why a base URL was rejected.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum BaseUrlError {
    /// The value was empty.
    #[error("base url is empty")]
    Empty,
    /// The value exceeded [`MAX_BASE_URL_BYTES`].
    #[error("base url is too long")]
    TooLong,
    /// The scheme was missing or was not `http` or `https`.
    #[error("base url must be an absolute http or https URL")]
    UnsupportedScheme,
    /// The value contained whitespace or a control character.
    #[error("base url contains whitespace or a control character")]
    InvalidCharacter,
    /// The value embedded credentials in its userinfo section.
    #[error("base url must not embed credentials")]
    EmbeddedCredentials,
}

/// A validated provider base URL.
///
/// The URL never contains the credential: authentication is presented as a header.
/// [`Self::join`] therefore builds a path on a value that provably has no key in it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BaseUrl {
    value: String,
}

impl BaseUrl {
    /// Validates and normalizes a base URL.
    ///
    /// # Errors
    ///
    /// Returns [`BaseUrlError`] when the value is empty, oversized, not an absolute
    /// `http` or `https` URL, contains whitespace or a control character, or embeds
    /// credentials in its userinfo.
    pub fn new(value: impl Into<String>) -> Result<Self, BaseUrlError> {
        let value = value.into();
        let trimmed = value.trim();

        if trimmed.is_empty() {
            return Err(BaseUrlError::Empty);
        }
        if trimmed.len() > MAX_BASE_URL_BYTES {
            return Err(BaseUrlError::TooLong);
        }
        if trimmed
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
        {
            return Err(BaseUrlError::InvalidCharacter);
        }

        let Some((scheme, rest)) = trimmed.split_once("://") else {
            return Err(BaseUrlError::UnsupportedScheme);
        };
        if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
            return Err(BaseUrlError::UnsupportedScheme);
        }
        if rest.is_empty() {
            return Err(BaseUrlError::UnsupportedScheme);
        }
        // Userinfo is everything before the first `/`; a `@` there means the URL
        // carries a username or password, which would leak into every log line.
        let authority = rest.split('/').next().unwrap_or_default();
        if authority.contains('@') {
            return Err(BaseUrlError::EmbeddedCredentials);
        }

        Ok(Self {
            value: trimmed.trim_end_matches('/').to_owned(),
        })
    }

    /// Returns the normalized base URL.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.value
    }

    /// Appends an absolute path to the base URL.
    #[must_use]
    pub fn join(&self, path: &str) -> String {
        format!("{}{path}", self.value)
    }

    /// Returns whether the authority is a loopback address.
    ///
    /// Used only to infer placement. A non-loopback authority is reported as remote
    /// even when it might tunnel to this machine, because reporting remote when the
    /// prompt stays local is the safe direction.
    #[must_use]
    pub fn is_loopback(&self) -> bool {
        let Some((_, rest)) = self.value.split_once("://") else {
            return false;
        };
        let authority = rest.split('/').next().unwrap_or_default();
        // Strip a port, taking care not to mangle a bracketed IPv6 literal.
        let host = if let Some(closing) = authority.find(']') {
            &authority[..=closing]
        } else {
            authority.split(':').next().unwrap_or_default()
        };
        matches!(host, "127.0.0.1" | "localhost" | "[::1]")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pasted_url_is_rejected_as_an_api_key() {
        // This mistake has occurred in this project before, and it surfaces at the
        // provider as a generic auth error, which sends you to debug the wrong thing.
        for value in [
            "https://api.example.com/v1/chat/completions",
            "http://127.0.0.1:11434/v1",
            "sk-abc://not-a-key",
        ] {
            assert_eq!(
                ApiKey::new(value).err(),
                Some(ApiKeyError::LooksLikeUrl),
                "{value} must be rejected"
            );
        }
    }

    #[test]
    fn a_copied_authorization_header_is_rejected() {
        assert_eq!(
            ApiKey::new("Bearer sk-abc123").err(),
            Some(ApiKeyError::IncludesScheme)
        );
    }

    #[test]
    fn a_key_with_whitespace_or_control_characters_is_rejected() {
        assert_eq!(ApiKey::new("sk abc").err(), Some(ApiKeyError::Whitespace));
        assert_eq!(
            ApiKey::new("sk-abc\nX-Injected: 1").err(),
            Some(ApiKeyError::ControlCharacter),
            "a newline could inject a header"
        );
        assert_eq!(ApiKey::new("   ").err(), Some(ApiKeyError::Empty));
    }

    #[test]
    fn the_key_value_never_appears_in_debug_or_display() {
        let key = ApiKey::new("sk-live-abcdef123456")
            .unwrap_or_else(|error| panic!("valid fixture key: {error}"));

        let debug = format!("{key:?}");
        let display = format!("{key}");
        assert!(!debug.contains("sk-live-abcdef123456"), "{debug}");
        assert!(!display.contains("sk-live-abcdef123456"), "{display}");
        assert_eq!(key.len(), "sk-live-abcdef123456".len());
        assert!(!key.is_empty());

        // The header value is the only place the value appears.
        assert_eq!(key.header_value(), "Bearer sk-live-abcdef123456");
    }

    #[test]
    fn a_key_containing_an_underscore_or_dash_is_accepted() {
        // Validation must not be so strict that it rejects real keys.
        for value in ["sk-proj_abc-123", "ollama", "abcDEF123_-"] {
            assert!(
                ApiKey::new(value).is_ok(),
                "{value} is a plausible key and must be accepted"
            );
        }
    }

    #[test]
    fn a_base_url_embedding_credentials_is_rejected() {
        assert_eq!(
            BaseUrl::new("https://user:secret@api.example.com/v1").err(),
            Some(BaseUrlError::EmbeddedCredentials),
            "userinfo would leak into every log line mentioning the endpoint"
        );
    }

    #[test]
    fn only_absolute_http_urls_are_accepted() {
        assert_eq!(
            BaseUrl::new("api.example.com/v1").err(),
            Some(BaseUrlError::UnsupportedScheme)
        );
        assert_eq!(
            BaseUrl::new("ftp://api.example.com").err(),
            Some(BaseUrlError::UnsupportedScheme)
        );
        assert_eq!(
            BaseUrl::new("file:///etc/passwd").err(),
            Some(BaseUrlError::UnsupportedScheme)
        );
        assert!(BaseUrl::new("http://127.0.0.1:11434/v1").is_ok());
    }

    #[test]
    fn a_trailing_slash_does_not_produce_a_doubled_path() {
        let base = BaseUrl::new("http://127.0.0.1:11434/v1/")
            .unwrap_or_else(|error| panic!("valid fixture base url: {error}"));
        assert_eq!(
            base.join("/chat/completions"),
            "http://127.0.0.1:11434/v1/chat/completions"
        );
    }

    #[test]
    fn loopback_is_detected_but_a_remote_host_is_not_assumed_local() {
        for value in [
            "http://127.0.0.1:11434/v1",
            "http://localhost:11434/v1",
            "http://[::1]:11434/v1",
        ] {
            let base = BaseUrl::new(value).unwrap_or_else(|error| panic!("valid: {error}"));
            assert!(base.is_loopback(), "{value} must be recognized as loopback");
        }

        let remote = BaseUrl::new("https://api.openai.com/v1")
            .unwrap_or_else(|error| panic!("valid: {error}"));
        assert!(!remote.is_loopback());
    }

    #[test]
    fn a_remote_host_that_merely_begins_with_loopback_text_is_not_local() {
        // `127.0.0.1.evil.example` starts with the loopback string but is remote.
        // Treating it as local would permit private content to leave the machine.
        let base = BaseUrl::new("https://127.0.0.1.evil.example/v1")
            .unwrap_or_else(|error| panic!("valid: {error}"));
        assert!(!base.is_loopback());
    }

    #[test]
    fn a_port_does_not_defeat_loopback_detection() {
        let base =
            BaseUrl::new("http://127.0.0.1:8080").unwrap_or_else(|error| panic!("valid: {error}"));
        assert!(base.is_loopback());
    }
}
