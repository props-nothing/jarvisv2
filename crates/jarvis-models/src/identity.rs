use std::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use thiserror::Error;

/// Maximum byte length of a provider or model identifier.
///
/// Providers accept longer opaque model names than a human would type, but an
/// unbounded identifier would reach logs, URLs, and database columns unchecked.
pub const MAX_IDENTIFIER_BYTES: usize = 128;

/// Explains why a provider or model identifier was rejected.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum IdentityError {
    /// The identifier was empty.
    #[error("model identifier is empty")]
    Empty,
    /// The identifier exceeded [`MAX_IDENTIFIER_BYTES`].
    #[error("model identifier exceeds 128 bytes")]
    TooLong,
    /// The identifier contained whitespace, control characters, or unsupported punctuation.
    #[error("model identifier contains unsupported characters")]
    InvalidCharacter,
}

/// Validates a provider or model identifier.
///
/// The allowed set is deliberately narrow: letters, digits, `.`, `_`, `-`, `:`,
/// `/`, and `@`. A model name is copied into diagnostics, so a value containing
/// whitespace or control characters could forge a log line, and a value containing
/// `?` or `#` could change the meaning of a URL path.
fn validate(value: &str) -> Result<(), IdentityError> {
    if value.is_empty() {
        return Err(IdentityError::Empty);
    }
    if value.len() > MAX_IDENTIFIER_BYTES {
        return Err(IdentityError::TooLong);
    }
    if !value.bytes().all(|byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':' | b'/' | b'@')
    }) {
        return Err(IdentityError::InvalidCharacter);
    }
    Ok(())
}

macro_rules! identifier {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(String);

        impl $name {
            /// Creates a validated identifier.
            ///
            /// # Errors
            ///
            /// Returns [`IdentityError`] when the value is empty, exceeds
            /// [`MAX_IDENTIFIER_BYTES`], or contains unsupported characters.
            pub fn new(value: impl Into<String>) -> Result<Self, IdentityError> {
                let value = value.into();
                validate(&value)?;
                Ok(Self(value))
            }

            /// Borrows the identifier text.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter
                    .debug_tuple(stringify!($name))
                    .field(&self.0)
                    .finish()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl FromStr for $name {
            type Err = IdentityError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Self::new(value)
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::new(value).map_err(de::Error::custom)
            }
        }
    };
}

identifier!(
    /// Identifies the entity that authenticates and transports model requests.
    ///
    /// A provider is a transport, not a capability. The same provider may expose
    /// many models, and the same model may be reachable through several providers.
    ProviderId
);

identifier!(
    /// Identifies a selectable model capability.
    ///
    /// Model and provider identifiers are intentionally not interchangeable, so a
    /// configuration mistake cannot be compiled.
    ///
    /// ```compile_fail
    /// use jarvis_models::{ModelId, ProviderId};
    /// let model = ModelId::new("gpt-oss:20b")?;
    /// let _provider: ProviderId = model;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ModelId
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_are_treated_as_one_flat_identifier() {
        // A model name legitimately contains `.`, `-`, `:`, `/`, and `@`, which a
        // UUID-style validator would reject. Proving that avoids a later "fix"
        // that breaks real model names.
        let model = ModelId::new("meta-llama/Llama-3.3-70B-Instruct@q4_k_m");
        assert!(model.is_ok(), "real model names must validate: {model:?}");
    }

    #[test]
    fn identifiers_reject_forgeable_or_url_altering_text() {
        assert_eq!(ModelId::new("").err(), Some(IdentityError::Empty));
        assert_eq!(
            ModelId::new("gpt\nmalformed: true").err(),
            Some(IdentityError::InvalidCharacter),
            "a newline in a model name could forge a JSON log line"
        );
        assert_eq!(
            ModelId::new("gpt?stream=true").err(),
            Some(IdentityError::InvalidCharacter),
            "a query delimiter could change the meaning of a URL path"
        );
        assert_eq!(
            ModelId::new("a".repeat(MAX_IDENTIFIER_BYTES + 1)).err(),
            Some(IdentityError::TooLong)
        );
    }

    #[test]
    fn identifiers_round_trip_through_the_wire_form() {
        let provider = ProviderId::new("openai-compatible")
            .unwrap_or_else(|error| panic!("valid fixture identifier: {error}"));
        let json = serde_json::to_string(&provider).unwrap_or_default();
        assert_eq!(json, "\"openai-compatible\"");
        let decoded: Result<ProviderId, _> = serde_json::from_str(&json);
        assert_eq!(decoded.ok().as_ref(), Some(&provider));
    }

    #[test]
    fn deserialization_enforces_the_same_rules_as_construction() {
        // If `Deserialize` bypassed validation, a hostile config file could inject a
        // model name that construction rejects.
        let decoded: Result<ModelId, _> = serde_json::from_str("\"bad model name\"");
        assert!(
            decoded.is_err(),
            "whitespace must not survive deserialization"
        );
    }
}
