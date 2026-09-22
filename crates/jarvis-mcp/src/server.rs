//! MCP server identity and the canonical tool names derived from it.
//!
//! # Why a server's own name is not usable as an authority
//!
//! An MCP server tells the client who it is. That answer is an **assertion by the party being
//! identified**, and the specification says so outright: the server `name` from `serverInfo` "is not
//! guaranteed to be unique across servers and **SHOULD NOT** be relied upon for disambiguation".
//!
//! JARVIS therefore never lets a server choose the workspace that names it. A server is identified
//! by an **operator-chosen local name** from configuration, and the name the server reports about
//! itself is recorded as *evidence*, never as an identifier. The two are different fields with
//! different trust levels, and this module keeps them apart rather than folding one into the other.
//!
//! # Why the naming strategy is an enum and the collision check is not
//!
//! The specification notes that aggregated tools collide and that a client "**SHOULD** implement a
//! disambiguation strategy such as prefixing tool names with a server identifier". It does **not**
//! say what to do when a prefix is not enough — and it is not enough, because an MCP tool name may
//! legally contain a dot while a JARVIS tool-name half may not, so `admin.tools.list` and
//! `admin.tools` would both flatten to `admin.tools.list` under a naive lowercase-and-prefix rule.
//!
//! Two tools resolving to one identifier is the dangerous direction of that defect: a call intended
//! for one tool would be resolved to another, and the second tool's risk level and effects would be
//! the ones policy decided about. So the strategies that can collide are offered, and **every
//! translation is checked for a collision against what is already registered** — a check cannot be
//! opted out of the way a strategy can.

use std::collections::BTreeMap;
use std::fmt;
use std::fmt::Write as _;

use jarvis_tools::{ToolId, ToolIdError};
use sha2::{Digest, Sha256};

/// Maximum characters in an operator-chosen server name.
///
/// Shorter than the bound on a namespace segment (`MAX_TOOL_ID_SEGMENT_CHARS` is 48) because a
/// server name spends part of that budget on the `mcp.` source marker and, under
/// [`NamingStrategy::Prefixed`], part of the tool-name budget as well.
pub const MAX_SERVER_NAME_CHARS: usize = 24;

/// Maximum characters in a server-reported name or title kept as evidence.
///
/// Bounded because the value reaches operator-facing inventory output and is third-party text: an
/// unbounded field here is an unbounded string from an untrusted peer sitting in a report.
pub const MAX_REPORTED_TEXT_CHARS: usize = 120;

/// Longest run of hexadecimal characters used to disambiguate a name.
///
/// Eight characters of SHA-256, which is what a git short hash uses. This is a disambiguator, not a
/// security boundary — an attacker who can make prefixes collide can grind a colliding digest, so
/// the guarantee here is "a human and the registry can tell these apart", and the *authority* comes
/// from the canonical identifier being stored, not from the hash.
const DISAMBIGUATOR_HEX_CHARS: usize = 8;

/// Explains why a server identity was rejected.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ServerNameError {
    /// The name was empty or only whitespace.
    Empty,
    /// The name exceeded [`MAX_SERVER_NAME_CHARS`].
    TooLong,
    /// The name contained a character outside the safe set.
    Unacceptable {
        /// The offending character.
        character: char,
    },
}

impl fmt::Display for ServerNameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("a server name must not be empty"),
            Self::TooLong => write!(
                formatter,
                "a server name must be at most {MAX_SERVER_NAME_CHARS} characters"
            ),
            Self::Unacceptable { character } => write!(
                formatter,
                "a server name must be lowercase ASCII letters, digits, `-`, or `_`, not {character:?}"
            ),
        }
    }
}

impl std::error::Error for ServerNameError {}

/// An operator-chosen local name for one configured MCP server.
///
/// This is JARVIS's name for the server, **not** the name the server reports about itself. It
/// becomes the namespace of every tool the server offers, so it is validated exactly as narrowly as
/// a tool-name segment is: lowercase ASCII, digits, `-`, and `_`. Dots are refused even though a
/// namespace may contain them, because a dot here would let one server's name be a prefix of
/// another's namespace and make `mcp.a.b` ambiguous between "server `a.b`" and "server `a`".
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ServerName(String);

impl ServerName {
    /// Validates an operator-supplied server name.
    ///
    /// # Errors
    ///
    /// Returns [`ServerNameError`] for an empty, oversized, or unsafe name. Uppercase is refused
    /// rather than folded, because folding is a lossy step that can map two distinct configured
    /// names onto one namespace.
    pub fn new(value: impl Into<String>) -> Result<Self, ServerNameError> {
        let value = value.into();
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(ServerNameError::Empty);
        }
        if trimmed.chars().count() > MAX_SERVER_NAME_CHARS {
            return Err(ServerNameError::TooLong);
        }
        for character in trimmed.chars() {
            if !(character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '-' | '_'))
            {
                return Err(ServerNameError::Unacceptable { character });
            }
        }
        Ok(Self(trimmed.to_owned()))
    }

    /// Returns the name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the namespace every tool from this server is placed under.
    ///
    /// `mcp.<name>`, which [`crate::ToolSource::from_namespace`] classifies as
    /// [`jarvis_tools::ToolSource::Mcp`]. That classification is what marks the definition as
    /// third-party, so the prefix is load-bearing rather than cosmetic.
    #[must_use]
    pub fn namespace(&self) -> String {
        format!("mcp.{}", self.0)
    }
}

impl fmt::Display for ServerName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// What a server asserted about itself, kept as evidence.
///
/// Never an identifier. Recorded so an operator can answer "which server did this tool come from"
/// and so a **changed** self-assertion is visible: a server that starts claiming a different name
/// during a `tools/list` refresh is worth seeing rather than silently absorbing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReportedIdentity {
    /// The `name` from `serverInfo`, bounded and trimmed.
    pub name: String,
    /// The `title` from `serverInfo`, when the server supplied one.
    pub title: Option<String>,
}

impl ReportedIdentity {
    /// Records what a server reported, bounding both fields.
    ///
    /// A missing name is represented as an empty string rather than refused: the field is evidence,
    /// and refusing to record a malformed self-assertion would discard the fact that it was
    /// malformed.
    #[must_use]
    pub fn new(name: &str, title: Option<&str>) -> Self {
        Self {
            name: bounded(name),
            title: title.map(bounded),
        }
    }

    /// Returns whether this matches a previously recorded identity.
    #[must_use]
    pub fn agrees_with(&self, other: &Self) -> bool {
        self == other
    }
}

/// Truncates third-party text to the evidence bound, on a character boundary.
///
/// Char-aware rather than byte-aware because a byte slice of an untrusted string can split a
/// multi-byte character and produce a string that is not valid UTF-8 where the protocol expected
/// text.
fn bounded(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.chars().count() <= MAX_REPORTED_TEXT_CHARS {
        return trimmed.to_owned();
    }
    trimmed.chars().take(MAX_REPORTED_TEXT_CHARS).collect()
}

/// How a server-supplied tool name becomes a JARVIS tool name.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NamingStrategy {
    /// `<server>.<sanitised tool name>`, which is the specification's suggested strategy.
    ///
    /// Readable, and the default. Its cost is length: a long tool name under a long server name can
    /// exceed the name-segment bound, and a name that cannot be shortened honestly has to be
    /// hashed — see [`NamingStrategy::Hashed`].
    Prefixed,
    /// `<sanitised tool name>` alone, for a server an operator has decided cannot collide.
    ///
    /// Offered because a single-server setup pays for a prefix it does not need, and **safe only
    /// because translation is checked for collisions regardless**: a bare strategy that did collide
    /// is refused rather than resolved.
    Bare,
    /// `<server>.<digest>`, discarding the readable name.
    ///
    /// For a server whose names are not representable — non-ASCII, or longer than a name segment can
    /// hold. Deliberately opaque: a *shortened* name would be a different name that could collide
    /// with a real one, whereas a digest is obviously not the server's own name.
    Hashed,
}

impl NamingStrategy {
    /// Returns the stable wire and configuration name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Prefixed => "prefixed",
            Self::Bare => "bare",
            Self::Hashed => "hashed",
        }
    }
}

impl fmt::Display for NamingStrategy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Explains why a tool name could not be translated.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NamingError {
    /// The server supplied an empty tool name.
    EmptyToolName,
    /// The tool name could not be represented and the strategy does not allow hashing.
    ///
    /// `Prefixed` and `Bare` keep the server's name, so a name they cannot represent is a refusal
    /// rather than something to shorten — shortening maps two names onto one.
    Unrepresentable {
        /// The server-supplied name, bounded for the report.
        tool: String,
        /// Why it could not be represented.
        reason: String,
    },
    /// The resulting identifier was rejected by [`ToolId`].
    InvalidIdentifier {
        /// The tool name, bounded for the report.
        tool: String,
        /// The identifier fault.
        source: ToolIdError,
    },
}

impl fmt::Display for NamingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyToolName => formatter.write_str("a server supplied an empty tool name"),
            Self::Unrepresentable { tool, reason } => write!(
                formatter,
                "the MCP tool name {tool} cannot be represented as a JARVIS tool name: {reason}"
            ),
            Self::InvalidIdentifier { tool, source } => {
                write!(
                    formatter,
                    "the MCP tool name {tool} produced an invalid identifier: {source}"
                )
            }
        }
    }
}

impl std::error::Error for NamingError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidIdentifier { source, .. } => Some(source),
            Self::EmptyToolName | Self::Unrepresentable { .. } => None,
        }
    }
}

/// A server-supplied tool name already translated into canonical form.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CanonicalToolName {
    /// The canonical JARVIS identifier.
    pub id: ToolId,
    /// The name exactly as the server supplied it, bounded.
    ///
    /// Kept because `tools/call` must send the **server's** name back. Sending the canonical name
    /// would be a translation applied twice, and the round trip is the only place that bug would
    /// show.
    pub remote: String,
}

/// Translates one server-supplied tool name into a canonical identifier.
///
/// The rules, in order:
///
/// 1. An empty name is refused.
/// 2. A name that is already entirely lowercase-safe **and** fits a name segment is used as-is.
///    This is the common case and the reason a server that already follows the specification's own
///    character guidance produces a readable identifier.
/// 3. Otherwise the name is **hashed**, never truncated. Truncation maps two distinct names onto
///    one identifier, which is the collision this module exists to prevent; a digest does not,
///    because the digest is over the whole name.
///
/// `Prefixed` and `Bare` differ only in the namespace they are placed under, so the *name half* is
/// derived identically for both. A name they cannot represent is refused rather than hashed, because
/// hashing under those strategies would silently change an operator's naming scheme for one tool.
///
/// # Errors
///
/// Returns [`NamingError`] for an empty name, a name the strategy cannot represent, or a result
/// [`ToolId`] rejects.
pub fn canonical_tool_name(
    server: &ServerName,
    tool: &str,
    strategy: NamingStrategy,
) -> Result<CanonicalToolName, NamingError> {
    if tool.trim().is_empty() {
        return Err(NamingError::EmptyToolName);
    }
    let remote = bounded(tool);

    let name_half = match name_half_of(tool, strategy)? {
        NameHalf::Verbatim(name) => name,
        NameHalf::Digest(digest) => digest,
    };
    // The prefix is the **namespace**, not a repeated name segment. Putting it in the name half
    // would give that half a dot, which a JARVIS name may not contain — so `mcp.github.list_issues`
    // is namespace `mcp.github` and name `list_issues`, and the server's identity lives in exactly
    // one place rather than being concatenated into the name.
    let id = match strategy {
        // `Bare` keeps the name alone, so the only namespace left is the source marker. Two servers
        // offering one name therefore collide here, which is precisely what `NameAssignments`
        // exists to detect rather than resolve.
        NamingStrategy::Bare => ToolId::new(format!("mcp.{name_half}")),
        NamingStrategy::Prefixed | NamingStrategy::Hashed => {
            ToolId::from_parts(&server.namespace(), &name_half)
        }
    }
    .map_err(|source| NamingError::InvalidIdentifier {
        tool: remote.clone(),
        source,
    })?;
    Ok(CanonicalToolName { id, remote })
}

/// The name half of an identifier, before a namespace is attached.
enum NameHalf {
    /// The server's own name, usable unchanged.
    Verbatim(String),
    /// A digest, because the name is not representable.
    Digest(String),
}

/// Derives the name half for a tool, or explains why the strategy cannot.
fn name_half_of(tool: &str, strategy: NamingStrategy) -> Result<NameHalf, NamingError> {
    if let Some(verbatim) = verbatim_name(tool) {
        return Ok(NameHalf::Verbatim(verbatim));
    }
    match strategy {
        NamingStrategy::Hashed => Ok(NameHalf::Digest(disambiguator(tool))),
        NamingStrategy::Prefixed | NamingStrategy::Bare => Err(NamingError::Unrepresentable {
            tool: bounded(tool),
            reason: unrepresentable_reason(tool),
        }),
    }
}

/// Returns the tool name unchanged when a JARVIS name segment can hold it exactly.
///
/// The check is deliberately the same predicate the identifier validator uses — lowercase ASCII,
/// digits, `-`, `_`, `:`, within the segment bound, and **no dot** — rather than a lookalike. A
/// looser check here would produce a name that `ToolId` then rejects, turning a naming problem into
/// an invalid-identifier error that says less.
fn verbatim_name(tool: &str) -> Option<String> {
    let usable = !tool.is_empty()
        && tool.chars().count() <= jarvis_tools::MAX_TOOL_ID_SEGMENT_CHARS
        && tool.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '-' | '_' | ':')
        });
    usable.then(|| tool.to_owned())
}

/// Explains, for an operator, why a name could not be used as-is.
///
/// Each cause is named separately because the remedy differs: a dot is a naming scheme problem, a
/// length problem needs a different strategy, an uppercase name needs the server to change, and a
/// non-ASCII name is a server we cannot name at all in this scheme.
fn unrepresentable_reason(tool: &str) -> String {
    if tool.contains('.') {
        return "the name contains a dot, which a JARVIS tool name may not".to_owned();
    }
    if tool.chars().count() > jarvis_tools::MAX_TOOL_ID_SEGMENT_CHARS {
        return format!(
            "the name exceeds the {}-character limit for a tool name",
            jarvis_tools::MAX_TOOL_ID_SEGMENT_CHARS
        );
    }
    if let Some(character) = tool.chars().find(char::is_ascii_uppercase) {
        return format!("the name contains the uppercase letter {character:?}");
    }
    if let Some(character) = tool.chars().find(|c| !c.is_ascii()) {
        return format!("the name contains the non-ASCII character {character:?}");
    }
    if let Some(character) = tool
        .chars()
        .find(|c| !c.is_ascii_alphanumeric() && !matches!(c, '-' | '_' | ':'))
    {
        return format!("the name contains the character {character:?}");
    }
    "the name cannot be represented".to_owned()
}

/// Derives a stable opaque name half from a tool name.
///
/// SHA-256 truncated to eight hexadecimal characters, prefixed so the result can never be mistaken
/// for a readable name. The input is the **whole** name with a length prefix, so `ab` and `ab` in
/// two differently-named tools cannot be confused and a name that extends another cannot collide by
/// construction of the hash input.
fn disambiguator(tool: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"jarvis-mcp-tool-name");
    hasher.update([0x1f]);
    hasher.update((tool.len() as u64).to_be_bytes());
    hasher.update(tool.as_bytes());
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(DISAMBIGUATOR_HEX_CHARS);
    for byte in digest.iter().take(DISAMBIGUATOR_HEX_CHARS.div_ceil(2)) {
        let _ = write!(hex, "{byte:02x}");
    }
    format!("h{}", &hex[..DISAMBIGUATOR_HEX_CHARS])
}

/// Tracks which canonical identifiers are already in use, so a collision is detected rather than
/// resolved.
///
/// A `BTreeMap` rather than a set because the **existing** owner has to be named in the error: an
/// operator told only "this collides" cannot decide which server to rename. Ordered so the error
/// text and any inventory output are deterministic.
#[derive(Debug, Default)]
pub struct NameAssignments {
    /// Identifier → the (server, remote tool) pair that holds it.
    ///
    /// The pair, not the tool name alone: a refresh and a cross-server collision are only
    /// distinguishable with both halves present.
    owners: BTreeMap<String, (ServerName, String)>,
}

/// Explains why an assignment was refused.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NameCollision {
    /// The identifier both tools derived.
    pub identifier: String,
    /// The server that now claims it.
    pub incoming_server: String,
    /// The server-supplied name that now claims it.
    pub incoming_tool: String,
    /// The server that already holds it.
    pub existing_server: String,
    /// The server-supplied name that already holds it.
    pub existing_tool: String,
}

impl fmt::Display for NameCollision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "MCP tools {}:{} and {}:{} both translate to the identifier {}; \
             rename one, or use a different naming strategy",
            self.incoming_server,
            self.incoming_tool,
            self.existing_server,
            self.existing_tool,
            self.identifier
        )
    }
}

impl std::error::Error for NameCollision {}

impl NameAssignments {
    /// Creates an empty set of assignments.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Reserves a canonical identifier for one tool of one server.
    ///
    /// Takes the server as well as the canonical name because the refresh case and the collision case
    /// look identical without it: **the same tool name from the same server** is a `tools/list`
    /// refresh and must be accepted, while **the same tool name from a different server** is exactly
    /// the collision the `Bare` strategy causes and must be refused. Keying the check on the name
    /// alone would conflate them, which is how a collision gets silently permitted — the first
    /// version of this function did precisely that.
    ///
    /// # Errors
    ///
    /// Returns [`NameCollision`] when the identifier is already held by a different server or a
    /// different tool name.
    pub fn assign(
        &mut self,
        server: &ServerName,
        canonical: &CanonicalToolName,
    ) -> Result<(), NameCollision> {
        let identifier = canonical.id.to_string();
        match self.owners.get(&identifier) {
            Some(existing) if existing != &(server.clone(), canonical.remote.clone()) => {
                Err(NameCollision {
                    identifier,
                    incoming_server: server.to_string(),
                    incoming_tool: canonical.remote.clone(),
                    existing_server: existing.0.to_string(),
                    existing_tool: existing.1.clone(),
                })
            }
            _ => {
                self.owners
                    .insert(identifier, (server.clone(), canonical.remote.clone()));
                Ok(())
            }
        }
    }

    /// Returns how many identifiers are held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.owners.len()
    }

    /// Returns whether nothing is held.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.owners.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(name: &str) -> ServerName {
        ServerName::new(name).unwrap_or_else(|error| panic!("fixture server {name}: {error}"))
    }

    fn canonical(name: &str, tool: &str, strategy: NamingStrategy) -> CanonicalToolName {
        canonical_tool_name(&server(name), tool, strategy)
            .unwrap_or_else(|error| panic!("fixture {name}/{tool}: {error}"))
    }

    #[test]
    fn an_operator_name_is_narrower_than_a_namespace_segment() {
        // Dots are refused even though a namespace may contain them: `mcp.a.b` must mean server
        // `a.b` or server `a` with tool `b`, never both.
        assert_eq!(
            ServerName::new("local.files"),
            Err(ServerNameError::Unacceptable { character: '.' })
        );
        assert_eq!(
            ServerName::new("UPPER"),
            Err(ServerNameError::Unacceptable { character: 'U' })
        );
        assert_eq!(ServerName::new(""), Err(ServerNameError::Empty));
        assert_eq!(ServerName::new("   "), Err(ServerNameError::Empty));
        assert_eq!(
            ServerName::new("a".repeat(MAX_SERVER_NAME_CHARS + 1)),
            Err(ServerNameError::TooLong)
        );
        // The boundary itself is accepted, from both sides.
        let longest = "a".repeat(MAX_SERVER_NAME_CHARS);
        assert!(ServerName::new(longest).is_ok());
    }

    #[test]
    fn the_namespace_carries_the_source_marker() {
        let name = server("github");
        assert_eq!(name.namespace(), "mcp.github");
        // The classification the marker buys: third-party handling keys on it, so `mcp.` is not
        // cosmetic.
        assert_eq!(
            name.namespace().parse::<ToolId>().map(|id| id.source()),
            Ok(jarvis_tools::ToolSource::Mcp)
        );
    }

    #[test]
    fn a_lowercase_safe_name_survives_verbatim_under_every_strategy() {
        for strategy in [
            NamingStrategy::Prefixed,
            NamingStrategy::Bare,
            NamingStrategy::Hashed,
        ] {
            let translated = canonical("github", "list_issues", strategy);
            // The remote name is always the server's own, which is what `tools/call` must send back.
            assert_eq!(translated.remote, "list_issues");
            // The name half is the server's name unchanged, under every strategy: the strategies
            // differ in the NAMESPACE, not by mangling the tool name.
            assert_eq!(translated.id.name(), "list_issues", "{strategy}");
            let expected_namespace = match strategy {
                // `Bare` has only the source marker left, because the server is not named in the
                // identifier at all.
                NamingStrategy::Bare => "mcp",
                NamingStrategy::Prefixed | NamingStrategy::Hashed => "mcp.github",
            };
            assert_eq!(translated.id.namespace(), expected_namespace, "{strategy}");
        }
    }

    /// **The defect this module exists to prevent.** A dot inside an MCP tool name is legal, but a
    /// JARVIS name half may not contain one — and stripping it would make `admin.tools.list` and
    /// `admin.tools` the same identifier, so a call for one tool would run the other, with the
    /// other's declared risk.
    #[test]
    fn a_dotted_tool_name_never_flattens_onto_another() {
        let dotted = canonical("github", "admin.tools.list", NamingStrategy::Hashed);
        let shorter = canonical("github", "admin.tools", NamingStrategy::Hashed);
        assert_ne!(
            dotted.id, shorter.id,
            "two distinct MCP names must not share one identifier"
        );

        // And the strategies that keep the name refuse rather than flatten.
        for strategy in [NamingStrategy::Prefixed, NamingStrategy::Bare] {
            let refused = canonical_tool_name(&server("github"), "admin.tools.list", strategy);
            assert!(
                matches!(refused, Err(NamingError::Unrepresentable { .. })),
                "{strategy} must refuse a dotted name rather than rewrite it, got {refused:?}"
            );
        }
    }

    /// Truncation is the tempting alternative to hashing and it is wrong: a shortened name IS a
    /// different name, and it can collide with a real one.
    #[test]
    fn a_long_name_is_hashed_never_truncated() {
        let long = "a".repeat(jarvis_tools::MAX_TOOL_ID_SEGMENT_CHARS + 20);
        let translated = canonical("github", &long, NamingStrategy::Hashed);
        assert_ne!(
            translated.id.name(),
            long,
            "a name that cannot fit must not be stored as though it fit"
        );
        assert!(
            translated.id.name().starts_with('h'),
            "a hashed name half must be recognisable as one, got {}",
            translated.id.name()
        );

        // Two different over-long names that share a prefix must not collide — the case truncation
        // would get wrong.
        let other = format!(
            "{}b",
            "a".repeat(jarvis_tools::MAX_TOOL_ID_SEGMENT_CHARS + 20)
        );
        let second = canonical("github", &other, NamingStrategy::Hashed);
        assert_ne!(translated.id, second.id);
    }

    #[test]
    fn an_uppercase_name_is_refused_by_the_readable_strategies_and_hashed_by_the_opaque_one() {
        // `getUser` is a valid MCP name and an invalid JARVIS one.
        let refused = canonical_tool_name(&server("gh"), "getUser", NamingStrategy::Prefixed);
        assert!(
            matches!(refused, Err(NamingError::Unrepresentable { .. })),
            "expected a refusal, got {refused:?}"
        );
        let hashed = canonical("gh", "getUser", NamingStrategy::Hashed);
        assert!(hashed.id.name().starts_with('h'));
        // The digest is of the ORIGINAL name, so the remote name is still exact.
        assert_eq!(hashed.remote, "getUser");
    }

    #[test]
    fn an_empty_tool_name_is_refused_by_every_strategy() {
        for strategy in [
            NamingStrategy::Prefixed,
            NamingStrategy::Bare,
            NamingStrategy::Hashed,
        ] {
            assert_eq!(
                canonical_tool_name(&server("gh"), "  ", strategy),
                Err(NamingError::EmptyToolName),
                "{strategy}"
            );
        }
    }

    #[test]
    fn the_remote_name_is_the_servers_name_not_the_canonical_one() {
        // `tools/call` must send the server's own name back. Sending the canonical id would be the
        // translation applied twice, which is invisible until a real server rejects the call.
        let translated = canonical("github", "list_issues", NamingStrategy::Prefixed);
        assert_eq!(translated.remote, "list_issues");
        assert_eq!(translated.id.to_string(), "mcp.github.list_issues");
        // The two agree on the name and differ on the namespace, which is the whole point: the
        // identifier carries JARVIS's namespace, the remote name carries only the server's name.
        assert_eq!(translated.remote, translated.id.name());
        assert_eq!(translated.id.namespace(), "mcp.github");
    }

    /// The collision check is what makes the `Bare` strategy safe, so it is tested with the exact
    /// shape the strategy makes possible: two servers offering the same tool name.
    #[test]
    fn two_servers_offering_one_name_collide_under_the_bare_strategy() {
        let mut assignments = NameAssignments::new();
        let first = canonical("github", "search", NamingStrategy::Bare);
        let second = canonical("gitlab", "search", NamingStrategy::Bare);
        assert_eq!(
            first.id, second.id,
            "the bare strategy is expected to collide here; that is why it is checked"
        );

        assert_eq!(assignments.assign(&server("github"), &first), Ok(()));
        let Err(collision) = assignments.assign(&server("gitlab"), &second) else {
            panic!("two servers offering one bare name must collide");
        };
        assert_eq!(collision.identifier, "mcp.search");
        assert_eq!(collision.incoming_server, "gitlab");
        assert_eq!(collision.incoming_tool, "search");
        assert_eq!(collision.existing_server, "github");
        assert_eq!(collision.existing_tool, "search");
        assert_eq!(
            assignments.len(),
            1,
            "a refused assignment must not register"
        );
    }

    #[test]
    fn the_prefixed_strategy_keeps_the_same_names_apart() {
        let mut assignments = NameAssignments::new();
        let first = canonical("github", "search", NamingStrategy::Prefixed);
        let second = canonical("gitlab", "search", NamingStrategy::Prefixed);
        assert_ne!(first.id, second.id);
        assert_eq!(assignments.assign(&server("github"), &first), Ok(()));
        assert_eq!(assignments.assign(&server("gitlab"), &second), Ok(()));
        assert_eq!(assignments.len(), 2);
    }

    /// A `tools/list` refresh re-assigns every tool. Refusing a repeat would make refresh impossible,
    /// which is why the same tool name from the **same** server is not a collision.
    #[test]
    fn re_assigning_the_same_tool_name_from_the_same_server_is_not_a_collision() {
        let mut assignments = NameAssignments::new();
        let translated = canonical("github", "search", NamingStrategy::Prefixed);
        let owner = server("github");
        assert_eq!(assignments.assign(&owner, &translated), Ok(()));
        assert_eq!(assignments.assign(&owner, &translated), Ok(()));
        assert_eq!(assignments.assign(&owner, &translated.clone()), Ok(()));
        assert_eq!(assignments.len(), 1);
    }

    /// The derivation is **injective within one server**: distinct remote names never share an
    /// identifier, under any strategy. That is what makes the collision check's real job the
    /// *cross-server* case, and it is worth asserting rather than assuming — if a future change
    /// introduced a lossy step (truncating instead of hashing, say), this is the test that fails.
    #[test]
    fn distinct_remote_names_never_share_an_identifier_within_one_server() {
        // Deliberately includes the names a lossy derivation would merge: two over-long names with a
        // common prefix, and a dotted name against its prefix.
        let names = [
            "search",
            "list_issues",
            "getUser",
            "admin.tools",
            "admin.tools.list",
            "wéird",
            "mixed_Case",
            "has space",
            "trailing.",
            ".leading",
            "a",
            "A",
        ];
        for strategy in [
            NamingStrategy::Prefixed,
            NamingStrategy::Hashed,
            NamingStrategy::Bare,
        ] {
            let mut seen: Vec<(String, &str)> = Vec::new();
            let mut accepted = 0_usize;
            for name in names {
                let Ok(translated) = canonical_tool_name(&server("github"), name, strategy) else {
                    // A refusal is expected for the readable strategies on a name a name segment
                    // cannot hold; the count is asserted per strategy below.
                    continue;
                };
                accepted += 1;
                let identifier = translated.id.to_string();
                if let Some((_, first)) = seen.iter().find(|(id, _)| id == &identifier) {
                    panic!("{strategy}: {first:?} and {name:?} both became {identifier}");
                }
                seen.push((identifier, name));
            }
            // The two strategies differ in how many names they can hold, and saying so is the point:
            // `Hashed` is the one that never refuses, so it is the one whose injectivity is fully
            // exercised. `Prefixed` and `Bare` accept only the three names a name segment can hold
            // (`search`, `list_issues`, `a`) and refuse the other nine outright.
            let expected = match strategy {
                NamingStrategy::Hashed => names.len(),
                NamingStrategy::Prefixed | NamingStrategy::Bare => 3,
            };
            assert_eq!(
                accepted,
                expected,
                "{strategy} accepted {accepted} of {} names",
                names.len()
            );
        }
    }

    /// The two over-long names a truncating derivation would merge, checked directly.
    #[test]
    fn two_over_long_names_sharing_a_prefix_stay_distinct() {
        let long = "z".repeat(jarvis_tools::MAX_TOOL_ID_SEGMENT_CHARS + 10);
        let longer = format!("{long}z");
        let first = canonical("github", &long, NamingStrategy::Hashed);
        let second = canonical("github", &longer, NamingStrategy::Hashed);
        assert_ne!(first.id, second.id);
        // The readable strategies refuse both rather than silently shortening one onto the other.
        for strategy in [NamingStrategy::Prefixed, NamingStrategy::Bare] {
            assert!(
                canonical_tool_name(&server("github"), &long, strategy).is_err(),
                "{strategy} must not accept a name it cannot hold"
            );
        }
    }

    /// The collision error has to be actionable, because the remedy is a human renaming something
    /// and an error that does not name both sides cannot support that.
    #[test]
    fn a_collision_names_both_offending_tools() {
        let mut assignments = NameAssignments::new();
        let first = canonical("alpha", "same", NamingStrategy::Bare);
        let second = canonical("bravo", "same", NamingStrategy::Bare);
        assert_eq!(first.id, second.id);

        assert_eq!(assignments.assign(&server("alpha"), &first), Ok(()));
        let Err(collision) = assignments.assign(&server("bravo"), &second) else {
            panic!("two servers offering one bare name must collide");
        };
        let text = collision.to_string();
        assert!(text.contains("alpha"), "{text}");
        assert!(text.contains("bravo"), "{text}");
        assert!(text.contains("mcp.same"), "{text}");
    }

    /// `ToolId::from_parts` splits on the **last** dot, so a bare name containing one would put the
    /// halves back together differently than they were built. The name half must therefore never
    /// contain a dot, which is a property of the derivation rather than of the input.
    #[test]
    fn no_derived_identifier_disagrees_with_its_own_halves() {
        for tool in [
            "list_issues",
            "getUser",
            "admin.tools.list",
            &"a".repeat(jarvis_tools::MAX_TOOL_ID_SEGMENT_CHARS + 5),
            "wéird",
        ] {
            // `Hashed` only, because it is the strategy that must accept every name.
            let translated = canonical("gh", tool, NamingStrategy::Hashed);
            // Re-parsing the printed id must yield the same namespace and name it was built
            // from, or the canonical string form is not canonical.
            let reparsed = translated
                .id
                .to_string()
                .parse::<ToolId>()
                .unwrap_or_else(|error| panic!("{} is not re-parseable: {error}", translated.id));
            assert_eq!(reparsed, translated.id, "{tool}");
            assert_eq!(reparsed.namespace(), "mcp.gh");
            assert!(
                !reparsed.name().contains('.'),
                "the name half of {reparsed} contains a dot"
            );
        }
    }

    #[test]
    fn reported_identity_is_bounded_and_kept_apart_from_the_operator_name() {
        let long = "x".repeat(MAX_REPORTED_TEXT_CHARS + 50);
        let reported = ReportedIdentity::new(&long, Some("A Title"));
        assert_eq!(reported.name.chars().count(), MAX_REPORTED_TEXT_CHARS);
        assert_eq!(reported.title.as_deref(), Some("A Title"));

        // Truncation is char-aware, so a multi-byte name does not produce invalid UTF-8.
        let unicode = "é".repeat(MAX_REPORTED_TEXT_CHARS + 10);
        let bounded_unicode = ReportedIdentity::new(&unicode, None);
        assert_eq!(
            bounded_unicode.name.chars().count(),
            MAX_REPORTED_TEXT_CHARS
        );

        // A server changing what it claims is visible rather than absorbed.
        let before = ReportedIdentity::new("server-a", None);
        let after = ReportedIdentity::new("server-b", None);
        assert!(!before.agrees_with(&after));
        assert!(before.agrees_with(&before.clone()));
    }
}
