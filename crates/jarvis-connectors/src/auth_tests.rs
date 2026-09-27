//! Tests for the auth flow contracts.
//!
//! The PKCE derivations are asserted against **RFC 7636's own published example vector**, not against a
//! recomputation of this module's own arithmetic. That distinction is the whole value of the test: a
//! self-consistent wrong derivation would pass a round-trip assertion, and `P4-006` records the same lesson
//! ("a test that restates the code cannot check it").

use super::*;

fn must<T, E: std::fmt::Display>(result: Result<T, E>, what: &str) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("{what}: {error}"),
    }
}

#[test]
fn the_s256_challenge_matches_rfc_7636_appendix_b() {
    // RFC 7636 Appendix B's own vector. The verifier is 43 characters, which is also the minimum, so this
    // exercises the lower bound at the same time.
    let verifier = must(
        PkceVerifier::new("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
        "the RFC's own verifier",
    );
    let challenge = verifier.challenge(PkceMethod::S256);
    assert_eq!(challenge.method(), PkceMethod::S256);
    assert_eq!(
        challenge.value(),
        "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM",
        "the S256 challenge must match RFC 7636's published value"
    );
    // The challenge is URL-safe, which is why the base64 alphabet is the URL one: a `+` in a query string
    // decodes as a space, producing a challenge the provider rejects with no explanation.
    assert!(
        !challenge.value().contains(['+', '/', '=']),
        "the challenge must use the URL-safe alphabet with no padding: {}",
        challenge.value()
    );
}

#[test]
fn the_plain_challenge_is_the_verifier_itself() {
    // RFC 7636 permits `plain`, and it is representable because a provider may require it — but the derived
    // value is not defended, which is why the manifest refuses to let a PKCE method be omitted.
    let verifier = must(PkceVerifier::new("a".repeat(43)), "a minimal verifier");
    let challenge = verifier.challenge(PkceMethod::Plain);
    assert_eq!(challenge.method(), PkceMethod::Plain);
    assert_eq!(challenge.value(), verifier.expose());
    // The wire values are the RFC's own spelling, and case matters: a provider expecting `S256` would not
    // recognise `s256`.
    assert_eq!(PkceMethod::S256.as_str(), "S256");
    assert_eq!(PkceMethod::Plain.as_str(), "plain");
}

#[test]
fn a_pkce_verifier_outside_the_rfcs_bounds_is_refused_at_both_ends() {
    // RFC 7636 §4.1's own bounds. Both ends are asserted, because an implementation that only checked the
    // lower one would accept an unbounded verifier and a server would reject it.
    assert!(
        PkceVerifier::new("a".repeat(MIN_PKCE_VERIFIER_CHARS - 1)).is_err(),
        "42 characters is below the RFC's minimum"
    );
    assert!(
        PkceVerifier::new("a".repeat(MIN_PKCE_VERIFIER_CHARS)).is_ok(),
        "43 characters is the RFC's minimum and must be accepted"
    );
    assert!(
        PkceVerifier::new("a".repeat(MAX_PKCE_VERIFIER_CHARS)).is_ok(),
        "128 characters is the RFC's maximum and must be accepted"
    );
    assert!(
        PkceVerifier::new("a".repeat(MAX_PKCE_VERIFIER_CHARS + 1)).is_err(),
        "129 characters is above the RFC's maximum"
    );
    // And the alphabet, because a verifier holding a space or a `%` would be re-escaped by a client and the
    // derived challenge would differ from the one the provider hashes.
    for unusable in [
        "a".repeat(42) + " ",
        "a".repeat(42) + "%",
        "a".repeat(42) + "+",
        "é".repeat(43),
    ] {
        assert!(
            PkceVerifier::new(unusable).is_err(),
            "a verifier holding a character outside the unreserved set must be refused"
        );
    }
}

#[test]
fn a_pkce_verifier_never_prints_its_value() {
    // A verifier in a log line is a flow anyone reading the log can complete, so `Debug` prints a length.
    // The same shape `ApiKey` and `DecisionNonce` use, and the assertion checks the VALUE is absent rather
    // than checking the format.
    let secret = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    let verifier = must(PkceVerifier::new(secret), "the RFC's verifier");
    let rendered = format!("{verifier:?}");
    assert!(
        !rendered.contains(secret),
        "the verifier's `Debug` must not contain its value: {rendered}"
    );
    assert!(rendered.contains("43"), "got {rendered}");
    // A challenge is NOT a secret — it travels in the authorization URL — so its `Debug` may show it.
    let challenge = verifier.challenge(PkceMethod::S256);
    assert!(
        format!("{challenge:?}").contains(challenge.value()),
        "a challenge is public and its `Debug` should show it"
    );
}

#[test]
fn a_state_value_never_prints_and_compares_in_constant_time_by_content() {
    let secret = must(SecretValue::new("state-abc-123"), "a state");
    assert!(secret.matches("state-abc-123"));
    // Every way a comparison can be wrong, asserted separately, because a comparison with an early return
    // would pass the matching case and leak the value one byte at a time.
    for wrong in [
        "",
        "state-abc-12",
        "state-abc-1234",
        "state-abc-124",
        "State-ABC-123",
    ] {
        assert!(!secret.matches(wrong), "`{wrong}` must not match the state");
    }
    assert!(matches!(SecretValue::new(""), Err(AuthError::State)));
    assert!(matches!(
        SecretValue::new("x".repeat(MAX_STATE_CHARS + 1)),
        Err(AuthError::State)
    ));
    assert!(SecretValue::new("x".repeat(MAX_STATE_CHARS)).is_ok());
    let rendered = format!("{secret:?}");
    assert!(
        !rendered.contains("state-abc-123"),
        "a state value must not appear in `Debug`: {rendered}"
    );
}

#[test]
fn a_pkce_flow_must_name_its_method_and_a_loopback_redirect() {
    // The requirements' own two rules: a missing PKCE method would let a provider default to `plain`, and a
    // non-loopback redirect would deliver the code to a host this process does not own.
    for endpoint in [
        "https://provider.invalid/auth",
        "https://provider.invalid/auth?x=1",
    ] {
        // A PKCE method is required for an OAuth method.
        assert!(
            matches!(
                AuthFlow::new(
                    AuthMethod::OAuthPkce,
                    None,
                    Some("http://127.0.0.1:8080/cb".to_owned()),
                    endpoint
                ),
                Err(AuthError::Flow { .. })
            ),
            "an OAuth flow with no PKCE method must be refused"
        );
        // So is a redirect URI.
        assert!(
            matches!(
                AuthFlow::new(
                    AuthMethod::OAuthPkce,
                    Some(PkceMethod::S256),
                    None,
                    endpoint
                ),
                Err(AuthError::Flow { .. })
            ),
            "an OAuth flow with no redirect URI must be refused"
        );
        // And it must be loopback.
        for remote in [
            "https://jarvis.example.com/cb",
            "http://192.0.2.10:8080/cb",
            // The whole-host rule: a prefix match on `127.0.0.1` would admit this host.
            "http://127.0.0.1.evil.example/cb",
            "http://localhost:8080/cb",
        ] {
            assert!(
                matches!(
                    AuthFlow::new(
                        AuthMethod::OAuthPkce,
                        Some(PkceMethod::S256),
                        Some(remote.to_owned()),
                        endpoint
                    ),
                    Err(AuthError::Flow { .. })
                ),
                "`{remote}` must be refused as a redirect URI"
            );
        }
        // A loopback redirect is accepted, including the bracketed IPv6 form.
        for local in ["http://127.0.0.1:8080/cb", "http://[::1]:8080/cb"] {
            assert!(
                AuthFlow::new(
                    AuthMethod::OAuthPkce,
                    Some(PkceMethod::S256),
                    Some(local.to_owned()),
                    endpoint
                )
                .is_ok(),
                "`{local}` must be accepted as a redirect URI"
            );
        }
    }
}

#[test]
fn a_non_oauth_method_may_not_carry_pkce_or_a_redirect() {
    // A PKCE method or a redirect URI on an API key is a declaration that cannot be acted on, and reading it
    // as configured is how a deployment waits for a callback that will never arrive.
    assert!(matches!(
        AuthFlow::new(
            AuthMethod::ApiKey,
            Some(PkceMethod::S256),
            None,
            "https://provider.invalid/auth"
        ),
        Err(AuthError::Flow { .. })
    ));
    assert!(matches!(
        AuthFlow::new(
            AuthMethod::ApiKey,
            None,
            Some("http://127.0.0.1:8080/cb".to_owned()),
            "https://provider.invalid/auth"
        ),
        Err(AuthError::Flow { .. })
    ));
    // With neither, it is a well-formed API-key flow.
    let flow = must(
        AuthFlow::new(
            AuthMethod::ApiKey,
            None,
            None,
            "https://provider.invalid/auth",
        ),
        "a valid API key flow",
    );
    assert!(!flow.needs_loopback_listener());
    assert_eq!(flow.pkce(), None);
    assert_eq!(flow.redirect_uri(), None);
}

#[test]
fn an_authorization_endpoint_must_be_https() {
    // The endpoint receives a request carrying the client identifier and the challenge, so `http` is a
    // credential-bearing request in the clear — and `file:` has no authority at all.
    for unsafe_endpoint in [
        "http://provider.invalid/auth",
        "file:///etc/passwd",
        "provider.invalid/auth",
        "",
    ] {
        assert!(
            matches!(
                AuthFlow::new(AuthMethod::ApiKey, None, None, unsafe_endpoint),
                Err(AuthError::Flow { .. })
            ),
            "`{unsafe_endpoint}` must be refused as an authorization endpoint"
        );
    }
}

#[test]
fn each_auth_methods_properties_come_from_its_own_shape() {
    // A table with a column per property, so a method whose answer is wrong is visible rather than folded
    // into a total. The properties decide whether a flow can run headless, whether a secret is needed, and
    // whether a user can revoke it themselves.
    let table = [
        // method, loopback, client secret, interactive, user can revoke
        (AuthMethod::OAuthPkce, true, false, true, true),
        (AuthMethod::OAuthConfidential, true, true, true, true),
        (AuthMethod::PersonalAccessToken, false, false, false, false),
        (AuthMethod::ApiKey, false, false, false, false),
        (AuthMethod::ServiceAccount, false, true, false, false),
        (AuthMethod::PairingCode, false, false, true, false),
    ];
    for (method, loopback, secret, interactive, revocable) in table {
        assert_eq!(
            method.needs_loopback_callback(),
            loopback,
            "{method:?} loopback"
        );
        assert_eq!(
            method.requires_client_secret(),
            secret,
            "{method:?} client secret"
        );
        assert_eq!(
            method.is_interactive(),
            interactive,
            "{method:?} interactive"
        );
        assert_eq!(method.user_can_revoke(), revocable, "{method:?} revocable");
        // Every method has a distinct tag, so a manifest cannot declare two of one method.
        let tags: Vec<&str> = table.iter().map(|(method, ..)| method.tag()).collect();
        let mut unique = tags.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), tags.len(), "the method tags must be distinct");
    }
}

#[test]
fn a_challenge_states_what_the_user_must_do_and_whether_a_listener_is_needed() {
    let state = must(SecretValue::new("state-1"), "a state");
    // The device flow needs NO loopback listener, which is what makes it usable on a headless host — and
    // treating every interactive flow as needing one would make a device-code deployment unconfigurable.
    let device = AuthChallenge {
        state: state.clone(),
        nonce: None,
        action: ChallengeAction::ShowCode {
            url: "https://provider.invalid/device".to_owned(),
            code: "ABCD-EFGH".to_owned(),
        },
    };
    assert!(!device.action.needs_loopback_listener());
    let browser = AuthChallenge {
        state,
        nonce: Some(must(SecretValue::new("nonce-1"), "a nonce")),
        action: ChallengeAction::OpenUrl {
            url: "https://provider.invalid/auth?state=state-1".to_owned(),
        },
    };
    assert!(browser.action.needs_loopback_listener());
    // The `state` and the `nonce` are separate values: `state` defends the redirect and `nonce` the id
    // token, so collapsing them would leave one unprotected while looking complete.
    assert!(browser.nonce.is_some());
    let pasted = AuthChallenge {
        state: must(SecretValue::new("state-2"), "a state"),
        nonce: None,
        action: ChallengeAction::PasteValue {
            instruction: "paste the token from the provider's console".to_owned(),
        },
    };
    assert!(!pasted.action.needs_loopback_listener());
}

#[test]
fn an_auth_state_says_whether_a_new_challenge_may_be_issued() {
    // `Superseded` exists because a second setup must invalidate the first: two live transactions for one
    // account means a callback can be matched to the wrong one, which is the CSRF `state` prevents.
    assert!(AuthState::Connected.permits_new_challenge());
    assert!(AuthState::Failed.permits_new_challenge());
    assert!(AuthState::Superseded.permits_new_challenge());
    assert!(
        !AuthState::AwaitingUser.permits_new_challenge(),
        "a live transaction must not be silently replaced"
    );
}

#[test]
fn a_refresh_outcome_distinguishes_rotation_from_expiry_and_from_revocation() {
    // Three outcomes that need three different responses. Collapsing them into a `Result` would make "the
    // user removed access" look like a transient failure worth retrying.
    assert_eq!(RefreshOutcome::Refreshed, RefreshOutcome::Refreshed);
    assert!(
        RefreshOutcome::Rotated != RefreshOutcome::Refreshed,
        "a rotation must be distinguishable, because the old refresh token is now invalid"
    );
    for outcome in [RefreshOutcome::Expired, RefreshOutcome::Revoked] {
        assert!(outcome.needs_user(), "{outcome:?} must need a user");
        assert!(!outcome.is_safe_to_retry(), "{outcome:?} must not retry");
    }
    assert!(RefreshOutcome::Transient.is_safe_to_retry());
    assert!(!RefreshOutcome::Transient.needs_user());
    assert!(!RefreshOutcome::Refreshed.needs_user());
    assert!(!RefreshOutcome::Rotated.is_safe_to_retry());
}

#[test]
fn a_scope_comparison_is_a_set_operation_and_loss_takes_precedence() {
    // A provider may return its scopes in any order, so a sequence comparison would report a change on every
    // response — and a spurious loss causes a reauth prompt for a grant that is intact. `P3-002`'s
    // `NameAssignments` keyed a collision check on the wrong value and failed in both directions.
    let previous = vec!["read".to_owned(), "send".to_owned()];
    let reordered = vec!["send".to_owned(), "read".to_owned()];
    assert_eq!(
        ScopeChange::between(&previous, &reordered),
        ScopeChange::Unchanged,
        "a reordering is not a change"
    );
    assert_eq!(
        ScopeChange::between(
            &previous,
            &["read".to_owned(), "send".to_owned(), "delete".to_owned()]
        ),
        ScopeChange::Gained {
            gained: vec!["delete".to_owned()]
        }
    );
    // Loss takes precedence over gain, because telling a caller it gained scopes while it also lost some
    // would understate what it can no longer do.
    let mixed = ScopeChange::between(
        &["read".to_owned(), "send".to_owned()],
        &["read".to_owned(), "delete".to_owned()],
    );
    assert_eq!(
        mixed,
        ScopeChange::Lost {
            lost: vec!["send".to_owned()]
        }
    );
    assert!(mixed.is_loss());
    // A gained set is not a loss, which is the distinction a reauth decision reads.
    assert!(
        !ScopeChange::Gained {
            gained: vec!["x".to_owned()]
        }
        .is_loss()
    );
    assert!(!ScopeChange::Unchanged.is_loss());
    // And the loss is sorted deterministically, so a diagnostic naming it is stable across runs.
    let many = ScopeChange::between(
        &["a".to_owned(), "b".to_owned(), "c".to_owned()],
        &["b".to_owned()],
    );
    match many {
        ScopeChange::Lost { lost } => assert_eq!(lost, vec!["a".to_owned(), "c".to_owned()]),
        other => panic!("expected a loss, got {other:?}"),
    }
}

#[test]
fn a_callback_reports_the_providers_own_answer_and_carries_no_label() {
    // The shape `P5-002` constructs. There is no field for a user-supplied label, which is the mechanical
    // form of "account identity verified from the provider, not user-entered labels" — so the type is
    // asserted here even though the slice that produces one is later: a contract slice that declared a type
    // nothing could describe would be one whose consumer changes it.
    let refused = AuthCallback {
        state: "state-1".to_owned(),
        code: None,
        provider_error: Some("access_denied".to_owned()),
    };
    // A provider refusal is REPRESENTED rather than treated as a missing code, because "the user clicked
    // deny" and "the user closed the tab" need different messages and only the provider can distinguish them.
    assert!(refused.code.is_none());
    assert!(refused.provider_error.is_some());
    let granted = AuthCallback {
        state: "state-1".to_owned(),
        code: Some("auth-code".to_owned()),
        provider_error: None,
    };
    assert!(granted.code.is_some());
    assert!(granted.provider_error.is_none());
    // The state the callback presented is compared against the challenge's, which is what the `state`
    // parameter is for — and the comparison is the constant-time one.
    let issued = must(SecretValue::new("state-1"), "the issued state");
    assert!(issued.matches(&granted.state));
    assert!(!issued.matches("state-2"));
    // A callback that presented nothing is refused as well as one that presented a wrong value, which is why
    // `AuthError::StateMismatch` is a SINGLE variant: distinguishing the two would tell an attacker which half
    // of a forged callback was correct.
    assert!(!issued.matches(""));
    let mismatch = AuthError::StateMismatch.to_string();
    assert!(!mismatch.is_empty());
    assert!(
        !mismatch.contains("state-1"),
        "the refusal must not echo the value it compared: {mismatch}"
    );
    assert!(!AuthError::ProviderRefused.to_string().is_empty());
}
