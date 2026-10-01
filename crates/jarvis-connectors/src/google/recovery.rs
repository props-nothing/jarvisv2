//! Recovering from a rejected credential: refresh first, reconnect only if refreshing failed.
//!
//! # The gap this module closes
//!
//! The crate had every **piece** of credential recovery and no **composition**:
//!
//! - [`RefreshOutcome`] (`crate::auth`) distinguishes a plain refresh from a rotation from an expiry from a
//!   revocation from a transient failure.
//! - [`ReauthReason`] (`crate::health`) names *why* a user must reconnect, and is careful to separate a
//!   `ScopeLoss` from a `Revoked`.
//! - [`crate::google::client::classify`] flags an `authError` as [`RetryClass::Authentication`].
//! - [`crate::ratelimit::RetryGuidance::Reauthenticate`] says *do not retry; a user must reconnect*.
//!
//! **Nothing connected a rejected call to a refresh attempt and then to a reauth.** A caller holding a
//! `RetryDecision` for an expired token had exactly one documented answer available — `Reauthenticate` — and
//! that is the guide's **second** remedy, not its first.
//!
//! # The finding: the documented remedy is two steps, and the vocabulary named only the second
//!
//! The errors guide is explicit about what to do with `authError` (a `401`):
//!
//! > "To fix this error, **refresh the access token** using the long-lived refresh token. If you're using a
//! > client library, it automatically handles token refresh. **If this fails, direct the user through the OAuth
//! > flow**."
//!
//! **Refresh, and only if that fails, reconnect.** The connector's `RetryGuidance::Reauthenticate` is the
//! second half of that sentence rendered as the whole of it, so read literally it tells a caller to send a user
//! through a consent screen as the **first** response to an **expired token** — the failure a silent refresh
//! repairs. The first step was not a type anywhere in the crate.
//!
//! # And `authError` is two causes wearing one word
//!
//! The same page: *"This error occurs when the access token you're using is either expired or invalid.
//! **Missing authorization for the requested scopes can also cause this error.**"* So the same `401` +
//! `authError` is **an expired token** (fixed by refreshing) or **a scope the grant never had** (fixed only by
//! re-consenting). **A refresh distinguishes them**, and only a refresh: refreshing cannot grant a scope, so a
//! call refused with a **fresh** token is not a token problem at all. That is the guide's own *"if this fails"*
//! branch reached from the other direction, and it is why the recovery decision takes a [`TokenState`] rather
//! than deciding from the refusal alone.
//!
//! # What is not verified
//!
//! **No request has been sent and no token has been refreshed.** Every rule here is a decision about values the
//! crate owns — a `RetryDecision`, a `RefreshOutcome` — rather than a provider fact, which is why each is
//! argued from the guide above rather than observed.

use crate::auth::RefreshOutcome;
use crate::health::ReauthReason;
use crate::ratelimit::{RetryClass, RetryDecision};

/// Whether the access token in hand is the one that was just refused, or a fresher one.
///
/// # Why this is a type and not a `bool`
///
/// The two states are not "old" and "new" — they are **"a refresh may still help"** and **"a refresh cannot
/// help, because one was already tried"**, and that difference is the whole basis of the two-step remedy. A
/// `bool` named `refreshed` reads as a fact about the token when the decision turns on a fact about the
/// **attempt**: the caller refreshed *in response to this same failure* and was refused again. Naming the two
/// states for what they license is what keeps [the second step](Self::JustRefreshed) from being taken first.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TokenState {
    /// No refresh has been attempted since the refusal, so the token may simply be stale.
    PossiblyStale,
    /// The token was refreshed **in response to this same failure** and the call was refused again.
    ///
    /// So the token is not the problem, and no further refresh can help — a refresh issues a token, it does not
    /// grant a scope the grant does not have. This is the guide's *"if this fails"* branch, reached after a
    /// refresh that **succeeded** rather than one that failed.
    JustRefreshed,
}

/// What a rejected call means the caller must do about its credential.
///
/// # Why the first variant is `Refresh` and not `Reauthenticate`
///
/// `Reauthenticate` is the guide's **second** remedy. An outcome enum whose only credential variant sent a
/// user to a consent screen would make the documented first step unrepresentable, which is the gap this module
/// exists to close. Reconnecting is still an answer here — [`Self::Reauth`] — but it is reached **after** a
/// refresh has been tried, never instead of trying one.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CallRecovery {
    /// Refresh the access token, then retry the call. The documented first remedy for `authError`.
    Refresh,
    /// A refresh cannot help: the token was already fresh and the call was refused anyway, so a user must
    /// re-consent.
    ///
    /// The reason is [`ReauthReason::ScopeLoss`], and the argument is the guide's own list of causes: with the
    /// token excluded (it is fresh), the documented remaining cause of `authError` is *"missing authorization
    /// for the requested scopes"*. A persistent client-level problem
    /// ([`ReauthReason::ProviderRefused`](crate::health::ReauthReason::ProviderRefused)) is the other
    /// possibility and is **not distinguishable here** — only a re-consent that fails again would tell them
    /// apart — so the reason named is the one the documentation supports, not the one that sounds most severe.
    Reauth {
        /// Why a user must act.
        reason: ReauthReason,
    },
    /// The refusal is not a credential problem, so recovery runs through the ordinary path
    /// ([`RetryDecision`]'s own guidance) and nothing here applies.
    ///
    /// A rate limit, a domain policy, a backend fault and a malformed request all reach this arm. Returning it
    /// rather than a "no action" is deliberate: it says **this module has an opinion and it is that the
    /// credential is not the cause**, which is different from "not considered".
    NotCredential,
}

/// What a refresh outcome means the caller must do next.
///
/// # Why `Refreshed` and `Rotated` are separate variants
///
/// `RefreshOutcome::Rotated` means the provider issued a **new** refresh token and invalidated the old one, so
/// the caller must **store** it — while `Refreshed` means the token is unchanged and nothing needs storing.
/// [`Self::RetryCallAndStore`] versus [`Self::RetryCall`] carries that obligation in the type, because a
/// caller that treated a rotation as an ordinary refresh keeps using a token the provider has already
/// invalidated, and the **next** attempt then reads as a broken account rather than as a missed store. The
/// distinction already exists on `RefreshOutcome` (`Rotated` is its own variant); this type is where it reaches
/// a caller as an **action** rather than a value to inspect.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RefreshRecovery {
    /// The refresh succeeded and the refresh token is unchanged: retry the original call.
    RetryCall,
    /// The refresh succeeded **and rotated the refresh token**: retry the original call **and store the new
    /// token** before anything else uses it.
    RetryCallAndStore,
    /// The refresh failed transiently: retry the **refresh** (not the call), respecting the provider's backoff.
    ///
    /// The one case where the next attempt is another refresh rather than the original call, and the direction
    /// matters — retrying the *call* with a token that was never obtained fails identically, while retrying the
    /// *refresh* is what a rate-limited token endpoint eventually honours.
    RetryRefresh,
    /// A user must reconnect.
    Reauth {
        /// Why a user must act.
        reason: ReauthReason,
    },
}

/// Decides what to do about a **rejected call**, given whether a refresh was already tried for it.
///
/// # The decision, both ways
///
/// - **Not an authentication refusal** → [`CallRecovery::NotCredential`]. Only
///   [`RetryClass::Authentication`] reaches a refresh, so a `429`, a `403` or a `5xx` is left to its own
///   guidance rather than refreshing a token that is not the problem.
/// - **Authentication, token possibly stale** → [`CallRecovery::Refresh`]. The documented first remedy.
/// - **Authentication, token just refreshed** → [`CallRecovery::Reauth`]. The token is fresh and the call was
///   still refused, so refreshing again would issue another token that will be refused identically — the
///   remaining documented cause is a scope the grant lacks, which only re-consenting fixes.
///
/// # Why the caller supplies the state
///
/// The same inference/decision split the cursor module and the pub/sub acknowledgement use: the component with
/// the evidence — the one that knows whether *it* just refreshed — reads it, and this pure function decides.
/// The function cannot ask a clock or a store because it has neither.
#[must_use]
pub fn recover_from_call(decision: &RetryDecision, state: TokenState) -> CallRecovery {
    if decision.class != RetryClass::Authentication {
        return CallRecovery::NotCredential;
    }
    match state {
        TokenState::PossiblyStale => CallRecovery::Refresh,
        // A fresh token refused again: no number of refreshes changes a scope the grant does not have.
        TokenState::JustRefreshed => CallRecovery::Reauth {
            reason: ReauthReason::ScopeLoss,
        },
    }
}

/// Decides what to do after a **refresh attempt**.
///
/// # Every outcome maps to exactly one action, and the direction is the point
///
/// | outcome | action |
/// | --- | --- |
/// | `Refreshed` | retry the call |
/// | `Rotated` | retry the call **and store** |
/// | `Transient` | retry the **refresh**, after the provider's backoff |
/// | `Expired` | reauth, `Expired` |
/// | `Revoked` | reauth, `Revoked` |
///
/// The two reauth arms keep their **distinct reasons**, because the remedies differ and the requirement
/// *"preserves account references without hiding lost scopes"* means the reason a user is shown must be the
/// true one: `Revoked` says the user (or the provider) removed access, while `Expired` says the token simply
/// aged out.
#[must_use]
pub fn recover_from_refresh(outcome: RefreshOutcome) -> RefreshRecovery {
    match outcome {
        RefreshOutcome::Refreshed => RefreshRecovery::RetryCall,
        // The rotation is the arm that carries an obligation the plain refresh does not.
        RefreshOutcome::Rotated => RefreshRecovery::RetryCallAndStore,
        RefreshOutcome::Transient => RefreshRecovery::RetryRefresh,
        RefreshOutcome::Expired => RefreshRecovery::Reauth {
            reason: ReauthReason::Expired,
        },
        RefreshOutcome::Revoked => RefreshRecovery::Reauth {
            reason: ReauthReason::Revoked,
        },
    }
}

#[cfg(test)]
#[path = "recovery_tests.rs"]
mod tests;
