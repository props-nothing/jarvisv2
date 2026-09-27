//! Tests for the token response, refresh rotation, and revocation.
//!
//! The central assertion is the **absence**: [`TokenSet`] has no access-token field, so the storage rule
//! ("no token material in model context, URLs/logs, diagnostics, or normal database columns") is enforced by
//! there being nowhere to put one. The rest of the tests pin the classifications a caller has to act on —
//! rotation, the two refresh failures, and the three revocation outcomes — because each has a different remedy
//! and collapsing any two of them would make a real condition look like a different one.

use jarvis_core::SecretRef;

use crate::auth::RefreshOutcome;
use crate::token::{
    BEARER_TOKEN_TYPE, MAX_ACCESS_TOKEN_SECONDS, RefreshExchange, RevocationKind,
    RevocationOutcome, TokenEndpointFailure, TokenRequestOutcome, TokenResponse, TokenSet,
    split_scope,
};

fn must<T, E: std::fmt::Display>(result: Result<T, E>, what: &str) -> T {
    result.unwrap_or_else(|error| panic!("{what}: {error}"))
}

fn reference(locator: &str) -> SecretRef {
    must(
        SecretRef::new("connector-secret-store", locator, "refresh-token"),
        "a usable secret reference",
    )
}

fn scopes(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn response(
    expires_in: Option<i64>,
    scope: Option<&str>,
    has_refresh_token: bool,
) -> TokenResponse {
    TokenResponse::new(
        Some(BEARER_TOKEN_TYPE.to_owned()),
        expires_in,
        scope.map(ToOwned::to_owned),
        has_refresh_token,
        false,
    )
}

#[test]
fn a_token_set_records_a_lifetime_and_a_reference_but_never_the_token() {
    // The type's whole purpose. There is no `access_token` field and no `refresh_token` field, so the storage
    // rule is unexpressible rather than merely documented — and the `Debug` rendering is asserted to contain
    // neither, which is the check that would catch a field being added later.
    let set = must(
        TokenSet::from_response(
            &response(Some(3_600), Some("mail.read calendar.read"), true),
            Some(reference("accounts/alice")),
            &[],
            &scopes(&["mail.read", "calendar.read"]),
        ),
        "a usable token set",
    );
    assert_eq!(set.lifetime_seconds, Some(3_600));
    assert_eq!(set.granted_scopes, scopes(&["mail.read", "calendar.read"]));
    assert!(set.refresh_reference.is_some());
    assert!(!set.lost_scopes());

    let rendered = format!("{set:?}");
    for forbidden in ["access_token", "refresh_token", "token_type"] {
        assert!(
            !rendered.contains(forbidden),
            "`{forbidden}` must not be a field of a token set, got {rendered}"
        );
    }
    // The reference renders redacted, which is `SecretRef`'s own property and is asserted here because this is
    // the type that carries it.
    let with_reference = format!("{set:?}");
    assert!(
        with_reference.contains("[REDACTED]") || !with_reference.contains("accounts/alice"),
        "a secret reference's locator must not print, got {with_reference}"
    );
}

#[test]
fn a_lifetime_is_reported_only_when_one_is_usable() {
    // Absent, zero, and negative all mean "no usable lifetime was stated" and share one remedy, so they share
    // one answer. Distinguishing them would invite a caller to treat `0` as a valid instant and refuse every
    // token, which is the failure direction that breaks a working connector.
    for unusable in [None, Some(0), Some(-1), Some(-3_600)] {
        let response = response(unusable, None, false);
        assert_eq!(response.lifetime_seconds(), None, "for {unusable:?}");
        assert!(response.lifetime_is_bounded());
    }
    assert_eq!(response(Some(1), None, false).lifetime_seconds(), Some(1));
    assert_eq!(
        response(
            Some(i64::try_from(MAX_ACCESS_TOKEN_SECONDS).unwrap_or(i64::MAX)),
            None,
            false
        )
        .lifetime_seconds(),
        Some(MAX_ACCESS_TOKEN_SECONDS)
    );
}

#[test]
fn a_lifetime_beyond_the_platform_bound_is_refused_rather_than_clamped() {
    // A refusal rather than a clamp: `expires_in` is the server's statement about the token's lifetime, and a
    // client that shortened it would discard the only information it has. A value above the bound is either a
    // server defect or a response that is not a token, and both are worth failing on.
    let too_long = i64::try_from(MAX_ACCESS_TOKEN_SECONDS).unwrap_or(i64::MAX) + 1;
    let response = response(Some(too_long), None, false);
    assert!(!response.lifetime_is_bounded());
    let outcome = TokenSet::from_response(&response, None, &[], &[]);
    match outcome {
        Err(error) => assert!(format!("{error}").contains("lifetime"), "got {error}"),
        Ok(_) => panic!("a lifetime beyond the bound must be refused"),
    }
}

#[test]
fn a_token_type_this_platform_cannot_use_is_refused_but_an_absent_one_is_accepted() {
    // RFC 6750 §2.1 defines `Bearer` and that is the only type a resource request can carry, so a different one
    // is a mis-authenticated request rather than a degraded one. An ABSENT type is accepted because RFC 6749
    // §5.1 requires it but a deployment that omitted it completed the flow anyway — refusing it would fail a
    // flow that worked. The comparison is case-insensitive because HTTP's rule for a scheme token is, so a
    // server that sent `bearer` is conforming.
    for unusable in [Some("MAC"), Some("DPoP"), Some("Basic")] {
        let response = TokenResponse::new(
            unusable.map(ToOwned::to_owned),
            Some(3_600),
            None,
            false,
            false,
        );
        assert!(!response.token_type_is_usable(), "for {unusable:?}");
        assert!(TokenSet::from_response(&response, None, &[], &[]).is_err());
    }
    for usable in [
        None,
        Some("Bearer"),
        Some("bearer"),
        Some("BEARER"),
        Some(" Bearer "),
    ] {
        let response = TokenResponse::new(
            usable.map(ToOwned::to_owned),
            Some(3_600),
            None,
            false,
            false,
        );
        assert!(response.token_type_is_usable(), "for {usable:?}");
        assert!(TokenSet::from_response(&response, None, &[], &[]).is_ok());
    }
}

#[test]
fn an_absent_scope_means_the_requested_scopes_were_granted() {
    // RFC 6749 §5.1: an absent `scope` means "the scope granted is the one requested". Reading it as "no
    // scopes" would report a loss on every refresh from a server that simply omitted the field, and a false
    // loss is what makes a real one easy to dismiss.
    let set = must(
        TokenSet::from_response(
            &response(Some(3_600), None, false),
            None,
            &scopes(&["mail.read", "calendar.read"]),
            &scopes(&["mail.read", "calendar.read"]),
        ),
        "a usable token set",
    );
    assert_eq!(set.granted_scopes, scopes(&["mail.read", "calendar.read"]));
    assert!(!set.lost_scopes());

    // A stated scope that drops one IS a loss, and the loss takes precedence over a gain. The fixture states
    // BOTH: `calendar.read` disappears and `new.scope` appears. Loss must win, because telling a caller it
    // gained scopes while it also lost some would understate what it can no longer do.
    let narrower = must(
        TokenSet::from_response(
            &response(Some(3_600), Some("mail.read new.scope"), false),
            None,
            &scopes(&["mail.read", "calendar.read"]),
            &scopes(&["mail.read", "calendar.read"]),
        ),
        "a usable token set",
    );
    assert!(
        narrower.lost_scopes(),
        "a lost scope must be reported even when another was gained"
    );
    assert_eq!(narrower.granted_scopes, scopes(&["mail.read", "new.scope"]));
}

#[test]
fn a_scope_string_is_split_on_spaces_and_empties_are_dropped() {
    // RFC 6749 §3.3 defines the scope parameter as space-delimited. An empty item would otherwise become a
    // granted scope that no provider issued and no tool requires, and it would then be compared against the
    // previous set as if it were a scope.
    assert_eq!(split_scope("a b c"), scopes(&["a", "b", "c"]));
    assert_eq!(split_scope("  a   b  "), scopes(&["a", "b"]));
    assert_eq!(split_scope(""), Vec::<String>::new());
    assert_eq!(split_scope("   "), Vec::<String>::new());
}

#[test]
fn a_request_that_was_never_sent_may_retry_and_one_that_was_sent_may_not() {
    // RFC 9700 §4.2.4 makes a code single-use and says a double redemption SHOULD revoke the tokens the first
    // attempt issued. So a retry of a request whose answer was lost can destroy a working grant, while a request
    // that never left can be retried freely. The two are indistinguishable from the request side, which is why a
    // caller has to choose and why this is a variant rather than a `bool`.
    assert!(TokenRequestOutcome::NeverSent { reason: "offline" }.permits_automatic_retry());
    assert!(!TokenRequestOutcome::SentAnswerUnknown.permits_automatic_retry());
    // An answered refusal is not retried either: the code is consumed or the request was malformed, so the
    // second answer will be the same. `RetryClass`'s rule — an answered "no" is not retried — one layer down.
    assert!(
        !TokenRequestOutcome::Refused(TokenEndpointFailure::new("invalid_grant", None, false))
            .permits_automatic_retry()
    );

    let set = must(
        TokenSet::from_response(&response(Some(3_600), None, false), None, &[], &[]),
        "a usable token set",
    );
    let answered = TokenRequestOutcome::Answered(Box::new(set));
    assert!(answered.token_set().is_some());
    assert!(!answered.permits_automatic_retry());
    assert!(TokenRequestOutcome::SentAnswerUnknown.token_set().is_none());
}

#[test]
fn rotation_is_detected_from_the_presence_of_a_new_refresh_token() {
    // RFC 9700 §4.14.2 defines rotation as "the authorization server issues a new refresh token with every
    // access token refresh response", and the replay detection works only if the client notices which shape came
    // back. So this is not bookkeeping: a client that ignored the new token would keep using the old one and
    // throw away the defence.
    let rotated = RefreshExchange::classify(
        Some(&response(Some(3_600), None, true)),
        None,
        Some(reference("accounts/alice/2")),
        &[],
        &[],
        false,
    );
    assert_eq!(rotated.outcome, RefreshOutcome::Rotated);
    assert!(
        rotated.rotated_reference.is_some(),
        "a rotation must yield the reference the caller has to store"
    );

    let plain = RefreshExchange::classify(
        Some(&response(Some(3_600), None, false)),
        None,
        Some(reference("accounts/alice/2")),
        &[],
        &[],
        false,
    );
    assert_eq!(plain.outcome, RefreshOutcome::Refreshed);
    assert!(
        plain.rotated_reference.is_none(),
        "a plain refresh must not claim the reference moved"
    );
    // A response that rotated but for which the caller supplied no reference is STILL `Rotated`. Reporting
    // `Refreshed` would hide the half of the exchange that makes replay detectable, and the caller's failure to
    // store the new material is its own defect.
    let unstored = RefreshExchange::classify(
        Some(&response(Some(3_600), None, true)),
        None,
        None,
        &[],
        &[],
        false,
    );
    assert_eq!(unstored.outcome, RefreshOutcome::Rotated);
    assert!(unstored.rotated_reference.is_none());
}

#[test]
fn an_invalid_grant_needs_the_user_and_a_transient_failure_does_not() {
    // The classification a caller acts on. `invalid_grant` (RFC 6749 §5.2) means the grant is "invalid, expired,
    // revoked, does not match the redirection URI […] or was issued to another client" — every one of those
    // needs a new authorization. A transient failure needs a retry instead, and the caller supplies that fact
    // because RFC 6749 §5.2's error values describe the request, not the server's health.
    let expired = RefreshExchange::classify(
        None,
        Some(&TokenEndpointFailure::new("invalid_grant", None, false)),
        None,
        &[],
        &[],
        false,
    );
    assert_eq!(expired.outcome, RefreshOutcome::Expired);
    assert!(expired.needs_user());
    assert!(expired.token_set.is_none());

    let revoked = RefreshExchange::classify(
        None,
        Some(&TokenEndpointFailure::new("invalid_grant", None, false)),
        None,
        &[],
        &[],
        true,
    );
    assert_eq!(revoked.outcome, RefreshOutcome::Revoked);
    assert!(revoked.needs_user());

    let transient = RefreshExchange::classify(
        None,
        Some(&TokenEndpointFailure::new("server_error", None, true)),
        None,
        &[],
        &[],
        false,
    );
    assert_eq!(transient.outcome, RefreshOutcome::Transient);
    assert!(
        !transient.needs_user(),
        "a transient failure must not send the user to reauthorize"
    );
    assert!(transient.outcome.is_safe_to_retry());

    // The ordering case, and the one that found a real gap: a 503 whose body carries `invalid_grant`. If the
    // grant check ran first, a provider OUTAGE would send the user to a consent screen — the same failure,
    // reported as a broken connector. So `transient` must outrank the error CODE, and this fixture is the only
    // one for which the two orders give different answers: with a `server_error` code both orders say
    // `Transient`, which is why the mutation that swapped them SURVIVED until this case existed.
    let outage_carrying_a_grant_code = RefreshExchange::classify(
        None,
        Some(&TokenEndpointFailure::new("invalid_grant", None, true)),
        None,
        &[],
        &[],
        true,
    );
    assert_eq!(
        outage_carrying_a_grant_code.outcome,
        RefreshOutcome::Transient,
        "a transient failure outranks the error code, or an outage sends the user to reauthorize"
    );
    assert!(!outage_carrying_a_grant_code.needs_user());

    // A client-side misconfiguration is NOT reported as needing the user, because sending someone through a
    // consent screen lands on the same failure. It is also not safe to retry blindly, which is what
    // `Transient`'s pair of answers gives.
    let misconfigured = RefreshExchange::classify(
        None,
        Some(&TokenEndpointFailure::new("invalid_client", None, false)),
        None,
        &[],
        &[],
        false,
    );
    assert!(!misconfigured.needs_user());
}

#[test]
fn a_refresh_that_produced_nothing_does_not_read_as_success() {
    // Neither a response nor a failure is a caller defect, and the honest outcome is a retryable failure rather
    // than a success. A refresh that returned "fine" with no token set would leave a caller with a live account
    // reference and no credential.
    let nothing = RefreshExchange::classify(None, None, None, &[], &[], false);
    assert_eq!(nothing.outcome, RefreshOutcome::Transient);
    assert!(nothing.token_set.is_none());
    assert!(nothing.rotated_reference.is_none());

    // A response this platform will not accept is a completed exchange with an unusable answer, so it is
    // `Transient` (retry may help) rather than `needs_user`.
    let unusable = RefreshExchange::classify(
        Some(&TokenResponse::new(
            Some("MAC".to_owned()),
            Some(3_600),
            None,
            false,
            false,
        )),
        None,
        None,
        &[],
        &[],
        false,
    );
    assert_eq!(unusable.outcome, RefreshOutcome::Transient);
    assert!(unusable.token_set.is_none());
}

#[test]
fn only_invalid_grant_is_treated_as_needing_a_new_authorization() {
    // The predicate, asserted directly so a later edit to the error list has to be deliberate.
    assert!(TokenEndpointFailure::new("invalid_grant", None, false).requires_reauth());
    assert!(TokenEndpointFailure::new("INVALID_GRANT", None, false).requires_reauth());
    for other in [
        "invalid_client",
        "unauthorized_client",
        "invalid_request",
        "server_error",
    ] {
        assert!(
            !TokenEndpointFailure::new(other, None, false).requires_reauth(),
            "{other} must not be treated as needing a new authorization"
        );
    }
}

#[test]
fn revocation_failure_is_not_the_same_claim_as_revocation_success() {
    // RFC 7009 §2.2 makes "already invalid" a SUCCESS — "the authorization server responds with HTTP status code
    // 200 if the token has been revoked successfully or if the client submitted an invalid token" — so a caller
    // that treated it as an error would report a failure for a disconnect that worked. And the three failure
    // modes are distinct claims: an overclaimed revocation leaves a live credential behind while telling the
    // user it was withdrawn, which is the direction worth designing against.
    assert!(RevocationOutcome::Revoked.is_withdrawn());
    assert!(RevocationOutcome::AlreadyInvalid.is_withdrawn());
    for unwithdrawn in [
        RevocationOutcome::Unsupported {
            reason: "no endpoint",
        },
        RevocationOutcome::Refused {
            error: "invalid_request".to_owned(),
        },
        RevocationOutcome::Unreachable,
    ] {
        assert!(
            !unwithdrawn.is_withdrawn(),
            "{unwithdrawn:?} must not claim withdrawal"
        );
        assert!(!unwithdrawn.as_str().is_empty());
        assert!(!unwithdrawn.to_string().is_empty());
    }
}

#[test]
fn a_whole_grant_revocation_has_no_token_type_hint_and_a_single_token_does() {
    // RFC 7009 revokes ONE token at a time and defines hints for `access_token`/`refresh_token` only, so a
    // whole-grant revocation is two calls (or a provider extension) and the caller has to know that rather than
    // send a hint no server understands. RFC 7009 also makes the hint advisory, which is why it selects nothing
    // locally.
    assert_eq!(
        RevocationKind::AccessToken.token_type_hint(),
        Some("access_token")
    );
    assert_eq!(
        RevocationKind::RefreshToken.token_type_hint(),
        Some("refresh_token")
    );
    assert_eq!(
        RevocationKind::Grant.token_type_hint(),
        None,
        "the protocol defines no hint for revoking a whole grant"
    );
    // And only the two that remove the ability to refresh leave the account needing the user.
    assert!(!RevocationKind::AccessToken.requires_reauth_afterwards());
    assert!(RevocationKind::RefreshToken.requires_reauth_afterwards());
    assert!(RevocationKind::Grant.requires_reauth_afterwards());
    for kind in [
        RevocationKind::AccessToken,
        RevocationKind::RefreshToken,
        RevocationKind::Grant,
    ] {
        assert!(!kind.as_str().is_empty());
    }
}
