use std::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use thiserror::Error;
use time::{OffsetDateTime, UtcOffset, format_description::well_known::Rfc3339};

/// Supplies UTC wall-clock time to domain and application services.
pub trait Clock: Send + Sync {
    /// Returns the current UTC timestamp.
    fn now(&self) -> UtcTimestamp;
}

/// Reads UTC time from the operating-system clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> UtcTimestamp {
        UtcTimestamp(OffsetDateTime::now_utc())
    }
}

/// Indicates that a timestamp was malformed or outside the supported UTC range.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum InvalidTimestamp {
    /// The timestamp text is not RFC 3339.
    #[error("timestamp must be valid RFC 3339")]
    Malformed,
    /// The timestamp cannot be represented by the selected time library.
    #[error("timestamp is outside the supported UTC range")]
    OutOfRange,
}

/// A nanosecond-precision instant normalized to UTC.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct UtcTimestamp(OffsetDateTime);

impl UtcTimestamp {
    /// Reads the current timestamp from an injected clock.
    #[must_use]
    pub fn now(clock: &impl Clock) -> Self {
        clock.now()
    }

    /// Constructs a UTC timestamp from nanoseconds since the Unix epoch.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidTimestamp::OutOfRange`] when the instant cannot be represented.
    pub fn from_unix_nanos(value: i128) -> Result<Self, InvalidTimestamp> {
        OffsetDateTime::from_unix_timestamp_nanos(value)
            .map(Self)
            .map_err(|_| InvalidTimestamp::OutOfRange)
    }

    /// Returns nanoseconds since the Unix epoch.
    #[must_use]
    pub const fn unix_nanos(self) -> i128 {
        self.0.unix_timestamp_nanos()
    }
}

impl fmt::Display for UtcTimestamp {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = self.0.format(&Rfc3339).map_err(|_| fmt::Error)?;
        formatter.write_str(&value)
    }
}

impl FromStr for UtcTimestamp {
    type Err = InvalidTimestamp;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let parsed =
            OffsetDateTime::parse(value, &Rfc3339).map_err(|_| InvalidTimestamp::Malformed)?;
        let utc = parsed
            .checked_to_offset(UtcOffset::UTC)
            .ok_or(InvalidTimestamp::OutOfRange)?;
        Ok(Self(utc))
    }
}

impl Serialize for UtcTimestamp {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for UtcTimestamp {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FixedClock(UtcTimestamp);

    impl Clock for FixedClock {
        fn now(&self) -> UtcTimestamp {
            self.0
        }
    }

    #[test]
    fn timestamp_is_injectable_and_preserves_nanoseconds() {
        let expected = UtcTimestamp::from_unix_nanos(1_700_000_000_123_456_789)
            .unwrap_or_else(|error| panic!("valid fixture timestamp: {error}"));
        let clock = FixedClock(expected);

        assert_eq!(UtcTimestamp::now(&clock), expected);
        let json = serde_json::to_string(&expected).unwrap_or_default();
        let decoded: Result<UtcTimestamp, _> = serde_json::from_str(&json);
        assert_eq!(decoded.as_ref().ok(), Some(&expected));
        assert_eq!(
            decoded.ok().map(UtcTimestamp::unix_nanos),
            Some(1_700_000_000_123_456_789)
        );
    }

    /// **The stored form does NOT sort lexicographically, so this text must not be compared in SQL.**
    ///
    /// Found while designing a table with an `expires_at < now` predicate, by printing real values
    /// rather than assuming. `Rfc3339` **omits** the fractional part when it is zero, so:
    ///
    /// ```text
    /// 2026-09-21T08:00:00Z   <  2026-09-21T08:00:00.5Z   (string order)
    /// ```
    ///
    /// `'0'` (0x30) is less than `'Z'` (0x5A), so the whole-second value sorts **last** among values
    /// in its own second. A predicate like `expires_at < ?1` would therefore treat an expired row as
    /// unexpired. This is a live defect class: `inspect.rs` orders `daemon_instances` by `started_at`,
    /// and `run_events_recorded_idx` is a `recorded_at DESC` index, so a row written at a whole
    /// second sorts after rows written later in the same second.
    ///
    /// The test pins the behaviour rather than fixing it, because changing the stored format is a
    /// migration across every timestamp column in the schema — and a schema-version bump is a
    /// decision, not a drive-by. What it prevents is the assumption being made again silently.
    ///
    /// **Until the format is fixed, an expiry check must compare `unix_nanos()` in Rust**, or store a
    /// separate integer column. A string comparison in SQL will be wrong.
    #[test]
    fn the_stored_form_is_not_lexicographically_sortable() {
        // The same instant with a zero fraction is the case that breaks ordering.
        let base: i128 = 1_700_000_000_000_000_000;
        let whole = UtcTimestamp::from_unix_nanos(base)
            .unwrap_or_else(|error| panic!("{error}"))
            .to_string();
        let fraction = UtcTimestamp::from_unix_nanos(base + 500_000_000)
            .unwrap_or_else(|error| panic!("{error}"))
            .to_string();

        assert!(!whole.contains('.'), "a whole second omits the fraction");
        assert!(
            fraction.contains('.'),
            "a fraction is emitted when non-zero"
        );
        assert!(
            fraction.as_str() < whole.as_str(),
            "the LATER instant sorts EARLIER as text, which is the defect"
        );

        // Time order and string order disagree, stated as one assertion.
        let mut time_ordered = vec![whole.clone(), fraction.clone()];
        time_ordered.sort_by_key(|value| {
            value
                .parse::<UtcTimestamp>()
                .map(UtcTimestamp::unix_nanos)
                .unwrap_or_default()
        });
        let mut string_ordered = vec![whole, fraction];
        string_ordered.sort();
        assert_ne!(
            time_ordered, string_ordered,
            "if these ever agree, the stored format changed and SQL comparisons became safe"
        );
    }

    #[test]
    fn parsed_offsets_are_normalized_to_utc() {
        let timestamp: UtcTimestamp = "2026-09-20T14:30:00.123456789+02:00"
            .parse()
            .unwrap_or_else(|error| panic!("valid RFC 3339 fixture: {error}"));

        assert_eq!(timestamp.to_string(), "2026-09-20T12:30:00.123456789Z");
    }
}
