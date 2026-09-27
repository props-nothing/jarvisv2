//! The authorization request, its loopback redirect, and the single-use transaction that binds them.
//!
//! `P5-001` defined the *vocabulary* of a flow ([`AuthFlow`], [`PkceVerifier`], [`SecretValue`], [`AuthMethod`])
//! and deliberately held no randomness and no listener. This module is the protocol walk that uses them, and
//! it is where `tools-and-connectors.md`'s OAuth requirements become properties of types:
//!
//! > Authorization Code plus PKCE for user-facing public clients; random state and nonce bound to a
//! > short-lived setup transaction; exact redirect URI validation and loopback listener hardening.
//!
//! The controlling external documents are RFC 7636 (PKCE), RFC 8252 (OAuth 2.0 for Native Apps, BCP 212), and
//! RFC 9700 (Best Current Practice for OAuth 2.0 Security, BCP 240, which updates RFC 6749/6750/6819). Every
//! rule below cites the sentence it comes from, because the interesting ones are counter-intuitive and a
//! reader who does not see the source will "simplify" them away.
//!
//! # The three properties this module exists to make structural
//!
//! 1. **A transaction is consumable once.** [`AuthorizationTransaction::consume`] takes `self` by value, so a
//!    second answer to the same setup is not expressible. RFC 9700 §4.2.4 asks the client to invalidate the
//!    `state` "after its first use at the redirection endpoint"; a by-value receiver makes that a property of
//!    the type rather than a discipline a caller has to remember.
//! 2. **A redirect URI cannot be a non-loopback host, and cannot be `localhost`.** [`LoopbackHost`] has
//!    exactly two variants (`127.0.0.1` and `::1`), so `http://evil.example/cb` and `http://localhost/cb` are
//!    *unrepresentable* rather than rejected. RFC 8252 §8.3 is the reason `localhost` is excluded and it is
//!    not stylistic: "Specifying a redirect URI with the loopback IP literal rather than localhost avoids
//!    inadvertently listening on network interfaces other than the loopback interface. It is also less
//!    susceptible to client-side firewalls and misconfigured host name resolution." A hostname whose meaning
//!    depends on a resolver cannot be the basis of "this listener is only reachable from this machine".
//! 3. **The listener reports what it guarantees, and the requirements are checked against those claims.** The
//!    socket is the caller's (this crate has no runtime and no sockets), so [`LoopbackListener`] carries a
//!    [`ListenerCapabilities`] value and [`ListenerCapabilities::unmet_requirements`] names what is missing.
//!    This is `ADR-0041`'s shape — a guarantee is a named capability that is refused when it cannot be
//!    enforced — applied to a port instead of a cgroup.
//!
//! # What is deliberately absent
//!
//! **No HTTP, no socket, and no URL decoding.** The listener supplies already-decoded parameters, because a
//! percent-decoding bug is a security bug in the field every check below reads, and duplicating that logic
//! here would put two implementations in the path.
//!
//! **No `nonce` validation.** RFC 9700 §4.5.3.2 makes the `nonce` meaningful only when an ID Token is issued,
//! and verifying one needs OIDC's ID-token verification (signature, `iss`, `aud`, `at_hash`). The transaction
//! carries a nonce and hands it back with the grant so a later ID-token check can use it, but nothing here
//! claims to have validated it.

use std::fmt;

use jarvis_core::UtcTimestamp;
use serde::{Deserialize, Serialize};

use crate::auth::{
    AuthError, AuthFlow, AuthMethod, PkceChallenge, PkceMethod, PkceVerifier, SecretValue,
};

/// The longest accepted redirect path.
pub const MAX_REDIRECT_PATH_CHARS: usize = 256;

/// How long a setup transaction may stay unconsumed.
///
/// The requirements say "short-lived setup transaction". Ten minutes is the conventional authorization-code
/// lifetime and it has to outlast a human typing a password and completing a second factor, so the bound is
/// about a stale transaction being replayed rather than about impatience.
pub const DEFAULT_TRANSACTION_SECONDS: i128 = 600;

/// The two hosts a loopback redirect may use.
///
/// The variants ARE the validation: RFC 8252 §7.3 gives exactly two shapes,
/// `http://127.0.0.1:{port}/{path}` for IPv4 and `http://[::1]:{port}/{path}` for IPv6, so an enum with two
/// variants makes every other host unrepresentable. That is a stronger guarantee than a parser that refuses
/// them, because a parser can be bypassed by a new code path and a missing variant cannot be constructed.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopbackHost {
    /// `127.0.0.1`.
    V4,
    /// `[::1]`.
    V6,
}

impl LoopbackHost {
    /// Returns the host as it appears in a URI authority, brackets included for IPv6.
    #[must_use]
    pub const fn as_authority(self) -> &'static str {
        match self {
            Self::V4 => "127.0.0.1",
            Self::V6 => "[::1]",
        }
    }

    /// Returns the stable snake-case token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::V4 => "v4",
            Self::V6 => "v6",
        }
    }
}

impl fmt::Display for LoopbackHost {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_authority())
    }
}

/// A `http` redirect URI on a loopback IP literal.
///
/// # The port is optional, and that is not laxity
///
/// A native app **registers** a redirect URI with the authorization server and then **requests** one with a
/// port the operating system chose at request time. Both documents say so, and they say the port is the one
/// component that may differ:
///
/// - RFC 8252 §7.3: "The authorization server MUST allow any port to be specified at the time of the request
///   for loopback IP redirect URIs, to accommodate clients that obtain an available ephemeral port from the
///   operating system at the time of the request."
/// - RFC 9700 §2.1: "authorization servers MUST utilize exact string matching except for port numbers in
///   localhost redirection URIs of native apps".
///
/// So [`Self::registered`] produces the portless form and [`Self::listening`] the ported one, and the
/// comparison that joins them is [`Self::matches_except_port`]. Making the port mandatory would refuse to
/// describe a registration, and making the comparison exact would refuse a real flow.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LoopbackRedirect {
    host: LoopbackHost,
    port: Option<u16>,
    path: String,
}

impl LoopbackRedirect {
    /// The shape an app **registers**: scheme, host, and path, with no port.
    ///
    /// # Errors
    ///
    /// Returns [`RedirectError::Path`] when the path is not an absolute path with no query, no fragment, and
    /// no traversal.
    pub fn registered(host: LoopbackHost, path: &str) -> Result<Self, RedirectError> {
        Self::build(host, None, path)
    }

    /// The shape an app **listens on**: the same, plus the ephemeral port.
    ///
    /// # Errors
    ///
    /// Returns [`RedirectError::Path`] as [`Self::registered`] does, and [`RedirectError::Port`] for port 0.
    /// Port 0 is refused because it means "any free port" to the operating system, which is a request to bind
    /// rather than a URI a browser can be sent to — so a redirect carrying it would be a document that
    /// describes nothing.
    pub fn listening(host: LoopbackHost, port: u16, path: &str) -> Result<Self, RedirectError> {
        if port == 0 {
            return Err(RedirectError::Port);
        }
        Self::build(host, Some(port), path)
    }

    fn build(host: LoopbackHost, port: Option<u16>, path: &str) -> Result<Self, RedirectError> {
        if !path.starts_with('/') {
            return Err(RedirectError::Path {
                reason: "a redirect path must be absolute and begin with `/`",
            });
        }
        if path.chars().count() > MAX_REDIRECT_PATH_CHARS {
            return Err(RedirectError::Path {
                reason: "a redirect path may be at most 256 characters",
            });
        }
        // A query or fragment in a redirect path is not cosmetic: everything after `?` is supplied by the
        // authorization server, and a client that had already put its own parameters there would be unable to
        // tell its own from the response's — which is the shape of the open-redirector attacks in RFC 9700
        // §4.1.2 and §4.11.1.
        if path.contains(['?', '#']) {
            return Err(RedirectError::Path {
                reason: "a redirect path may not contain a query or a fragment",
            });
        }
        if path.contains("..") {
            return Err(RedirectError::Path {
                reason: "a redirect path may not contain a traversal segment",
            });
        }
        Ok(Self {
            host,
            port,
            path: path.to_owned(),
        })
    }

    /// Parses a redirect URI, accepting only the two loopback `http` shapes.
    ///
    /// This exists so a **manifest's** declared redirect URI can be checked without hand-writing a parse at
    /// the call site. It refuses anything that is not `http://` followed by one of the two literals, which
    /// includes `localhost`, `https://`, and every host name — see the module doc for why `localhost` is a
    /// refusal rather than a convenience.
    ///
    /// # Errors
    ///
    /// Returns [`RedirectError::Scheme`] for anything but `http`, [`RedirectError::Host`] for a host that is
    /// not a loopback literal, and [`RedirectError::PortText`] for a port that is not a `u16`.
    ///
    /// # A URI with no path is normalised to `/`, which is what RFC 3986 says it is
    ///
    /// `http://127.0.0.1:51004` and `http://127.0.0.1:51004/` are the same URI, because an empty path is
    /// equivalent to `/` (RFC 3986 §6.2.3, syntax-based normalisation). So the pathless form parses to a root
    /// path rather than being refused. An earlier revision of this comment claimed the opposite — that `build`
    /// would refuse it — and the test written from that claim failed, which is how the disagreement surfaced.
    pub fn parse(uri: &str) -> Result<Self, RedirectError> {
        let remainder = uri.strip_prefix("http://").ok_or(RedirectError::Scheme)?;
        // The authority ends at the first `/`, and the path begins there. A URI that names no path at all
        // therefore has an empty one, which `build` below turns into `/` per RFC 3986's normalisation.
        let (authority, path) = remainder
            .split_once('/')
            .map_or((remainder, ""), |(authority, path)| (authority, path));
        let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
            let (literal, rest) = rest.split_once(']').ok_or(RedirectError::Host)?;
            if literal != "::1" {
                return Err(RedirectError::Host);
            }
            (LoopbackHost::V6, rest.strip_prefix(':'))
        } else {
            let (literal, port) = authority
                .split_once(':')
                .map_or((authority, None), |(literal, port)| (literal, Some(port)));
            if literal != "127.0.0.1" {
                return Err(RedirectError::Host);
            }
            (LoopbackHost::V4, port)
        };
        let port = match port {
            None => None,
            Some(text) => Some(text.parse::<u16>().map_err(|_| RedirectError::PortText)?),
        };
        // `build` needs the leading slash back, which is what makes a URI with no path normalise to `/` per
        // RFC 3986 rather than becoming a redirect to the provider's origin with an empty path.
        let path = if path.is_empty() {
            "/".to_owned()
        } else {
            format!("/{path}")
        };
        Self::build(host, port, &path)
    }

    /// Returns the host.
    #[must_use]
    pub const fn host(&self) -> LoopbackHost {
        self.host
    }

    /// Returns the port, when this is a listening URI.
    #[must_use]
    pub const fn port(&self) -> Option<u16> {
        self.port
    }

    /// Returns the path, leading slash included.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Renders the URI.
    #[must_use]
    pub fn as_uri(&self) -> String {
        match self.port {
            None => format!("http://{}{}", self.host.as_authority(), self.path),
            Some(port) => format!("http://{}:{port}{}", self.host.as_authority(), self.path),
        }
    }

    /// Compares scheme, host, and path, **ignoring the port**.
    ///
    /// The comparison RFC 9700 §2.1 mandates for a loopback redirect, and the one that joins a registration to
    /// a listener. The port is excluded because it is the component the client chooses at request time.
    ///
    /// Note what is *not* excluded: a different **path** is a mismatch, and a different **host** is a mismatch.
    /// RFC 8252 §8.10 requires the app to store the redirect URI with the transaction and "verify that the URI
    /// on which the authorization response was received exactly matches it", so loosening the path or host
    /// would defeat the rule that makes the check meaningful.
    #[must_use]
    pub fn matches_except_port(&self, other: &Self) -> bool {
        self.host == other.host && self.path == other.path
    }

    /// Compares every component including the port.
    #[must_use]
    pub fn matches_exactly(&self, other: &Self) -> bool {
        self.matches_except_port(other) && self.port == other.port
    }

    /// Returns whether this shape is registrable at a provider (no port).
    ///
    /// Registered with the provider's "allow any port" rule; a registration that pinned a port would collide
    /// with a second instance of the same app on the machine.
    #[must_use]
    pub const fn is_registrable(&self) -> bool {
        self.port.is_none()
    }
}

impl fmt::Display for LoopbackRedirect {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.as_uri())
    }
}

/// Why a redirect URI is unusable.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum RedirectError {
    /// The scheme is not `http`.
    #[error(
        "a loopback redirect must use the `http` scheme; RFC 8252 §7.3 gives `http://127.0.0.1:{{port}}/{{path}}` and `http://[::1]:{{port}}/{{path}}`"
    )]
    Scheme,
    /// The host is not a loopback IP literal.
    #[error(
        "a loopback redirect must use the `127.0.0.1` or `[::1]` literal; `localhost` is NOT RECOMMENDED by RFC 8252 §8.3 because it can resolve to a non-loopback interface"
    )]
    Host,
    /// The port text is unusable.
    #[error("the redirect port is not a valid TCP port number")]
    PortText,
    /// The port is zero.
    #[error(
        "a listening redirect needs the ephemeral port the operating system returned; port 0 means `any free port` and describes no URI"
    )]
    Port,
    /// The path is unusable.
    #[error("the redirect path is unusable: {reason}")]
    Path {
        /// What is wrong.
        reason: &'static str,
    },
}

/// What a loopback listener guarantees.
///
/// The requirements this encodes are RFC 8252 §8.3 (listen on the loopback interface only; close the port once
/// the response arrives) and its platform notes B.3/B.5, which are the same rule stated for two operating
/// systems:
///
/// - B.3, Windows: "apps SHOULD set the '`SO_EXCLUSIVEADDRUSE`' socket option to prevent other apps binding to
///   the same socket";
/// - B.5, Linux: "Apps SHOULD NOT set the '`SO_REUSEPORT`' or '`SO_REUSEADDR`' socket options in order to prevent
///   other apps binding to the same socket."
///
/// Both are about **no second binder on the port**, which is why the field is `exclusive_bind` rather than a
/// per-platform option: a caller reports the property it achieved, and the platform decides how.
///
/// # Four independent facts, and why they are booleans rather than a set of failures
///
/// `clippy::struct_excessive_bools` is allowed here with a reason, because the alternative representation is
/// **less safe**. A set of unmet requirements would have `Default` = empty = "everything is met", so a caller
/// that forgot to report a capability would silently claim it; four `bool`s default to `false` = not met, so an
/// unreported capability fails closed. The booleans are also genuinely independent — each has its own remedy,
/// and [`UnmetListenerRequirement::guidance`] names it — rather than two spellings of one fact.
#[allow(
    clippy::struct_excessive_bools,
    reason = "four independent capabilities whose omitted default fails closed; see the type's own doc"
)]
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ListenerCapabilities {
    /// The socket accepts connections only from the loopback interface.
    pub loopback_only: bool,
    /// No other process can bind the same port.
    pub exclusive_bind: bool,
    /// The port is closed once the response has been received.
    pub closes_after_response: bool,
    /// A listener exists for both IP stacks.
    ///
    /// Not a security property: RFC 8252 §7.3 says "Clients SHOULD NOT assume that the device supports a
    /// particular version of the Internet Protocol. It is RECOMMENDED that clients attempt to bind to the
    /// loopback interface using both IPv4 and IPv6 and use whichever is available." So a single-stack listener
    /// is a **compatibility** shortfall, and [`Self::unmet_requirements`] reports it separately from the
    /// security ones because the remedy differs.
    pub both_ip_stacks: bool,
}

impl ListenerCapabilities {
    /// The capabilities every loopback listener must have.
    #[must_use]
    pub const fn required() -> Self {
        Self {
            loopback_only: true,
            exclusive_bind: true,
            closes_after_response: true,
            both_ip_stacks: true,
        }
    }

    /// Returns the requirements this listener does **not** meet.
    ///
    /// A list rather than a verdict, because the names are the operator's remedy. And the security items
    /// precede the compatibility one so a caller that stops at the first entry does not report a
    /// single-stack listener as safe when it also allows off-machine connections.
    #[must_use]
    pub fn unmet_requirements(self) -> Vec<UnmetListenerRequirement> {
        let mut unmet = Vec::new();
        if !self.loopback_only {
            unmet.push(UnmetListenerRequirement::NotLoopbackOnly);
        }
        if !self.exclusive_bind {
            unmet.push(UnmetListenerRequirement::NotExclusivelyBound);
        }
        if !self.closes_after_response {
            unmet.push(UnmetListenerRequirement::NotClosed);
        }
        if !self.both_ip_stacks {
            unmet.push(UnmetListenerRequirement::SingleIpStack);
        }
        unmet
    }

    /// Returns whether every requirement is met.
    #[must_use]
    pub fn is_sufficient(self) -> bool {
        self.unmet_requirements().is_empty()
    }
}

/// A requirement a loopback listener does not meet.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum UnmetListenerRequirement {
    /// Connections are not restricted to the loopback interface.
    NotLoopbackOnly,
    /// Another process could bind the same port.
    NotExclusivelyBound,
    /// The port stays open after the response.
    NotClosed,
    /// Only one IP stack is served.
    SingleIpStack,
}

impl UnmetListenerRequirement {
    /// Returns the stable snake-case code.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotLoopbackOnly => "not_loopback_only",
            Self::NotExclusivelyBound => "not_exclusively_bound",
            Self::NotClosed => "not_closed",
            Self::SingleIpStack => "single_ip_stack",
        }
    }

    /// Returns whether failing this requirement is a security failure rather than a compatibility one.
    ///
    /// `SingleIpStack` is the only `false`: it means the flow may fail to complete on a host that lacks that
    /// stack, not that a response can arrive from somewhere it should not. Conflating the two would make a
    /// compatibility shortfall read as a vulnerability, which is how a real one gets ignored.
    #[must_use]
    pub const fn is_security(self) -> bool {
        !matches!(self, Self::SingleIpStack)
    }

    /// Returns the operator's remedy.
    #[must_use]
    pub const fn guidance(self) -> &'static str {
        match self {
            Self::NotLoopbackOnly => {
                "bind to 127.0.0.1 and [::1]; RFC 8252 §8.3 requires listening on the loopback interface only"
            }
            Self::NotExclusivelyBound => {
                "set SO_EXCLUSIVEADDRUSE on Windows or clear SO_REUSEPORT/SO_REUSEADDR on unix so no second \
                 process can bind the port (RFC 8252 §B.3, §B.5)"
            }
            Self::NotClosed => {
                "close the port once the authorization response has been received (RFC 8252 §8.3)"
            }
            Self::SingleIpStack => {
                "attempt both IP stacks and use whichever is available (RFC 8252 §7.3)"
            }
        }
    }
}

impl fmt::Display for UnmetListenerRequirement {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A loopback listener, described by the caller that owns the socket.
///
/// This crate opens no sockets and has no runtime, and that is a boundary rather than an omission: a socket is
/// a platform facility with its own hardening rules, and `P5-001` fixed this crate's dependency set. The trait
/// is therefore narrow on purpose — it reports the URI a response will arrive on and the capabilities achieved
/// — so the flow can be verified without a listener existing at all.
pub trait LoopbackListener {
    /// Returns the URI a response will arrive on, ephemeral port included.
    fn redirect(&self) -> &LoopbackRedirect;

    /// Returns what this listener guarantees.
    fn capabilities(&self) -> ListenerCapabilities;
}

/// The `state` and `nonce` bound to one setup transaction, plus what the response must prove.
///
/// # Why this is one value rather than five arguments
///
/// RFC 8252 §8.10 is explicit that the pieces have to travel together:
///
/// > The native app MUST store the redirect URI used in the authorization request with the authorization
/// > session data (i.e., along with "state" and other related data) and MUST verify that the URI on which the
/// > authorization response was received exactly matches it.
///
/// Five loose arguments is exactly the `P3-005`/`P3-006a` defect — two values that must agree with nothing
/// holding both — so a redirect URI that did not come from this request cannot be compared against it, and
/// the verifier cannot be separated from the challenge that proves it.
///
/// # Single use
///
/// [`Self::consume`] takes `self` by value. A transaction that has produced a grant no longer exists, so "use
/// the same transaction twice" has no expression in the type system rather than being a runtime check that a
/// new caller could skip.
pub struct AuthorizationTransaction {
    auth_method: AuthMethod,
    verifier: PkceVerifier,
    method: PkceMethod,
    state: SecretValue,
    nonce: Option<SecretValue>,
    redirect: LoopbackRedirect,
    issuer: Option<String>,
    issued_at: UtcTimestamp,
}

impl AuthorizationTransaction {
    /// Opens a setup transaction.
    ///
    /// # Errors
    ///
    /// Returns [`AuthError::Flow`] when the flow cannot carry a transaction — no PKCE method (PKCE is
    /// mandatory for a public native client, RFC 8252 §6), no redirect URI, or a redirect URI that does not
    /// name the listener's host and path.
    ///
    /// The redirect check is the one worth stating. A flow declares the URI it registered with the provider;
    /// the listener reports the URI it is bound on. A transaction whose listener is on a *different*
    /// **path** would send the provider a redirect URI this process is not listening on, so the response
    /// would either never arrive or arrive somewhere the caller never checked. Comparing them here is what
    /// makes RFC 8252 §8.10's "verify that the URI … matches" rule have something to verify against.
    pub fn begin(
        flow: &AuthFlow,
        verifier: PkceVerifier,
        state: SecretValue,
        nonce: Option<SecretValue>,
        redirect: LoopbackRedirect,
        issued_at: UtcTimestamp,
    ) -> Result<Self, AuthError> {
        let method = flow.pkce().ok_or(AuthError::Flow {
            reason: "a public native client must use PKCE, so a transaction needs a code challenge method",
        })?;
        let declared = flow.redirect_uri().ok_or(AuthError::Flow {
            reason: "a transaction needs the redirect URI that was registered with the provider",
        })?;
        let registered = LoopbackRedirect::parse(declared).map_err(|_| AuthError::Flow {
            reason: "the flow's redirect URI is not a loopback `http` URI on `127.0.0.1` or `[::1]`",
        })?;
        if !registered.matches_except_port(&redirect) {
            return Err(AuthError::Flow {
                reason: "the listener is on a different host or path than the URI the flow registered, so \
                         the provider's response would arrive where nothing was checked",
            });
        }
        Ok(Self {
            auth_method: flow.method(),
            verifier,
            method,
            state,
            nonce,
            redirect,
            issuer: None,
            issued_at,
        })
    }

    /// Returns the auth method this transaction is performing.
    ///
    /// Retained rather than derived, because a reauth path has to know **how** an account is connected before
    /// it can decide whether a new challenge is needed at all — an `ApiKey` account cannot be reauthorized by
    /// a browser visit, and a `ServiceAccount` has no user to send one to.
    #[must_use]
    pub const fn auth_method(&self) -> AuthMethod {
        self.auth_method
    }

    /// Narrows the transaction to one authorization server, enabling the mix-up defence.
    ///
    /// RFC 9700 §2.1 makes a defence **REQUIRED** once a client can interact with more than one authorization
    /// server, and §4.4.2.1 gives the `iss` parameter as the preferred one. Storing the issuer is what lets
    /// [`Self::consume`] confirm the response came from the server that was asked; without it, the grant
    /// reports that the defence did not run. See [`MixUpDefence`].
    #[must_use]
    pub fn with_issuer(mut self, issuer: impl Into<String>) -> Self {
        self.issuer = Some(issuer.into());
        self
    }

    /// Returns the transaction's `state` value, which the response must echo.
    #[must_use]
    pub const fn state(&self) -> &SecretValue {
        &self.state
    }

    /// Returns the listener's redirect URI, the value that must be sent and matched.
    #[must_use]
    pub const fn redirect(&self) -> &LoopbackRedirect {
        &self.redirect
    }

    /// Returns the PKCE method this transaction proves with.
    #[must_use]
    pub const fn method(&self) -> PkceMethod {
        self.method
    }

    /// Returns the issuer the transaction was narrowed to, when the caller narrowed it.
    #[must_use]
    pub fn issuer(&self) -> Option<&str> {
        self.issuer.as_deref()
    }

    /// Returns when the transaction was opened.
    #[must_use]
    pub const fn issued_at(&self) -> UtcTimestamp {
        self.issued_at
    }

    /// Derives the challenge to send, from the stored verifier and method.
    #[must_use]
    pub fn challenge(&self) -> PkceChallenge {
        self.verifier.challenge(self.method)
    }

    /// Returns the parameters of the authorization request, in a stable order.
    ///
    /// A list rather than a URL, because this crate has no URL builder and because a caller that has one should
    /// be able to put these into whatever request shape its transport uses. The order is fixed so a test can
    /// assert the set without depending on a hash map's iteration order.
    ///
    /// `code_challenge_method` is always sent, even for `S256`, which RFC 7636 §4.3 lists as OPTIONAL defaulting
    /// to `plain`. Sending it removes the default's effect: a server that read the omission as `plain` would
    /// compare the verifier directly, which is precisely the weakness §7.2 exists to prevent. The extra
    /// parameter is what makes "no downgrade" independent of the server's defaulting.
    #[must_use]
    pub fn parameters(&self, client_id: &str, scopes: &[String]) -> Vec<(&'static str, String)> {
        let challenge = self.challenge();
        let mut parameters = vec![
            ("response_type", "code".to_owned()),
            ("client_id", client_id.to_owned()),
            ("redirect_uri", self.redirect.as_uri()),
            ("state", self.state.expose().to_owned()),
            ("code_challenge", challenge.value().to_owned()),
            (
                "code_challenge_method",
                challenge.method().as_str().to_owned(),
            ),
        ];
        if !scopes.is_empty() {
            parameters.push(("scope", scopes.join(" ")));
        }
        if let Some(nonce) = &self.nonce {
            parameters.push(("nonce", nonce.expose().to_owned()));
        }
        parameters
    }

    /// Returns whether the transaction is too old to answer.
    ///
    /// # A transaction dated in the future is treated as expired, and the asymmetry is deliberate
    ///
    /// The age of a transaction we cannot compute is not a fact we can rely on, and the two directions are not
    /// equal: accepting a stale transaction admits a replay, while refusing one costs a second attempt. So a
    /// negative age means the clock moved and the safe answer is `true`. `ConnectorHealth::is_fresh_at` refuses
    /// a future observation for the same reason from the other side, and both are recorded because the
    /// "obvious" reading of each is the wrong one.
    #[must_use]
    pub fn is_expired(&self, now: UtcTimestamp, maximum_seconds: i128) -> bool {
        let Some(elapsed) = now.unix_nanos().checked_sub(self.issued_at.unix_nanos()) else {
            return true;
        };
        let Some(elapsed) = elapsed.checked_div(1_000_000_000) else {
            return true;
        };
        if elapsed < 0 {
            return true;
        }
        elapsed > maximum_seconds
    }

    /// Consumes the transaction with the provider's answer, yielding the code to exchange.
    ///
    /// # Errors
    ///
    /// Returns the first [`AuthRefusal`] that applies, in an order chosen so an attacker learns nothing from
    /// which check fired:
    ///
    /// 1. a provider **error** is reported before the `state` is compared, because an erroring provider will
    ///    not have echoed a state and reporting a state mismatch for a genuine refusal would send an operator
    ///    hunting a forgery;
    /// 2. the `state` must be present and match — RFC 9700 §4.7.1 and RFC 8252 §8.9;
    /// 3. the response must have arrived on this transaction's redirect URI — RFC 8252 §8.10;
    /// 4. the issuer, when one was stored, must match — RFC 9700 §4.4.2.1, where a mismatch is a **MUST abort**.
    ///
    /// A missing `iss` is **not** a refusal: RFC 9207's parameter is optional, so its absence is not evidence
    /// of an attack. It is reported on the grant instead, as [`MixUpDefence::NotSatisfied`], so a caller that
    /// talks to several issuers can refuse it itself while a single-issuer caller is not blocked.
    pub fn consume(self, callback: &Callback) -> Result<Grant, AuthRefusal> {
        if let Some(error) = &callback.error {
            return Err(AuthRefusal::ProviderError {
                error: error.clone(),
                description: callback.error_description.clone(),
            });
        }
        let state = callback.state.as_deref().ok_or(AuthRefusal::StateMissing)?;
        if !self.state.matches(state) {
            return Err(AuthRefusal::StateMismatch);
        }
        if !self.redirect.matches_except_port(&callback.received_on) {
            return Err(AuthRefusal::RedirectMismatch);
        }
        let mix_up_defence = match (&self.issuer, &callback.issuer) {
            (Some(expected), Some(received)) if expected == received => {
                MixUpDefence::IssuerConfirmed
            }
            (Some(expected), Some(received)) => {
                return Err(AuthRefusal::IssuerMismatch {
                    expected: expected.clone(),
                    received: received.clone(),
                });
            }
            // The transaction did not narrow to an issuer, so no mix-up defence was attempted. Reported as
            // such rather than as satisfied, because a single-issuer deployment is safe by construction and a
            // multi-issuer one must not read this as a confirmation that never happened.
            (None, _) => MixUpDefence::NotNeeded,
            (Some(_), None) => MixUpDefence::NotSatisfied,
        };
        let code = callback
            .code
            .as_deref()
            .filter(|code| !code.trim().is_empty())
            .ok_or(AuthRefusal::CodeMissing)?;
        Ok(Grant {
            code: AuthorizationCode::new(code),
            mix_up_defence,
            nonce: self.nonce,
            redirect: self.redirect,
        })
    }
}

/// One authorization code, to be exchanged exactly once.
///
/// RFC 9700 §4.2.4: authorization codes "MUST be invalidated by the authorization server after their first use
/// at the token endpoint". A code is a credential, so it has no `Display` and a `Debug` that prints a length —
/// the same shape as [`PkceVerifier`] and [`SecretValue`].
#[derive(Clone, Eq, PartialEq)]
pub struct AuthorizationCode(String);

impl AuthorizationCode {
    /// Records a code the provider issued.
    ///
    /// No length or alphabet bound is imposed. A code is provider-defined ([RFC 6749] §4.1.2 says only that it
    /// is "opaque to the client"), so imposing a shape here would refuse a conforming provider. What *is*
    /// enforced is that it is not empty, which `consume` checks before calling this.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the code, for the token request and nowhere else.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for AuthorizationCode {
    /// Prints a length: a code in a log line is a flow anyone reading the log can finish, inside its lifetime.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "AuthorizationCode({} chars)", self.0.len())
    }
}

/// A validated authorization response.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Grant {
    /// The code to exchange at the token endpoint.
    pub code: AuthorizationCode,
    /// How the mix-up defence was satisfied, if it was attempted.
    pub mix_up_defence: MixUpDefence,
    /// The `nonce` this transaction sent, for a later ID-token check.
    ///
    /// Handed back rather than validated: see the module doc. A caller running a pure OAuth flow ignores it; a
    /// caller verifying an ID token compares the `nonce` claim against it.
    pub nonce: Option<SecretValue>,
    /// The redirect URI the response arrived on, carried forward so the token request can repeat it.
    ///
    /// RFC 6749 §4.1.3 requires the token request to include `redirect_uri` **if** it was in the authorization
    /// request, and that the two be identical. Carrying the value that was sent is what makes "identical" a
    /// property rather than a coincidence.
    pub redirect: LoopbackRedirect,
}

/// How a mix-up defence was satisfied.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MixUpDefence {
    /// No issuer was stored, so none was needed: the client only talks to one authorization server.
    ///
    /// RFC 9700 §4.4.2: "When an OAuth client can only interact with one authorization server, a mix-up
    /// defense is not required."
    NotNeeded,
    /// An issuer was stored and the response confirmed it (RFC 9207's `iss`).
    IssuerConfirmed,
    /// An issuer was stored and the response carried none.
    ///
    /// **Not an attack.** RFC 9207's `iss` is optional, so a conforming server may omit it. RFC 9700 §4.4.2.1
    /// additionally warns that "just storing the authorization server URL is not sufficient to identify
    /// mix-up attacks. An attacker might declare an uncompromised authorization server's authorization
    /// endpoint URL as 'their' authorization server URL, but declare a token endpoint under their own
    /// control" — so this state means the defence could not run, and a caller that talks to more than one
    /// issuer must refuse it.
    NotSatisfied,
}

impl MixUpDefence {
    /// Returns whether the defence ran and confirmed the issuer.
    #[must_use]
    pub const fn is_confirmed(self) -> bool {
        matches!(self, Self::IssuerConfirmed)
    }

    /// Returns the stable snake-case code.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotNeeded => "not_needed",
            Self::IssuerConfirmed => "issuer_confirmed",
            Self::NotSatisfied => "not_satisfied",
        }
    }
}

/// The provider's answer to an authorization request, as the listener parsed it.
///
/// Parameters arrive **already decoded**, because this crate has no URL decoder and duplicating one would put a
/// second percent-decoding implementation in the path that all four checks below read. That division is the
/// same one `P3-008e` used for transport framing: the byte-level work belongs to the transport, the security
/// decision belongs here.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Callback {
    /// The `state` the provider echoed back, if any.
    pub state: Option<String>,
    /// The authorization code, if the request succeeded.
    pub code: Option<String>,
    /// The `error` code, if the request failed.
    pub error: Option<String>,
    /// The `error_description`, which RFC 6749 §4.1.2.1 marks as *not* for display to the user.
    pub error_description: Option<String>,
    /// The issuer, from RFC 9207's `iss`.
    pub issuer: Option<String>,
    /// The URI the listener received this response on.
    pub received_on: LoopbackRedirect,
}

impl Callback {
    /// Parses the **request target** a loopback listener received into the answer it carries.
    ///
    /// # Why this exists, and why it takes a request target rather than a URL
    ///
    /// [`Callback`]'s own doc says parameters arrive "already decoded, because this crate has no URL decoder"
    /// — and that division then had no implementation, so every `Callback` was assembled by hand and the
    /// decoding step was never written or tested. A listener that parses a request target badly turns a
    /// perfectly good authorization into a refusal, and its failure mode is not a crash: a `+` left as a `+`
    /// produces a `state` that is *slightly* wrong, which is the silent shape.
    ///
    /// `request_target` is what an HTTP listener has: an origin-form target such as
    /// `/callback?code=4/0A&state=abc`. Taking a target rather than a full URL means the caller cannot
    /// accidentally pass a request for a **different origin** that would then be treated as this listener's —
    /// [`LoopbackRedirect::parse`] still recovers the received-on URI, and the transaction's
    /// `matches_except_port` check is what refuses a mismatch.
    ///
    /// # The encoding is `application/x-www-form-urlencoded`, and it is NOT the request side's encoding
    ///
    /// RFC 6749 §4.1.2 says the authorization server adds the parameters "to the query component of the
    /// redirection URI using the `application/x-www-form-urlencoded` format, per Appendix B". That format
    /// encodes a space as **`+`**, which is the opposite of
    /// [`crate::google::request::percent_encode`], where a space is `%20` and a literal `+` is `%2B` because
    /// RFC 3986 has no form semantics. So the decoder here treats `+` as a space and this is deliberate, not
    /// an oversight: a `state` of `a b` arrives as `state=a+b`, and a decoder that kept the `+` would compare
    /// `a+b` against `a b` and refuse a legitimate response. RFC 9253 later registered a `+`-free form for
    /// this reason, but §4.1.2 is what the provider implements.
    ///
    /// # Errors
    ///
    /// Returns [`AuthRefusal::CallbackUnparsable`] when the target has no loopback origin, when a percent
    /// escape is malformed or is not valid UTF-8, or when a parameter name repeats.
    pub fn from_request_target(request_target: &str) -> Result<Self, AuthRefusal> {
        let (path, query) = request_target
            .split_once('?')
            .map_or((request_target, ""), |(path, query)| (path, query));
        // The received-on URI is recovered from the target, so `matches_except_port` has something real to
        // compare. A target with no path is `/` by RFC 3986's normalisation, which `LoopbackRedirect::parse`
        // already applies.
        let received_on = LoopbackRedirect::parse(&format!("http://{LOOPBACK_AUTHORITY}{path}"))
            .map_err(|_| AuthRefusal::CallbackMalformed {
                reason: "the callback target does not name a loopback redirect this client could have \
                         registered",
            })?;
        let mut parameters = FormParameters::new();
        if !query.is_empty() {
            for pair in query.split('&') {
                parameters.push(pair)?;
            }
        }
        Ok(Self {
            state: parameters.take("state"),
            code: parameters.take("code"),
            error: parameters.take("error"),
            error_description: parameters.take("error_description"),
            issuer: parameters.take("iss"),
            received_on,
        })
    }
}

/// The loopback authority a callback target is reconstructed against.
///
/// The port is not carried here because a callback target an HTTP listener sees is **origin-form** — the
/// `Host` header holds the port, and this function is not given one. What the target must supply is the
/// path, and that is what [`LoopbackRedirect::parse`] checks; the port comparison happens later, against the
/// transaction's own registration, in [`AuthorizationTransaction::consume`].
const LOOPBACK_AUTHORITY: &str = "127.0.0.1";

/// The query parameters of a callback, decoded and checked for repetition.
///
/// A small type rather than a `HashMap` because two properties matter and a map has neither: a **repeated
/// name is refused** (RFC 6749 §3.1 and §3.2 both require that request and response parameters "MUST NOT be
/// included more than once", and `take` on a map would silently keep the last), and a value is consumed
/// exactly once so a later `take` of the same name cannot resurrect it.
struct FormParameters {
    values: Vec<(String, String)>,
}

impl FormParameters {
    fn new() -> Self {
        Self { values: Vec::new() }
    }

    /// Decodes one `name=value` pair and records it.
    ///
    /// # Errors
    ///
    /// Returns [`AuthRefusal::CallbackUnparsable`] for a malformed escape, invalid UTF-8, or a repeated name.
    fn push(&mut self, pair: &str) -> Result<(), AuthRefusal> {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        let name = crate::form::decode_component(name).map_err(callback_malformed)?;
        let value = crate::form::decode_component(value).map_err(callback_malformed)?;
        if self.values.iter().any(|(existing, _)| existing == &name) {
            // Refused rather than last-wins. A repeated parameter is either an attack or a broken server, and
            // picking one of the two values is exactly how a `state` check is defeated: the server's first
            // value is the one a client would compare while the second is what an attacker appended.
            return Err(AuthRefusal::ParameterRepeated);
        }
        self.values.push((name, value));
        Ok(())
    }

    /// Removes and returns a parameter, so a name is read at most once.
    fn take(&mut self, name: &str) -> Option<String> {
        let position = self.values.iter().position(|(key, _)| key == name)?;
        Some(self.values.remove(position).1)
    }
}

/// Maps a codec refusal onto this module's vocabulary, carrying the codec's own bounded reason.
///
/// The codec reports a **cause class**, not a message, so the reason text lives in one place and a new
/// `FormError` variant is a compile error here rather than an unhandled string.
fn callback_malformed(error: crate::form::FormError) -> AuthRefusal {
    AuthRefusal::CallbackMalformed {
        reason: error.reason(),
    }
}

/// Why an authorization response was refused.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AuthRefusal {
    /// The provider reported an error rather than a code.
    ProviderError {
        /// The `error` value.
        error: String,
        /// The `error_description`, when present.
        description: Option<String>,
    },
    /// The response carried no `state`.
    StateMissing,
    /// The response carried a `state` that is not this transaction's.
    ///
    /// One variant for both "wrong value" and "someone else's transaction", because distinguishing them would
    /// tell an attacker which half of a forged callback was right. The `state` comparison itself is
    /// constant-time (`SecretValue::matches`).
    StateMismatch,
    /// The response arrived on a redirect URI that is not this transaction's.
    RedirectMismatch,
    /// The issuer confirmed a different authorization server.
    IssuerMismatch {
        /// The issuer the transaction was narrowed to.
        expected: String,
        /// The issuer the response claimed.
        received: String,
    },
    /// The response carried no usable code.
    CodeMissing,
    /// The request target the listener received could not be read as a callback at all.
    ///
    /// About the **bytes**, not about a decision: a malformed percent escape, invalid UTF-8, or a target that
    /// does not name a loopback redirect. Separating it from a mismatch matters because the remedy differs — a
    /// mismatch means "do not trust this response", while this means "the listener is not reading its own
    /// requests", which a developer fixes.
    CallbackMalformed {
        /// A bounded explanation, safe to show an operator and never echoing the value.
        reason: &'static str,
    },
    /// A callback parameter appeared more than once.
    ///
    /// A **separate variant from [`Self::CallbackMalformed`]** even though both are produced while reading the
    /// target, because the two mean different things and have different verdicts: RFC 6749 §3.1 and §3.2
    /// require a parameter to appear at most once, so a repeat is the shape an appended value takes — a
    /// possible attack — while a truncated escape is a fault. One variant carrying both would make
    /// [`Self::indicates_forgery`] unable to answer honestly, which is this repository's "two values standing
    /// for more than two situations" defect.
    ParameterRepeated,
}

impl AuthRefusal {
    /// Returns the stable snake-case code.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::ProviderError { .. } => "provider_error",
            Self::StateMissing => "state_missing",
            Self::StateMismatch => "state_mismatch",
            Self::RedirectMismatch => "redirect_mismatch",
            Self::IssuerMismatch { .. } => "issuer_mismatch",
            Self::CodeMissing => "code_missing",
            Self::CallbackMalformed { .. } => "callback_malformed",
            Self::ParameterRepeated => "parameter_repeated",
        }
    }

    /// Returns whether the refusal is evidence that someone forged a response.
    ///
    /// The distinction decides the response: a forgery is worth surfacing loudly and a provider error is not.
    /// `RedirectMismatch` and `IssuerMismatch` count — both mean the answer did not come from the flow that
    /// asked, which is the mix-up and open-redirector shape. `ProviderError` does not: the provider answered,
    /// and answered "no".
    #[must_use]
    pub const fn indicates_forgery(&self) -> bool {
        matches!(
            self,
            Self::StateMissing
                | Self::StateMismatch
                | Self::RedirectMismatch
                | Self::IssuerMismatch { .. }
                | Self::ParameterRepeated
        )
    }
}

impl fmt::Display for AuthRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProviderError { error, description } => match description {
                Some(description) => write!(formatter, "the provider refused the request: {error} ({description})"),
                None => write!(formatter, "the provider refused the request: {error}"),
            },
            Self::StateMissing => formatter.write_str(
                "the authorization response carried no `state`, so it cannot be bound to a pending request",
            ),
            Self::StateMismatch => {
                formatter.write_str("the authorization response did not present the state this request issued")
            }
            Self::RedirectMismatch => formatter.write_str(
                "the authorization response arrived on a redirect URI that is not the one this request used",
            ),
            Self::IssuerMismatch { expected, received } => {
                write!(formatter, "the response came from `{received}` but this request went to `{expected}`")
            }
            Self::CodeMissing => {
                formatter.write_str("the authorization response carried no usable authorization code")
            }
            Self::CallbackMalformed { reason } => {
                write!(formatter, "the callback could not be read: {reason}")
            }
            Self::ParameterRepeated => formatter.write_str(
                "the callback repeated a parameter, which RFC 6749 requires a server to send at most once",
            ),
        }
    }
}

#[cfg(test)]
#[path = "authorization_tests.rs"]
mod tests;
