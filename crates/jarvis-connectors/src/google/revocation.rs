//! Revocation: what Google's endpoint takes, and what a `200` actually means.
//!
//! # The finding this module exists for
//!
//! `crate::token::RevocationKind` is **protocol-correct** — its rules come from RFC 7009, where a
//! `token_type_hint` selects which token to revoke and the others are untouched. Google's behaviour is not
//! RFC 7009's, and the difference is not a detail:
//!
//! > **"Revocation removes all OAuth 2.0 scopes previously granted to a project, invalidating any issued access
//! > or refresh tokens for all clients registered under that project."**
//! >
//! > **"the token can be an access token or a refresh token. If the token is an access token and it has a
//! > corresponding refresh token, the refresh token will also be revoked"**
//! > — `https://developers.google.com/identity/protocols/oauth2/web-server` (last updated 2026-09-14)
//!
//! So on Google, revoking an **access** token revokes the refresh token that pairs with it. That directly
//! contradicts `RevocationKind::AccessToken.requires_reauth_afterwards()`, which returns `false` on the
//! reasonable protocol assumption that a hint selecting the access token leaves the grant intact. A caller that
//! trusted it would tell a user their connection was disconnected-but-still-authorized, when in fact the account
//! needs a new consent.
//!
//! The shared type is **not** changed, because it is right about the protocol and this is a property of one
//! provider. Instead [`effect_of`] supplies the provider's own answer, and the contradiction is **asserted** so
//! that a future change to either side fails a test rather than silently re-opening the gap.
//!
//! # The second finding: a `200` means "accepted", not "in effect"
//!
//! The same page: **"Following a successful revocation response, it might take some time before the revocation
//! has full effect."** So [`crate::token::RevocationOutcome::Revoked`] means the provider **accepted** the
//! request, and `is_withdrawn()` must not be read as "no token of ours works any more". The delay is carried in
//! [`RevokedEffect::takes_effect_later`] rather than added to the shared enum, because "how long" is a provider
//! fact and the enum is a protocol one.
//!
//! # What is not built
//!
//! No request is sent. There is no transport for a form `POST`, so this module owns what is verifiable without a
//! socket: which parameters, and what an answer means.

use serde_json::Value;

use crate::google::token::Secret;
use crate::token::{RevocationKind, RevocationOutcome};

/// The form parameter Google's revocation endpoint takes.
pub const TOKEN_PARAMETER: &str = "token";

/// The RFC 7009 parameter that says which kind of token is being presented.
pub const TOKEN_TYPE_HINT_PARAMETER: &str = "token_type_hint";

/// The status Google's revocation endpoint returns on success.
///
/// RFC 7009 §2.2 also defines `200` as covering "the client submitted an invalid token", so a successful status
/// carries **no information about whether the token was live** — which is why
/// [`RevocationOutcome::AlreadyInvalid`] exists and why this module never reports it from a status alone.
pub const REVOCATION_SUCCESS_STATUS: u16 = 200;

/// What happened to one piece of token material.
///
/// **Three states, not a boolean.** "Invalidated" and "there was none" are different facts and they have
/// different consequences: a caller that learned only `refresh_invalidated: false` could not tell whether its
/// refresh token was still live or whether it never had one, and those call for opposite next steps. The first
/// version of this type had five booleans and collapsed exactly that distinction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaterialState {
    /// The provider invalidated it.
    Invalidated,
    /// The account never had any, so nothing was invalidated.
    Absent,
    /// It may still work. **Never produced for this provider** — kept so that "we do not know" is
    /// representable rather than being rounded to one of the other two.
    Untouched,
}

impl MaterialState {
    /// Returns whether the material cannot be used again.
    #[must_use]
    pub const fn is_dead(self) -> bool {
        matches!(self, Self::Invalidated)
    }

    /// Returns whether the account had any to begin with.
    ///
    /// The question a caller asks before reporting "your refresh token was revoked": a user who never had one
    /// should not be told one was.
    #[must_use]
    pub const fn existed(self) -> bool {
        !matches!(self, Self::Absent)
    }
}

/// When a revocation is complete.
///
/// An enum rather than a `bool` because "it might take some time" is Google's documented wording, and a caller
/// needs to know that a `200` is not proof that a concurrent call will now fail. A `bool` named
/// `takes_effect_later` reads as a warning; this reads as a state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EffectTiming {
    /// The effect was complete when the provider answered.
    Immediate,
    /// The provider may need time, so a `200` means **accepted** rather than **in force**.
    MayTakeTime,
}

/// What a revocation actually revoked.
///
/// This is the type that holds Google's answer rather than the protocol's: three of its cells differ from
/// `RevocationKind`'s, which is correct for RFC 7009 and wrong for this provider.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RevokedEffect {
    /// Whether the **granted scopes** were removed, so no new token can be minted from this authorization.
    ///
    /// Always true on Google, whatever token is presented: the documented unit is the *project's* grants.
    pub scopes_removed: bool,
    /// What became of the refresh material.
    pub refresh_material: MaterialState,
    /// What became of the access material.
    pub access_material: MaterialState,
    /// When the effect is complete.
    pub timing: EffectTiming,
}

impl RevokedEffect {
    /// Returns whether the account needs a person before it can be used again.
    ///
    /// **Derived rather than stored.** Needing a person follows from the grant being gone: with the scopes
    /// removed there is no refresh token to mint from and no consent to reuse, which is why the answer is the
    /// same as [`Self::scopes_removed`]. Storing it separately would be a second field that must agree with the
    /// first, which is the defect this repository keeps recording.
    #[must_use]
    pub const fn requires_reauth(self) -> bool {
        self.scopes_removed
    }

    /// Returns whether the account can still make a call with what it held.
    #[must_use]
    pub const fn can_still_be_used(self) -> bool {
        !self.scopes_removed && !self.access_material.is_dead() && !self.refresh_material.is_dead()
    }

    /// Returns whether the provider may need time before the effect is complete.
    #[must_use]
    pub const fn takes_effect_later(self) -> bool {
        matches!(self.timing, EffectTiming::MayTakeTime)
    }
}

/// Returns Google's answer to "what did revoking this actually revoke".
///
/// # The two corrections to the shared type
///
/// - **The grant always goes, so the account always needs a person.** `RevocationKind::AccessToken
///   .requires_reauth_afterwards()` is `false`, which is correct for RFC 7009 and wrong for Google: revoking an
///   access token also revokes the refresh token that pairs with it, and the scope removal is project-wide.
///   There is no input for which Google's answer is "no person needed".
/// - **`scopes_removed` is true for every kind**, because the documented unit is the project's grants rather
///   than the presented token's.
///
/// `has_refresh_token` is a parameter rather than something inferred, because it is a fact only the caller's
/// store knows and it selects between [`MaterialState::Invalidated`] and [`MaterialState::Absent`] — which are
/// different answers, not a `true` and a `false`.
#[must_use]
pub const fn effect_of(kind: RevocationKind, has_refresh_token: bool) -> RevokedEffect {
    // The one cell of the table that depends on the caller's state, and the only place a bool is needed. An
    // access token presented with no refresh token beside it cannot have revoked one, and saying "invalidated"
    // would tell a user their refresh token was revoked when they never had one.
    let refresh_material = match kind {
        RevocationKind::AccessToken => {
            if has_refresh_token {
                MaterialState::Invalidated
            } else {
                MaterialState::Absent
            }
        }
        RevocationKind::RefreshToken | RevocationKind::Grant => MaterialState::Invalidated,
    };
    RevokedEffect {
        scopes_removed: true,
        refresh_material,
        // Dead in every case: it is either the token presented, or — when a refresh token is presented — a token
        // minted from the grant whose scopes were removed. Stated because it is the least obvious constant here.
        access_material: MaterialState::Invalidated,
        timing: EffectTiming::MayTakeTime,
    }
}

/// Builds the parameters for a revocation request.
///
/// # The hint is sent, and it does not mean what a caller might expect
///
/// RFC 7009 says the hint "is used to indicate the type of the token being revoked" and that the server **MAY**
/// ignore it, which is why the hint is advisory and never used to *select* anything locally. Sending it is still
/// right: it is the one part of the request that tells a conforming server which token the caller means, and
/// omitting it would make the request ambiguous for a provider that revokes one token at a time.
///
/// The trap is on the **reading** side, not here: a caller who assumes the hint means "only revoke this one"
/// has assumed RFC 7009 semantics from a provider that documents project-wide scope removal. [`effect_of`] is
/// where that is corrected.
///
/// [`RevocationKind::Grant`] has no hint because RFC 7009 defines none — a hint no server understands is worse
/// than an absent one. On Google a single call achieves a whole-grant revocation anyway, which is why
/// [`effect_of`] reports the widest blast radius for every kind.
///
/// # Errors
///
/// As [`Secret::new`] — the token is a credential and is refused if its shape is unusable.
pub fn revocation_parameters(kind: RevocationKind, token: &Secret) -> Vec<(String, String)> {
    let mut parameters = vec![(
        TOKEN_PARAMETER.to_owned(),
        token.with_exposed(str::to_owned),
    )];
    if let Some(hint) = kind.token_type_hint() {
        // Encoded like every other value. The hints are already unreserved, so this is the "make an unusual
        // value harmless" argument rather than the "make a typical value work" one — but the two parameters
        // must not disagree about whether encoding happens, because that is how one of them ends up wrong.
        parameters.push((
            TOKEN_TYPE_HINT_PARAMETER.to_owned(),
            crate::google::request::percent_encode(hint),
        ));
    }
    parameters
}

/// Reads Google's answer to a revocation request.
///
/// # Why "already invalid" is never reported
///
/// RFC 7009 §2.2 makes `200` cover **both** "the token has been revoked successfully" and "the client submitted
/// an invalid token", and Google's endpoint follows the RFC's `200`/`400` split. So a success status carries no
/// signal that would let this function distinguish [`RevocationOutcome::Revoked`] from
/// [`RevocationOutcome::AlreadyInvalid`], and **inventing** that distinction from a status would be reading a
/// fact out of a response that does not carry it. `AlreadyInvalid` remains reachable — a provider could report
/// it out of band, and a caller that learned it from a later `invalid_grant` can construct it — but not from
/// here, and that is stated rather than left as a gap.
///
/// `reached` is a parameter for the same reason `transient` is on the token path: whether the socket connected
/// is the transport's observation, and a `Result` from a transport cannot say "the provider refused" *and*
/// "nothing answered" with one value.
#[must_use]
pub fn parse_revocation_answer(status: u16, body: &str, reached: bool) -> RevocationOutcome {
    if !reached {
        return RevocationOutcome::Unreachable;
    }
    if status == REVOCATION_SUCCESS_STATUS {
        return RevocationOutcome::Revoked;
    }
    // A non-200 is a refusal. The `error` code is taken from the body when it is there and described by the
    // status when it is not, because a proxy or a gateway in front of the endpoint answers with a body that is
    // not the protocol's -- and a refusal that cannot be named is still a refusal.
    let error = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("error")
                .and_then(Value::as_str)
                .filter(|code| !code.trim().is_empty())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| format!("http_{status}"));
    RevocationOutcome::Refused { error }
}

/// Returns whether a revocation status is one the endpoint can produce.
///
/// A small honesty helper for a caller that wants to assert it is looking at the documented pair. Google returns
/// `200` on success and `400` on error; anything else is a layer in front of the endpoint rather than the
/// endpoint itself, and a caller that cannot tell the difference will classify a gateway's `502` as a protocol
/// refusal.
#[must_use]
pub const fn status_is_documented(status: u16) -> bool {
    matches!(status, 200 | 400)
}

#[cfg(test)]
#[path = "revocation_tests.rs"]
mod tests;
