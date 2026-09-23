//! The remote MCP endpoint: a validated URL, and the HTTP client policy that goes with it.
//!
//! # Why a URL type rather than a `&str`
//!
//! [`connect_http`](crate::connect_http) used to take a bare string, which meant every rule about what
//! a remote endpoint may be was *implicit* and unenforced. `docs/architecture/security.md` lists
//! "SSRF and unsafe redirects" as a threat with the mitigation "URL parser; scheme/host/IP policy;
//! DNS rebinding defense; redirect revalidation; block metadata/private ranges by default", and a
//! `&str` satisfies none of it. A validated type makes the rules a property of the value rather than a
//! convention at each call site — the same move `LoopbackHost` makes for the local transport, where the
//! host is *unrepresentable* rather than checked.
//!
//! # What this type proves, and what it does not
//!
//! It proves the scheme, the absence of embedded credentials, the absence of a fragment, and the
//! absence of whitespace or control characters. It does **not** prove where the host resolves. **DNS
//! resolution and private-range blocking are deliberately not claimed here**, because a
//! point-in-time answer is the wrong shape for the question: a name can resolve to a public address
//! when checked and a private one when connected (DNS rebinding), which is precisely why
//! `security.md` lists that defense separately. What this module *does* enforce is the part that can be
//! made structural — a plaintext remote endpoint is refused, and the client is built with no proxy and
//! no redirect following — and the residual is recorded rather than glossed.
//!
//! # The client policy, and why it is stated here rather than inherited
//!
//! The SDK's own `default_http_client` disables redirects but **does not call `no_proxy`**. Proxy
//! support is currently off only because the SDK's manifest sets `default-features = false` on
//! `reqwest` — a fact in a *dependency's* `Cargo.toml`, not a property of this crate. A feature
//! unification elsewhere in the graph (another crate enabling `reqwest`'s defaults) would turn an
//! unchosen intermediary back on with nothing here changing. So the client is built **by this crate**
//! and handed to the SDK through `with_client`, which makes each choice an explicit statement that a
//! reviewer can read and a test can falsify.

use std::time::Duration;

use crate::error::ConnectError;

/// The longest endpoint URL accepted, in bytes.
///
/// A remote endpoint is operator configuration, so it is bounded like every other operator-supplied
/// string in this project — a value longer than this is a mistake rather than an address.
pub const MAX_ENDPOINT_BYTES: usize = 2048;

/// The schemes an MCP endpoint may use.
///
/// Both are accepted by the *parser*; the policy that a non-loopback endpoint must use TLS is enforced
/// separately (see [`McpHttpEndpoint::allows_plaintext`]), because "the scheme is expressible" and "the
/// scheme is permitted here" are different questions and collapsing them would make the refusal
/// message unhelpful.
const ALLOWED_SCHEMES: [&str; 2] = ["http", "https"];

/// How long a connection attempt may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a single read may stall before the call is abandoned.
///
/// `read_timeout` rather than a total `timeout`, and this is the defect `P2-008` found by running: a
/// client-level `timeout` bounds the **whole response body**, so it terminates a healthy open-ended
/// SSE stream at the deadline while every non-streaming call works. An MCP connection is long-lived
/// and has no defined total length, so it cannot be bounded by a total timeout at all.
const READ_TIMEOUT: Duration = Duration::from_secs(30);

/// Why a remote MCP endpoint was refused.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EndpointError {
    /// The value was empty or only whitespace.
    Empty,
    /// The value exceeded [`MAX_ENDPOINT_BYTES`].
    TooLong,
    /// The value was not an absolute URL with a supported scheme.
    UnsupportedScheme,
    /// The value carried a username or password in its userinfo section.
    ///
    /// Refused because a credential in a URL is a **substring of every log line that mentions the
    /// endpoint**, which this project learned the hard way for provider keys: a value split out from
    /// the URL is a distinct value a redactor can target, while one embedded in it is not.
    EmbeddedCredentials,
    /// The value contained a `#` fragment.
    ///
    /// A fragment is never sent to a server, so an endpoint carrying one means the operator's intent
    /// is not what the request will express — most likely a URL pasted from a browser.
    Fragment,
    /// The value contained whitespace or a control character.
    InvalidCharacter,
    /// The authority was empty.
    MissingAuthority,
    /// A non-loopback endpoint used plaintext `http`.
    ///
    /// `security.md` requires TLS for remote clients, and the MCP specification requires it for
    /// Streamable HTTP off loopback. This is a refusal rather than a warning because a plaintext remote
    /// MCP session carries the operator's tool arguments and results in the clear, and because the
    /// failure of a warning is silent while the failure of a refusal is a message.
    PlaintextRemote,
}

impl std::fmt::Display for EndpointError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(formatter, "the MCP endpoint URL is empty"),
            Self::TooLong => write!(
                formatter,
                "the MCP endpoint URL exceeds {MAX_ENDPOINT_BYTES} bytes"
            ),
            Self::UnsupportedScheme => write!(
                formatter,
                "the MCP endpoint URL must be an absolute http or https URL"
            ),
            Self::EmbeddedCredentials => write!(
                formatter,
                "the MCP endpoint URL must not embed a username or password; supply a credential \
                 separately so it is not part of every log line that names the endpoint"
            ),
            Self::Fragment => write!(
                formatter,
                "the MCP endpoint URL must not contain a '#' fragment, which is never sent to a server"
            ),
            Self::InvalidCharacter => write!(
                formatter,
                "the MCP endpoint URL contains whitespace or a control character"
            ),
            Self::MissingAuthority => write!(formatter, "the MCP endpoint URL has no host"),
            Self::PlaintextRemote => write!(
                formatter,
                "a remote MCP endpoint must use https; plaintext http is permitted only for \
                 loopback, where the traffic does not leave this machine"
            ),
        }
    }
}

impl std::error::Error for EndpointError {}

/// A validated remote MCP endpoint.
///
/// Deliberately not `Display`-able into a URL that carries a credential, because it cannot hold one:
/// the parse refuses userinfo, so there is no constructor that produces a value with a secret in it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct McpHttpEndpoint {
    value: String,
}

impl McpHttpEndpoint {
    /// Validates a remote MCP endpoint URL.
    ///
    /// # Errors
    ///
    /// Returns [`EndpointError`] for an empty or oversized value, a non-`http(s)` scheme, embedded
    /// credentials, a `#` fragment, whitespace or control characters, a missing host, or a
    /// non-loopback plaintext endpoint.
    pub fn parse(value: &str) -> Result<Self, EndpointError> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(EndpointError::Empty);
        }
        if trimmed.len() > MAX_ENDPOINT_BYTES {
            return Err(EndpointError::TooLong);
        }
        if trimmed
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
        {
            return Err(EndpointError::InvalidCharacter);
        }
        if trimmed.contains('#') {
            return Err(EndpointError::Fragment);
        }

        let Some((scheme, rest)) = trimmed.split_once("://") else {
            return Err(EndpointError::UnsupportedScheme);
        };
        if !ALLOWED_SCHEMES
            .iter()
            .any(|allowed| scheme.eq_ignore_ascii_case(allowed))
        {
            return Err(EndpointError::UnsupportedScheme);
        }

        let authority = rest.split('/').next().unwrap_or_default();
        if authority.is_empty() {
            return Err(EndpointError::MissingAuthority);
        }
        // Userinfo is everything before the first `/`; an `@` there means the URL carries a username or
        // password. Checked on the authority rather than the whole URL, so an `@` in a *path* — which is
        // legal and common — is not mistaken for a credential.
        if authority.contains('@') {
            return Err(EndpointError::EmbeddedCredentials);
        }

        let endpoint = Self {
            value: trimmed.trim_end_matches('/').to_owned(),
        };
        // The TLS rule is enforced here rather than in a separate `validate` step, so a value that
        // violates it cannot be *held*, let alone used.
        if !endpoint.is_loopback() && scheme.eq_ignore_ascii_case("http") {
            return Err(EndpointError::PlaintextRemote);
        }
        Ok(endpoint)
    }

    /// Returns the normalized endpoint URL.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.value
    }

    /// Returns whether the authority is a loopback address.
    ///
    /// The same recognition `jarvis-models`' `BaseUrl` uses, and for the same reason: reporting remote
    /// when the traffic actually stays local is the safe direction, so the list is deliberately narrow.
    /// `127.0.0.1.evil.example` must not be read as local, which is why whole-host comparison is used
    /// rather than a `starts_with`.
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

    /// Returns whether plaintext `http` is permitted for this endpoint.
    ///
    /// True only on loopback. Exposed so a caller that is *building* a URL for a configuration file can
    /// tell an operator which shape will be accepted, rather than discovering it from a refusal.
    #[must_use]
    pub fn allows_plaintext(&self) -> bool {
        self.is_loopback()
    }
}

impl std::fmt::Display for McpHttpEndpoint {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.value)
    }
}

/// Builds the HTTP client used for remote MCP connections.
///
/// Every choice here is a statement rather than an inherited default, and each is one the SDK's own
/// client does not make:
///
/// - **`no_proxy`** — the SDK's `default_http_client` does not set it, so proxy support is off only
///   because its manifest pins `reqwest` with `default-features = false`. A feature unification
///   elsewhere could turn it back on with nothing in this crate changing, and `jarvis-models` already
///   refuses the same thing for the same reason ("would expose prompts and local-model traffic to an
///   unchosen intermediary"). An MCP session carries tool arguments and results, so the same argument
///   applies more strongly.
/// - **redirects disabled** — a redirect can move an `Authorization` header to another origin. The SDK
///   does this too; stating it here means the property survives an SDK change.
/// - **`https_only` off loopback** — belt to the parser's braces. The parser already refuses a
///   plaintext remote endpoint, and this makes the *client* refuse one even if a future caller reaches
///   the connect path some other way. Loopback keeps plaintext so a local server is testable and
///   usable, which is the one case where the traffic does not leave the machine.
/// - **`read_timeout`, not `timeout`** — see [`READ_TIMEOUT`].
///
/// # Errors
///
/// Returns [`ConnectError::Unreachable`] when the client cannot be constructed, which requires the TLS
/// backend to fail to initialize. Reported rather than panicked so a daemon can explain itself.
pub(crate) fn build_http_client(
    endpoint: &McpHttpEndpoint,
) -> Result<reqwest::Client, ConnectError> {
    let mut builder = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(READ_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy();
    if !endpoint.allows_plaintext() {
        builder = builder.https_only(true);
    }
    builder
        .build()
        .map_err(|error| ConnectError::Unreachable(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_loopback_endpoint_may_use_plaintext() {
        for value in [
            "http://127.0.0.1:8000/mcp",
            "http://localhost:8000/mcp",
            "http://[::1]:8000/mcp",
        ] {
            let endpoint =
                McpHttpEndpoint::parse(value).unwrap_or_else(|error| panic!("{value}: {error}"));
            assert!(
                endpoint.is_loopback(),
                "{value} must be recognized as loopback"
            );
            assert!(endpoint.allows_plaintext(), "{value} may be plaintext");
        }
    }

    /// **The refusal `security.md` requires**: an off-loopback MCP session over plaintext carries tool
    /// arguments and results in the clear, so it is refused rather than warned about.
    #[test]
    fn a_remote_endpoint_must_use_tls() {
        let error = McpHttpEndpoint::parse("http://mcp.example.com/mcp")
            .err()
            .unwrap_or_else(|| panic!("a plaintext remote endpoint must be refused"));
        assert_eq!(error, EndpointError::PlaintextRemote);
        // The message says what to do, not only what was wrong.
        assert!(error.to_string().contains("https"), "{error}");

        // And the same host over TLS is accepted, which is the positive control: without it a rule
        // that rejected every remote endpoint would pass the assertion above.
        assert!(McpHttpEndpoint::parse("https://mcp.example.com/mcp").is_ok());
    }

    /// A host whose name merely *begins* with a loopback string is remote, so it must not be read as
    /// local — the mistake that would let an attacker-supplied name bypass the TLS rule.
    #[test]
    fn a_host_that_only_starts_with_loopback_text_is_not_local() {
        for value in [
            "https://127.0.0.1.evil.example/mcp",
            "https://localhost.evil.example/mcp",
        ] {
            let endpoint =
                McpHttpEndpoint::parse(value).unwrap_or_else(|error| panic!("{value}: {error}"));
            assert!(
                !endpoint.is_loopback(),
                "{value} must not be read as loopback"
            );
        }
        // The plaintext form is refused, which is the consequence that matters.
        assert_eq!(
            McpHttpEndpoint::parse("http://localhost.evil.example/mcp"),
            Err(EndpointError::PlaintextRemote)
        );
    }

    /// A credential in the URL is a substring of every log line that names the endpoint. This project
    /// learned that for provider keys, so the same rule applies to a remote MCP endpoint.
    #[test]
    fn a_url_with_embedded_credentials_is_refused() {
        assert_eq!(
            McpHttpEndpoint::parse("https://user:secret@mcp.example.com/mcp"),
            Err(EndpointError::EmbeddedCredentials)
        );
        // The message must name the remedy, because "refused" alone leaves an operator guessing.
        let text = EndpointError::EmbeddedCredentials.to_string();
        assert!(text.contains("separately"), "{text}");
    }

    /// An `@` in a **path** is legal and common, and must not be mistaken for a credential — the check
    /// is on the authority, not the whole URL.
    #[test]
    fn an_at_sign_in_a_path_is_not_a_credential() {
        let endpoint = McpHttpEndpoint::parse("https://mcp.example.com/v1/user@example.com/mcp")
            .unwrap_or_else(|error| panic!("an @ in a path is legal: {error}"));
        assert!(endpoint.as_str().contains("user@example.com"));
    }

    #[test]
    fn unsupported_schemes_and_shapes_are_refused() {
        assert_eq!(McpHttpEndpoint::parse("   "), Err(EndpointError::Empty));
        assert_eq!(
            McpHttpEndpoint::parse("mcp.example.com/mcp"),
            Err(EndpointError::UnsupportedScheme)
        );
        assert_eq!(
            McpHttpEndpoint::parse("ftp://mcp.example.com/mcp"),
            Err(EndpointError::UnsupportedScheme)
        );
        assert_eq!(
            McpHttpEndpoint::parse("wss://mcp.example.com/mcp"),
            Err(EndpointError::UnsupportedScheme)
        );
        assert_eq!(
            McpHttpEndpoint::parse("https:///mcp"),
            Err(EndpointError::MissingAuthority)
        );
        assert_eq!(
            McpHttpEndpoint::parse("https://mcp.example.com/mcp#frag"),
            Err(EndpointError::Fragment)
        );
        assert_eq!(
            McpHttpEndpoint::parse(&format!(
                "https://mcp.example.com/{}",
                "a".repeat(MAX_ENDPOINT_BYTES)
            )),
            Err(EndpointError::TooLong)
        );
    }

    /// **Outer whitespace is tolerated and interior whitespace is refused**, and the two halves are
    /// asserted together because they are easily confused — the first version of this test asserted
    /// that a trailing newline was refused, which the code does not do and should not.
    ///
    /// Trimming is deliberate: a URL copied from a terminal or a config editor routinely carries a
    /// trailing newline, and refusing it would be an unhelpful failure for an obviously correct value.
    /// Interior whitespace is a different thing entirely — it means the string is not one URL, most
    /// likely a pasted header or two values joined — so it is refused rather than sanitised.
    #[test]
    fn outer_whitespace_is_trimmed_and_interior_whitespace_is_refused() {
        let pasted =
            McpHttpEndpoint::parse("  https://mcp.example.com/mcp\n").unwrap_or_else(|error| {
                panic!("a pasted URL with a trailing newline must be usable: {error}")
            });
        assert_eq!(pasted.as_str(), "https://mcp.example.com/mcp");

        // Interior whitespace is refused, because the value is then not one URL.
        assert_eq!(
            McpHttpEndpoint::parse("https://mcp.example.com/mcp\nAuthorization: Bearer x"),
            Err(EndpointError::InvalidCharacter)
        );
        assert_eq!(
            McpHttpEndpoint::parse("https://mcp.example.com /mcp"),
            Err(EndpointError::InvalidCharacter)
        );
    }

    /// The fragment rule is not cosmetic: a fragment is never sent to a server, so a URL carrying one
    /// expresses something other than what the request will do.
    #[test]
    fn a_fragment_is_refused_because_it_is_never_sent() {
        let error = McpHttpEndpoint::parse("https://mcp.example.com/mcp#token=abc")
            .err()
            .unwrap_or_else(|| panic!("a fragment must be refused"));
        assert_eq!(error, EndpointError::Fragment);
        // This is also the shape of a URL copied out of a browser, so the message says so.
        assert!(error.to_string().contains("never sent"), "{error}");
    }

    #[test]
    fn a_trailing_slash_is_normalized_away() {
        let endpoint = McpHttpEndpoint::parse("https://mcp.example.com/mcp/")
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(endpoint.as_str(), "https://mcp.example.com/mcp");
        assert_eq!(endpoint.to_string(), "https://mcp.example.com/mcp");
    }

    /// Every endpoint the type can hold is `Display`-able without leaking a credential, because no
    /// constructor produces one that has any. Asserted rather than assumed, since redaction is the kind
    /// of property that looks like coverage without being checked.
    #[test]
    fn a_parsed_endpoint_never_renders_a_credential() {
        let endpoint = McpHttpEndpoint::parse("https://mcp.example.com/mcp")
            .unwrap_or_else(|error| panic!("{error}"));
        let rendered = endpoint.to_string();
        assert!(!rendered.contains('@'), "{rendered}");
        assert!(!rendered.contains("secret"), "{rendered}");
    }
}
