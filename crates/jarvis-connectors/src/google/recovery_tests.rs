//! Tests for the credential-recovery decisions.
//!
//! The load-bearing ones are: an `authError` on a possibly-stale token asks to **refresh** rather than to
//! reconnect (the documented first remedy); the **same** refusal on a just-refreshed token asks to reconnect
//! instead (a refresh cannot grant a scope); a **rotation** carries a store obligation a plain refresh does not;
//! and a **transient** refresh failure retries the *refresh*, not the call.
//!
//! Falsification record in `TODO.md`.

use super::*;
use crate::ratelimit::{RetryDecision, RetryGuidance};

/// A decision with the given class and guidance, and no provider request id.
fn decision(class: RetryClass, guidance: RetryGuidance) -> RetryDecision {
    RetryDecision {
        class,
        guidance,
        provider_request_id: None,
    }
}

/// The decision `google::client::classify` produces for a `401 authError`.
fn authentication_refusal() -> RetryDecision {
    decision(RetryClass::Authentication, RetryGuidance::Reauthenticate)
}

#[test]
fn an_auth_error_on_a_possibly_stale_token_asks_to_refresh_not_to_reconnect() {
    // **The finding, in one assertion.** The guide's first remedy for `authError` is *"refresh the access
    // token"*, and only *"if this fails"* does a user reconnect. The classifier's `Reauthenticate` names the
    // second half, so a caller with only that value would send a user through a consent screen for an expired
    // token a silent refresh repairs. The recovery decision makes the documented **first** step reachable.
    let recovery = recover_from_call(&authentication_refusal(), TokenState::PossiblyStale);
    assert_eq!(recovery, CallRecovery::Refresh);
    assert_ne!(
        recovery,
        CallRecovery::Reauth {
            reason: ReauthReason::Expired
        },
        "an expired token is refreshed, not re-consented"
    );
}

#[test]
fn the_same_auth_error_on_a_just_refreshed_token_asks_to_reconnect() {
    // **The same refusal, the opposite answer, and the state is what differs.** With a fresh token the call
    // was refused anyway, and a refresh issues a token rather than granting a scope — so refreshing again would
    // produce another token refused identically. The documented remaining cause is a scope the grant lacks.
    let refusal = authentication_refusal();
    let on_stale = recover_from_call(&refusal, TokenState::PossiblyStale);
    let on_fresh = recover_from_call(&refusal, TokenState::JustRefreshed);
    assert_eq!(
        on_fresh,
        CallRecovery::Reauth {
            reason: ReauthReason::ScopeLoss
        }
    );
    assert_ne!(
        on_stale, on_fresh,
        "one refusal with two token states must not produce one answer"
    );
    // The reason is `ScopeLoss` and not `ProviderRefused`, because the guide lists the token and the scopes as
    // the causes and the fresh token excludes the first. A persistent client problem is the other possibility
    // and is **not distinguishable here**, so the reason named is the documented one.
    assert!(
        ReauthReason::ScopeLoss.is_user_resolvable(),
        "a scope the grant lacks is fixed by re-consenting"
    );
}

#[test]
fn a_non_authentication_refusal_is_left_to_its_own_guidance() {
    // Only `Authentication` is a credential problem, so nothing else reaches a refresh: refreshing a token that
    // is not the cause would spend the token endpoint's budget and change nothing. Each of these is a real
    // answer a caller must not mistake for "the credential is fine" — the variant says **this module has an
    // opinion, and it is that the credential is not the cause**.
    let table = [
        decision(RetryClass::Throttled, RetryGuidance::BackoffSeconds(1)),
        decision(RetryClass::Permanent, RetryGuidance::DoNotRetry),
        decision(RetryClass::ProviderFault, RetryGuidance::BackoffSeconds(1)),
        decision(RetryClass::Unknown, RetryGuidance::Reconcile),
        decision(RetryClass::Transient, RetryGuidance::BackoffSeconds(1)),
    ];
    for decision in table {
        assert_eq!(
            recover_from_call(&decision, TokenState::PossiblyStale),
            CallRecovery::NotCredential,
            "{:?} is not a credential problem",
            decision.class
        );
        // And the state does not matter for a non-credential refusal, so both agree.
        assert_eq!(
            recover_from_call(&decision, TokenState::JustRefreshed),
            CallRecovery::NotCredential
        );
    }
}

#[test]
fn a_plain_refresh_retries_the_call_and_a_rotation_adds_a_store_obligation() {
    // **The distinction the type carries as an action.** A rotation invalidates the old refresh token, so a
    // caller that treats it as an ordinary refresh keeps using a token the provider has already invalidated —
    // and the *next* attempt then reads as a broken account rather than as a missed store. `Rotated` is already
    // its own `RefreshOutcome`; this is where it reaches a caller as an obligation.
    assert_eq!(
        recover_from_refresh(RefreshOutcome::Refreshed),
        RefreshRecovery::RetryCall
    );
    assert_eq!(
        recover_from_refresh(RefreshOutcome::Rotated),
        RefreshRecovery::RetryCallAndStore
    );
    assert_ne!(
        recover_from_refresh(RefreshOutcome::Refreshed),
        recover_from_refresh(RefreshOutcome::Rotated),
        "a rotation is not an ordinary refresh"
    );
}

#[test]
fn a_transient_refresh_failure_retries_the_refresh_and_not_the_call() {
    // The one case where the next attempt is another **refresh**. Retrying the *call* with a token that was
    // never obtained fails identically, while retrying the *refresh* is what a rate-limited token endpoint
    // eventually honours — so the direction is the decision.
    assert_eq!(
        recover_from_refresh(RefreshOutcome::Transient),
        RefreshRecovery::RetryRefresh
    );
    assert_ne!(
        recover_from_refresh(RefreshOutcome::Transient),
        RefreshRecovery::RetryCall
    );
    // A transient outcome does **not** need a user: `RefreshOutcome::Transient.needs_user()` is false, and the
    // recovery agrees by not producing a `Reauth`.
    assert!(!RefreshOutcome::Transient.needs_user());
    assert!(!matches!(
        recover_from_refresh(RefreshOutcome::Transient),
        RefreshRecovery::Reauth { .. }
    ));
}

#[test]
fn the_two_reauth_outcomes_keep_their_distinct_reasons() {
    // A revoked grant and an expired token both need a user, and the **reason** is what `tools-and-connectors.md`
    // requires a reauth path to show honestly: "revoked" says the user or provider removed access, while
    // "expired" says the token aged out. Collapsing them into one "reconnect" would hide which happened — the
    // same "without hiding lost scopes" rule the requirement states for scopes.
    assert_eq!(
        recover_from_refresh(RefreshOutcome::Expired),
        RefreshRecovery::Reauth {
            reason: ReauthReason::Expired
        }
    );
    assert_eq!(
        recover_from_refresh(RefreshOutcome::Revoked),
        RefreshRecovery::Reauth {
            reason: ReauthReason::Revoked
        }
    );
    assert_ne!(
        recover_from_refresh(RefreshOutcome::Expired),
        recover_from_refresh(RefreshOutcome::Revoked),
        "an expiry and a revocation are different reasons and must not share one"
    );
    // And neither is `ProviderRefused`, which is reserved for a client-level problem a user cannot fix.
    for outcome in [RefreshOutcome::Expired, RefreshOutcome::Revoked] {
        let RefreshRecovery::Reauth { reason } = recover_from_refresh(outcome) else {
            panic!("both must need a user");
        };
        assert!(
            reason.is_user_resolvable(),
            "{outcome:?} must be user-resolvable"
        );
    }
}

#[test]
fn the_full_two_step_remedy_is_walkable_from_the_decision_alone() {
    // The guide's sentence — *"refresh the access token … if this fails, direct the user through the OAuth
    // flow"* — as a sequence a caller can actually follow: a refusal asks to refresh; a failed refresh (expired)
    // asks to reconnect; a successful one asks to retry, storing if rotated. Before this module the first step
    // had no value, so the sequence was not expressible.
    let must_refresh = recover_from_call(&authentication_refusal(), TokenState::PossiblyStale);
    assert_eq!(must_refresh, CallRecovery::Refresh);
    // Step 2a — the refresh fails on an expired grant: the user must reconnect, with the true reason.
    let after_failed_refresh = recover_from_refresh(RefreshOutcome::Expired);
    assert_eq!(
        after_failed_refresh,
        RefreshRecovery::Reauth {
            reason: ReauthReason::Expired
        }
    );
    // Step 2b — the refresh succeeds and rotates: retry, storing the new token first.
    let after_rotation = recover_from_refresh(RefreshOutcome::Rotated);
    assert_eq!(after_rotation, RefreshRecovery::RetryCallAndStore);
    // Step 2c — the refresh succeeds but the call is still refused: NOW the user must reconnect, and the state
    // is what says so (a `JustRefreshed` token, not a `PossiblyStale` one).
    let after_retry = recover_from_call(&authentication_refusal(), TokenState::JustRefreshed);
    assert_eq!(
        after_retry,
        CallRecovery::Reauth {
            reason: ReauthReason::ScopeLoss
        }
    );
}

#[test]
fn an_authentication_refusal_forbids_an_automatic_retry_of_the_call() {
    // The class already forbids an automatic retry for `Authentication`, which is what keeps the connector from
    // repeating a call against a credential the provider just rejected. The recovery decision then says **what
    // to do instead** — refresh first — rather than retrying the call unchanged.
    assert!(
        !RetryClass::Authentication
            .permits_automatic_retry(crate::manifest::ProviderIdempotency::Declared)
    );
    assert!(
        !RetryClass::Authentication
            .permits_automatic_retry(crate::manifest::ProviderIdempotency::Unknown)
    );
    assert!(RetryClass::Authentication.needs_user());
    // And no recovery answer means "retry the call as-is": a stale token refreshes first, a fresh one reconnects.
    assert_eq!(
        recover_from_call(&authentication_refusal(), TokenState::PossiblyStale),
        CallRecovery::Refresh
    );
    assert!(matches!(
        recover_from_call(&authentication_refusal(), TokenState::JustRefreshed),
        CallRecovery::Reauth { .. }
    ));
}
