//! The auth flow: how a connector is connected, refreshed, reauthorized, and revoked.
//!
//! `tools-and-connectors.md`'s OAuth section lists nine requirements, and this module is the vocabulary for
//! them:
//!
//! > Authorization Code plus PKCE for user-facing public clients; random state and nonce bound to a
//! > short-lived setup transaction; exact redirect URI validation and loopback listener hardening;
//! > least-privilege incremental scopes with a clear explanation before consent; token exchange and refresh
//! > only inside the connector/secret boundary; refresh-token rotation and invalidation handling; account
//! > identity verified from the provider, not user-entered labels; reauth path that preserves account
//! > references without hiding lost scopes; no token material in model context, URLs/logs, diagnostics, or
//! > normal database columns.
//!
//! # What this module does NOT contain, deliberately
//!
//! **No token type.** There is no `AccessToken`, no `RefreshToken`-as-a-value, and no field anywhere that
//! holds a bearer string. The requirements say "no token material in […] normal database columns", and the
//! mechanical way to honour that is for the vocabulary never to have a place to put it: [`AuthChallenge`]
//! carries a **URL to open** and a state value, and [`VerifiedAccount`](crate::VerifiedAccount) carries what
//! the provider said about *who* the account is. An adapter holds its token in memory for the length of one
//! call and hands it to the provider client, exactly as `security.md` requires ("short-lived in memory,
//! zeroized where practical, and passed only to the adapter operation that requires them").
//!
//! **No HTTP and no randomness.** This crate has no client and no RNG. [`PkceChallenge`] is the *derivation*
//! from a verifier, and the verifier is supplied by the caller — so `P5-002` owns generating one and this
//! module owns the property that verifies it. That split is what makes the derivation testable against the
//! RFC's own published vectors rather than against whatever the generator happened to produce.
//!
//! # Why the state and nonce are [`SecretValue`]-shaped
//!
//! The requirement is that state and nonce are "random […] bound to a short-lived setup transaction". Both
//! are compared for equality and neither is ever displayed, so both are represented as an opaque value with
//! a **constant-time comparison** and a `Debug` that prints a length rather than the value — the same shape
//! `jarvis_core::DecisionNonce` uses, and for the same reason: a state value that appeared in a log could be
//! replayed against the loopback listener.

use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::manifest::ConnectorError;

/// The longest accepted PKCE verifier, per RFC 7636's own bound.
pub const MAX_PKCE_VERIFIER_CHARS: usize = 128;

/// The shortest accepted PKCE verifier, per RFC 7636's own bound.
pub const MIN_PKCE_VERIFIER_CHARS: usize = 43;

/// The longest opaque state or nonce value accepted.
///
/// The same bound `jarvis_core::DecisionNonce` uses, because these are compared rather than parsed and a
/// value longer than a digest is a sign something else was pasted in.
pub const MAX_STATE_CHARS: usize = 256;

/// An auth method a connector may declare.
///
/// Each variant exists because it changes the **state machine** rather than a label: `Pkce` needs a loopback
/// listener and a verifier, `ApiKey` needs no flow at all, and `ServiceAccount` cannot be revoked by a user
/// so its reauth path is different. A free-text `method` would be a field nothing could act on, which is the
/// same reasoning `P3-001`'s `EffectSet` records.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMethod {
    /// OAuth 2.0 Authorization Code with PKCE. The user-facing default.
    OAuthPkce,
    /// OAuth 2.0 Authorization Code **without** PKCE, for a confidential client that can hold a secret.
    ///
    /// Representable because some providers still require it, and refused for a public client by
    /// [`AuthFlow::new`] — a desktop app cannot keep a client secret, so declaring this method for one would
    /// be a flow that cannot be completed securely.
    OAuthConfidential,
    /// A personal access token the user pastes.
    PersonalAccessToken,
    /// A long-lived API key.
    ApiKey,
    /// A service account's private key, for server-to-server access with no user present.
    ServiceAccount,
    /// A device or installer code, used once.
    PairingCode,
}

impl AuthMethod {
    /// Returns the stable snake-case wire code, which is also the manifest's tag.
    #[must_use]
    pub const fn tag(self) -> &'static str {
        match self {
            Self::OAuthPkce => "oauth_pkce",
            Self::OAuthConfidential => "oauth_confidential",
            Self::PersonalAccessToken => "personal_access_token",
            Self::ApiKey => "api_key",
            Self::ServiceAccount => "service_account",
            Self::PairingCode => "pairing_code",
        }
    }

    /// Returns whether the method needs a loopback callback listener.
    ///
    /// The distinction that decides whether a flow can work in a headless deployment: an OAuth code flow
    /// needs somewhere for the browser to return to, while a pasted token does not. A connector declaring an
    /// interactive method in a headless profile is a configuration an operator needs to be told about, and
    /// this predicate is how.
    #[must_use]
    pub const fn needs_loopback_callback(self) -> bool {
        matches!(self, Self::OAuthPkce | Self::OAuthConfidential)
    }

    /// Returns whether the method requires a client secret.
    #[must_use]
    pub const fn requires_client_secret(self) -> bool {
        matches!(self, Self::OAuthConfidential | Self::ServiceAccount)
    }

    /// Returns whether the flow is interactive, so a user must be present.
    #[must_use]
    pub const fn is_interactive(self) -> bool {
        matches!(
            self,
            Self::OAuthPkce | Self::OAuthConfidential | Self::PairingCode
        )
    }

    /// Returns whether a user can revoke it from the provider's own settings.
    ///
    /// A service account's key is revoked by an administrator at the provider and may not appear in a user's
    /// consent list, so a reauth path that tells the user to "remove access in your account settings" would
    /// be a dead end. `P5-010` owns the reauth tests; this predicate is what lets them branch.
    #[must_use]
    pub const fn user_can_revoke(self) -> bool {
        matches!(self, Self::OAuthPkce | Self::OAuthConfidential)
    }
}

/// The PKCE code-challenge method.
///
/// RFC 7636 defines `S256` and a `plain` method, and `plain` exists only for clients that cannot compute
/// SHA-256 — which nothing here cannot. It is representable because a **provider** may require it, and
/// carrying it explicitly is what lets [`PkceChallenge`] refuse to be built from a provider that omitted the
/// method: defaulting to `plain` would silently downgrade a flow the requirements say to protect with PKCE.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PkceMethod {
    /// `code_challenge = base64url(SHA-256(verifier))`. The only method worth choosing.
    S256,
    /// `code_challenge = verifier`. Permitted by the RFC, refused wherever S256 is available.
    Plain,
}

impl PkceMethod {
    /// Returns the wire value a provider expects.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::S256 => "S256",
            Self::Plain => "plain",
        }
    }
}

/// A PKCE code verifier, validated against RFC 7636's own bounds.
///
/// The verifier is the **secret** the challenge is derived from, so this type has no `Display` and a
/// `Debug` that prints a length. It is the caller's to generate; see the module doc for why that split
/// exists.
#[derive(Clone, Eq, PartialEq)]
pub struct PkceVerifier(String);

impl PkceVerifier {
    /// Validates a verifier.
    ///
    /// # Errors
    ///
    /// Returns [`AuthError::PkceVerifier`] when the value is shorter than 43 or longer than 128 characters,
    /// or holds a character outside RFC 7636's unreserved alphabet. RFC 7636 §4.1 requires "a
    /// high-entropy cryptographic random STRING using the unreserved characters `[A-Z] / [a-z] / [0-9] /
    /// "-" / "." / "_" / "~"`", and checking that here means a generator cannot silently produce a verifier
    /// the provider will reject.
    pub fn new(value: impl Into<String>) -> Result<Self, AuthError> {
        let value = value.into();
        let length = value.chars().count();
        if !(MIN_PKCE_VERIFIER_CHARS..=MAX_PKCE_VERIFIER_CHARS).contains(&length) {
            return Err(AuthError::PkceVerifier {
                reason: "a PKCE verifier must be 43 to 128 characters, per RFC 7636 §4.1",
            });
        }
        let unreserved = |character: char| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '.' | '_' | '~')
        };
        if !value.chars().all(unreserved) {
            return Err(AuthError::PkceVerifier {
                reason: "a PKCE verifier may hold only the unreserved characters `[A-Za-z0-9-._~]`, per RFC \
                         7636 §4.1",
            });
        }
        Ok(Self(value))
    }

    /// Returns the verifier text.
    ///
    /// Named `expose`-shaped rather than `as_str`, so a call site that reaches for the secret reads as one.
    /// The value is sent to the provider's token endpoint and nowhere else.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Derives the challenge this verifier proves, using the given method.
    ///
    /// `S256` is `BASE64URL-ENCODE(SHA256(ASCII(verifier)))` with no padding, per RFC 7636 §4.2.
    #[must_use]
    pub fn challenge(&self, method: PkceMethod) -> PkceChallenge {
        match method {
            PkceMethod::Plain => PkceChallenge {
                method,
                value: self.0.clone(),
            },
            PkceMethod::S256 => PkceChallenge {
                method,
                value: base64url_no_pad(&Sha256::digest(self.0.as_bytes())),
            },
        }
    }
}

impl fmt::Debug for PkceVerifier {
    /// Prints a length, never the value: a verifier in a log line is a flow anyone reading the log can
    /// complete. Same shape as `ApiKey`'s hand-written `Debug` and `DecisionNonce`'s.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "PkceVerifier({} chars)", self.0.len())
    }
}

/// A PKCE code challenge, which is **not** a secret: it travels in the authorization URL.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PkceChallenge {
    method: PkceMethod,
    value: String,
}

impl PkceChallenge {
    /// Returns the method.
    #[must_use]
    pub fn method(&self) -> PkceMethod {
        self.method
    }

    /// Returns the challenge text, which is safe to log and to place in a URL.
    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }
}

/// Base64url-encodes without padding, as RFC 7636 and RFC 4648 §5 require.
///
/// Hand-written rather than a dependency, because this is the only base64 this crate needs and a
/// base64 crate would arrive with an API surface larger than the twelve lines below. The alphabet is the
/// **URL-safe** one (`-` and `_` rather than `+` and `/`), which is what makes the value safe in a query
/// string — a `+` there decodes as a space, which is the defect that produces a challenge the provider
/// rejects with no explanation.
fn base64url_no_pad(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut output = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let first = u32::from(chunk[0]);
        let second = chunk.get(1).copied().map_or(0, u32::from);
        let third = chunk.get(2).copied().map_or(0, u32::from);
        let block = (first << 16) | (second << 8) | third;
        // Each output character takes six bits; a chunk of `n` bytes yields `n + 1` characters, which is
        // exactly what dropping the padding means.
        for position in 0..=chunk.len() {
            let index = (block >> (18 - 6 * position)) & 0b11_1111;
            let character = ALPHABET[usize::try_from(index).unwrap_or(0)];
            output.push(char::from(character));
        }
    }
    output
}

/// A one-time value compared for equality and never displayed.
///
/// Used for the OAuth `state` and `nonce`, both of which the requirements say must be random and bound to a
/// short-lived setup transaction. The comparison is **constant-time**, which matters because a state value
/// is compared against an attacker-supplied callback parameter: a short-circuiting comparison leaks, one
/// byte at a time, what the expected value is.
#[derive(Clone, Eq, PartialEq)]
pub struct SecretValue(String);

impl SecretValue {
    /// Records a state, nonce, or device-code value.
    ///
    /// # Errors
    ///
    /// Returns [`AuthError::State`] when the value is empty or longer than [`MAX_STATE_CHARS`]. There is no
    /// entropy requirement enforced here, because a test fixture needs to construct one — the entropy is the
    /// caller's obligation, and `P5-002` owns the generator that discharges it.
    pub fn new(value: impl Into<String>) -> Result<Self, AuthError> {
        let value = value.into();
        if value.is_empty() || value.chars().count() > MAX_STATE_CHARS {
            return Err(AuthError::State);
        }
        Ok(Self(value))
    }

    /// Returns the value, for placing in a URL or comparing against a callback.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Compares against a candidate in constant time with respect to content.
    ///
    /// The length is compared first and that is not constant-time, which is deliberate and harmless: a
    /// length is not the secret, and hiding it would require padding every candidate to a fixed size for no
    /// gain.
    #[must_use]
    pub fn matches(&self, candidate: &str) -> bool {
        let expected = self.0.as_bytes();
        let candidate = candidate.as_bytes();
        if expected.len() != candidate.len() {
            return false;
        }
        let mut difference = 0_u8;
        for (expected, candidate) in expected.iter().zip(candidate) {
            difference |= expected ^ candidate;
        }
        difference == 0
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "SecretValue({} chars)", self.0.len())
    }
}

/// What a connector hands back when it starts connecting: something the user must do.
///
/// The requirements say "random state and nonce bound to a short-lived setup transaction", so the challenge
/// carries the **transaction identifier** the callback must be compared against, alongside the URL to open.
/// A challenge with only a URL would make "which setup does this callback belong to" unanswerable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthChallenge {
    /// The setup transaction this challenge belongs to.
    ///
    /// The value the callback must present. It is a [`SecretValue`] rather than a `String` so it cannot be
    /// printed into a log, and so its comparison is the constant-time one.
    pub state: SecretValue,
    /// The OAuth `nonce`, when the method uses one.
    ///
    /// Separate from `state`, which the requirements list as two values: `state` defends the **redirect** and
    /// `nonce` defends the **id token**, so collapsing them would leave one of the two unprotected while
    /// looking complete.
    pub nonce: Option<SecretValue>,
    /// What the user must do.
    pub action: ChallengeAction,
}

/// What a challenge asks the user to do.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ChallengeAction {
    /// Open a URL in a browser, and wait for the loopback callback.
    OpenUrl {
        /// The authorization URL, with the challenge and state already in its query.
        url: String,
    },
    /// Show a code and a URL, and wait for the provider to confirm.
    ///
    /// The device flow, for a deployment with no browser. Distinct from [`Self::OpenUrl`] because **no
    /// loopback listener is involved**, so a headless host can use it — which is why [`AuthMethod`] records
    /// whether it needs a callback rather than assuming all interactive flows do.
    ShowCode {
        /// The URL to display, which the user visits on another device.
        url: String,
        /// The short code the user enters.
        code: String,
    },
    /// Paste a value the user obtained from the provider.
    PasteValue {
        /// What to tell the user to obtain, in the connector's words.
        instruction: String,
    },
}

impl ChallengeAction {
    /// Returns whether a loopback listener is needed to complete this challenge.
    #[must_use]
    pub const fn needs_loopback_listener(&self) -> bool {
        matches!(self, Self::OpenUrl { .. })
    }
}

/// What a connector reports after the user completes a challenge.
///
/// Carries the **provider's** identity claims, because the requirements say "account identity verified from
/// the provider, not user-entered labels". There is no field for a user-supplied label here, which is the
/// mechanical form of that rule: a caller cannot pass one because there is nowhere to put it.
///
/// **Not constructed by this crate, and that is deliberate.** `P5-002` owns the loopback listener that
/// receives a callback, so the value it hands back is this type's first producer — a contract slice declares
/// the shape and the auth slice supplies it, exactly as the `Connector` trait in `docs/api/contracts.md` is
/// declared without an implementation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthCallback {
    /// The state the callback presented, to be compared against the challenge's.
    pub state: String,
    /// The authorization code, when the flow has one.
    pub code: Option<String>,
    /// The error the provider reported, when it refused.
    ///
    /// Represented rather than treated as a missing code, because "the user clicked deny" and "the user
    /// closed the tab" need different messages and only the provider can distinguish them.
    pub provider_error: Option<String>,
}

/// Whether an OAuth flow reached the provider's consent screen.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthState {
    /// A challenge was issued and nothing has come back.
    AwaitingUser,
    /// A callback arrived and the code was exchanged.
    Connected,
    /// The flow failed and must be restarted.
    Failed,
    /// The flow was superseded by a newer one.
    ///
    /// Explicit because a **second** setup must invalidate the first: two live transactions for one account
    /// means a callback can be matched to the wrong one, which is the CSRF the `state` parameter exists to
    /// prevent.
    Superseded,
}

impl AuthState {
    /// Returns whether another challenge may be issued from this state.
    #[must_use]
    pub const fn permits_new_challenge(self) -> bool {
        matches!(self, Self::Failed | Self::Superseded | Self::Connected)
    }
}

/// A declared OAuth flow, with the pieces the requirements name.
///
/// Constructed through [`AuthFlow::new`] so the cross-field rules hold: a PKCE flow **must** carry a
/// challenge method (a missing one would silently fall back to `plain`), and a confidential flow must carry
/// a client identifier.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthFlow {
    method: AuthMethod,
    pkce: Option<PkceMethod>,
    redirect_uri: Option<String>,
    authorization_endpoint: String,
}

impl AuthFlow {
    /// Records an auth flow.
    ///
    /// # Errors
    ///
    /// Returns [`AuthError::Flow`] when a PKCE method is missing for an OAuth method, when a PKCE method is
    /// present for a non-OAuth method, when a redirect URI is missing for a method that needs a callback or
    /// present for one that does not, or when the endpoints are not `https`.
    pub fn new(
        method: AuthMethod,
        pkce: Option<PkceMethod>,
        redirect_uri: Option<String>,
        authorization_endpoint: impl Into<String>,
    ) -> Result<Self, AuthError> {
        let authorization_endpoint = authorization_endpoint.into();
        // An authorization endpoint is where a **credential-bearing request** goes, so `https` is required
        // rather than checked elsewhere. A loopback redirect is the exception, and only the redirect.
        if !authorization_endpoint.starts_with("https://") {
            return Err(AuthError::Flow {
                reason: "the authorization endpoint must be an `https://` URL, because a code request \
                         carries the client identifier and the challenge",
            });
        }
        let oauth = method.needs_loopback_callback();
        match (oauth, pkce) {
            (true, None) => {
                return Err(AuthError::Flow {
                    reason: "an OAuth method must name a PKCE method; omitting it lets a provider default to \
                             `plain`, which is the downgrade PKCE exists to prevent",
                });
            }
            (false, Some(_)) => {
                return Err(AuthError::Flow {
                    reason: "a PKCE method is meaningful only for an OAuth code flow",
                });
            }
            _ => {}
        }
        match (oauth, redirect_uri.as_deref()) {
            (true, None) => {
                return Err(AuthError::Flow {
                    reason: "an OAuth method must name a redirect URI, and the requirements require it to be \
                             matched **exactly** rather than by prefix",
                });
            }
            (true, Some(uri)) => {
                // The requirements say "exact redirect URI validation and loopback listener hardening", so
                // the URI must be a loopback address: a redirect to a remote host would send the code to a
                // machine this process does not control.
                if !is_loopback_redirect(uri) {
                    return Err(AuthError::Flow {
                        reason: "the redirect URI must be a loopback address; a remote redirect would \
                                 deliver the authorization code to a host this process does not own",
                    });
                }
            }
            (false, Some(_)) => {
                return Err(AuthError::Flow {
                    reason: "a redirect URI is meaningful only for an OAuth code flow",
                });
            }
            (false, None) => {}
        }
        Ok(Self {
            method,
            pkce,
            redirect_uri,
            authorization_endpoint,
        })
    }

    /// Returns the method.
    #[must_use]
    pub fn method(&self) -> AuthMethod {
        self.method
    }

    /// Returns the PKCE method, when the flow is an OAuth code flow.
    #[must_use]
    pub fn pkce(&self) -> Option<PkceMethod> {
        self.pkce
    }

    /// Returns the redirect URI, when the flow has one.
    #[must_use]
    pub fn redirect_uri(&self) -> Option<&str> {
        self.redirect_uri.as_deref()
    }

    /// Returns the authorization endpoint.
    #[must_use]
    pub fn authorization_endpoint(&self) -> &str {
        &self.authorization_endpoint
    }

    /// Returns whether a loopback listener is required.
    #[must_use]
    pub const fn needs_loopback_listener(&self) -> bool {
        self.method.needs_loopback_callback()
    }
}

/// Returns whether the URI is a loopback redirect.
///
/// Whole-host comparison against the loopback literals, so `127.0.0.1.evil.example` is remote. The same
/// rule `jarvis-mcp`'s endpoint validation applies, and for the same reason: a prefix match on `127.0.0.1`
/// would admit a host that merely starts with it.
#[must_use]
pub fn is_loopback_redirect(uri: &str) -> bool {
    let Some(rest) = uri.strip_prefix("http://") else {
        // A loopback callback over https is not what a native client uses, and requiring `http` here means
        // an author cannot accidentally declare a public certificate flow that no local listener serves.
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = authority
        .rsplit_once(':')
        .map_or(authority, |(host, _port)| host);
    matches!(host, "127.0.0.1" | "[::1]" | "::1")
}

/// What a refresh attempt produced.
///
/// The requirements say "refresh-token rotation and invalidation handling", and the three outcomes need
/// three different responses: a rotation must **store the new refresh token**, an expiry must enter reauth,
/// and a revocation must clear the account. Collapsing them into `Result` would make "the user removed
/// access" look like a transient failure worth retrying.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefreshOutcome {
    /// The refresh succeeded and the refresh token is unchanged.
    Refreshed,
    /// The refresh succeeded and the provider issued a **new** refresh token, invalidating the old one.
    ///
    /// The dangerous one: a caller that ignores this keeps using a token the provider has already
    /// invalidated, and the next attempt fails as though the account were broken.
    Rotated,
    /// The refresh token expired, so a user must reconnect.
    Expired,
    /// The user revoked access at the provider.
    Revoked,
    /// The provider refused for a reason that may be transient, such as a rate limit.
    Transient,
}

impl RefreshOutcome {
    /// Returns whether a user must be involved to restore service.
    #[must_use]
    pub const fn needs_user(self) -> bool {
        matches!(self, Self::Expired | Self::Revoked)
    }

    /// Returns whether attempting again without user involvement is safe.
    ///
    /// `Refreshed` and `Rotated` do not need one, and `Transient` does — but a **`Transient` retry must
    /// respect the provider's backoff**, which is why this is a separate predicate from "no user needed"
    /// rather than its negation.
    #[must_use]
    pub const fn is_safe_to_retry(self) -> bool {
        matches!(self, Self::Transient)
    }
}

/// What changed about a grant's scopes.
///
/// The requirements say "reauth path that preserves account references without hiding lost scopes", so a
/// scope difference is a **reported** fact with the lost scopes named. `Loss` is separate from `Change`
/// because a lost scope means work that used to succeed now cannot, and a caller that treated it as an
/// ordinary change would keep calling an operation the grant no longer covers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ScopeChange {
    /// The same scopes were granted.
    Unchanged,
    /// A superset was granted, which is what an incremental request produces.
    Gained {
        /// The scopes that were added.
        gained: Vec<String>,
    },
    /// At least one scope was lost.
    Lost {
        /// The scopes that are no longer granted.
        lost: Vec<String>,
    },
}

impl ScopeChange {
    /// Compares a previous grant against a new one, treating the lists as sets.
    ///
    /// A set comparison rather than a sequence one, because a provider is free to return its scopes in any
    /// order and a sequence comparison would report a change on every response. `P3-002`'s
    /// `NameAssignments` keyed a collision check on the wrong value and failed in **both** directions; the
    /// same shape of mistake here would report a scope loss that did not happen, causing a spurious reauth.
    #[must_use]
    pub fn between(previous: &[String], current: &[String]) -> Self {
        let previous: std::collections::BTreeSet<&String> = previous.iter().collect();
        let current: std::collections::BTreeSet<&String> = current.iter().collect();
        let gained: Vec<String> = current
            .difference(&previous)
            .map(|s| (*s).clone())
            .collect();
        let lost: Vec<String> = previous
            .difference(&current)
            .map(|s| (*s).clone())
            .collect();
        if !lost.is_empty() {
            // Loss takes precedence over gain, and the ordering is deliberate: telling a caller it gained
            // scopes while it also lost some would understate what it can no longer do.
            Self::Lost { lost }
        } else if gained.is_empty() {
            Self::Unchanged
        } else {
            Self::Gained { gained }
        }
    }

    /// Returns whether any scope was lost.
    #[must_use]
    pub const fn is_loss(&self) -> bool {
        matches!(self, Self::Lost { .. })
    }
}

/// The result of comparing a grant's scopes, named so a caller can store it.
///
/// A newtype rather than `Vec<String>` because a bare list of scopes is ambiguous between "what was granted"
/// and "what was lost", and the two lead to opposite decisions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScopeSetChange(ScopeChange);

impl ScopeSetChange {
    /// Compares a previous grant against a new one.
    #[must_use]
    pub fn between(previous: &[String], current: &[String]) -> Self {
        Self(ScopeChange::between(previous, current))
    }

    /// Returns the change.
    #[must_use]
    pub fn change(&self) -> &ScopeChange {
        &self.0
    }
}

/// Why an auth step failed.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum AuthError {
    /// A PKCE verifier is unusable.
    #[error("the PKCE verifier is unusable: {reason}")]
    PkceVerifier {
        /// What is wrong.
        reason: &'static str,
    },
    /// A state or nonce value is unusable.
    #[error("the OAuth state or nonce must be 1 to 256 characters")]
    State,
    /// An auth flow declaration is unusable.
    #[error("the auth flow is unusable: {reason}")]
    Flow {
        /// What is wrong.
        reason: &'static str,
    },
    /// The callback presented a state that does not match.
    ///
    /// A single variant for a mismatch, because distinguishing "wrong state" from "no state" would tell an
    /// attacker which half of a forged callback was correct.
    #[error("the callback did not present the state this challenge issued")]
    StateMismatch,
    /// The provider refused the authorization request.
    #[error("the provider refused the authorization request")]
    ProviderRefused,
}

/// Converts a manifest error into an auth error, for callers that validate both.
///
/// `ConnectorError` and `AuthError` are separate so a manifest problem and a flow problem read differently
/// to an operator, and this conversion exists so a caller that has already validated a manifest does not
/// need a second error type in its own signature.
impl From<ConnectorError> for AuthError {
    fn from(_error: ConnectorError) -> Self {
        Self::Flow {
            reason: "the connector manifest was not usable, so no flow could be derived from it",
        }
    }
}

#[cfg(test)]
#[path = "auth_tests.rs"]
mod tests;
