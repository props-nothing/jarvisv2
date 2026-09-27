//! Tests for the Google token endpoint's request construction and answer reading.
//!
//! The load-bearing ones are the **authority rule** (the body decides whether an answer is a refusal, not the
//! status) and the **credential boundary** (an access token cannot be copied into a value here, and a refresh
//! token cannot be rendered). Both are the kind of property that a plausible-looking implementation gets
//! backwards, so each is asserted with its falsifying case rather than its happy path alone.
//!
//! Falsification record in `TODO.md`.

use super::*;
use crate::auth::RefreshOutcome;

fn must<T, E: std::fmt::Display>(result: Result<T, E>, what: &str) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("{what}: {error}"),
    }
}

fn identity() -> ExchangeIdentity {
    must(
        ExchangeIdentity::new(
            "123456789.apps.googleusercontent.invalid",
            "http://127.0.0.1/",
        ),
        "a valid identity",
    )
}

fn verifier() -> Secret {
    // 43 characters, the RFC's minimum, and all unreserved.
    must(
        Secret::new(
            "code_verifier",
            "abcdefghijklmnopqrstuvwxyz-._~0123456789ABCDEFG",
            MAX_CODE_VERIFIER_CHARS,
        ),
        "a valid verifier",
    )
}

/// The params of a parameter list, as a map, for order-independent assertions.
fn as_map(parameters: &[(String, String)]) -> std::collections::BTreeMap<String, String> {
    parameters
        .iter()
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

#[test]
fn the_exchange_carries_exactly_the_documented_parameters() {
    // RFC 6749 §4.1.3 and Google's native-app page. Asserted as an exact SET rather than by presence, so an
    // extra parameter is a failing test: an added `client_secret` would be a value this connector has no field
    // to hold and no manifest declaration for.
    let code = must(
        Secret::new(
            "code",
            "4/0AeanS0bsecretvalue",
            MAX_AUTHORIZATION_CODE_CHARS,
        ),
        "a valid code",
    );
    let parameters = must(
        exchange_code(&code, &verifier(), &identity()),
        "the exchange must build",
    );
    let map = as_map(&parameters);
    let names: Vec<&str> = map.keys().map(String::as_str).collect();
    assert_eq!(
        names,
        [
            "client_id",
            "code",
            "code_verifier",
            "grant_type",
            "redirect_uri"
        ],
        "the parameter set is the documented one, with no additions"
    );
    assert_eq!(map["grant_type"], "authorization_code");
    assert!(
        !map.contains_key("client_secret"),
        "there must be no secret parameter"
    );
}

#[test]
fn the_refresh_carries_a_grant_type_and_the_previous_token() {
    let token = must(
        Secret::new(
            REFRESH_TOKEN_PARAMETER,
            "1//0gLongLivedRefresh",
            MAX_REFRESH_TOKEN_CHARS,
        ),
        "a valid refresh token",
    );
    let map = as_map(&refresh(&token, &identity()));
    let names: Vec<&str> = map.keys().map(String::as_str).collect();
    assert_eq!(
        names,
        ["client_id", "grant_type", "redirect_uri", "refresh_token"]
    );
    assert_eq!(map["grant_type"], "refresh_token");
    assert_eq!(map["refresh_token"], "1//0gLongLivedRefresh");
    assert!(!map.contains_key("client_secret"));
}

#[test]
fn every_parameter_value_is_percent_encoded() {
    // The redirect URI is the case that proves the encoding is applied rather than assumed: it always contains
    // `:` and `/`, so a request that did not encode it would be the one that breaks in practice.
    let map = as_map(&refresh(
        &must(
            Secret::new("refresh_token", "abc", MAX_REFRESH_TOKEN_CHARS),
            "valid",
        ),
        &identity(),
    ));
    assert_eq!(
        map["redirect_uri"], "http%3A%2F%2F127.0.0.1%2F",
        "a redirect URI must be encoded"
    );
    assert_eq!(map["client_id"], "123456789.apps.googleusercontent.invalid");
}

#[test]
fn a_verifier_outside_the_rfc_bounds_is_refused() {
    // RFC 7636 §4.1: 43 to 128. The lower bound is the one that matters, because a short verifier is what a
    // hand-written test or a truncated paste produces -- and the provider would answer a challenge mismatch,
    // which reads as a PKCE bug rather than as malformed input.
    let code = must(
        Secret::new("code", "abc", MAX_AUTHORIZATION_CODE_CHARS),
        "valid",
    );
    // An EMPTY verifier never reaches this function: `Secret::new` refuses it as a credential-shape fault, which
    // is the correct ordering (an empty value is a paste mistake, not an RFC-bound one). Asserted here so the
    // distinction is recorded rather than merely true.
    assert!(
        Secret::new("code_verifier", "", MAX_CODE_VERIFIER_CHARS).is_err(),
        "an empty verifier is refused before the bounds are consulted"
    );
    for short in ["a", &"a".repeat(MIN_CODE_VERIFIER_CHARS - 1)] {
        let bad = must(
            Secret::new("code_verifier", short, MAX_CODE_VERIFIER_CHARS),
            "a short verifier is still wrappable",
        );
        assert!(
            matches!(
                exchange_code(&code, &bad, &identity()),
                Err(TokenRequestError::Argument {
                    field: "code_verifier",
                    ..
                })
            ),
            "{} characters must be refused",
            short.len()
        );
    }
    let too_long = "a".repeat(MAX_CODE_VERIFIER_CHARS + 1);
    let bad = must(
        Secret::new("code_verifier", too_long, MAX_CODE_VERIFIER_CHARS + 1),
        "a long verifier is wrappable with a larger bound",
    );
    assert!(exchange_code(&code, &bad, &identity()).is_err());
    // The control: the boundary values themselves are accepted, so the refusals above are about the bounds.
    for allowed in [
        "a".repeat(MIN_CODE_VERIFIER_CHARS),
        "a".repeat(MAX_CODE_VERIFIER_CHARS),
    ] {
        let good = must(
            Secret::new("code_verifier", allowed, MAX_CODE_VERIFIER_CHARS),
            "a boundary verifier",
        );
        assert!(exchange_code(&code, &good, &identity()).is_ok());
    }
}

#[test]
fn a_verifier_with_a_character_outside_the_rfc_alphabet_is_refused() {
    // §4.1's set is `ALPHA / DIGIT / "-" / "." / "_" / "~"`. A `+` or `/` would be base64-standard rather than
    // base64url, and a verifier that cannot be re-encoded identically produces a challenge mismatch.
    let code = must(
        Secret::new("code", "abc", MAX_AUTHORIZATION_CODE_CHARS),
        "valid",
    );
    for bad_char in ["+", "/", "=", "%", " ", "é"] {
        let value = format!("{}{bad_char}{}", "a".repeat(30), "b".repeat(20));
        // A space is refused earlier, by the credential shape, which is the correct ordering: it is a
        // whitespace fault rather than an alphabet one. The others reach the alphabet check.
        let Ok(wrapped) = Secret::new("code_verifier", value, MAX_CODE_VERIFIER_CHARS) else {
            continue;
        };
        assert!(
            matches!(
                exchange_code(&code, &wrapped, &identity()),
                Err(TokenRequestError::Argument {
                    field: "code_verifier",
                    ..
                })
            ),
            "`{bad_char}` must not be accepted in a verifier"
        );
    }
}

#[test]
fn a_credential_with_a_trailing_newline_is_refused_at_construction() {
    // The paste mistake, and the reason it is refused here rather than at the provider: a credential that
    // arrives with a newline reaches Google as a *different* string, and the answer is a generic auth error
    // that sends a reader to debug the credential's validity instead of its shape.
    for value in ["abc\n", "abc ", "\tabc", "ab c", "abc\r\n"] {
        assert!(
            matches!(
                Secret::new("refresh_token", value, MAX_REFRESH_TOKEN_CHARS),
                Err(TokenRequestError::Argument { .. })
            ),
            "{value:?} must be refused"
        );
    }
    assert!(Secret::new("refresh_token", "", MAX_REFRESH_TOKEN_CHARS).is_err());
    assert!(Secret::new("refresh_token", "   ", MAX_REFRESH_TOKEN_CHARS).is_err());
    // A control character that is not whitespace is refused by its own check.
    assert!(Secret::new("refresh_token", "abc\u{7}", MAX_REFRESH_TOKEN_CHARS).is_err());
    // And a normal value passes, so the refusals above are about the shape.
    assert!(Secret::new("refresh_token", "1//0gABC-._~", MAX_REFRESH_TOKEN_CHARS).is_ok());
}

#[test]
fn a_secret_cannot_be_rendered() {
    // `ADR-0061`'s rule as a check rather than a comment: a hand-written `Debug` is exactly the impl a later
    // derive would silently replace, so the rendering is asserted.
    let secret = must(
        Secret::new(
            "refresh_token",
            "1//0gSuperSecretValue",
            MAX_REFRESH_TOKEN_CHARS,
        ),
        "a valid secret",
    );
    let rendered = format!("{secret:?}");
    assert!(
        !rendered.contains("SuperSecret"),
        "the value must never be rendered: {rendered}"
    );
    assert!(rendered.contains("REDACTED"), "{rendered}");
    assert!(
        rendered.contains(&secret.char_len().to_string()),
        "the length is not the value and is what a diagnostic needs: {rendered}"
    );
    // The material is reachable only through a closure, and the closure is what receives it -- not the
    // `Secret` itself, so a caller cannot move it somewhere a later `Debug` reaches.
    let length = secret.with_exposed(str::len);
    assert_eq!(length, "1//0gSuperSecretValue".len());
}

#[test]
fn a_successful_answer_is_read_from_the_body_and_not_from_the_status() {
    // The documented success shape. The status here is 200, and the test below is the one that shows the body
    // is what decides.
    let body = r#"{
        "access_token": "ya29.a0AfH6SMBtoken",
        "expires_in": 3599,
        "scope": "openid https://www.googleapis.com/auth/gmail.readonly",
        "token_type": "Bearer",
        "refresh_token": "1//0gRefreshValue",
        "id_token": "eyJhbGciOiJSUzI1NiJ9.payload.signature"
    }"#;
    let answer = must(parse_answer(200, body), "the documented success must parse");
    assert!(answer.failure().is_none(), "a grant is not a refusal");
    assert_eq!(answer.refresh_token().map(Secret::char_len), Some(17));
    let response = answer
        .response()
        .unwrap_or_else(|| panic!("a grant has a response"));
    assert_eq!(response.token_type.as_deref(), Some("Bearer"));
    assert_eq!(response.lifetime_seconds(), Some(3599));
    // `has_refresh_token` and `has_id_token` describe ARRIVAL, which is all `RefreshExchange` needs to detect
    // rotation. Google returns an `id_token` because the manifest requests `openid`.
    assert!(response.has_refresh_token);
    assert!(response.has_id_token);
}

#[test]
fn a_failure_is_read_from_the_error_parameter_and_not_from_the_status() {
    // The rule that makes this module's reading correct. RFC 6749 §5.2 makes a refusal an `error` parameter and
    // omits the token parameters, so **the body decides**. Here a 200 carries an error and must be a refusal.
    let body = r#"{
        "error": "invalid_grant",
        "error_description": "Bad Request"
    }"#;
    let answer = must(
        parse_answer(200, body),
        "a 200 carrying an error must parse",
    );
    assert!(
        answer.failure().is_some(),
        "the presence of `error` decides, not the status"
    );
    let failure = answer.failure().unwrap_or_else(|| panic!("a refusal"));
    assert_eq!(failure.error, "invalid_grant");
    assert!(
        failure.requires_reauth(),
        "`invalid_grant` is the one code that needs the user"
    );
    assert!(
        !failure.transient,
        "a 200 is not an outage, so the refusal is not transient"
    );
}

#[test]
fn a_400_without_an_error_parameter_is_not_a_refusal() {
    // The mirror case, and the one a status-first reading gets wrong in the other direction: a proxy's HTML
    // page or a misrouted request arrives as a non-JSON body. Reading the status first would report "the server
    // refused" and attribute a decision to a server that never made one.
    let answer = parse_answer(400, "<html>Bad Request</html>");
    assert!(
        matches!(answer, Err(TokenRequestError::Body { .. })),
        "a body that is not a token response is a decoding failure, not a refusal: {answer:?}"
    );
    // And a JSON object with neither a token nor an error is the same: unreadable, not refused.
    assert!(matches!(
        parse_answer(400, r#"{"message":"nope"}"#),
        Err(TokenRequestError::Body { .. })
    ));
}

#[test]
fn only_a_server_error_marks_a_refusal_transient() {
    // The one thing the status contributes. RFC 6749 §5.2's codes describe the *request*, so they cannot say
    // whether the provider is unwell -- that is the transport's observation. A 503 carrying `invalid_grant` is a
    // real shape (a proxy in front of a provider), and reading the code first would send the user to a consent
    // screen during an outage.
    let body = r#"{"error":"invalid_grant","error_description":"upstream unavailable"}"#;
    let transient = must(parse_answer(503, body), "a 503 refusal must parse");
    assert!(
        transient
            .failure()
            .unwrap_or_else(|| panic!("a refusal"))
            .transient,
        "a 5xx marks the refusal transient, whatever code it carries"
    );
    for status in [400, 401, 403, 429] {
        let answer = must(parse_answer(status, body), "a 4xx refusal must parse");
        assert!(
            !answer
                .failure()
                .unwrap_or_else(|| panic!("a refusal"))
                .transient,
            "{status} must not be read as an outage"
        );
    }
}

#[test]
fn the_transient_refusal_is_classified_before_the_error_code() {
    // The ordering, asserted rather than described. A 503 with `invalid_grant` must classify as `Transient`
    // rather than `Expired`: the transport's observation is the more specific one, and the alternative sends a
    // user to a consent screen during an outage, which finds the same failure and looks like a broken connector.
    let outage = must(
        parse_answer(503, r#"{"error":"invalid_grant"}"#),
        "the outage refusal",
    );
    let exchange = outage.classify_refresh(None, &[], &[], false);
    assert_eq!(exchange.outcome, RefreshOutcome::Transient);

    // The control: the same code without the outage IS the user's problem, so the ordering is not simply
    // "ignore the code".
    let expired = must(
        parse_answer(400, r#"{"error":"invalid_grant"}"#),
        "the grant refusal",
    );
    assert_eq!(
        expired.classify_refresh(None, &[], &[], false).outcome,
        RefreshOutcome::Expired
    );
}

#[test]
fn a_revocation_and_an_expiry_are_distinguished_only_by_vendor_knowledge() {
    // RFC 6749 §5.2's `invalid_grant` covers "invalid, expired, revoked, does not match the redirection URI,
    // or was issued to another client" and does not say which. So the connector's vendor knowledge is the input
    // it actually is -- passed as a parameter rather than invented from the error text.
    let refusal = must(
        parse_answer(400, r#"{"error":"invalid_grant"}"#),
        "the refusal",
    );
    assert_eq!(
        refusal.classify_refresh(None, &[], &[], false).outcome,
        RefreshOutcome::Expired
    );
    assert_eq!(
        refusal.classify_refresh(None, &[], &[], true).outcome,
        RefreshOutcome::Revoked
    );
}

#[test]
fn a_misconfigured_client_is_not_reported_as_needing_the_user() {
    // `invalid_client`/`unauthorized_client` mean the *client* is wrong, so sending a user to a consent screen
    // lands on the same failure. It is reported as transient: `is_safe_to_retry` is false and `needs_user` is
    // false, which is the honest pair for "something is wrong that the user cannot fix".
    for error in [
        "invalid_client",
        "unauthorized_client",
        "unsupported_grant_type",
    ] {
        let refused = must(
            parse_answer(400, &format!(r#"{{"error":"{error}"}}"#)),
            "a refusal",
        );
        let failure = refused.failure().unwrap_or_else(|| panic!("a refusal"));
        assert!(!failure.requires_reauth(), "{error} must not need the user");
        assert_eq!(
            refused.classify_refresh(None, &[], &[], false).outcome,
            RefreshOutcome::Transient,
            "{error}"
        );
    }
}

#[test]
fn a_refresh_without_a_new_token_is_a_plain_refresh_and_not_a_rotation() {
    // Google's documented refresh does not return a new refresh token, so this is the EXPECTED outcome for this
    // provider. The distinction still matters: a provider that started rotating would otherwise go unnoticed,
    // and a caller that treated rotation as the only success would report every Google refresh as degraded.
    let body =
        r#"{"access_token":"ya29.token","expires_in":3599,"scope":"openid","token_type":"Bearer"}"#;
    let answer = must(parse_answer(200, body), "the documented refresh must parse");
    assert!(answer.refresh_token().is_none());
    let exchange = answer.classify_refresh(
        Some(make_reference("refresh-a")),
        &[],
        &["openid".to_owned()],
        false,
    );
    assert_eq!(exchange.outcome, RefreshOutcome::Refreshed);
    assert!(
        exchange.rotated_reference.is_none(),
        "nothing rotated, so the stored reference still stands"
    );
}

#[test]
fn a_refresh_that_does_return_a_token_is_reported_as_a_rotation() {
    // The falsifying pair to the test above. Rotation is decided by whether material ARRIVED, never by whether
    // the caller supplied a reference for it -- a caller that failed to store one has a defect of its own, and
    // reporting `Refreshed` would hide the half of the exchange that makes replay detectable.
    let body = r#"{
        "access_token":"ya29.token",
        "expires_in":3599,
        "token_type":"Bearer",
        "refresh_token":"1//0gNewRefreshValue"
    }"#;
    let answer = must(parse_answer(200, body), "a rotating refresh must parse");
    let exchange = answer.classify_refresh(Some(make_reference("refresh-b")), &[], &[], false);
    assert_eq!(exchange.outcome, RefreshOutcome::Rotated);
    assert_eq!(
        exchange
            .rotated_reference
            .map(|reference| reference.expose_locator().to_owned()),
        Some("refresh-b".to_owned()),
        "the caller must be told where the material now lives"
    );
    // And the control that makes the rule about ARRIVAL: a rotation with no reference supplied reports no
    // rotation reference, which is a caller defect rather than a provider one -- it does not downgrade the
    // outcome to `Refreshed`.
    let unrecorded = answer.classify_refresh(None, &[], &[], false);
    assert_eq!(unrecorded.outcome, RefreshOutcome::Rotated);
    assert!(unrecorded.rotated_reference.is_none());
}

#[test]
fn a_grant_with_a_token_type_this_platform_cannot_use_is_refused() {
    // A response that completed but cannot be used is different from a refusal, so it is an error rather than
    // `Refused`: reporting it as a refusal would say the provider declined when it actually granted something
    // this platform will not send.
    let body = r#"{"access_token":"t","token_type":"MAC","expires_in":3599}"#;
    assert!(
        matches!(
            parse_answer(200, body),
            Err(TokenRequestError::UnusableGrant { .. })
        ),
        "a non-Bearer token type must be refused"
    );
    // An absent type is accepted, because RFC 6749 makes it required but a server that omitted it completed the
    // flow -- and `Bearer` is the only type OAuth 2.0 defines. A lowercase `bearer` is conforming too, since
    // HTTP's rule for a scheme token is case-insensitive.
    for body in [
        r#"{"access_token":"t","expires_in":3599}"#,
        r#"{"access_token":"t","token_type":"bearer","expires_in":3599}"#,
    ] {
        assert!(parse_answer(200, body).is_ok(), "{body}");
    }
}

#[test]
fn a_lifetime_beyond_the_accepted_bound_is_refused_rather_than_clamped() {
    // `MAX_ACCESS_TOKEN_SECONDS` is a refusal and not a clamp: `expires_in` is the server's statement about the
    // token, so shortening it would discard the only information the client has. A value above the bound is a
    // server defect or a response that is not a token.
    let too_long = format!(
        r#"{{"access_token":"t","token_type":"Bearer","expires_in":{}}}"#,
        MAX_ACCESS_TOKEN_SECONDS + 1
    );
    assert!(matches!(
        parse_answer(200, &too_long),
        Err(TokenRequestError::UnusableGrant { .. })
    ));
    // The boundary itself is accepted, and a Google-typical 3599 is accepted.
    for seconds in [3599, MAX_ACCESS_TOKEN_SECONDS] {
        let ok = format!(r#"{{"access_token":"t","token_type":"Bearer","expires_in":{seconds}}}"#);
        assert!(parse_answer(200, &ok).is_ok(), "{seconds}");
    }
    // A zero or negative lifetime means "none stated" rather than "expired instantly", which is
    // `TokenResponse::lifetime_seconds`'s rule and is NOT a refusal here.
    for body in [
        r#"{"access_token":"t","token_type":"Bearer","expires_in":0}"#,
        r#"{"access_token":"t","token_type":"Bearer","expires_in":-1}"#,
        r#"{"access_token":"t","token_type":"Bearer"}"#,
    ] {
        let answer = must(
            parse_answer(200, body),
            "no usable lifetime is not a refusal",
        );
        assert_eq!(
            answer.response().and_then(TokenResponse::lifetime_seconds),
            None
        );
    }
}

#[test]
fn a_token_set_is_built_from_a_grant_and_reports_an_absent_scope_as_the_requested_one() {
    // RFC 6749 §5.1: an absent `scope` means "the scope granted is the one requested". Treating it as "no
    // scopes" would report a scope loss on every response from a server that simply omitted the field, which is
    // the false positive that makes a real loss easy to dismiss.
    let requested = vec!["openid".to_owned(), "mail.read".to_owned()];
    let body = r#"{"access_token":"t","token_type":"Bearer","expires_in":3599}"#;
    let answer = must(parse_answer(200, body), "a grant");
    let set = must(
        answer.token_set(None, &requested, &requested),
        "the token set must build",
    )
    .unwrap_or_else(|| panic!("a grant produces a token set"));
    assert_eq!(set.granted_scopes, requested);
    assert!(!set.lost_scopes());

    // And a granted scope that drops one IS a loss, so the absence rule above is not simply "never a loss".
    let narrowed = r#"{
        "access_token":"t","token_type":"Bearer","expires_in":3599,"scope":"openid"
    }"#;
    let answer = must(parse_answer(200, narrowed), "a narrowed grant");
    let set = must(
        answer.token_set(None, &requested, &requested),
        "the token set must build",
    )
    .unwrap_or_else(|| panic!("a grant produces a token set"));
    assert!(set.lost_scopes(), "losing `mail.read` is a loss");
}

#[test]
fn a_refusal_produces_no_token_set_so_the_two_accessors_are_disjoint() {
    // The property the whole split rests on: exactly one of `response` and `failure` is `Some`, so a call site
    // cannot confuse "the provider said no" with "the provider answered".
    let granted = must(
        parse_answer(200, r#"{"access_token":"t","token_type":"Bearer"}"#),
        "a grant",
    );
    let refused = must(
        parse_answer(400, r#"{"error":"invalid_grant"}"#),
        "a refusal",
    );
    for answer in [&granted, &refused] {
        assert_ne!(
            answer.response().is_some(),
            answer.failure().is_some(),
            "exactly one of the two must be present: {answer:?}"
        );
        // A token set EXISTS exactly when a response does. The assertion is about `Ok(Some(_))` rather than
        // about `is_ok()`: a refusal converts to `Ok(None)`, so a check on the `Result` alone would report a
        // refusal as having produced a set -- which is what an earlier version of this test did.
        assert_eq!(
            must(
                answer.token_set(None, &[], &[]),
                "the conversion must not fail"
            )
            .is_some(),
            answer.response().is_some(),
            "a token set exists exactly when a response does"
        );
    }
    assert!(
        must(
            refused.token_set(None, &[], &[]),
            "a refusal is not an error"
        )
        .is_none()
    );
}

#[test]
fn the_granted_type_cannot_hold_an_access_token() {
    // `ADR-0061`'s rule applied to this module, and the reason an earlier draft was wrong: that draft read
    // `access_token` into an owned `String` and dropped it, which satisfies the letter of the rule and breaks
    // its purpose -- a copy existed, with a lifetime. So the *type* is what is checked: a granted answer
    // renders no token material, and the only `String`-bearing field it has is the refresh secret, whose own
    // rendering is redacted.
    let body = r#"{
        "access_token":"ya29.ThisIsTheAccessToken",
        "token_type":"Bearer",
        "expires_in":3599,
        "refresh_token":"1//0gTheRefreshToken"
    }"#;
    let answer = must(parse_answer(200, body), "a grant");
    let rendered = format!("{answer:?}");
    assert!(
        !rendered.contains("ThisIsTheAccessToken"),
        "no field of a granted answer may carry the access token: {rendered}"
    );
    assert!(
        !rendered.contains("TheRefreshToken"),
        "the refresh token must be redacted too: {rendered}"
    );
    assert!(rendered.contains("REDACTED"), "{rendered}");
}

#[test]
fn an_answer_needs_a_non_empty_access_token_or_an_error() {
    // The documented refusal omits the token parameters entirely, so a body with neither an error nor a token
    // is not a refusal and must not read as one.
    for body in [
        "{}",
        r#"{"token_type":"Bearer","expires_in":3599}"#,
        r#"{"access_token":"","token_type":"Bearer"}"#,
        "[]",
        "null",
        "not json",
    ] {
        assert!(
            matches!(parse_answer(200, body), Err(TokenRequestError::Body { .. })),
            "{body} must be refused as unreadable"
        );
    }
}

#[test]
fn an_empty_error_parameter_is_still_a_refusal() {
    // RFC 6749 §5.2 makes `error` required in an error response, so its PRESENCE is the signal and an empty
    // value is a malformed refusal rather than a grant that mentions the word. Reading the value instead would
    // turn this into a decoding failure, which points a reader at the body rather than at the provider.
    let answer = must(
        parse_answer(400, r#"{"error":""}"#),
        "an empty error is still an error",
    );
    assert!(answer.failure().is_some());
    assert_eq!(
        answer
            .failure()
            .unwrap_or_else(|| panic!("a refusal"))
            .error,
        ""
    );
    assert!(
        !answer
            .failure()
            .unwrap_or_else(|| panic!("a refusal"))
            .requires_reauth(),
        "an empty code is not `invalid_grant`"
    );
}

#[test]
fn an_identity_without_a_client_identifier_is_refused() {
    // An empty identifier produces a 400 whose code is about the client rather than about anything a reader
    // could see, so it is refused locally.
    for empty in ["", "   ", "\t"] {
        assert!(matches!(
            ExchangeIdentity::new(empty, "http://127.0.0.1/"),
            Err(TokenRequestError::Argument {
                field: "client_id",
                ..
            })
        ));
    }
    assert!(
        ExchangeIdentity::new("client", "").is_ok(),
        "a redirect is not checked here"
    );
}

#[test]
fn the_scope_comparison_is_the_shared_one() {
    // Offered so a caller does not reach for a second implementation. Asserted by using it, so removing the
    // re-export without replacing it is a compile error rather than a silently different comparison.
    let change = scope_change(&["a".to_owned(), "b".to_owned()], &["b".to_owned()]);
    assert!(change.change().is_loss());
    assert_eq!(
        granted_scopes("openid  mail.read "),
        vec!["openid".to_owned(), "mail.read".to_owned()],
        "the splitter drops empty items, so a trailing space is not a granted scope"
    );
}

/// Builds a `SecretRef` for the rotation tests.
///
/// Three components, because a reference is metadata about *where* a secret lives: the provider, the locator,
/// and the purpose. A `SecretRef` and not a raw locator, so the test cannot pass by comparing two bare strings
/// that no type ever validated. The call sites wrap it in `Some` themselves, which is what the parameter they
/// feed wants — returning an `Option` here would be a wrapper whose only content is the caller's own choice.
fn make_reference(locator: &str) -> jarvis_core::SecretRef {
    must(
        jarvis_core::SecretRef::new("google-oauth", locator, "connector-refresh"),
        "a valid secret reference",
    )
}
