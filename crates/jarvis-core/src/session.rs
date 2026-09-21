//! Session vocabulary: the channel a conversation arrived on and its lifecycle status.
//!
//! `docs/architecture/identity-and-workspaces.md` distinguishes a **session** (a
//! conversation) from a **run** (one piece of work inside it). The distinction is durable
//! rather than cosmetic: a session outlives any single run, which is why archiving happens
//! on an explicit request rather than when a run completes.
//!
//! # Why the channel is a closed set here
//!
//! The channel records how a conversation *reached* the daemon, and
//! `docs/architecture/security.md` makes channel a policy input: a capability granted to a
//! CLI session need not be granted to a voice session. Storing a free-form string would
//! mean the policy layer compares against text it has never seen, and a typo
//! (`"destkop"`) would silently produce a session with no matching policy rather than a
//! rejection. The set is therefore closed and validated by the same values the schema's
//! `CHECK` constraint allows.

use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Maximum session title length, matching the storage schema's `CHECK`.
pub const MAX_SESSION_TITLE_CHARS: usize = 256;

/// How a session reached the daemon.
///
/// The wire form is the lowercase name, which is also the stored form.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionChannel {
    /// A terminal client.
    Cli,
    /// The desktop or web view.
    Desktop,
    /// An HTTP or IPC API client.
    Api,
    /// A voice or telephony session.
    Voice,
}

impl SessionChannel {
    /// Returns the stable stored and wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::Desktop => "desktop",
            Self::Api => "api",
            Self::Voice => "voice",
        }
    }

    /// Returns every channel, for exhaustive tests and schema agreement checks.
    #[must_use]
    pub const fn all() -> [Self; 4] {
        [Self::Cli, Self::Desktop, Self::Api, Self::Voice]
    }
}

impl fmt::Display for SessionChannel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The lifecycle status of a session.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    /// The session accepts new runs.
    Active,
    /// The session is closed to new runs but retained for history.
    Archived,
}

impl SessionStatus {
    /// Returns the stable stored and wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Archived => "archived",
        }
    }
}

impl fmt::Display for SessionStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Explains why a session field was rejected.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum InvalidSessionField {
    /// The channel name was not one of the supported values.
    #[error("the session channel is not supported")]
    Channel,
    /// The status name was not one of the supported values.
    #[error("the session status is not supported")]
    Status,
    /// The title was empty, whitespace-only, or longer than the schema allows.
    #[error("the session title is empty or too long")]
    Title,
}

impl SessionChannel {
    /// Parses a stored channel name.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidSessionField::Channel`] for any unsupported name, so a stored value
    /// outside the closed set is reported rather than defaulted. A default would turn a
    /// corrupt row into a session with the wrong policy.
    pub fn parse(value: &str) -> Result<Self, InvalidSessionField> {
        match value {
            "cli" => Ok(Self::Cli),
            "desktop" => Ok(Self::Desktop),
            "api" => Ok(Self::Api),
            "voice" => Ok(Self::Voice),
            _ => Err(InvalidSessionField::Channel),
        }
    }
}

impl SessionStatus {
    /// Parses a stored status name.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidSessionField::Status`] for any unsupported name.
    pub fn parse(value: &str) -> Result<Self, InvalidSessionField> {
        match value {
            "active" => Ok(Self::Active),
            "archived" => Ok(Self::Archived),
            _ => Err(InvalidSessionField::Status),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every channel round-trips through its stored name.
    #[test]
    fn every_channel_round_trips_through_its_name() {
        for channel in SessionChannel::all() {
            assert_eq!(SessionChannel::parse(channel.as_str()), Ok(channel));
        }
        assert_eq!(
            SessionChannel::parse("destkop"),
            Err(InvalidSessionField::Channel),
            "a misspelled channel must be refused, not defaulted"
        );
    }

    /// The wire form is the lowercase name, so a serde rename cannot silently diverge from
    /// the value the schema stores.
    #[test]
    fn the_wire_form_matches_the_stored_form() {
        for channel in SessionChannel::all() {
            let encoded = serde_json::to_string(&channel)
                .unwrap_or_else(|error| panic!("encode {channel}: {error}"));
            assert_eq!(encoded, format!("\"{}\"", channel.as_str()));
        }
        assert_eq!(
            serde_json::to_string(&SessionStatus::Active)
                .unwrap_or_else(|error| panic!("encode status: {error}")),
            "\"active\""
        );
    }

    /// A status outside the closed set is reported rather than coerced.
    #[test]
    fn an_unknown_status_is_refused() {
        assert_eq!(
            SessionStatus::parse("closed"),
            Err(InvalidSessionField::Status)
        );
        assert_eq!(
            SessionStatus::parse("archived"),
            Ok(SessionStatus::Archived)
        );
    }
}
