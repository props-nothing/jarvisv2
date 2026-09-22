//! Tool identifiers: a stable `namespace.name` that may not collide.
//!
//! # Why the namespace is structural rather than a naming convention
//!
//! `docs/architecture/tools-and-connectors.md` requires that "tool names are namespaced and
//! collisions are errors". A convention alone does not achieve that: two connectors can both ship a
//! tool called `gmail.search`, and the collision is discovered when one silently shadows the other.
//! Making the namespace a **field of the type** means `P3-002`'s registry has something to key on,
//! so registering two tools with the same identifier is a refusal at registration rather than a
//! lookup that returns whichever was inserted last.
//!
//! # Why the two halves are validated separately
//!
//! The namespace identifies the **source** of a capability (a connector, a runtime, JARVIS itself)
//! and the name identifies the **operation**. They have different rules and different failure
//! meanings, so a single opaque string would lose the ability to say which part was wrong.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::policy::ToolSource;

/// Maximum characters in a whole tool identifier.
///
/// Bounded because an identifier reaches model-facing discovery output and stored audit records. An
/// unbounded identifier is a place for content to be smuggled into metadata that nothing treats as
/// content.
pub const MAX_TOOL_ID_CHARS: usize = 128;

/// Maximum characters in the namespace or the name half.
pub const MAX_TOOL_ID_SEGMENT_CHARS: usize = 48;

/// Maximum characters in a tool version string.
///
/// Versions are recorded so a stored intent names the behaviour it was written against. This is a
/// plain bounded string rather than semver parsing: a connector's version may be a date, an API
/// revision, or a hash, and forcing semver would make those unrepresentable for no gain.
pub const MAX_TOOL_VERSION_CHARS: usize = 32;

/// Explains why a tool identifier was rejected.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ToolIdError {
    /// The identifier was empty or whitespace-only.
    #[error("a tool identifier must not be empty")]
    Empty,
    /// The identifier exceeded the bounded length.
    #[error("a tool identifier exceeds {MAX_TOOL_ID_CHARS} characters")]
    TooLong,
    /// The identifier had no `.`, so it names no source.
    #[error("a tool identifier must be qualified as namespace.name")]
    NotQualified,
    /// A half was empty, too long, or contained a character outside the allowed set.
    #[error("the tool identifier half {segment:?} is invalid")]
    Segment {
        /// Which half was rejected, without echoing the offending value.
        segment: &'static str,
    },
    /// The version was empty or contained a character outside the allowed set.
    #[error("a tool version must be 1 to {MAX_TOOL_VERSION_CHARS} safe characters")]
    InvalidVersion,
}

/// A canonical tool identifier: `namespace.name`.
///
/// The stored form is the whole dotted string, which is also the wire form. The two halves stay
/// available because the namespace is what a collision is checked against and what a policy grant
/// names.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct ToolId {
    namespace: String,
    name: String,
}

impl ToolId {
    /// Parses and validates an identifier.
    ///
    /// # Errors
    ///
    /// Returns [`ToolIdError`] for an empty, oversized, unqualified, or malformed identifier.
    pub fn new(value: impl Into<String>) -> Result<Self, ToolIdError> {
        value.into().parse()
    }

    /// Builds an identifier from its halves.
    ///
    /// # Errors
    ///
    /// Returns [`ToolIdError::Segment`] when either half is unusable. Provided alongside
    /// [`Self::new`] because a connector declares its namespace once and then names operations:
    /// building from parts avoids re-parsing a string it already has.
    ///
    /// The halves have **different** rules, and the difference is what keeps this type round-trip
    /// safe. A namespace may be dotted (`mcp.github`), because the source class is a qualified path
    /// and [`ToolSource::from_namespace`] reads those prefixes. A name may not, because the name is
    /// by construction everything after the final dot: if a name could contain a dot,
    /// `from_parts("mcp.github", "a.b")` would print `mcp.github.a.b`, which re-parses as the
    /// namespace `mcp.github.a` and the name `b` â€” a different tool that happens to have the same
    /// string. Refusing the dot in the name is what makes the string form canonical.
    pub fn from_parts(namespace: &str, name: &str) -> Result<Self, ToolIdError> {
        validate_segment(namespace, "namespace")?;
        validate_operation(name)?;
        Ok(Self {
            namespace: namespace.to_owned(),
            name: name.to_owned(),
        })
    }

    /// Returns the namespace half, which identifies the source of the capability.
    #[must_use]
    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    /// Returns the name half, which identifies the operation.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the source class a namespace belongs to.
    ///
    /// Derived from the namespace rather than declared separately, so a tool cannot claim to be
    /// native while using a connector's namespace. The mapping is a convention this type enforces,
    /// which is the point: `docs/architecture/tools-and-connectors.md` lists `native`, `connector`,
    /// `MCP`, `runtime`, and `extension`, and the namespace is where that becomes checkable.
    #[must_use]
    pub fn source(&self) -> ToolSource {
        ToolSource::from_namespace(&self.namespace)
    }

    /// Validates a version string.
    ///
    /// # Errors
    ///
    /// Returns [`ToolIdError::InvalidVersion`] for an empty version or one containing a character
    /// outside the safe set.
    pub fn validate_version(value: &str) -> Result<(), ToolIdError> {
        let usable = !value.is_empty()
            && value.chars().count() <= MAX_TOOL_VERSION_CHARS
            && value
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+'));
        if usable {
            Ok(())
        } else {
            Err(ToolIdError::InvalidVersion)
        }
    }
}

/// Validates one half of an identifier.
///
/// The character set is deliberately narrow: lowercase ASCII letters, digits, `_`, `-`, `.`, and
/// `:`. A name reaches a model in a discovery list and is used as a lookup key, so an uppercase
/// letter or a space would create two keys that look identical to a reader, and a character with a
/// canonical form (a combining accent) would create two the database can distinguish but a human
/// cannot.
///
/// Dots are allowed here because a namespace is a qualified path (`mcp.github`, `jarvis.files`).
/// [`validate_operation`] is the stricter form for the name half.
fn validate_segment(value: &str, segment: &'static str) -> Result<(), ToolIdError> {
    let acceptable = !value.is_empty()
        && value.chars().count() <= MAX_TOOL_ID_SEGMENT_CHARS
        && value.chars().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-' | '.' | ':')
        });
    if acceptable {
        Ok(())
    } else {
        Err(ToolIdError::Segment { segment })
    }
}

/// Validates the operation half, which may not contain a dot.
///
/// The last dot in an identifier is the separator, so a dot inside the name would make the parsed
/// halves disagree with the halves that produced the string.
fn validate_operation(value: &str) -> Result<(), ToolIdError> {
    let acceptable = !value.is_empty()
        && value.chars().count() <= MAX_TOOL_ID_SEGMENT_CHARS
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-' | ':'));
    if acceptable {
        Ok(())
    } else {
        Err(ToolIdError::Segment { segment: "name" })
    }
}

impl fmt::Display for ToolId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}", self.namespace, self.name)
    }
}

impl FromStr for ToolId {
    type Err = ToolIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.trim().is_empty() {
            return Err(ToolIdError::Empty);
        }
        if value.chars().count() > MAX_TOOL_ID_CHARS {
            return Err(ToolIdError::TooLong);
        }
        // Split on the **last** dot, so the namespace may be a qualified path. `mcp.github.search`
        // is namespace `mcp.github` and name `search`, which is what
        // `ToolSource::from_namespace` expects: it reads the `mcp.` prefix, so a source class has to
        // survive into the namespace rather than being flattened into the name.
        let Some((namespace, name)) = value.rsplit_once('.') else {
            return Err(ToolIdError::NotQualified);
        };
        Self::from_parts(namespace, name)
    }
}

impl TryFrom<String> for ToolId {
    type Error = ToolIdError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl From<ToolId> for String {
    fn from(id: ToolId) -> Self {
        id.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A well-formed identifier round-trips through its string form.
    #[test]
    fn a_qualified_identifier_round_trips() {
        let id = ToolId::new("gmail.send_message").unwrap_or_else(|error| panic!("valid: {error}"));
        assert_eq!(id.namespace(), "gmail");
        assert_eq!(id.name(), "send_message");
        assert_eq!(id.to_string(), "gmail.send_message");
        assert_eq!(id.source(), ToolSource::Connector);

        let encoded =
            serde_json::to_string(&id).unwrap_or_else(|error| panic!("serialize: {error}"));
        assert_eq!(encoded, "\"gmail.send_message\"");
        let decoded: ToolId =
            serde_json::from_str(&encoded).unwrap_or_else(|error| panic!("deserialize: {error}"));
        assert_eq!(decoded, id);
    }

    /// An unqualified name is refused, because a name with no namespace has no source.
    #[test]
    fn an_unqualified_identifier_is_refused() {
        assert_eq!(ToolId::new("send_message"), Err(ToolIdError::NotQualified));
        assert_eq!(ToolId::new(""), Err(ToolIdError::Empty));
        assert_eq!(ToolId::new("   "), Err(ToolIdError::Empty));
        assert_eq!(
            ToolId::new(".name"),
            Err(ToolIdError::Segment {
                segment: "namespace"
            })
        );
        assert_eq!(
            ToolId::new("ns."),
            Err(ToolIdError::Segment { segment: "name" })
        );
    }

    /// A third qualifier is read as a dotted namespace, not folded into the name.
    ///
    /// The split is on the **last** dot because the source class is a qualified path: `mcp.github`
    /// is what [`ToolSource::from_namespace`] recognizes, so `mcp.github.search` has to parse as
    /// namespace `mcp.github` + name `search` rather than namespace `mcp` + name `github.search`.
    /// The other reading would report every MCP tool as coming from a connector.
    #[test]
    fn a_dotted_namespace_is_kept_whole() {
        let id = ToolId::new("mcp.github.search")
            .unwrap_or_else(|error| panic!("dotted namespace: {error}"));
        assert_eq!(id.namespace(), "mcp.github");
        assert_eq!(id.name(), "search");
        assert_eq!(id.source(), ToolSource::Mcp);
        // And it prints back to exactly what was parsed.
        assert_eq!(id.to_string(), "mcp.github.search");

        let nested = ToolId::new("runtime.agent.session.run")
            .unwrap_or_else(|error| panic!("nested namespace: {error}"));
        assert_eq!(nested.namespace(), "runtime.agent.session");
        assert_eq!(nested.name(), "run");
        assert_eq!(nested.source(), ToolSource::Runtime);
    }

    /// **A dot inside the name is refused, which is what keeps the string form canonical.**
    ///
    /// The halves and the printed string have to agree. If `from_parts("mcp.github", "a.b")` were
    /// allowed it would print `mcp.github.a.b`, which re-parses as namespace `mcp.github.a` and name
    /// `b` — a different tool with the same identifier, which is a collision the registry could not
    /// see.
    #[test]
    fn a_dot_inside_the_name_is_refused() {
        assert_eq!(
            ToolId::from_parts("mcp.github", "a.b"),
            Err(ToolIdError::Segment { segment: "name" })
        );
        assert_eq!(
            ToolId::from_parts("mcp.github", ""),
            Err(ToolIdError::Segment { segment: "name" })
        );
    }

    /// Every identifier that parses prints to a string that parses to the same identifier.
    ///
    /// The property the registry depends on: an identifier is a key, so its string form and its
    /// halves must not be able to disagree.
    #[test]
    fn the_string_form_round_trips_for_every_accepted_identifier() {
        for source in [
            "jarvis.files.read",
            "gmail.send_message",
            "mcp.github.search",
            "runtime.openclaw.run",
            "extension.example.act",
            "a.b",
        ] {
            let parsed = ToolId::new(source).unwrap_or_else(|error| panic!("{source}: {error}"));
            let printed = parsed.to_string();
            let reparsed = ToolId::new(printed.clone())
                .unwrap_or_else(|error| panic!("reparse {printed}: {error}"));
            assert_eq!(
                reparsed, parsed,
                "{printed} must round-trip to the same identifier"
            );
            assert_eq!(reparsed.namespace(), parsed.namespace());
            assert_eq!(reparsed.name(), parsed.name());
            assert_eq!(reparsed.source(), parsed.source());
        }
    }

    /// Uppercase is refused so two identifiers cannot look identical to a reader.
    #[test]
    fn an_identifier_with_uppercase_is_refused() {
        assert!(matches!(
            ToolId::new("Gmail.send"),
            Err(ToolIdError::Segment { .. })
        ));
        assert!(matches!(
            ToolId::new("gmail.Send"),
            Err(ToolIdError::Segment { .. })
        ));
    }

    /// A path separator or a template character is refused, because an identifier reaches a path
    /// and a shell-like context in later slices and this is the type that gates them all.
    #[test]
    fn characters_that_change_a_context_are_refused() {
        for hostile in [
            "gmail/send",
            "gmail send",
            "gmail\\send",
            "gmail?send",
            "gmail#send",
            "gmail%2fsend",
            "gmail\nsend",
        ] {
            let parsed = ToolId::new(hostile);
            assert!(
                parsed.is_err(),
                "{hostile:?} must not be a usable tool identifier"
            );
        }
    }

    /// Overlong identifiers and halves are refused.
    #[test]
    fn overlong_identifiers_are_refused() {
        assert_eq!(
            ToolId::new("a".repeat(MAX_TOOL_ID_CHARS + 1)),
            Err(ToolIdError::TooLong)
        );
        let long_half = "a".repeat(MAX_TOOL_ID_SEGMENT_CHARS + 1);
        assert_eq!(
            ToolId::new(format!("{long_half}.name")),
            Err(ToolIdError::Segment {
                segment: "namespace"
            })
        );
    }

    /// The source class is derived from the namespace, so a tool cannot misdeclare where it is from.
    #[test]
    fn the_source_follows_the_namespace() {
        for (id, expected) in [
            ("jarvis.files", ToolSource::Native),
            ("jarvis.files.read", ToolSource::Native),
            ("gmail.send_message", ToolSource::Connector),
            ("mcp.github.search", ToolSource::Mcp),
            ("runtime.openclaw.run", ToolSource::Runtime),
            ("extension.example.act", ToolSource::Extension),
        ] {
            let parsed = ToolId::new(id).unwrap_or_else(|error| panic!("{id}: {error}"));
            assert_eq!(parsed.source(), expected, "{id} has the wrong source");
        }
    }

    /// A version is bounded and restricted, and every accepted form is one a provider could use.
    #[test]
    fn a_version_is_bounded_and_restricted() {
        for acceptable in ["1", "1.0.0", "2026-09-22", "v2-rc1", "0.1.0+build.5"] {
            assert!(
                ToolId::validate_version(acceptable).is_ok(),
                "{acceptable:?} must be a usable version"
            );
        }
        for refused in ["", " ", "1.0 ", "1/0", "v1\n2", &"9".repeat(40)] {
            assert!(
                ToolId::validate_version(refused).is_err(),
                "{refused:?} must not be a usable version"
            );
        }
    }

    /// Two identifiers that differ only in case are NOT distinguishable, because both are refused.
    ///
    /// This is the collision property stated as a test: a registry keyed on `ToolId` cannot hold
    /// `gmail.send` and `Gmail.Send` as different tools, because the second cannot be constructed.
    #[test]
    fn case_variants_cannot_both_be_registered() {
        let lower = ToolId::new("gmail.send");
        let mixed = ToolId::new("Gmail.Send");
        assert!(lower.is_ok());
        assert!(mixed.is_err(), "a case variant must not be constructible");
    }
}
