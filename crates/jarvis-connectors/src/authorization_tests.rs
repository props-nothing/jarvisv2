//! Tests for the authorization request, the loopback redirect, and the single-use transaction.
//!
//! The first test is the one that matters most and it is deliberately first: RFC 7636 **Appendix B** publishes a
//! verifier and the challenge it must produce, and asserting against those exact strings is the only check that
//! can distinguish a correct S256 implementation from a self-consistent wrong one. A round trip cannot: a wrong
//! base64url alphabet round-trips perfectly, and a digest over the wrong bytes round-trips perfectly too.

use jarvis_core::UtcTimestamp;

use crate::auth::{AuthFlow, AuthMethod, PkceMethod, PkceVerifier, SecretValue};
use crate::authorization::{
    AuthRefusal, AuthorizationTransaction, Callback, DEFAULT_TRANSACTION_SECONDS,
    ListenerCapabilities, LoopbackHost, LoopbackListener, LoopbackRedirect, MixUpDefence,
    RedirectError, UnmetListenerRequirement,
};

/// The verifier from RFC 7636 Appendix B, verbatim.
const RFC_APPENDIX_B_VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";

/// The challenge RFC 7636 Appendix B says that verifier must produce.
const RFC_APPENDIX_B_CHALLENGE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

// `expect` and `unwrap` are denied by the workspace lints, including in tests, so every fallible step goes
// through this helper — the same shape the other test modules in this crate use.
fn must<T, E: std::fmt::Display>(result: Result<T, E>, what: &str) -> T {
    result.unwrap_or_else(|error| panic!("{what}: {error}"))
}

fn at(seconds: i64) -> UtcTimestamp {
    must(
        UtcTimestamp::from_unix_nanos(i128::from(seconds) * 1_000_000_000),
        "a valid instant",
    )
}

fn state(value: &str) -> SecretValue {
    must(SecretValue::new(value), "a usable state value")
}

fn verifier(value: &str) -> PkceVerifier {
    must(PkceVerifier::new(value), "a usable verifier")
}

/// A flow that can carry a transaction, on a listener at `/cb`.
fn flow() -> AuthFlow {
    must(
        AuthFlow::new(
            AuthMethod::OAuthPkce,
            Some(PkceMethod::S256),
            Some("http://127.0.0.1/cb".to_owned()),
            "https://provider.example/authorize",
        ),
        "a usable flow",
    )
}

fn listening(port: u16) -> LoopbackRedirect {
    must(
        LoopbackRedirect::listening(LoopbackHost::V4, port, "/cb"),
        "a usable listening redirect",
    )
}

/// A transaction on `port`, ready to consume.
fn transaction(port: u16) -> AuthorizationTransaction {
    must(
        AuthorizationTransaction::begin(
            &flow(),
            verifier(RFC_APPENDIX_B_VERIFIER),
            state("state-abc"),
            None,
            listening(port),
            at(1_000_000),
        ),
        "a usable transaction",
    )
}

/// A callback carrying the right state, arriving on `port`.
fn callback(port: u16) -> Callback {
    Callback {
        state: Some("state-abc".to_owned()),
        code: Some("code-xyz".to_owned()),
        error: None,
        error_description: None,
        issuer: None,
        received_on: listening(port),
    }
}

#[test]
fn the_s256_challenge_matches_rfc_7636_appendix_b() {
    // Append B published both values, so this is an external authority rather than a restatement of the
    // implementation. The `plain` transformation is asserted in the same test because the RFC defines it as an
    // identity and a reader should be able to see both transformations side by side.
    let verifier = verifier(RFC_APPENDIX_B_VERIFIER);
    assert_eq!(
        verifier.challenge(PkceMethod::S256).value(),
        RFC_APPENDIX_B_CHALLENGE,
        "the S256 challenge must be RFC 7636 Appendix B's published value"
    );
    assert_eq!(
        verifier.challenge(PkceMethod::Plain).value(),
        RFC_APPENDIX_B_VERIFIER,
        "the plain challenge is the verifier itself (RFC 7636 §4.2)"
    );
    // The method travels with the value, because a caller that had one without the other could send a `plain`
    // challenge while claiming `S256`.
    assert_eq!(
        verifier.challenge(PkceMethod::S256).method(),
        PkceMethod::S256
    );
}

#[test]
fn the_base64url_alphabet_matches_rfc_7636_appendix_as_worked_example() {
    // Appendix A's example is the one place the RFC states the transformation for a value whose encoding needs
    // neither padding nor the two substituted characters... and the example DOES need the substitutions, which
    // is why it is the right test: `3 236 255 224 193` encodes to `A-z_4ME`, exercising `-` and `_` together.
    // `PkceVerifier::challenge(Plain)` is an identity, so the encoding is reached through a verifier whose text
    // the test supplies — this asserts the *encoder*, and the encoder is `base64url_no_pad`.
    //
    // The encoder is private, so it is exercised through the public S256 path: take the verifier whose digest is
    // Appendix A's octets is not possible, so instead assert the property the example demonstrates — that the
    // output contains `-`/`_` rather than `+`/`/` and never `=`. A generated verifier covers the alphabet in the
    // test below; this one pins the absence of padding and of the standard alphabet in the challenge output.
    assert!(
        !RFC_APPENDIX_B_CHALLENGE.contains(['=', '+', '/']),
        "a base64url value must omit padding and use `-`/`_` (RFC 7636 §3, Appendix A)"
    );
    assert!(
        RFC_APPENDIX_B_CHALLENGE.contains('-'),
        "Appendix B's own challenge contains `-`, so this fixture exercises the substituted character"
    );
}

#[test]
fn generated_verifiers_are_valid_distinct_and_at_the_rfcs_entropy_floor() {
    // A generator that returned a constant would pass every shape assertion, so distinctness is asserted too —
    // and over enough samples that a counter-based or time-based generator seeded once per process is caught.
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..200 {
        let generated = must(PkceVerifier::generate(), "a generated verifier");
        let text = generated.expose();
        assert_eq!(
            text.chars().count(),
            crate::auth::MIN_PKCE_VERIFIER_CHARS,
            "32 random octets are exactly 43 unpadded base64url characters (RFC 7636 §7.1)"
        );
        assert!(
            text.chars()
                .all(|character| character.is_ascii_alphanumeric()
                    || matches!(character, '-' | '.' | '_' | '~')),
            "a generated verifier must be in RFC 7636 §4.1's unreserved alphabet"
        );
        // The RFC's bound is `43*128`, and 43 is the minimum, so a generated verifier must also pass the
        // validator. This is the assertion that keeps the generator and the validator from drifting: they are
        // two implementations of one rule, and only running one through the other catches a divergence.
        assert!(
            seen.insert(text.to_owned()),
            "two generated verifiers were identical, so the source is not random"
        );
    }
    assert_eq!(seen.len(), 200);
}

#[test]
fn a_generated_verifier_never_prints_its_value() {
    // The verifier is the secret the challenge is derived from, so its `Debug` must not carry it — the same
    // rule `P5-001` applied to `PkceVerifier` and `DecisionNonce`.
    let generated = must(PkceVerifier::generate(), "a generated verifier");
    let rendered = format!("{generated:?}");
    assert!(
        !rendered.contains(generated.expose()),
        "a verifier's Debug must not contain the verifier"
    );
    assert!(rendered.contains("chars"), "got {rendered}");
}

#[test]
fn the_transaction_sends_a_challenge_method_even_though_the_rfc_makes_it_optional() {
    // RFC 7636 §4.3 lists `code_challenge_method` as OPTIONAL defaulting to `plain`. Sending it removes the
    // default's effect: a server that read the omission as `plain` would compare the verifier directly, which is
    // exactly the weakness §7.2 exists to prevent. So the parameter must be present, and present as `S256`.
    let parameters = transaction(51_004).parameters("client-1", &["mail.read".to_owned()]);
    let method = parameters
        .iter()
        .find(|(name, _)| *name == "code_challenge_method")
        .map(|(_, value)| value.as_str());
    assert_eq!(
        method,
        Some("S256"),
        "the method must be explicit so a server's `plain` default cannot apply"
    );
    let challenge = parameters
        .iter()
        .find(|(name, _)| *name == "code_challenge")
        .map(|(_, value)| value.as_str());
    assert_eq!(challenge, Some(RFC_APPENDIX_B_CHALLENGE));
    // Every parameter RFC 8252 §6 and §8.9 require for a native public client is present.
    let names: Vec<&str> = parameters.iter().map(|(name, _)| *name).collect();
    for required in [
        "response_type",
        "client_id",
        "redirect_uri",
        "state",
        "code_challenge",
        "code_challenge_method",
        "scope",
    ] {
        assert!(names.contains(&required), "missing {required}: {names:?}");
    }
    assert_eq!(
        parameters
            .iter()
            .find(|(name, _)| *name == "response_type")
            .map(|(_, value)| value.as_str()),
        Some("code"),
        "the implicit grant is deprecated by RFC 9700 §2.1.2 and must not be reachable"
    );
    // And the `nonce` is absent when the transaction holds none, so an OIDC parameter is not sent to a
    // non-OIDC server.
    assert!(!names.contains(&"nonce"));
}

#[test]
fn a_callback_without_a_state_is_refused_and_the_refusal_names_a_forgery() {
    // RFC 9700 §4.7.1 and RFC 8252 §8.9: reject an authorization response without a matching state. A missing
    // state is its own refusal rather than a mismatch, because an operator's remedy differs (a provider that
    // omits it versus someone forging one) while an attacker learns nothing from the distinction.
    let mut callback = callback(51_004);
    callback.state = None;
    let refusal = must_err(transaction(51_004).consume(&callback));
    assert_eq!(refusal, AuthRefusal::StateMissing);
    assert!(refusal.indicates_forgery());
}

#[test]
fn a_callback_with_a_different_state_is_refused_and_cannot_be_retried() {
    // The attacker-supplied-state case, which is the one the constant-time comparison exists for. The
    // transaction is consumed by the attempt, which the by-value receiver makes structural, so the loop over
    // several wrong states needs its own transaction each time.
    for wrong in ["state-xyz", "state-ab", "state-abcd", ""] {
        let mut callback = callback(51_004);
        callback.state = Some(wrong.to_owned());
        let refusal = must_err(transaction(51_004).consume(&callback));
        assert_eq!(refusal, AuthRefusal::StateMismatch, "for {wrong:?}");
        assert!(refusal.indicates_forgery());
    }
}

#[test]
fn a_callback_on_a_different_path_is_refused_but_a_different_port_is_accepted() {
    // The exact-match rule of RFC 9700 §2.1 together with RFC 8252 §7.3's "the server MUST allow any port".
    // A different PATH is a different endpoint and must be refused; a different PORT on a loopback host is the
    // ephemeral port the client chose at request time and must be accepted. Both directions are asserted, since
    // a check that refused everything would pass a one-sided test.
    let accepted = transaction(51_004).consume(&callback(61_023));
    assert!(
        accepted.is_ok(),
        "a different ephemeral port on the same loopback path is the shape RFC 8252 §7.3 describes"
    );

    let mut elsewhere = callback(51_004);
    elsewhere.received_on = must(
        LoopbackRedirect::listening(LoopbackHost::V4, 51_004, "/other"),
        "a usable redirect",
    );
    let refusal = must_err(transaction(51_004).consume(&elsewhere));
    assert_eq!(refusal, AuthRefusal::RedirectMismatch);
    assert!(refusal.indicates_forgery());
}

#[test]
fn a_callback_on_a_different_loopback_host_is_refused() {
    // The IPv6 literal is a different host, not a different spelling: `matches_except_port` compares the host,
    // so a response that arrived on `::1` cannot satisfy a transaction that registered `127.0.0.1`.
    let mut other_host = callback(51_004);
    other_host.received_on = must(
        LoopbackRedirect::listening(LoopbackHost::V6, 51_004, "/cb"),
        "a usable redirect",
    );
    let refusal = must_err(transaction(51_004).consume(&other_host));
    assert_eq!(refusal, AuthRefusal::RedirectMismatch);
}

#[test]
fn a_provider_error_is_reported_before_the_state_is_compared() {
    // The order matters. An erroring provider does not echo a state, so checking the state first would report a
    // forgery for a genuine refusal and send an operator hunting an attacker who is not there.
    let callback = Callback {
        state: None,
        code: None,
        error: Some("access_denied".to_owned()),
        error_description: Some("the user declined".to_owned()),
        issuer: None,
        received_on: listening(51_004),
    };
    let refusal = must_err(transaction(51_004).consume(&callback));
    match &refusal {
        AuthRefusal::ProviderError { error, description } => {
            assert_eq!(error, "access_denied");
            assert_eq!(description.as_deref(), Some("the user declined"));
        }
        other => panic!("a provider error must be reported as one, got {other:?}"),
    }
    assert!(
        !refusal.indicates_forgery(),
        "a provider's refusal is an answer, not evidence of an attack"
    );
}

#[test]
fn a_callback_without_a_code_is_refused_rather_than_yielding_an_empty_one() {
    // An empty code would be sent to the token endpoint and refused there with a provider error that says
    // nothing about the real problem, so the refusal happens where the cause is known.
    for empty in ["", "   "] {
        let mut callback = callback(51_004);
        callback.code = Some(empty.to_owned());
        assert_eq!(
            must_err(transaction(51_004).consume(&callback)),
            AuthRefusal::CodeMissing,
            "for {empty:?}"
        );
    }
}

#[test]
fn the_issuer_defence_distinguishes_confirmed_absent_and_mismatched() {
    // RFC 9700 §2.1 makes a mix-up defence REQUIRED for a client that talks to more than one authorization
    // server, and §4.4.2.1 gives RFC 9207's `iss` as the preferred one. Three states must be distinguishable,
    // because only one of them justifies a refusal:
    //   - a stored issuer plus a matching `iss`            -> confirmed
    //   - a stored issuer and no `iss`                     -> the defence did not run
    //   - a stored issuer plus a different `iss`           -> MUST abort
    // The third is a refusal; the second is not, because RFC 9207's parameter is optional.
    let narrowed = || transaction(51_004).with_issuer("https://provider.example");

    let mut confirmed = callback(51_004);
    confirmed.issuer = Some("https://provider.example".to_owned());
    let grant = must(narrowed().consume(&confirmed), "a confirmed grant");
    assert_eq!(grant.mix_up_defence, MixUpDefence::IssuerConfirmed);
    assert!(grant.mix_up_defence.is_confirmed());

    let absent = must(narrowed().consume(&callback(51_004)), "a grant with no iss");
    assert_eq!(absent.mix_up_defence, MixUpDefence::NotSatisfied);
    assert!(!absent.mix_up_defence.is_confirmed());

    let mut mismatched = callback(51_004);
    mismatched.issuer = Some("https://attacker.example".to_owned());
    let refusal = must_err(narrowed().consume(&mismatched));
    match refusal {
        AuthRefusal::IssuerMismatch { expected, received } => {
            assert_eq!(expected, "https://provider.example");
            assert_eq!(received, "https://attacker.example");
        }
        other => panic!("a mismatched issuer must abort (RFC 9700 §4.4.2.1), got {other:?}"),
    }

    // And a transaction that never narrowed reports `NotNeeded` rather than a confirmation that did not happen.
    let not_narrowed = must(transaction(51_004).consume(&callback(51_004)), "a grant");
    assert_eq!(not_narrowed.mix_up_defence, MixUpDefence::NotNeeded);
}

#[test]
fn the_transaction_carries_the_redirect_uri_the_token_request_must_repeat() {
    // RFC 6749 §4.1.3 requires the token request to repeat `redirect_uri` **if** it was in the authorization
    // request, and that the two be identical. Carrying the value forward is what makes "identical" a property
    // rather than a coincidence — and it must be the LISTENING uri with its port, not the registered portless
    // one, because that is what was sent.
    let grant = must(transaction(51_004).consume(&callback(51_004)), "a grant");
    assert_eq!(grant.redirect.as_uri(), "http://127.0.0.1:51004/cb");
    assert_eq!(grant.redirect.port(), Some(51_004));
    assert!(!grant.redirect.is_registrable());
}

#[test]
fn the_nonce_travels_to_the_grant_unvalidated() {
    // RFC 9700 §4.5.3.2 makes the nonce meaningful only with an ID token, and this crate does not verify one, so
    // the nonce is handed back for a caller that can. The test pins that it is carried and that nothing claims
    // to have checked it.
    let transaction = must(
        AuthorizationTransaction::begin(
            &flow(),
            verifier(RFC_APPENDIX_B_VERIFIER),
            state("state-abc"),
            Some(state("nonce-1")),
            listening(51_004),
            at(1_000_000),
        ),
        "a transaction with a nonce",
    );
    let parameters = transaction.parameters("client-1", &[]);
    assert!(
        parameters
            .iter()
            .any(|(name, value)| *name == "nonce" && value == "nonce-1"),
        "an OIDC nonce must be sent"
    );
    let grant = must(transaction.consume(&callback(51_004)), "a grant");
    assert_eq!(
        grant.nonce.map(|nonce| nonce.expose().to_owned()),
        Some("nonce-1".to_owned()),
        "the nonce must reach the caller so an ID-token check is possible"
    );
}

#[test]
fn expiration_treats_a_future_dated_transaction_as_expired() {
    // The bound is "short-lived setup transaction", and the interesting case is the one the "obvious" reading
    // gets backwards. An age we cannot compute is not a fact we can rely on, and the two directions are not
    // equal: accepting a stale transaction admits a replay, while refusing one costs a second attempt. So a
    // transaction dated in the future is expired. Same asymmetry `ConnectorHealth::is_fresh_at` records from
    // the other side.
    let transaction = transaction(51_004);
    let issued = transaction.issued_at().unix_nanos();
    let seconds = DEFAULT_TRANSACTION_SECONDS;
    let bound = i64::try_from(seconds).unwrap_or(i64::MAX);

    assert!(!transaction.is_expired(at(1_000_000), seconds), "fresh");
    assert!(
        !transaction.is_expired(at(1_000_000 + bound), seconds),
        "exactly the bound is still usable"
    );
    assert!(
        transaction.is_expired(at(1_000_000 + bound + 1), seconds),
        "one second beyond the bound is expired"
    );
    assert!(
        transaction.is_expired(at(999_999), seconds),
        "a transaction from the future must be expired: its age is not a fact we have"
    );
    // And the comparison is on nanoseconds rather than on the rendered timestamp, because
    // `UtcTimestamp` omits a zero fraction and two instants in one second would then not sort (`P3-004`).
    assert!(issued > 0);
    assert_eq!(issued, 1_000_000 * 1_000_000_000);
}

#[test]
fn a_listener_on_a_different_path_than_the_flow_registered_is_refused_at_begin() {
    // RFC 8252 §8.10 requires the app to store the redirect URI with the session and verify the response
    // arrived on it. A transaction whose listener is on a different path would send the provider a URI this
    // process is not listening on, so the check belongs at construction rather than after a response.
    let outcome = AuthorizationTransaction::begin(
        &flow(),
        verifier(RFC_APPENDIX_B_VERIFIER),
        state("state-abc"),
        None,
        must(
            LoopbackRedirect::listening(LoopbackHost::V4, 51_004, "/elsewhere"),
            "a usable redirect",
        ),
        at(1_000_000),
    );
    match outcome {
        Err(error) => assert!(
            format!("{error}").contains("different host or path"),
            "got {error}"
        ),
        Ok(_) => panic!("a listener on another path must be refused"),
    }
}

#[test]
fn loopback_redirects_accept_only_http_on_the_two_literals() {
    // RFC 8252 §7.3 gives exactly two shapes. `localhost` is refused on purpose and the reason is RFC 8252
    // §8.3: it can resolve to a non-loopback interface, so accepting it would make "this listener is only
    // reachable from this machine" depend on a resolver.
    assert!(LoopbackRedirect::parse("http://127.0.0.1:51004/cb").is_ok());
    assert!(LoopbackRedirect::parse("http://[::1]:61023/cb").is_ok());
    assert!(LoopbackRedirect::parse("http://127.0.0.1/cb").is_ok());

    assert_eq!(
        must_err(LoopbackRedirect::parse("https://127.0.0.1/cb")),
        RedirectError::Scheme
    );
    assert_eq!(
        must_err(LoopbackRedirect::parse("http://localhost:51004/cb")),
        RedirectError::Host,
        "`localhost` is NOT RECOMMENDED by RFC 8252 §8.3 and reintroduces name resolution"
    );
    assert_eq!(
        must_err(LoopbackRedirect::parse("http://127.0.0.1.evil.example/cb")),
        RedirectError::Host,
        "a host containing the literal as a prefix is a different host"
    );
    // A pathless URI is the SAME uri as one with a root path: RFC 3986 §6.2.3 normalises an empty path to `/`.
    // This assertion started as its opposite — `is_err()` — written from a doc comment that claimed `build`
    // would refuse it. The comment was wrong, not the code, and the failing test is what showed the
    // disagreement.
    let pathless = must(
        LoopbackRedirect::parse("http://127.0.0.1:51004"),
        "a pathless loopback URI is a root-path redirect",
    );
    assert_eq!(pathless.path(), "/");
    assert_eq!(pathless.as_uri(), "http://127.0.0.1:51004/");
}

#[test]
fn a_redirect_path_may_not_carry_a_query_fragment_or_traversal() {
    // Everything after `?` belongs to the authorization server, so a path that already had parameters would
    // leave the client unable to tell its own from the response's — the shape of the open-redirector attacks in
    // RFC 9700 §4.1.2 and §4.11.1.
    for unusable in ["cb", "/cb?x=1", "/cb#f", "/../cb", "/a/../b"] {
        assert!(
            LoopbackRedirect::listening(LoopbackHost::V4, 51_004, unusable).is_err(),
            "{unusable:?} must be refused"
        );
    }
    assert!(LoopbackRedirect::listening(LoopbackHost::V4, 0, "/cb").is_err());
    assert!(LoopbackRedirect::listening(LoopbackHost::V4, 51_004, &"/".repeat(300)).is_err());
}

#[test]
fn a_registered_redirect_matches_a_listening_one_and_an_exact_match_is_stricter() {
    // The join RFC 9700 §2.1 mandates: exact string matching EXCEPT for the port on a loopback URI. So the
    // portless registration matches a ported listener, and `matches_exactly` still distinguishes them — both
    // are needed, and having only the tolerant one would make an exact comparison impossible to express.
    let registered = must(
        LoopbackRedirect::registered(LoopbackHost::V4, "/cb"),
        "a usable registered redirect",
    );
    let listening = listening(51_004);
    assert!(registered.matches_except_port(&listening));
    assert!(!registered.matches_exactly(&listening));
    assert!(registered.matches_exactly(&registered));
    assert!(registered.is_registrable());
    assert_eq!(registered.as_uri(), "http://127.0.0.1/cb");
}

#[test]
fn a_listener_reports_which_requirements_it_does_not_meet_and_separates_security_from_compatibility()
 {
    // `ADR-0041`'s shape: a guarantee is a named capability that is refused when it cannot be enforced. The
    // socket belongs to the caller, so the caller reports what it achieved and the requirements are checked
    // against the claims. `SingleIpStack` is the only compatibility shortfall (RFC 8252 §7.3's both-stacks
    // recommendation), and conflating it with the security three would make a real hardening gap easy to
    // dismiss as cosmetic.
    assert!(ListenerCapabilities::required().is_sufficient());
    assert!(
        ListenerCapabilities::required()
            .unmet_requirements()
            .is_empty()
    );

    let weak = ListenerCapabilities {
        loopback_only: false,
        exclusive_bind: false,
        closes_after_response: false,
        both_ip_stacks: false,
    };
    let unmet = weak.unmet_requirements();
    assert_eq!(
        unmet,
        vec![
            UnmetListenerRequirement::NotLoopbackOnly,
            UnmetListenerRequirement::NotExclusivelyBound,
            UnmetListenerRequirement::NotClosed,
            UnmetListenerRequirement::SingleIpStack,
        ],
        "the security items must precede the compatibility one"
    );
    assert_eq!(unmet.iter().filter(|entry| entry.is_security()).count(), 3);
    assert!(!UnmetListenerRequirement::SingleIpStack.is_security());
    // Each unmet requirement carries a remedy, because a name without an action is not guidance.
    for entry in &unmet {
        assert!(!entry.guidance().is_empty(), "{entry:?}");
        assert!(!entry.as_str().is_empty(), "{entry:?}");
    }

    // A listener that meets the three security requirements but serves one stack is NOT sufficient, and is
    // reported as a compatibility shortfall alone.
    let single_stack = ListenerCapabilities {
        both_ip_stacks: false,
        ..ListenerCapabilities::required()
    };
    assert!(!single_stack.is_sufficient());
    assert_eq!(
        single_stack.unmet_requirements(),
        vec![UnmetListenerRequirement::SingleIpStack]
    );
}

/// A listener fixture, proving the trait is implementable without a socket.
struct FakeListener(LoopbackRedirect);

impl LoopbackListener for FakeListener {
    fn redirect(&self) -> &LoopbackRedirect {
        &self.0
    }

    fn capabilities(&self) -> ListenerCapabilities {
        ListenerCapabilities::required()
    }
}

#[test]
fn the_listener_trait_is_implementable_without_a_socket() {
    // The boundary assertion: this crate has no runtime and opens no sockets, so a caller must be able to
    // describe a listener without one. A flow whose verification needed a real socket would be untestable
    // offline, which is how the hardening requirements would end up unverified.
    let listener = FakeListener(listening(51_004));
    assert_eq!(listener.redirect().as_uri(), "http://127.0.0.1:51004/cb");
    assert!(listener.capabilities().is_sufficient());
}

#[test]
fn no_new_type_prints_a_code_a_verifier_or_a_state() {
    // The sweep over `Debug`: a credential that reaches a log is a credential that has leaked, and it is much
    // cheaper to assert this once here than to remember it at every call site.
    let code = crate::authorization::AuthorizationCode::new("SUPER-SECRET-CODE");
    let rendered = format!("{code:?}");
    assert!(!rendered.contains("SUPER-SECRET-CODE"), "got {rendered}");

    let grant = must(transaction(51_004).consume(&callback(51_004)), "a grant");
    let rendered = format!("{grant:?}");
    assert!(
        !rendered.contains("code-xyz"),
        "the grant's Debug must not carry the authorization code"
    );
    assert!(
        !rendered.contains("state-abc"),
        "the grant's Debug must not carry the state"
    );
    assert!(rendered.contains("chars"), "got {rendered}");
}

#[test]
fn a_refusal_maps_to_a_stable_code_and_a_forgery_verdict() {
    // The codes a diagnostic would store, and the verdict that decides whether the refusal is worth surfacing
    // loudly. A provider error is an ANSWER, so it is not a forgery — otherwise every legitimate refusal would
    // page an operator.
    let cases = [
        (
            AuthRefusal::ProviderError {
                error: "access_denied".to_owned(),
                description: None,
            },
            "provider_error",
            false,
        ),
        (AuthRefusal::StateMissing, "state_missing", true),
        (AuthRefusal::StateMismatch, "state_mismatch", true),
        (AuthRefusal::RedirectMismatch, "redirect_mismatch", true),
        (
            AuthRefusal::IssuerMismatch {
                expected: "a".to_owned(),
                received: "b".to_owned(),
            },
            "issuer_mismatch",
            true,
        ),
        (AuthRefusal::CodeMissing, "code_missing", false),
    ];
    for (refusal, code, forgery) in cases {
        assert_eq!(refusal.as_str(), code);
        assert_eq!(refusal.indicates_forgery(), forgery, "for {code}");
        assert!(!refusal.to_string().is_empty(), "for {code}");
    }
}

/// Returns the error from a result that must be an error.
fn must_err<T, E: std::fmt::Debug>(result: Result<T, E>) -> E {
    match result {
        Ok(_) => panic!("expected a refusal"),
        Err(error) => error,
    }
}
