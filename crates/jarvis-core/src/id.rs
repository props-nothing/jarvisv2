use std::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use thiserror::Error;
use uuid::Uuid;

/// Supplies raw RFC 9562 `UUIDv7` bytes to domain ID constructors.
pub trait IdGenerator: Send + Sync {
    /// Returns the next `UUIDv7` value as network-order bytes.
    fn next_uuid_v7(&self) -> [u8; 16];
}

/// Generates process-ordered `UUIDv7` values using the operating-system clock and RNG.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemIdGenerator;

impl IdGenerator for SystemIdGenerator {
    fn next_uuid_v7(&self) -> [u8; 16] {
        Uuid::now_v7().into_bytes()
    }
}

/// Explains why an ID value was rejected.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum InvalidIdReason {
    /// The text or bytes are not a syntactically valid UUID.
    #[error("malformed UUID")]
    Malformed,
    /// The UUID is valid but is not version 7.
    #[error("UUID is not version 7")]
    WrongVersion,
}

/// Indicates that a typed ID was not a valid RFC 9562 `UUIDv7`.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[error("{kind} must be a valid UUIDv7: {reason}")]
pub struct InvalidId {
    kind: &'static str,
    reason: InvalidIdReason,
}

impl InvalidId {
    /// Returns the domain ID type that rejected the value.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        self.kind
    }

    /// Returns the validation failure category.
    #[must_use]
    pub const fn reason(&self) -> InvalidIdReason {
        self.reason
    }
}

macro_rules! typed_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(Uuid);

        impl $name {
            /// Generates a new process-ordered `UUIDv7` value.
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }

            /// Generates an ID through an injected source and validates its UUID version.
            ///
            /// # Errors
            ///
            /// Returns [`InvalidId`] when the source does not supply a version 7 UUID.
            pub fn generate(generator: &impl IdGenerator) -> Result<Self, InvalidId> {
                Self::from_bytes(generator.next_uuid_v7())
            }

            /// Reconstructs the ID from network-order UUID bytes.
            ///
            /// # Errors
            ///
            /// Returns [`InvalidId`] when the bytes do not encode a version 7 UUID.
            pub fn from_bytes(bytes: [u8; 16]) -> Result<Self, InvalidId> {
                let value = Uuid::from_bytes(bytes);
                if value.get_version_num() != 7 {
                    return Err(InvalidId {
                        kind: stringify!($name),
                        reason: InvalidIdReason::WrongVersion,
                    });
                }
                Ok(Self(value))
            }

            /// Returns the network-order UUID bytes.
            #[must_use]
            pub const fn as_bytes(&self) -> &[u8; 16] {
                self.0.as_bytes()
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter
                    .debug_tuple(stringify!($name))
                    .field(&self.to_string())
                    .finish()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }

        impl FromStr for $name {
            type Err = InvalidId;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                let uuid = Uuid::try_parse(value).map_err(|_| InvalidId {
                    kind: stringify!($name),
                    reason: InvalidIdReason::Malformed,
                })?;
                Self::from_bytes(uuid.into_bytes())
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(&self.to_string())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                value.parse().map_err(de::Error::custom)
            }
        }
    };
}

typed_id!(
    /// Identifies one local or server profile.
    ///
    /// IDs for different entities are intentionally not interchangeable.
    ///
    /// ```compile_fail
    /// use jarvis_core::{ProfileId, RequestId};
    /// let profile = ProfileId::new();
    /// let _request: RequestId = profile;
    /// ```
    ProfileId
);
typed_id!(
    /// Identifies a local protocol client.
    ClientId
);
typed_id!(
    /// Identifies one client request.
    RequestId
);
typed_id!(
    /// Correlates related work across component boundaries.
    CorrelationId
);
typed_id!(
    /// Identifies one daemon process lifecycle record.
    DaemonRunId
);
typed_id!(
    /// Identifies one conversation session.
    ///
    /// Distinct from [`DaemonRunId`]: a session is a conversation, while a daemon run is one
    /// process lifetime. Collapsing them would let a restart be recorded as a new
    /// conversation.
    SessionId
);
typed_id!(
    /// Identifies one agent run within a session.
    RunId
);
typed_id!(
    /// Identifies one workspace, the unit that scopes data and authority.
    WorkspaceId
);
typed_id!(
    /// Identifies one durable approval request.
    ApprovalId
);

#[cfg(test)]
mod tests {
    use super::*;

    struct FixedGenerator([u8; 16]);

    impl IdGenerator for FixedGenerator {
        fn next_uuid_v7(&self) -> [u8; 16] {
            self.0
        }
    }

    #[test]
    fn generated_ids_are_uuid_v7_and_json_round_trip() {
        let id = RequestId::new();
        assert_eq!(id.as_bytes()[6] >> 4, 7);

        let encoded = serde_json::to_string(&id).unwrap_or_default();
        let decoded: Result<RequestId, _> = serde_json::from_str(&encoded);
        assert_eq!(decoded.ok(), Some(id));
    }

    #[test]
    fn injected_generation_is_deterministic() {
        let expected: RequestId = "01890f3e-93c8-7cc6-98c4-dc0c0c07398f"
            .parse()
            .unwrap_or_else(|error| panic!("valid UUIDv7 fixture: {error}"));
        let generator = FixedGenerator(*expected.as_bytes());

        assert_eq!(RequestId::generate(&generator), Ok(expected));
    }

    #[test]
    fn non_v7_and_malformed_values_fail_closed() {
        let version_four = "550e8400-e29b-41d4-a716-446655440000";
        let wrong_version = version_four.parse::<ProfileId>();
        assert_eq!(
            wrong_version.map_err(|error| error.reason()),
            Err(InvalidIdReason::WrongVersion)
        );

        let malformed = "not-an-id".parse::<ProfileId>();
        assert_eq!(
            malformed.map_err(|error| error.reason()),
            Err(InvalidIdReason::Malformed)
        );
    }
}
