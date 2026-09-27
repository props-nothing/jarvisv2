//! The token response, refresh rotation, and revocation.
//!
//! This module is where `tools-and-connectors.md`'s storage rule becomes a property of a type rather than a
//! convention:
//!
//! > No token material in model context, URLs/logs, diagnostics, or normal database columns.
//!
//! # The absence that does the work
//!
//! **[`TokenSet`] has no `access_token` field.** That is the whole design. RFC 6749 §5.1's response is
//! `{access_token, token_type, expires_in, refresh_token, scope}`, and this type records the lifetime, the
//! type, the granted scope, and a [`SecretRef`] that *locates* the refresh material — and nothing else. So
//! there is no expression in this crate for "hand the access token to the next step", which means a model
//! context, a log line, a diagnostic, and a database column cannot receive one even by mistake. `P5-001`
//! established the same absence for the manifest and the auth module; this extends it to the exchange.
//!
//! The access token still exists — an adapter needs it to call the provider — but it exists in the adapter's
//! local scope for the length of one call, which is what `security.md` asks for ("short-lived in memory,
//! zeroized where practical, and passed only to the adapter operation that requires them").
//!
//! # Why a rotation is *detected* rather than merely recorded
//!
//! RFC 9700 §2.2.2 requires that refresh tokens for public clients "MUST be sender-constrained or use refresh
//! token rotation", and §4.14.2 explains what rotation buys:
//!
//! > the authorization server issues a new refresh token with every access token refresh response. The
//! > previous refresh token is invalidated, but information about the relationship is retained […]. If a
//! > refresh token is compromised and subsequently used by both the attacker and the legitimate client, one
//! > of them will present an invalidated refresh token, which will inform the authorization server of the
//! > breach.
//!
//! So the *client's* job is to notice which of the two shapes came back, because that is what makes the server
//! able to detect the replay at all: a client that kept using its old refresh token would throw away the
//! defence. [`RefreshExchange::classify`] is that noticing step, and it is a separate function from the error
//! mapping so a test can pin both without a network.
//!
//! # What is deliberately absent
//!
//! **No HTTP and no JSON parsing.** A response arrives as a [`TokenResponse`] the caller constructed from
//! whatever it received, and an error arrives as [`TokenEndpointFailure`] the caller classified from a status
//! code and body. That is the `P3-008h` division — an adapter's outcome mapping is the honesty boundary — and
//! it means every branch below is verifiable as a function of its arguments.
//!
//! **No `invalid_grant` reasoning about *why*.** RFC 9700 §4.14.2 notes the server "cannot determine which
//! party submitted the invalid refresh token", and no error code distinguishes "revoked" from "expired", so
//! the classification is the connector's, informed by its vendor record. See Unresolved Question 2 in
//! `docs/research/integrations/oauth2-pkce-native-apps.md`.

use std::fmt;

use jarvis_core::SecretRef;
use serde::{Deserialize, Serialize};

use crate::auth::{AuthError, RefreshOutcome, ScopeSetChange};

/// The longest accepted `token_type` value.
pub const MAX_TOKEN_TYPE_CHARS: usize = 32;

/// The longest accepted access-token lifetime, in seconds.
///
/// 24 hours, and it is a **refusal rather than a clamp**: RFC 6749 makes `expires_in` the server's statement
/// about how long the access token is valid, and a client that shortened it would be discarding the only
/// information it has about the token's lifetime. A value above this is either a server defect or a response
/// that is not a token, and both are worth failing on. A year in seconds would be `31_536_000`, so the bound
/// admits every conventional value (Google 3600, Microsoft 3600–4800, GitHub absent) with room to spare.
pub const MAX_ACCESS_TOKEN_SECONDS: u64 = 86_400;

/// The access token type JARVIS expects, per RFC 6750.
pub const BEARER_TOKEN_TYPE: &str = "Bearer";

/// The provider's token-endpoint response, as the caller decoded it.
///
/// Every field is optional because every one of them is optional in some real response: RFC 6749 §5.1 makes
/// only `access_token` and `token_type` required, `expires_in` is RECOMMENDED, and `refresh_token` is optional
/// (a server may decline to issue one at all, §4.14.2's "authorization servers MUST determine, based on a risk
/// assessment, whether to issue refresh tokens").
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TokenResponse {
    /// The `token_type`, which RFC 6750 defines as `Bearer` for OAuth.
    pub token_type: Option<String>,
    /// The `expires_in`, in seconds.
    pub expires_in: Option<i64>,
    /// The `scope`, which may differ from what was requested.
    pub scope: Option<String>,
    /// Whether the response carried a `refresh_token`.
    ///
    /// A **bool rather than the value**: the caller holds the refresh token, and all this type may know is
    /// whether one arrived, which is exactly what [`RefreshExchange::classify`] needs. Storing the text here
    /// would put the long-lived credential in a value that gets `Debug`-printed, logged, and carried between
    /// layers — the defect `P5-001` removed from the manifest and this module exists to not reintroduce.
    pub has_refresh_token: bool,
    /// Whether the response carried an ID token.
    ///
    /// Recorded so a caller can see that the flow was `OpenID` Connect and that a `nonce` comparison was
    /// therefore possible. This crate does not verify an ID token (see the research record's Unresolved
    /// Question 5), and it does **not** read the token's claims, so nothing here can be mistaken for
    /// validation.
    pub has_id_token: bool,
}

impl TokenResponse {
    /// Records the fields a token response may carry.
    #[must_use]
    pub fn new(
        token_type: Option<String>,
        expires_in: Option<i64>,
        scope: Option<String>,
        has_refresh_token: bool,
        has_id_token: bool,
    ) -> Self {
        Self {
            token_type,
            expires_in,
            scope,
            has_refresh_token,
            has_id_token,
        }
    }

    /// Returns whether the `token_type` is one this platform can use.
    ///
    /// An **absent** `token_type` is accepted, because RFC 6749 §5.1 makes it required in the response but a
    /// deployment that omits it would otherwise fail a flow it actually completed, and `Bearer` is the only
    /// type OAuth 2.0 defines for a resource request (RFC 6750). A **different** type is refused: a token the
    /// platform would send as `Authorization: Bearer …` while the server meant something else is a
    /// mis-authenticated request, not a degraded one.
    ///
    /// The comparison is case-insensitive because RFC 6750 §2.1 defines the scheme as `Bearer` while HTTP's
    /// own rule for a scheme token (RFC 9110) is case-insensitive, so a server that sent `bearer` is
    /// conforming and must not be refused.
    #[must_use]
    pub fn token_type_is_usable(&self) -> bool {
        match &self.token_type {
            None => true,
            Some(token_type) => token_type.trim().eq_ignore_ascii_case(BEARER_TOKEN_TYPE),
        }
    }

    /// Returns the declared lifetime, when one is usable.
    ///
    /// `None` for absent, zero, or negative: all three mean "no usable lifetime was stated", and the caller's
    /// remedy is the same in each case (treat the token as short-lived and be ready for a `401`). Distinguishing
    /// them would invite a caller to treat `0` as a valid instant and refuse everything.
    #[must_use]
    pub fn lifetime_seconds(&self) -> Option<u64> {
        match self.expires_in {
            Some(seconds) if seconds > 0 => u64::try_from(seconds).ok(),
            _ => None,
        }
    }

    /// Returns whether the declared lifetime is within what this platform will accept.
    #[must_use]
    pub fn lifetime_is_bounded(&self) -> bool {
        match self.lifetime_seconds() {
            None => true,
            Some(seconds) => seconds <= MAX_ACCESS_TOKEN_SECONDS,
        }
    }
}

/// How a token request ended, with the retry-safety question answered.
///
/// # Why this is a separate type from an error
///
/// RFC 9700 §4.2.4 requires that a code "MUST be invalidated by the authorization server after their first use
/// at the token endpoint", and that when a code is redeemed twice the server "SHOULD revoke all tokens issued
/// previously based on that code". Put together with an unreliable network, that means:
///
/// - a request that was **never sent** can be retried freely;
/// - a request that was **sent** and whose answer was lost may, on retry, get `invalid_grant` **and destroy a
///   working grant** that the first attempt had already issued.
///
/// Those two are indistinguishable from the request side and completely different operationally, so a caller
/// has to choose. Making it a variant rather than a `bool` is what forces the choice to be made rather than
/// inherited from a retry loop's default. This is the same shape as `P3-001`'s
/// `RefusedBeforeReaching`/`AmbiguousAfterReaching`, one protocol over.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TokenRequestOutcome {
    /// The request produced a response, and the answer is a token set.
    Answered(Box<TokenSet>),
    /// The request produced a response, and the answer is a refusal.
    Refused(TokenEndpointFailure),
    /// The request was never sent.
    ///
    /// Safe to retry: nothing reached the provider, so no code was consumed.
    NeverSent {
        /// Why it was not sent.
        reason: &'static str,
    },
    /// The request was sent and no answer arrived.
    ///
    /// **Not safe to retry blindly.** The provider may have consumed the authorization code and issued tokens
    /// that this client will never see. The caller's options are to re-run the authorization flow (which
    /// requires the user) or to wait for the code to expire — but they must be *chosen*.
    SentAnswerUnknown,
}

impl TokenRequestOutcome {
    /// Returns whether an automatic retry is permitted.
    #[must_use]
    pub const fn permits_automatic_retry(&self) -> bool {
        match self {
            Self::NeverSent { .. } => true,
            // A refusal is a definite answer, so retrying the *same* code is pointless — the code is consumed
            // or the request was malformed. `RetryClass`'s reasoning applies one layer down: an answered "no"
            // is not retried because the answer will be the same.
            Self::Answered(_) | Self::Refused(_) | Self::SentAnswerUnknown => false,
        }
    }

    /// Returns the token set, when the request was answered with one.
    #[must_use]
    pub fn token_set(&self) -> Option<&TokenSet> {
        match self {
            Self::Answered(set) => Some(set),
            _ => None,
        }
    }
}

/// A usable token set, with no token material in it.
///
/// See the module doc for why `access_token` and `refresh_token` are absent rather than redacted. The short
/// version: a field that exists can be printed, logged, or stored, and a field that does not exist cannot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TokenSet {
    /// Where the refresh material lives, when the provider issued any.
    ///
    /// A [`SecretRef`] — `jarvis_core`'s "metadata that locates a secret without containing the secret value",
    /// which is deliberately not `Serialize` so it cannot reach a wire contract or a diagnostics payload.
    pub refresh_reference: Option<SecretRef>,
    /// The lifetime the provider stated for the access token.
    pub lifetime_seconds: Option<u64>,
    /// The scopes the provider granted, which may be fewer than were requested.
    pub granted_scopes: Vec<String>,
    /// How the granted scopes compare with what the previous grant held.
    ///
    /// Computed here rather than at the call site because the comparison needs *both* lists and a caller that
    /// only had the new one could not produce it. `ScopeChange::between` already makes a **loss take
    /// precedence** over a gain: a refresh that gains one scope and loses another is a loss, because the loss
    /// is what breaks calls.
    pub scope_change: ScopeSetChange,
}

impl TokenSet {
    /// Builds a token set from a response and the scopes the account previously held.
    ///
    /// # Errors
    ///
    /// Returns [`AuthError::Flow`] when the response's `token_type` is not one this platform can use, or when
    /// its `expires_in` exceeds [`MAX_ACCESS_TOKEN_SECONDS`]. Both are responses that completed but cannot be
    /// used, which is a different thing from a refusal and is reported as a flow error so a caller does not
    /// read it as "the provider said no".
    ///
    /// `refresh_reference` is supplied by the caller because only the caller can put the refresh material into
    /// a secret store; this type records where it went.
    pub fn from_response(
        response: &TokenResponse,
        refresh_reference: Option<SecretRef>,
        previous_scopes: &[String],
        requested_scopes: &[String],
    ) -> Result<Self, AuthError> {
        if !response.token_type_is_usable() {
            return Err(AuthError::Flow {
                reason: "the token response declared a token type this platform cannot use; OAuth defines \
                         only `Bearer` (RFC 6750 §2.1)",
            });
        }
        if !response.lifetime_is_bounded() {
            return Err(AuthError::Flow {
                reason: "the token response declared an access-token lifetime beyond what this platform \
                         accepts, which is a server defect or a response that is not a token",
            });
        }
        // The **granted** scopes are compared, and when the response states none the requested set is used
        // instead. RFC 6749 §5.1 says an absent `scope` means "the scope granted is the one requested"; treating
        // it as "no scopes" would report a scope loss on every refresh from a server that simply omitted the
        // field, which is the false positive that makes a real loss easy to dismiss.
        let granted = match &response.scope {
            Some(scope) => split_scope(scope),
            None => requested_scopes.to_vec(),
        };
        let scope_change = ScopeSetChange::between(previous_scopes, &granted);
        Ok(Self {
            refresh_reference,
            lifetime_seconds: response.lifetime_seconds(),
            granted_scopes: granted,
            scope_change,
        })
    }

    /// Returns whether any scope was lost relative to the previous grant.
    #[must_use]
    pub fn lost_scopes(&self) -> bool {
        self.scope_change.change().is_loss()
    }
}

/// Splits an OAuth `scope` parameter into its values.
///
/// RFC 6749 §3.3 defines it as space-delimited, and empty items are dropped: a scope string with a repeated or
/// trailing space would otherwise produce an empty scope, which would then be *reported* as a granted scope
/// that no tool requires and no provider issued.
#[must_use]
pub fn split_scope(scope: &str) -> Vec<String> {
    scope
        .split(' ')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

/// A token request the provider refused.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TokenEndpointFailure {
    /// The `error` code, e.g. `invalid_grant`, `invalid_client`, `unauthorized_client`.
    pub error: String,
    /// The `error_description`, which RFC 6749 §5.2 marks as *not* for display to the user.
    pub description: Option<String>,
    /// Whether the provider indicated a transient failure.
    ///
    /// **Caller-supplied**, because the protocol has no such code: RFC 6749 §5.2's error values describe the
    /// *request*, not the server's health, so "this is a 503" is knowledge the transport has and this type does
    /// not. That is why it is a field rather than something derived from `error`.
    pub transient: bool,
}

impl TokenEndpointFailure {
    /// Records a refusal.
    #[must_use]
    pub fn new(error: impl Into<String>, description: Option<String>, transient: bool) -> Self {
        Self {
            error: error.into(),
            description,
            transient,
        }
    }

    /// Returns whether the error means the grant cannot be used again without the user.
    ///
    /// RFC 6749 §5.2 defines `invalid_grant` as "the provided authorization grant […] is invalid, expired,
    /// revoked, does not match the redirection URI used in the authorization request, or was issued to another
    /// client". Every one of those requires a new authorization, which is why it is the one code that maps to
    /// a user-visible reauth rather than to a retry.
    ///
    /// `invalid_client`/`unauthorized_client` are **not** included, deliberately: they mean the *client* is
    /// misconfigured, so sending the user to reauthorize would have them complete a consent screen and land on
    /// the same failure. Those are configuration faults and are reported as such.
    #[must_use]
    pub fn requires_reauth(&self) -> bool {
        self.error.eq_ignore_ascii_case("invalid_grant")
    }
}

/// The refresh exchange: what happened, and whether it must be repeated by the user.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefreshExchange {
    /// The classified outcome.
    pub outcome: RefreshOutcome,
    /// The token set, when the exchange succeeded.
    pub token_set: Option<TokenSet>,
    /// Where the refresh material moved to, when the provider rotated it.
    ///
    /// `None` on a plain refresh (the previous reference still stands) and on every failure. Present on a
    /// rotation, because the old reference is now invalid at the provider and a caller that kept using it would
    /// be the very replay RFC 9700 §4.14.2 wants detected.
    pub rotated_reference: Option<SecretRef>,
}

impl RefreshExchange {
    /// Classifies a refresh response or failure into a [`RefreshOutcome`].
    ///
    /// # The rotation test is the point
    ///
    /// RFC 9700 §4.14.2 defines rotation as "the authorization server issues a new refresh token with every
    /// access token refresh response". So:
    ///
    /// - a response **with** a new refresh token is [`RefreshOutcome::Rotated`];
    /// - a response **without** one is [`RefreshOutcome::Refreshed`] — the provider does not rotate, which is
    ///   legal but is the configuration §2.2.2 requires sender-constraining to compensate for (Unresolved
    ///   Question 3 in the research record);
    /// - an `invalid_grant` is [`RefreshOutcome::Expired`] or [`RefreshOutcome::Revoked`] per the caller's
    ///   `vendor_says_revoked` signal, because the protocol does not distinguish them;
    /// - a transient or otherwise non-grant failure is [`RefreshOutcome::Transient`].
    ///
    /// The `vendor_says_revoked` argument is a **parameter rather than a field read from the error**, because
    /// no error code carries it: RFC 6749 §5.2 lists no revocation-specific value, and inventing one would mean
    /// guessing which of the two a given provider means. Naming it in the signature makes the connector's
    /// vendor knowledge the input it actually is.
    #[must_use]
    pub fn classify(
        response: Option<&TokenResponse>,
        failure: Option<&TokenEndpointFailure>,
        new_reference: Option<SecretRef>,
        previous_scopes: &[String],
        requested_scopes: &[String],
        vendor_says_revoked: bool,
    ) -> Self {
        if let Some(failure) = failure {
            // The transient check is FIRST, and the ordering is load-bearing rather than stylistic. A provider
            // that answers a refresh with HTTP 503 normally answers through the protocol's own error channel,
            // and a reverse proxy or a JSON API framework in front of it will not always pick a code that
            // describes the outage rather than the request — a 503 carrying `invalid_grant` is a real shape. If
            // the grant check ran first, that response would send the user to a consent screen during a
            // provider outage, which finds the same failure and looks like a broken connector. So "the
            // provider is unwell" outranks "the server said this code", because the transport's observation is
            // the more specific one.
            if failure.transient {
                return Self {
                    outcome: RefreshOutcome::Transient,
                    token_set: None,
                    rotated_reference: None,
                };
            }
            if failure.requires_reauth() {
                return Self {
                    outcome: if vendor_says_revoked {
                        RefreshOutcome::Revoked
                    } else {
                        RefreshOutcome::Expired
                    },
                    token_set: None,
                    rotated_reference: None,
                };
            }
            // Any other refusal is the client's own fault (`invalid_client`, `unauthorized_client`) or a
            // request the server rejected, and none of those is resolved by asking the user for consent. It is
            // reported as transient so `is_safe_to_retry` is false and `needs_user` is false, which is the
            // honest pair for "something is wrong that the user cannot fix".
            return Self {
                outcome: RefreshOutcome::Transient,
                token_set: None,
                rotated_reference: None,
            };
        }
        let Some(response) = response else {
            // Neither a response nor a failure is a caller defect, and it is reported as a transient failure
            // rather than as success: a refresh that produced nothing must not read as a refresh that worked.
            return Self {
                outcome: RefreshOutcome::Transient,
                token_set: None,
                rotated_reference: None,
            };
        };
        let Ok(token_set) = TokenSet::from_response(
            response,
            new_reference.clone(),
            previous_scopes,
            requested_scopes,
        ) else {
            // A response that cannot be used is not a refusal; it is a completed exchange whose answer this
            // platform will not accept, and the honest outcome is "retry may help" rather than "the user must
            // act".
            return Self {
                outcome: RefreshOutcome::Transient,
                token_set: None,
                rotated_reference: None,
            };
        };
        // Rotation is decided by whether a new refresh token arrived, NOT by whether the caller supplied a
        // reference for it. A caller that failed to store one has a defect of its own, and reporting
        // `Refreshed` would hide the half of the exchange that makes replay detectable.
        let outcome = if response.has_refresh_token {
            RefreshOutcome::Rotated
        } else {
            RefreshOutcome::Refreshed
        };
        Self {
            outcome,
            token_set: Some(token_set),
            rotated_reference: if response.has_refresh_token {
                new_reference
            } else {
                None
            },
        }
    }

    /// Returns whether the account must be reconnected.
    #[must_use]
    pub const fn needs_user(&self) -> bool {
        self.outcome.needs_user()
    }
}

/// What kind of token material is being revoked.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevocationKind {
    /// Only the access token, leaving the grant intact.
    AccessToken,
    /// Only the refresh token.
    RefreshToken,
    /// The whole grant, which is what a user-initiated disconnect does.
    Grant,
}

impl RevocationKind {
    /// Returns the stable snake-case token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AccessToken => "access_token",
            Self::RefreshToken => "refresh_token",
            Self::Grant => "grant",
        }
    }

    /// Returns the RFC 7009 `token_type_hint` to send, when the protocol defines one.
    ///
    /// `None` for [`Self::Grant`] because RFC 7009 revokes **one token at a time** and defines no hint for
    /// "everything". A whole-grant revocation is therefore two calls (refresh then access) or a provider
    /// extension, and the caller has to know that rather than send a hint no server understands. RFC 7009 also
    /// says the hint is advisory and the server "MAY" ignore it, which is why the hint is not used to *select*
    /// anything locally.
    #[must_use]
    pub const fn token_type_hint(self) -> Option<&'static str> {
        match self {
            Self::AccessToken => Some("access_token"),
            Self::RefreshToken => Some("refresh_token"),
            Self::Grant => None,
        }
    }

    /// Returns whether a successful revocation leaves the account needing the user.
    #[must_use]
    pub const fn requires_reauth_afterwards(self) -> bool {
        matches!(self, Self::RefreshToken | Self::Grant)
    }
}

/// How a revocation ended.
///
/// Three outcomes rather than a `Result<(), Error>`, because RFC 7009 §2.2 makes "already invalid" a
/// **success** — "the authorization server responds with HTTP status code 200 if the token has been revoked
/// successfully or if the client submitted an invalid token" — so a caller that treated it as an error would
/// report a failure for a disconnect that worked. And "the provider has no revocation endpoint" is neither of
/// those: it is an unsupported capability, whose remedy is different again.
///
/// # Local material is discarded on every outcome, including the failures
///
/// That is a caller obligation rather than a method here, because it is unconditional: keeping a credential the
/// user asked to disconnect serves nothing, and a later successful retry needs no local token to perform. The
/// reason it is worth stating is that it is the *opposite* of what [`Self::is_withdrawn`] reports, and those
/// two facts are what let a caller be honest ("we forgot it; we could not reach the provider") instead of
/// choosing between two lies. An always-`true` method was written here first and removed, because a predicate
/// with one answer is not a predicate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RevocationOutcome {
    /// The provider revoked it.
    Revoked,
    /// The provider reported the token was already invalid, which is success.
    AlreadyInvalid,
    /// The provider has no revocation endpoint, so the token cannot be withdrawn early.
    ///
    /// Recorded as a first-class outcome because it changes what a disconnect means: without a revocation
    /// endpoint the only way a grant ends is for it to expire or for the user to withdraw consent at the
    /// provider, and a caller that reported "disconnected" would be claiming an effect it did not achieve.
    Unsupported {
        /// Why the capability is absent.
        reason: &'static str,
    },
    /// The provider was reached and refused the revocation.
    Refused {
        /// The `error` code.
        error: String,
    },
    /// The provider could not be reached, so the revocation did not happen.
    Unreachable,
}

impl RevocationOutcome {
    /// Returns the stable snake-case code.
    ///
    /// Takes `&self` rather than `self` because the enum is not `Copy` — `Refused` carries a `String` — and a
    /// by-value receiver in a `const fn` cannot be compiled while the type has a destructor. Changing the
    /// receiver rather than the derive is the right direction: losing `Copy` on an error-shaped enum costs
    /// nothing, and shrinking `Refused` to a `&'static str` would lose the provider's own error code.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Revoked => "revoked",
            Self::AlreadyInvalid => "already_invalid",
            Self::Unsupported { .. } => "unsupported",
            Self::Refused { .. } => "refused",
            Self::Unreachable => "unreachable",
        }
    }

    /// Returns whether the token is no longer usable at the provider.
    ///
    /// `AlreadyInvalid` counts, because RFC 7009 §2.2 defines it as success. `Unsupported`, `Refused`, and
    /// `Unreachable` do not: in each the token may still work, and a caller that assumed otherwise would leave
    /// a live credential behind while telling the user it was withdrawn. That is the failure direction worth
    /// designing against — an overclaimed revocation.
    #[must_use]
    pub fn is_withdrawn(&self) -> bool {
        matches!(self, Self::Revoked | Self::AlreadyInvalid)
    }
}

impl fmt::Display for RevocationOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Revoked => formatter.write_str("the provider revoked the token"),
            Self::AlreadyInvalid => {
                formatter.write_str("the provider reported the token was already invalid")
            }
            Self::Unsupported { reason } => {
                write!(
                    formatter,
                    "the provider cannot revoke tokens early: {reason}"
                )
            }
            Self::Refused { error } => {
                write!(formatter, "the provider refused the revocation: {error}")
            }
            Self::Unreachable => formatter.write_str("the provider could not be reached"),
        }
    }
}

#[cfg(test)]
#[path = "token_tests.rs"]
mod tests;
