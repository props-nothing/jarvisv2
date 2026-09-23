//! How JARVIS exposes itself as an MCP **server**: the trust decisions a dependency's defaults
//! cannot make.
//!
//! # Why this is JARVIS's own type rather than the SDK's configuration
//!
//! `P3-009` makes JARVIS an MCP server. The SDK provides the transport, and its server configuration
//! has a `Default` — but **that default is permissive on exactly the obligations the specification
//! states as MUSTs.** Read from `rmcp-3.4.0`'s
//! `transport/streamable_http_server/tower.rs`:
//!
//! | SDK default | Spec obligation it leaves open |
//! | --- | --- |
//! | `allowed_origins: vec![]`, `validate_empty_origin_allowlist: false` | `validate_origin_header` returns `Ok(())` **immediately**, so `Origin` is never checked. The SDK's own doc comment says it "disables Origin validation for backward compatibility". |
//! | `legacy_session_mode: true` | Mints an `Mcp-Session-Id`, which `2026-07-28` (SEP-2567) removed. |
//! | `stateless_protocol_metadata_required: false` | The SDK's comment states it "preserv[es] today's legacy behavior where an absent header is treated as protocol version `2025-03-26`". |
//!
//! A default is the one value that changes without a line in this repository changing. So this module
//! states the policy as a **value built from an operator's configuration**, and the daemon maps it onto
//! the SDK's fields explicitly. The SDK is not uniformly permissive — `allowed_hosts` defaults to
//! loopback only and the `-32020` check is unconditional — which is precisely why inheriting is
//! dangerous: a permissive field and a strict field look identical at the call site.
//!
//! # Why the origin matcher is ours, and not the SDK's
//!
//! This is the finding that decided the module. The SDK's doc comment says an `Origin` "must match per
//! RFC 6454 `(scheme, host, port)`". Its implementation in `origin_is_allowed` is:
//!
//! ```text
//! a_scheme == o_scheme && a_host == o_host && (a_port.is_none() || a_port == o_port)
//! ```
//!
//! The `a_port.is_none()` arm means **a portless allowlist entry is a wildcard over every port, not a
//! request for the scheme's default port.** That matters because browsers omit the port for a default
//! one: a page on `https://jarvis.example.com` sends `Origin: https://jarvis.example.com`, whose parsed
//! port is `None`. So the SDK gives an operator no way to express the ordinary intent:
//!
//! - `https://jarvis.example.com` → matches **the default port and every other port** — far broader
//!   than it reads, so a service on `https://jarvis.example.com:8443` is admitted by an entry that
//!   looks like it names one origin;
//! - `https://jarvis.example.com:443` → matches only a literal `:443`, and **false-rejects the normal
//!   browser traffic** above.
//!
//! A control whose narrowest setting is a wildcard and whose exact setting is wrong is not a control, so
//! JARVIS owns the comparison. This module is a function of its arguments — no sockets, no SDK — so the
//! rule an operator is relying on can be read and tested without a peer standing. That is the same
//! reason `jarvis-mcp` exists at all.
//!
//! # Why the allowlist is not the only control
//!
//! `Origin` is a **browser** control. A non-browser client sends whatever it likes, and a client that
//! omits the header entirely is admitted by the specification's own rule ("if the `Origin` header is
//! present and invalid"). So the allowlist is not an authentication boundary, and this module does not
//! present it as one: [`ServerExposure::is_loopback_only`] is the boundary, and it is a refusal to be
//! reachable off-host at all. A remote MCP server of our own needs audience-bound tokens (RFC 9728
//! metadata and RFC 8707) that are a separate slice, and **a network-reachable JARVIS MCP server
//! without them would be an unauthenticated control plane.**

use std::fmt;

/// The longest an `Origin` allowlist entry may be.
///
/// An origin is a scheme, a host, and an optional port: `http://` (7) + a 253-character host + `:` plus
/// five digits is 266. The bound is generous rather than tight because its purpose is to stop a
/// configuration file from being an unbounded parse input, not to police a legal origin.
pub const MAX_ORIGIN_ENTRY_BYTES: usize = 512;

/// The most origins one server may allow.
///
/// The same reasoning as [`crate::MAX_MCP_SERVERS`]: a bound so a configuration file cannot make the
/// per-request check unbounded. Sixteen is far above any real deployment of a single-user assistant.
pub const MAX_ALLOWED_ORIGINS: usize = 16;

/// A browser origin JARVIS is willing to serve, compared as RFC 6454's `(scheme, host, port)`.
///
/// # Why this is a parsed value and not a string
///
/// The comparison has to decide what a missing port means, and a string comparison cannot: `https://x`
/// and `https://x:443` denote **one** origin, and a `contains` or prefix test would additionally admit
/// `https://x.evil.example`. Parsing once, at construction, makes "these two entries are the same
/// origin" a property of the value rather than a rule re-derived at each request.
///
/// # Why equality is implemented rather than derived
///
/// `PartialEq`, `Eq`, and `Ord` are **hand-written to compare the effective port**, because that is what
/// this type means by equal. Deriving them compares `port` as it was written, under which
/// `https://x` and `https://x:443` are unequal — and the duplicate check in [`ServerExposure::new`]
/// accordingly accepted both. That was a real defect found by
/// `the_same_origin_under_two_spellings_is_refused_as_a_duplicate` rather than by inspection: the
/// operator's policy then held one origin twice while the file read as two entries, and the
/// configuration would look broader than it was.
#[derive(Clone, Debug)]
pub struct AllowedOrigin {
    scheme: String,
    host: String,
    /// The port **as written**, so a refusal can show the operator which spelling collided.
    port: Option<u16>,
}

impl PartialEq for AllowedOrigin {
    fn eq(&self, other: &Self) -> bool {
        self.matches(other)
    }
}

impl Eq for AllowedOrigin {}

impl Ord for AllowedOrigin {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (&self.scheme, &self.host, self.effective_port()).cmp(&(
            &other.scheme,
            &other.host,
            other.effective_port(),
        ))
    }
}

impl PartialOrd for AllowedOrigin {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl AllowedOrigin {
    /// Parses one allowlist entry.
    ///
    /// # Errors
    ///
    /// Returns [`OriginError`] for an entry that is not a usable origin. Every failure is a
    /// configuration fault, reported when the configuration is read rather than when the first request
    /// arrives.
    pub fn parse(entry: &str) -> Result<Self, OriginError> {
        if entry.len() > MAX_ORIGIN_ENTRY_BYTES {
            return Err(OriginError::TooLong);
        }
        // Outer whitespace is trimmed because an entry pasted from a browser address bar routinely
        // carries it; interior whitespace is refused because it means the entry is two things.
        let entry = entry.trim();
        if entry.is_empty() {
            return Err(OriginError::Empty);
        }
        if entry.chars().any(char::is_whitespace) {
            return Err(OriginError::EmbeddedWhitespace);
        }
        // `"null"` is what a browser sends from a sandboxed frame or a `file://` page. It is a legal
        // `Origin` value and it is deliberately **unrepresentable here**: it names no origin an operator
        // could have intended, so admitting it would be admitting every opaque origin at once.
        if entry.eq_ignore_ascii_case("null") {
            return Err(OriginError::OpaqueIsNotAnOrigin);
        }

        let Some((scheme, rest)) = entry.split_once("://") else {
            // A bare host is the mistake an operator makes by writing what they typed in a browser,
            // where the scheme is implied. Refused rather than defaulted to `https`, because guessing
            // would silently admit a scheme the operator did not name.
            return Err(OriginError::MissingScheme);
        };
        let scheme = scheme.to_ascii_lowercase();
        // Only the two schemes a browser origin can carry for an MCP endpoint. `ws`, `file`, and
        // `chrome-extension` are all real origins and none of them is a thing to serve.
        if scheme != "http" && scheme != "https" {
            return Err(OriginError::UnsupportedScheme { scheme });
        }
        if rest.contains(['/', '?', '#']) {
            // An origin has no path, query, or fragment. A trailing slash is the most common form of
            // this, and it is not harmless: it reads as a directory and would make `https://x/` and
            // `https://x` two entries for one origin.
            return Err(OriginError::NotAnOrigin {
                reason: "an origin has no path, query, or fragment".to_owned(),
            });
        }

        let (host, port) = split_host_port(rest)?;
        if host.is_empty() {
            return Err(OriginError::NotAnOrigin {
                reason: "no host".to_owned(),
            });
        }
        Ok(Self { scheme, host, port })
    }

    /// Returns the scheme, lowercased.
    #[must_use]
    pub fn scheme(&self) -> &str {
        &self.scheme
    }

    /// Returns the host, lowercased.
    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
    }

    /// Returns the port exactly as it was written, if one was.
    ///
    /// Deliberately **not** defaulted to the scheme's well-known port. Normalizing here would erase the
    /// very distinction this type exists to keep, and [`Self::matches`] is where the implication is
    /// applied once.
    #[must_use]
    pub const fn port(&self) -> Option<u16> {
        self.port
    }

    /// Returns whether an incoming `Origin` header value names this origin.
    ///
    /// **A default port and an absent port are the same origin, and no other port is.** `https://x` and
    /// `https://x:443` both match a browser's `Origin: https://x`; `https://x:8443` does not. This is the
    /// rule the SDK's implementation gets wrong in both directions, and stating it here is the point of
    /// the module.
    ///
    /// The comparison applies the *well-known* port for the origin's own scheme rather than
    /// `Any`, so an entry for `https://x` cannot be satisfied by `http://x`.
    #[must_use]
    pub fn matches(&self, incoming: &Self) -> bool {
        if self.scheme != incoming.scheme {
            return false;
        }
        if self.host != incoming.host {
            return false;
        }
        // Each side's absent port becomes its scheme's default before comparison, so
        // `https://x:443` and `https://x` agree and `https://x:8443` does not.
        self.effective_port() == incoming.effective_port()
    }

    /// Returns the port with the scheme's well-known value applied.
    fn effective_port(&self) -> u16 {
        self.port.unwrap_or_else(|| default_port_for(&self.scheme))
    }
}

impl fmt::Display for AllowedOrigin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.port {
            Some(port) => write!(formatter, "{}://{}:{}", self.scheme, self.host, port),
            None => write!(formatter, "{}://{}", self.scheme, self.host),
        }
    }
}

/// Returns the well-known port for a scheme this type admits.
///
/// `http`/`https` are the only schemes [`AllowedOrigin::parse`] accepts, so the fallback cannot be
/// reached from a parsed value. It returns `0` rather than panicking, because a panic in a comparison
/// used to decide whether to serve a request would be a worse outcome than a comparison that cannot
/// match — and `0` is not a port any origin carries.
const fn default_port_for(scheme: &str) -> u16 {
    if scheme.eq_ignore_ascii_case("http") {
        80
    } else {
        443
    }
}

/// Splits `host` or `host:port`, refusing anything that is not exactly one of those.
fn split_host_port(rest: &str) -> Result<(String, Option<u16>), OriginError> {
    // A bracketed IPv6 literal carries colons inside the brackets, so the port separator is the last
    // colon **after** the closing bracket rather than the last colon overall.
    let (host, port_text) = if let Some(close) = rest.find(']') {
        if !rest.starts_with('[') {
            return Err(OriginError::NotAnOrigin {
                reason: "a closing bracket without an opening one".to_owned(),
            });
        }
        match rest[close..].split_once(':') {
            Some((_, port)) => (&rest[..=close], Some(port)),
            None => (rest, None),
        }
    } else {
        match rest.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (rest, None),
        }
    };

    let host = host.to_ascii_lowercase();
    let Some(port_text) = port_text else {
        return Ok((host, None));
    };
    if port_text.is_empty() {
        return Err(OriginError::NotAnOrigin {
            reason: "a colon with no port after it".to_owned(),
        });
    }
    let port = port_text
        .parse::<u16>()
        .map_err(|_| OriginError::NotAnOrigin {
            reason: "the port is not a number in 0..65535".to_owned(),
        })?;
    Ok((host, Some(port)))
}

/// Explains why an origin entry was refused.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OriginError {
    /// The entry was empty or only whitespace.
    Empty,
    /// The entry exceeded [`MAX_ORIGIN_ENTRY_BYTES`].
    TooLong,
    /// The entry contained whitespace between its parts.
    EmbeddedWhitespace,
    /// The entry had no `scheme://` prefix.
    MissingScheme,
    /// The scheme was not `http` or `https`.
    UnsupportedScheme {
        /// The scheme that was written.
        scheme: String,
    },
    /// The entry was not a bare origin.
    NotAnOrigin {
        /// Why it was refused.
        reason: String,
    },
    /// The entry was the opaque origin `null`.
    ///
    /// A browser sends this from a sandboxed frame or a `file://` page. It names no origin, so an
    /// allowlist entry for it would admit every opaque origin at once.
    OpaqueIsNotAnOrigin,
}

impl fmt::Display for OriginError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(formatter, "an allowed origin is empty"),
            Self::TooLong => write!(
                formatter,
                "an allowed origin is longer than {MAX_ORIGIN_ENTRY_BYTES} bytes"
            ),
            Self::EmbeddedWhitespace => write!(
                formatter,
                "an allowed origin contains whitespace, so it names more than one value"
            ),
            Self::MissingScheme => write!(
                formatter,
                "an allowed origin has no scheme; write it as it appears in a browser's address bar, \
                 including https://"
            ),
            Self::UnsupportedScheme { scheme } => write!(
                formatter,
                "the origin scheme {scheme} is not served; only http and https are browser origins for \
                 an MCP endpoint"
            ),
            Self::NotAnOrigin { reason } => {
                write!(formatter, "the allowed origin is not an origin: {reason}")
            }
            Self::OpaqueIsNotAnOrigin => write!(
                formatter,
                "the opaque origin \"null\" cannot be allowed, because it names no origin and would \
                 admit every sandboxed frame"
            ),
        }
    }
}

impl std::error::Error for OriginError {}

/// Explains why a client-supplied `Origin` header was refused.
///
/// Separate from [`OriginError`] because the two are **different audiences**: a configuration mistake is
/// an operator's to fix, and a refused `Origin` is a client's request being denied. Conflating them would
/// put an operator's remedy into a `403` body sent to a stranger.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OriginVerdict {
    /// The header was absent, which the specification admits.
    ///
    /// Recorded as its own variant rather than folded into "allowed", because its justification is
    /// different: the client is not a browser making a cross-origin request, so there is no browser
    /// origin to check. It is also the reason `Origin` is not an authentication boundary.
    Absent,
    /// The header named a configured origin.
    Allowed,
    /// The header was present but named no configured origin.
    NotAllowed,
    /// The header was present and is not a parseable origin.
    ///
    /// A refusal rather than a fall-through to [`Self::NotAllowed`]: the specification says a present and
    /// *invalid* `Origin` **MUST** be refused, and the two are worth distinguishing in a log because a
    /// malformed header usually means a broken client rather than an attack.
    Malformed,
    /// The header was the opaque origin `null`.
    Opaque,
}

impl OriginVerdict {
    /// Returns whether the request may be served.
    #[must_use]
    pub const fn permits(self) -> bool {
        matches!(self, Self::Absent | Self::Allowed)
    }

    /// Returns the HTTP status this verdict obliges, when it refuses.
    ///
    /// `403` for every refusal, which is what the specification requires for an invalid `Origin`. The
    /// distinction between the variants is for the log and the audit record, not for the wire: telling a
    /// hostile caller *why* their origin was refused would help them enumerate the allowlist.
    #[must_use]
    pub const fn refusal_status(self) -> Option<u16> {
        if self.permits() { None } else { Some(403) }
    }
}

/// The server-exposure policy: what JARVIS will serve, and to which browser origins.
///
/// Constructed from an operator's configuration. [`Self::loopback_only`] is the default in the sense
/// that a deployment which says nothing about exposure gets the shape that is not reachable off-host,
/// which is what `P3-009` implements.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServerExposure {
    allowed_origins: Vec<AllowedOrigin>,
}

impl ServerExposure {
    /// Builds a policy from an operator's origin allowlist.
    ///
    /// # Errors
    ///
    /// Returns [`OriginError`] for the first entry that is not a usable origin, and
    /// [`ExposureError::TooManyOrigins`] when the list exceeds [`MAX_ALLOWED_ORIGINS`]. A bad entry is
    /// refused rather than skipped: dropping it would leave the operator believing an origin they wrote
    /// is allowed when the server has quietly decided otherwise.
    pub fn new<'a>(origins: impl IntoIterator<Item = &'a str>) -> Result<Self, ExposureError> {
        let mut allowed: Vec<AllowedOrigin> = Vec::new();
        for entry in origins {
            if allowed.len() >= MAX_ALLOWED_ORIGINS {
                return Err(ExposureError::TooManyOrigins);
            }
            let origin = AllowedOrigin::parse(entry).map_err(ExposureError::Origin)?;
            // A duplicate is refused rather than tolerated, because `https://x` and `https://x:443` are
            // **one** origin written twice. Accepting both would make the configuration look broader
            // than it is and would hide that the operator wrote the same thing under two spellings.
            if allowed.contains(&origin) {
                return Err(ExposureError::DuplicateOrigin {
                    origin: origin.to_string(),
                });
            }
            allowed.push(origin);
        }
        // Sorted so two policies built from the same origins in a different order compare equal, and a
        // stored policy is reproducible. The same reasoning as the catalog's sorted entries.
        allowed.sort();
        Ok(Self {
            allowed_origins: allowed,
        })
    }

    /// Builds the policy a deployment gets when it says nothing: reachable only from this host, and from
    /// a browser only on its own origin.
    ///
    /// The allowlist is **empty and enforced**, which is the opposite of the SDK's default and the whole
    /// reason this type exists. An empty *and enforced* allowlist means every present `Origin` is refused,
    /// so the only admitted callers are those that send no `Origin` at all — which is what a local tool
    /// does, and is a shape that admits no web page.
    #[must_use]
    pub const fn loopback_only() -> Self {
        Self {
            allowed_origins: Vec::new(),
        }
    }

    /// Returns whether no browser origin is admitted.
    ///
    /// The **stronger** of the two states, and therefore named: an empty allowlist that is *enforced*
    /// refuses every present `Origin`, while an empty allowlist that is *not* enforced admits every one.
    /// The SDK's default is the second, so a caller reading only "the list is empty" would reach the
    /// wrong conclusion about which it has.
    #[must_use]
    pub fn is_loopback_only(&self) -> bool {
        self.allowed_origins.is_empty()
    }

    /// Returns the origins in a stable order.
    #[must_use]
    pub fn allowed_origins(&self) -> &[AllowedOrigin] {
        &self.allowed_origins
    }

    /// Returns whether any configured origin requires a **remote** (non-loopback) bind.
    ///
    /// `P3-009` refuses a remote bind outright, because a remotely reachable JARVIS MCP server without
    /// audience-bound tokens would be an unauthenticated control plane. So a configuration that
    /// configures a remote origin and a remote bind is refused at startup rather than served, and this
    /// accessor is how that refusal is decided rather than guessed.
    ///
    /// A loopback origin (`http://localhost:3000`) does **not** require a remote bind: a page served
    /// from this host is not a remote caller, which is why the test is on the host rather than on the
    /// presence of an entry.
    #[must_use]
    pub fn requires_remote_bind(&self) -> bool {
        self.allowed_origins
            .iter()
            .any(|origin| !is_loopback_host(origin.host()))
    }

    /// Decides one request's `Origin` header against this policy.
    ///
    /// `incoming` is the raw header value, or `None` when the header was absent.
    #[must_use]
    pub fn decide(&self, incoming: Option<&str>) -> OriginVerdict {
        let Some(raw) = incoming else {
            return OriginVerdict::Absent;
        };
        // The specification's rule is about a present and *invalid* value, so a value this parser cannot
        // read is refused rather than treated as absent. Treating it as absent would invert the control:
        // an attacker's malformed header would be the shape that is admitted.
        let trimmed = raw.trim();
        if trimmed.eq_ignore_ascii_case("null") {
            return OriginVerdict::Opaque;
        }
        let Ok(parsed) = AllowedOrigin::parse(trimmed) else {
            return OriginVerdict::Malformed;
        };
        if self
            .allowed_origins
            .iter()
            .any(|origin| origin.matches(&parsed))
        {
            OriginVerdict::Allowed
        } else {
            OriginVerdict::NotAllowed
        }
    }
}

/// Returns whether a host is a loopback name or address.
///
/// Whole-host comparison, never a prefix test: `127.0.0.1.evil.example` and `localhost.attacker.test`
/// are ordinary remote names, and a `starts_with` would admit both. The same rule
/// `McpHttpEndpoint::is_loopback` applies on the client side, stated here for the host side of the same
/// boundary.
#[must_use]
pub fn is_loopback_host(host: &str) -> bool {
    let host = host.trim_matches(['[', ']']);
    matches!(host, "localhost" | "127.0.0.1" | "::1" | "0:0:0:0:0:0:0:1")
}

/// Explains why an exposure policy could not be built.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExposureError {
    /// An origin entry was not usable.
    Origin(OriginError),
    /// Two entries named the same origin under different spellings.
    DuplicateOrigin {
        /// The origin, rendered canonically so the message shows which two entries collided.
        origin: String,
    },
    /// The allowlist exceeded [`MAX_ALLOWED_ORIGINS`].
    TooManyOrigins,
}

impl fmt::Display for ExposureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Origin(error) => write!(formatter, "{error}"),
            Self::DuplicateOrigin { origin } => write!(
                formatter,
                "{origin} is listed twice; a default port and an omitted port are the same origin"
            ),
            Self::TooManyOrigins => write!(
                formatter,
                "more than {MAX_ALLOWED_ORIGINS} origins are configured"
            ),
        }
    }
}

impl std::error::Error for ExposureError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Origin(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origin(entry: &str) -> AllowedOrigin {
        AllowedOrigin::parse(entry).unwrap_or_else(|error| panic!("{entry}: {error}"))
    }

    /// **The finding this module exists for.** A browser omits the port when it is the scheme's
    /// default, so an operator writing `https://jarvis.example.com` must match a real
    /// `Origin: https://jarvis.example.com`.
    ///
    /// Falsified by requiring port equality without the default-port implication
    /// (`self.port == incoming.port`): this test fails, because the parsed incoming origin's port is
    /// `None` while the rule would demand `443`. That is the SDK's `a_port.is_none() || …` arm inverted,
    /// and it is why the matcher is not the SDK's.
    #[test]
    fn an_omitted_port_and_the_schemes_default_port_are_one_origin() {
        assert!(
            origin("https://jarvis.example.com").matches(&origin("https://jarvis.example.com"))
        );
        assert!(
            origin("https://jarvis.example.com:443").matches(&origin("https://jarvis.example.com"))
        );
        assert!(
            origin("https://jarvis.example.com").matches(&origin("https://jarvis.example.com:443"))
        );
    }

    /// **The other half, and the one that makes the rule a control rather than a wildcard.** An entry
    /// that names the default origin must not admit a different port.
    ///
    /// Falsified by the SDK's actual rule (`a_port.is_none() || a_port == o_port`): the first assertion
    /// below fails, because a portless allowlist entry would match *any* port. That is the defect stated
    /// as evidence — the narrowest-looking entry would be the broadest.
    #[test]
    fn a_default_port_entry_does_not_admit_another_port() {
        assert!(
            !origin("https://jarvis.example.com")
                .matches(&origin("https://jarvis.example.com:8443"))
        );
        assert!(
            !origin("https://jarvis.example.com:443")
                .matches(&origin("https://jarvis.example.com:8443"))
        );
        // And an explicitly named non-default port matches only itself.
        assert!(
            origin("https://jarvis.example.com:8443")
                .matches(&origin("https://jarvis.example.com:8443"))
        );
    }

    /// The scheme is part of the comparison, so a default port cannot bridge two schemes.
    ///
    /// `http` and `https` have different well-known ports, so this also guards against a matcher that
    /// compared only ports.
    #[test]
    fn a_different_scheme_is_never_the_same_origin() {
        assert!(
            !origin("https://jarvis.example.com").matches(&origin("http://jarvis.example.com"))
        );
        assert!(
            !origin("https://jarvis.example.com:443")
                .matches(&origin("http://jarvis.example.com:443"))
        );
    }

    /// A host comparison must be whole-host, never a prefix or suffix test.
    ///
    /// Falsified by replacing the equality with `ends_with` or `contains`: every one of these asserts
    /// fails, and each names a real attack shape (`jarvis.example.com.evil.test` is the classic
    /// suffix attack, `evil-jarvis.example.com` the prefix one).
    #[test]
    fn a_lookalike_host_is_not_the_allowed_host() {
        let allowed = origin("https://jarvis.example.com");
        for hostile in [
            "https://jarvis.example.com.evil.test",
            "https://evil-jarvis.example.com",
            "https://notjarvis.example.com",
            "https://jarvis.example.co",
        ] {
            assert!(
                !allowed.matches(&origin(hostile)),
                "{hostile} must not satisfy {allowed}"
            );
        }
    }

    /// An `Origin` is a tuple, not a URL: a path, query, or fragment means the value is something else.
    ///
    /// A trailing slash is the common form of this and is not harmless — it would make `https://x/` and
    /// `https://x` two spellings of one origin, which is exactly what the duplicate check refuses.
    #[test]
    fn an_entry_with_a_path_is_refused_rather_than_trimmed() {
        assert!(matches!(
            AllowedOrigin::parse("https://jarvis.example.com/"),
            Err(OriginError::NotAnOrigin { .. })
        ));
        assert!(matches!(
            AllowedOrigin::parse("https://jarvis.example.com/mcp"),
            Err(OriginError::NotAnOrigin { .. })
        ));
        assert!(matches!(
            AllowedOrigin::parse("https://jarvis.example.com?a=b"),
            Err(OriginError::NotAnOrigin { .. })
        ));
    }

    /// A bare host is the mistake an operator makes by writing what they typed in a browser.
    ///
    /// Refused rather than defaulted to `https`, because guessing the scheme would admit a scheme the
    /// operator never named.
    #[test]
    fn an_entry_without_a_scheme_is_refused() {
        assert!(matches!(
            AllowedOrigin::parse("jarvis.example.com"),
            Err(OriginError::MissingScheme)
        ));
        assert!(matches!(
            AllowedOrigin::parse("localhost:3000"),
            Err(OriginError::MissingScheme)
        ));
    }

    /// The opaque origin names nothing, so it cannot be an entry.
    ///
    /// A browser sends `Origin: null` from a sandboxed frame or a `file://` page. Admitting it would
    /// admit **every** opaque origin at once, which is why it is refused at both ends: as a
    /// configuration entry and as a verdict of its own rather than a generic refusal.
    #[test]
    fn the_opaque_origin_cannot_be_allowed_and_is_refused_as_a_verdict() {
        assert!(matches!(
            AllowedOrigin::parse("null"),
            Err(OriginError::OpaqueIsNotAnOrigin)
        ));
        assert!(matches!(
            AllowedOrigin::parse("NULL"),
            Err(OriginError::OpaqueIsNotAnOrigin)
        ));
        let policy = ServerExposure::new(["https://jarvis.example.com"])
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(policy.decide(Some("null")), OriginVerdict::Opaque);
        assert!(!policy.decide(Some("null")).permits());
    }

    /// **An unparseable `Origin` is refused, not treated as absent.** Treating it as absent would invert
    /// the control, because the specification admits a missing header — so an attacker's malformed
    /// header would become the admitted shape.
    ///
    /// Falsified by folding `Malformed` into `Absent` in `decide`: the first assertion fails and a
    /// hostile request is served.
    #[test]
    fn a_malformed_origin_is_refused_rather_than_treated_as_absent() {
        let policy = ServerExposure::new(["https://jarvis.example.com"])
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            policy.decide(Some("not an origin")),
            OriginVerdict::Malformed
        );
        assert_eq!(
            policy.decide(Some("jarvis.example.com")),
            OriginVerdict::Malformed
        );
        assert!(!policy.decide(Some("not an origin")).permits());
        assert_eq!(
            policy.decide(Some("not an origin")).refusal_status(),
            Some(403)
        );
        // The positive control: absence really is admitted, so the test above is not passing because
        // everything is refused.
        assert_eq!(policy.decide(None), OriginVerdict::Absent);
        assert!(policy.decide(None).permits());
    }

    /// **`loopback_only` refuses every present origin and admits only an absent one.** This is the
    /// deployment that is not reachable from a web page at all.
    ///
    /// Falsified by making an empty allowlist mean "allow all" — the SDK's default — which fails the
    /// first assertion.
    #[test]
    fn the_default_policy_refuses_every_present_origin() {
        let policy = ServerExposure::loopback_only();
        assert!(policy.is_loopback_only());
        assert_eq!(policy.decide(None), OriginVerdict::Absent);
        for present in [
            "https://jarvis.example.com",
            "http://localhost:3000",
            "null",
            "garbage",
        ] {
            assert!(
                !policy.decide(Some(present)).permits(),
                "{present} must be refused by the loopback-only policy"
            );
        }
    }

    /// Two spellings of one origin are one origin, so the second is refused.
    ///
    /// Falsified by comparing raw strings instead of parsed values: `https://x` and `https://x:443` then
    /// look distinct and both are accepted, and the policy silently holds the same origin twice while
    /// reading as two entries.
    #[test]
    fn the_same_origin_under_two_spellings_is_refused_as_a_duplicate() {
        assert!(matches!(
            ServerExposure::new([
                "https://jarvis.example.com",
                "https://jarvis.example.com:443"
            ]),
            Err(ExposureError::DuplicateOrigin { .. })
        ));
        // Case is also not a distinction: hostnames are case-insensitive.
        assert!(matches!(
            ServerExposure::new(["https://jarvis.example.com", "https://JARVIS.EXAMPLE.COM"]),
            Err(ExposureError::DuplicateOrigin { .. })
        ));
    }

    /// A bad entry fails the whole configuration rather than being skipped.
    ///
    /// Skipping would leave an operator believing an origin they wrote is allowed when the server has
    /// quietly decided otherwise — the silent-narrowing failure ADR-0020 refuses for a grant.
    #[test]
    fn one_bad_entry_refuses_the_whole_allowlist() {
        assert!(matches!(
            ServerExposure::new(["https://good.example.com", "jarvis.example.com"]),
            Err(ExposureError::Origin(OriginError::MissingScheme))
        ));
    }

    /// The bound is enforced, so a configuration file cannot make the per-request check unbounded.
    #[test]
    fn more_origins_than_the_bound_are_refused() {
        let entries: Vec<String> = (0..=MAX_ALLOWED_ORIGINS)
            .map(|index| format!("https://host{index}.example.com"))
            .collect();
        let borrowed: Vec<&str> = entries.iter().map(String::as_str).collect();
        assert!(matches!(
            ServerExposure::new(borrowed),
            Err(ExposureError::TooManyOrigins)
        ));
    }

    /// Order is not part of a policy's identity, so two of the same list compare equal.
    #[test]
    fn the_order_of_the_allowlist_does_not_change_the_policy() {
        let first = ServerExposure::new(["https://b.example.com", "https://a.example.com"])
            .unwrap_or_else(|error| panic!("{error}"));
        let second = ServerExposure::new(["https://a.example.com", "https://b.example.com"])
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(first, second);
    }

    /// **An allowlist entry for a remote origin requires a remote bind, and a loopback one does not.**
    /// This is what `P3-009` uses to refuse a configuration that would expose JARVIS off-host without
    /// audience-bound tokens.
    ///
    /// Falsified by testing for the mere presence of an entry rather than its host: a
    /// `http://localhost:3000` entry would then be reported as requiring a remote bind.
    #[test]
    fn only_a_remote_origin_requires_a_remote_bind() {
        let local = ServerExposure::new(["http://localhost:3000", "http://127.0.0.1:8080"])
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(!local.requires_remote_bind());

        let remote = ServerExposure::new(["https://jarvis.example.com"])
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(remote.requires_remote_bind());

        // The strongest state is not "no origins" alone but an enforced empty list, which is why the
        // accessor is named `is_loopback_only` rather than derived from `is_empty`.
        assert!(!ServerExposure::loopback_only().requires_remote_bind());
    }

    /// A loopback **name** is compared whole, so a name that merely starts with one is not loopback.
    ///
    /// Falsified by a `starts_with` test: every hostile name here would be classified local.
    #[test]
    fn a_lookalike_loopback_name_is_not_loopback() {
        for local in ["localhost", "127.0.0.1", "::1", "[::1]"] {
            assert!(is_loopback_host(local), "{local} is loopback");
        }
        for remote in [
            "127.0.0.1.evil.example",
            "localhost.attacker.test",
            "127.0.0.2",
            "localhostx",
        ] {
            assert!(!is_loopback_host(remote), "{remote} must not be loopback");
        }
    }

    /// An IPv6 literal carries a colon inside its brackets, so the port separator is the colon after the
    /// closing bracket.
    ///
    /// Falsified by splitting on the last colon overall: `[::1]` has no port, but the split would read
    /// `[::` as the host and `1]` as a port, which fails to parse and is refused — so the entry becomes
    /// unusable rather than merely wrong.
    #[test]
    fn an_ipv6_literal_parses_with_and_without_a_port() {
        let bare = origin("http://[::1]");
        assert_eq!(bare.host(), "[::1]");
        assert_eq!(bare.port(), None);
        assert!(bare.matches(&origin("http://[::1]")));

        let ported = origin("http://[::1]:8443");
        assert_eq!(ported.host(), "[::1]");
        assert_eq!(ported.port(), Some(8443));
        assert!(!ported.matches(&origin("http://[::1]")));
    }

    /// An unsupported scheme is refused by name rather than accepted and never matched.
    #[test]
    fn a_scheme_that_is_not_a_browser_origin_is_refused() {
        assert!(matches!(
            AllowedOrigin::parse("file://jarvis.example.com"),
            Err(OriginError::UnsupportedScheme { .. })
        ));
        assert!(matches!(
            AllowedOrigin::parse("chrome-extension://abcdef"),
            Err(OriginError::UnsupportedScheme { .. })
        ));
    }

    /// A port that is not a port is refused, rather than defaulting to the scheme's.
    #[test]
    fn a_malformed_port_is_refused_rather_than_defaulted() {
        for entry in [
            "https://jarvis.example.com:",
            "https://jarvis.example.com:abc",
            "https://jarvis.example.com:70000",
        ] {
            assert!(
                matches!(
                    AllowedOrigin::parse(entry),
                    Err(OriginError::NotAnOrigin { .. })
                ),
                "{entry} must be refused"
            );
        }
    }

    /// Outer whitespace is trimmed because a pasted entry routinely carries it; interior whitespace is
    /// refused because it means the entry is two things.
    ///
    /// This mirrors `McpHttpEndpoint::parse` on the client side, deliberately: an operator writing
    /// either configuration should meet the same rule.
    #[test]
    fn outer_whitespace_is_trimmed_and_interior_whitespace_is_refused() {
        assert!(AllowedOrigin::parse("  https://jarvis.example.com  ").is_ok());
        assert!(matches!(
            AllowedOrigin::parse("https://jarvis.example.com https://evil.example.com"),
            Err(OriginError::EmbeddedWhitespace)
        ));
    }

    /// Every refusal obliges the same status, and a permitted verdict obliges none.
    ///
    /// The variants exist for the log and the audit record; the wire answer is uniform so a hostile
    /// caller cannot enumerate the allowlist by the refusal it receives.
    #[test]
    fn every_refusal_answers_the_same_status() {
        for verdict in [
            OriginVerdict::NotAllowed,
            OriginVerdict::Malformed,
            OriginVerdict::Opaque,
        ] {
            assert_eq!(verdict.refusal_status(), Some(403));
            assert!(!verdict.permits());
        }
        for verdict in [OriginVerdict::Absent, OriginVerdict::Allowed] {
            assert_eq!(verdict.refusal_status(), None);
            assert!(verdict.permits());
        }
    }
}
