use std::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use thiserror::Error;

/// Explains why sensitivity text was rejected.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum InvalidSensitivity {
    /// The text did not name a known sensitivity level.
    #[error("sensitivity must be one of public, internal, confidential, restricted")]
    Unknown,
}

/// How widely content may be disclosed.
///
/// This is a **flow policy**, not a label: the ordered levels exist so that a
/// destination can be compared against content rather than merely recorded
/// alongside it. Several documents reference a `sensitivity` field
/// (`docs/data/schema.md`, `docs/architecture/events-and-workflows.md`, and
/// `docs/api/contracts.md`); this type is that field's vocabulary.
///
/// The level increases from [`Self::Public`] to [`Self::Restricted`]. Content may
/// flow **outward** to a destination whose own maximum is at least as restrictive,
/// which is what [`Self::can_flow_to`] decides.
///
/// The default is [`Self::Internal`] rather than [`Self::Public`]. A row whose
/// classification was never set must not be treated as safe to disclose, so the
/// default fails closed.
///
/// Serialization is implemented manually rather than derived so that the wire form
/// is the same stable snake-case string used in storage, and so a future variant
/// cannot silently acquire a different spelling through a derive attribute.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Sensitivity {
    /// Safe to disclose broadly, including outside the machine.
    Public,
    /// Normal private content for the owning workspace.
    ///
    /// This is the default: a value whose classification was never set must not be
    /// treated as safe to disclose.
    #[default]
    Internal,
    /// Content whose exposure would be harmful, for example personal documents.
    Confidential,
    /// Content requiring explicit handling, for example credentials or health data.
    Restricted,
}

impl Sensitivity {
    /// Returns the stable snake-case wire/storage code.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Internal => "internal",
            Self::Confidential => "confidential",
            Self::Restricted => "restricted",
        }
    }

    /// Returns the ordered level, where a larger value is more restrictive.
    #[must_use]
    pub const fn level(self) -> u8 {
        match self {
            Self::Public => 0,
            Self::Internal => 1,
            Self::Confidential => 2,
            Self::Restricted => 3,
        }
    }

    /// Returns whether content at this level may be sent to `destination`.
    ///
    /// Flow is permitted only toward a destination that is at least as restrictive.
    /// Sending `Restricted` content to an `Internal` destination returns `false`, so
    /// a caller cannot widen its own classification by relabelling the target.
    #[must_use]
    pub const fn can_flow_to(self, destination: Self) -> bool {
        destination.level() >= self.level()
    }

    /// Returns whether the content is restricted enough to require local execution.
    ///
    /// Used as one input to placement policy; it is an input, not a decision, because
    /// workspace policy and requested capability also apply.
    #[must_use]
    pub const fn requires_local_handling(self) -> bool {
        self.level() >= Self::Confidential.level()
    }

    /// Returns all levels from least to most restrictive.
    #[must_use]
    pub const fn all() -> [Self; 4] {
        [
            Self::Public,
            Self::Internal,
            Self::Confidential,
            Self::Restricted,
        ]
    }
}

impl fmt::Display for Sensitivity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for Sensitivity {
    type Err = InvalidSensitivity;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "public" => Ok(Self::Public),
            "internal" => Ok(Self::Internal),
            "confidential" => Ok(Self::Confidential),
            "restricted" => Ok(Self::Restricted),
            _ => Err(InvalidSensitivity::Unknown),
        }
    }
}

impl Serialize for Sensitivity {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Sensitivity {
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

    #[test]
    fn the_default_is_not_the_most_disclosable_level() {
        // A row whose classification was never set must not be treated as safe to
        // publish. Defaulting to `Public` would fail open.
        assert_eq!(Sensitivity::default(), Sensitivity::Internal);
        assert_ne!(Sensitivity::default(), Sensitivity::Public);

        let parsed: Result<Sensitivity, _> = serde_json::from_str("\"internal\"");
        assert_eq!(parsed.ok(), Some(Sensitivity::Internal));
    }

    #[test]
    fn a_default_classified_value_may_not_flow_to_a_public_destination() {
        // This is the concrete consequence of the default: unclassified content is
        // not allowed to reach a public destination.
        assert!(!Sensitivity::default().can_flow_to(Sensitivity::Public));
    }

    #[test]
    fn flow_is_permitted_outward_but_never_into_a_looser_destination() {
        assert!(Sensitivity::Public.can_flow_to(Sensitivity::Restricted));
        assert!(Sensitivity::Internal.can_flow_to(Sensitivity::Confidential));
        assert!(Sensitivity::Restricted.can_flow_to(Sensitivity::Restricted));
        assert!(!Sensitivity::Restricted.can_flow_to(Sensitivity::Confidential));
        assert!(!Sensitivity::Confidential.can_flow_to(Sensitivity::Internal));
    }

    #[test]
    fn levels_are_ordered_and_unique() {
        let levels: Vec<u8> = Sensitivity::all()
            .iter()
            .map(|value| value.level())
            .collect();
        assert_eq!(levels, vec![0, 1, 2, 3]);
        assert_eq!(Sensitivity::all().len(), 4);
        assert!(
            Sensitivity::Public < Sensitivity::Restricted,
            "derived ordering must match the flow ordering, or a comparison in a \
             repository would disagree with `can_flow_to`"
        );
    }

    #[test]
    fn confidential_and_restricted_require_local_handling() {
        assert!(!Sensitivity::Public.requires_local_handling());
        assert!(!Sensitivity::Internal.requires_local_handling());
        assert!(Sensitivity::Confidential.requires_local_handling());
        assert!(Sensitivity::Restricted.requires_local_handling());
    }

    #[test]
    fn every_level_round_trips_through_text_and_json() {
        for value in Sensitivity::all() {
            assert_eq!(value.to_string(), value.as_str());
            assert_eq!(value.as_str().parse::<Sensitivity>().ok(), Some(value));

            let json = serde_json::to_string(&value).unwrap_or_default();
            let decoded: Result<Sensitivity, _> = serde_json::from_str(&json);
            assert_eq!(decoded.ok(), Some(value));
        }
    }

    #[test]
    fn an_unknown_level_is_rejected_rather_than_coerced() {
        // Coercing to the most restrictive value would be safe but silently wrong for
        // a future level; rejecting surfaces the need to widen the vocabulary.
        assert_eq!(
            "secret".parse::<Sensitivity>(),
            Err(InvalidSensitivity::Unknown)
        );
        assert_eq!(
            "Public".parse::<Sensitivity>(),
            Err(InvalidSensitivity::Unknown)
        );
        let decoded: Result<Sensitivity, _> = serde_json::from_str("\"top_secret\"");
        assert!(decoded.is_err());
    }
}
