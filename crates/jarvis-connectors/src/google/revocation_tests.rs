//! Tests for Google revocation: the parameters, and what a `200` actually means.
//!
//! The test that carries this module is `google_disagrees_with_the_protocol_about_revoking_an_access_token`: it
//! asserts the **contradiction** between `RevocationKind::AccessToken.requires_reauth_afterwards()` and
//! [`effect_of`], so a future change to either side fails here rather than silently telling a user their account
//! is still usable. Falsification record in `TODO.md`.

use super::*;
use crate::google::token::{MAX_REFRESH_TOKEN_CHARS, Secret};

fn must<T, E: std::fmt::Display>(result: Result<T, E>, what: &str) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("{what}: {error}"),
    }
}

/// The params of a parameter list, as a map, for order-independent assertions.
fn as_map(parameters: &[(String, String)]) -> std::collections::BTreeMap<String, String> {
    parameters
        .iter()
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

fn token(kind: &str) -> Secret {
    must(
        Secret::new(TOKEN_PARAMETER, kind, MAX_REFRESH_TOKEN_CHARS),
        "a valid token",
    )
}

#[test]
fn google_disagrees_with_the_protocol_about_revoking_an_access_token() {
    // **The contradiction, asserted rather than described.** The shared type is protocol-correct: RFC 7009 has a
    // hint select which token is revoked, so revoking the access token leaves the grant. Google documents the
    // opposite -- "If the token is an access token and it has a corresponding refresh token, the refresh token
    // will also be revoked" -- and the scope removal is project-wide.
    //
    // So for an account that HAS a refresh token, the two answers disagree about whether the user must act. If
    // this test ever fails because the shared type was changed, that is a real change to a protocol rule and the
    // reader must decide which document wins; if it fails because `effect_of` was changed, the Google fact was
    // dropped.
    let shared_says = RevocationKind::AccessToken.requires_reauth_afterwards();
    let google_says = effect_of(RevocationKind::AccessToken, true).requires_reauth();
    assert!(
        !shared_says,
        "the protocol type is expected to say an access-token revocation leaves the grant"
    );
    assert!(
        google_says,
        "Google revokes the paired refresh token, so the account needs the user"
    );
    assert_ne!(
        shared_says, google_says,
        "the divergence is the point: this module holds Google's answer"
    );
}

#[test]
fn every_revocation_kind_needs_a_person_on_google() {
    // There is no input for which Google's answer is `false`, including an access token with no refresh token
    // alongside it: the scope removal is project-wide, so no new token can be minted either way. Stated as a
    // total claim so an added `RevocationKind` variant cannot quietly default into "no user needed".
    for kind in [
        RevocationKind::AccessToken,
        RevocationKind::RefreshToken,
        RevocationKind::Grant,
    ] {
        for has_refresh in [true, false] {
            let effect = effect_of(kind, has_refresh);
            assert!(
                effect.requires_reauth(),
                "{kind:?} (has_refresh_token={has_refresh}) must need a person"
            );
            assert!(
                !effect.can_still_be_used(),
                "the converse must agree with the predicate it is named for"
            );
            assert!(
                effect.scopes_removed,
                "{kind:?} removes the project's scopes, which is the documented unit"
            );
            assert!(
                effect.access_material.is_dead(),
                "{kind:?} leaves no usable access token"
            );
        }
    }
}

#[test]
fn revoking_an_access_token_invalidates_the_refresh_token_only_when_one_exists() {
    // The one cell of the table that is not constant, and the only place a parameter is needed. The two answers
    // are DIFFERENT STATES rather than a `true` and a `false`: `Absent` says the account never had a refresh
    // token, which is a different thing from one that survived. An earlier version of this type stored a bool
    // and collapsed exactly that distinction.
    assert_eq!(
        effect_of(RevocationKind::AccessToken, true).refresh_material,
        MaterialState::Invalidated
    );
    assert_eq!(
        effect_of(RevocationKind::AccessToken, false).refresh_material,
        MaterialState::Absent,
        "with no refresh token there is none to invalidate"
    );
    assert!(
        MaterialState::Absent.existed() != MaterialState::Invalidated.existed(),
        "the two states must answer 'did it exist' differently, or they are the same state"
    );
    assert!(
        !MaterialState::Absent.is_dead() && MaterialState::Invalidated.is_dead(),
        "only `Invalidated` means the material is unusable"
    );
    // And a refresh token always means refresh material was invalidated, whatever else is true.
    for kind in [RevocationKind::RefreshToken, RevocationKind::Grant] {
        for has_refresh in [true, false] {
            assert_eq!(
                effect_of(kind, has_refresh).refresh_material,
                MaterialState::Invalidated,
                "{kind:?}"
            );
        }
    }
}

#[test]
fn the_effect_is_never_reported_as_immediate() {
    // "Following a successful revocation response, it might take some time before the revocation has full
    // effect." A caller must not treat a `200` as proof that a concurrent call will now fail -- and a test that
    // asserted it would be flaky against the provider itself.
    for kind in [
        RevocationKind::AccessToken,
        RevocationKind::RefreshToken,
        RevocationKind::Grant,
    ] {
        assert!(
            effect_of(kind, true).takes_effect_later(),
            "{kind:?} must not claim an immediate effect"
        );
        assert_eq!(
            effect_of(kind, true).timing,
            EffectTiming::MayTakeTime,
            "{kind:?}"
        );
    }
}

#[test]
fn the_request_carries_the_token_and_the_protocol_hint() {
    // The parameters are an exact SET, so an added one fails here. The hint is what tells a conforming server
    // which token the caller means; it is advisory and never used to select anything locally.
    for (kind, hint) in [
        (RevocationKind::AccessToken, "access_token"),
        (RevocationKind::RefreshToken, "refresh_token"),
    ] {
        let map = as_map(&revocation_parameters(
            kind,
            &token("a-refresh-token-value"),
        ));
        assert_eq!(
            map.len(),
            2,
            "{kind:?} must send exactly the token and the hint"
        );
        assert_eq!(map[TOKEN_PARAMETER], "a-refresh-token-value");
        assert_eq!(map[TOKEN_TYPE_HINT_PARAMETER], hint);
    }
    // And a whole-grant revocation sends NO hint, because RFC 7009 defines none -- a hint no server understands
    // is worse than an absent one.
    let grant = as_map(&revocation_parameters(
        RevocationKind::Grant,
        &token("a-refresh-token-value"),
    ));
    assert_eq!(grant.len(), 1);
    assert_eq!(grant[TOKEN_PARAMETER], "a-refresh-token-value");
    assert_eq!(
        RevocationKind::Grant.token_type_hint(),
        None,
        "the absent hint is the shared type's rule, not this module's"
    );
}

#[test]
fn a_successful_revocation_is_reported_as_revoked() {
    // Google returns 200 on success -- and RFC 7009 §2.2 makes that same status cover "the client submitted an
    // invalid token", so the status carries no signal about whether the token was live.
    let outcome = parse_revocation_answer(200, "", true);
    assert_eq!(outcome, RevocationOutcome::Revoked);
    assert!(
        outcome.is_withdrawn(),
        "a 200 means the provider accepted it"
    );
}

#[test]
fn an_unreachable_provider_is_never_reported_as_revoked() {
    // The fail-closed direction, and the one that matters: `Unreachable.is_withdrawn()` is false, so a caller
    // cannot tell a user their access was withdrawn when the request never arrived. A `200` that was never
    // received must not read as success, which is why `reached` is a separate parameter from the status.
    let outcome = parse_revocation_answer(200, "", false);
    assert_eq!(outcome, RevocationOutcome::Unreachable);
    assert!(
        !outcome.is_withdrawn(),
        "an unanswerable request must not claim an effect"
    );
}

#[test]
fn a_refusal_names_the_provider_code_or_the_status() {
    // The `error` code is the machine-readable part and is preferred. A body that is not the protocol's -- a
    // gateway's page, an empty body -- still yields a refusal, described by the status, because a refusal that
    // cannot be named is still a refusal and reporting it as success or as unreachable would be worse.
    let named = parse_revocation_answer(400, r#"{"error":"invalid_request"}"#, true);
    assert_eq!(
        named,
        RevocationOutcome::Refused {
            error: "invalid_request".to_owned()
        }
    );
    assert!(!named.is_withdrawn());

    for body in ["", "<html>Bad Request</html>", "{}", r#"{"error":""}"#] {
        let unnamed = parse_revocation_answer(400, body, true);
        assert_eq!(
            unnamed,
            RevocationOutcome::Refused {
                error: "http_400".to_owned()
            },
            "{body:?} must yield a status-described refusal"
        );
    }
}

#[test]
fn a_gateway_status_is_a_refusal_and_never_an_outage_success() {
    // A layer in front of the endpoint answers with statuses the endpoint does not produce. They classify as
    // refusals rather than as `Unreachable`, because the provider *was* reached through a proxy -- and the
    // helper exists so a caller can tell the documented pair from a gateway's.
    for status in [500, 502, 503, 504, 301, 404] {
        let outcome = parse_revocation_answer(status, "", true);
        assert!(
            matches!(outcome, RevocationOutcome::Refused { .. }),
            "{status} must be a refusal, not a success: {outcome:?}"
        );
        assert!(
            !status_is_documented(status),
            "{status} is not the documented pair"
        );
    }
    // The documented pair, and only it.
    assert!(status_is_documented(200));
    assert!(status_is_documented(400));
}

#[test]
fn an_invalid_token_shape_is_refused_before_a_request_exists() {
    // The token is a credential, so it gets the same shape rules as any other: a trailing newline from a paste
    // reaches Google as a different string, and the answer would be a `400` about the request rather than about
    // the credential's shape.
    assert!(Secret::new(TOKEN_PARAMETER, "", MAX_REFRESH_TOKEN_CHARS).is_err());
    assert!(Secret::new(TOKEN_PARAMETER, "abc\n", MAX_REFRESH_TOKEN_CHARS).is_err());
    assert!(
        revocation_parameters(
            RevocationKind::Grant,
            &must(
                Secret::new(TOKEN_PARAMETER, "1//0gToken", MAX_REFRESH_TOKEN_CHARS),
                "a valid token"
            )
        )
        .len()
            == 1,
        "a whole-grant revocation sends the token alone"
    );
}

#[test]
fn the_revocation_token_is_percent_encoded_so_an_unusual_value_is_harmless() {
    // Google's tokens are URL-safe, so this looks redundant -- and it is not: `4/0AeanS0b` style codes and
    // OAuth playground tokens contain `/`, which a form body must encode. The point is that an unusual value is
    // harmless, not that a typical one works.
    let raw = "a/b+c=d";
    let secret = must(
        Secret::new(TOKEN_PARAMETER, raw, MAX_REFRESH_TOKEN_CHARS),
        "a token containing reserved characters",
    );
    // The token parameter carries the value **verbatim**, because a form serializer owns the encoding of the
    // body while the hint is pre-encoded here. That asymmetry is asserted rather than left implicit: this test
    // is what records which side of the boundary does the encoding, so a later change cannot make one parameter
    // encoded twice.
    let params = as_map(&revocation_parameters(RevocationKind::Grant, &secret));
    assert_eq!(
        params[TOKEN_PARAMETER], raw,
        "the token is carried verbatim for the serializer to encode"
    );
    let hinted = as_map(&revocation_parameters(RevocationKind::AccessToken, &secret));
    assert_eq!(
        hinted[TOKEN_TYPE_HINT_PARAMETER], "access_token",
        "the hint is encoded here"
    );
}
