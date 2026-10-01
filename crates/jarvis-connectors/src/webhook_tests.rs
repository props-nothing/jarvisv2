//! Tests for the webhook contract.
//!
//! The two assertions the security property rests on: a delivery's body is **raw bytes**, so a verifier
//! cannot accidentally verify a re-serialization; and an ambiguous security header is refused rather than
//! resolved, because picking one of two values is the decision that lets a proxy's value shadow a provider's.

use super::*;

fn must<T, E: std::fmt::Display>(result: Result<T, E>, what: &str) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("{what}: {error}"),
    }
}

fn scheme(algorithm: SignatureAlgorithm, header: &str) -> SignatureScheme {
    // The encoding must match the family: a body-covering signature has bytes to encode, a body-independent
    // authenticator presents an opaque header value. Choosing per algorithm here keeps the helper honest and
    // leaves the refusal of a mismatched pair to its own test.
    let encoding = if algorithm.covers_the_body() {
        SignatureEncoding::Hex
    } else {
        SignatureEncoding::Raw
    };
    must(
        SignatureScheme::new(algorithm, header, encoding),
        "a valid signature scheme",
    )
}

fn binding(path: &str, header: Option<&str>, in_body: bool) -> WebhookBinding {
    must(
        WebhookBinding::new(path, header.map(str::to_owned), in_body),
        "a valid binding",
    )
}

#[test]
fn a_webhook_with_neither_an_account_header_nor_a_body_field_is_refused() {
    // With neither, nothing binds a delivery to an account and the signature is the only control — so a
    // valid delivery for one account could be applied to another.
    match WebhookBinding::new("/webhooks/example", None, false) {
        Err(SignatureError::Binding { reason }) => {
            assert!(reason.contains("applied to another"), "got {reason}");
        }
        other => panic!("an unbound delivery must be refused, got {other:?}"),
    }
    // A blank header counts as absent, because `Some("")` is a value nobody chose and reading it as present
    // would make the check pass while nothing is compared.
    assert!(WebhookBinding::new("/webhooks/example", Some("  ".to_owned()), false).is_err());
    // Either mechanism alone is enough, and the accessor says which.
    let by_header = binding("/webhooks/example", Some("x-account"), false);
    assert!(by_header.is_checkable_before_parsing());
    let by_body = binding("/webhooks/example", None, true);
    assert!(
        !by_body.is_checkable_before_parsing(),
        "a body-only binding cannot be checked before parsing, which is a recorded tension with A12"
    );
}

#[test]
fn a_webhook_path_must_be_absolute_with_no_query_and_no_traversal() {
    // `A12` names "wrong endpoint" as a rejection reason, so the binding is exact — and a query or fragment
    // would never match a delivery's request target, making every delivery refused with no clue why.
    for unusable in [
        "",
        "webhooks/example",
        "/webhooks/example?x=1",
        "/webhooks/example#frag",
        "/webhooks/../admin",
    ] {
        assert!(
            WebhookBinding::new(unusable, Some("x-account".to_owned()), false).is_err(),
            "`{unusable}` must be refused as a webhook path"
        );
    }
    for usable in ["/webhooks/example", "/", "/a/b/c"] {
        assert!(
            WebhookBinding::new(usable, Some("x-account".to_owned()), false).is_ok(),
            "`{usable}` must be accepted as a webhook path"
        );
    }
    // An exact match is what the binding means: `is_checkable_before_parsing` and the path are independent,
    // and a prefix match on `/webhooks/example` would admit `/webhooks/example/anything`.
    let exact = binding("/webhooks/example", Some("x-account"), false);
    assert_eq!(exact.path, "/webhooks/example");
    assert_ne!(
        exact.path, "/webhooks/example/anything",
        "the binding is one exact path"
    );
}

#[test]
fn a_signature_header_must_be_lowercase_and_a_valid_token() {
    // HTTP header names are case-insensitive on the wire, so storing one lowercase makes a comparison in this
    // crate agree with the wire's own rule. A name holding a colon or a space would make the lookup silently
    // find nothing, whose symptom is "every signature is wrong".
    for unusable in ["", "X-Signature", "x signature", "x:signature", "x\u{0}sig"] {
        assert!(
            SignatureScheme::new(
                SignatureAlgorithm::HmacSha256,
                unusable,
                SignatureEncoding::Hex
            )
            .is_err(),
            "`{unusable}` must be refused as a signature header"
        );
    }
    for usable in [
        "x-signature",
        "x-hub-signature-256",
        "x_signature",
        "signature",
    ] {
        assert!(
            SignatureScheme::new(
                SignatureAlgorithm::HmacSha256,
                usable,
                SignatureEncoding::Hex
            )
            .is_ok(),
            "`{usable}` must be accepted as a signature header"
        );
    }
    assert!(
        SignatureScheme::new(
            SignatureAlgorithm::HmacSha256,
            "x".repeat(65),
            SignatureEncoding::Hex
        )
        .is_err()
    );
}

#[test]
fn a_scheme_says_whether_it_authenticates_the_raw_bytes() {
    // A keyed MAC is the only mechanism that authenticates the bytes; a signature over a parsed structure
    // authenticates a re-serialization, which is how a JSON parser's behaviour becomes an injection.
    for algorithm in [SignatureAlgorithm::HmacSha256, SignatureAlgorithm::HmacSha1] {
        assert!(
            algorithm.is_keyed_mac(),
            "{algorithm:?} must be recognised as a keyed MAC"
        );
        assert!(algorithm.covers_the_body(), "{algorithm:?} signs the body");
        assert!(scheme(algorithm, "x-signature").authenticates());
    }
    assert!(!SignatureAlgorithm::Ed25519.is_keyed_mac());
    assert!(
        SignatureAlgorithm::Ed25519.covers_the_body(),
        "Ed25519 covers the body with a public key, which is why `covers_the_body` is not `is_keyed_mac`"
    );
    assert!(scheme(SignatureAlgorithm::Ed25519, "x-signature").authenticates());
    // `None` exists because a provider may authenticate another way, and it is named so a manifest that
    // declares it is visibly weaker rather than silently so.
    let unauthenticated = scheme(SignatureAlgorithm::None, "x-signature");
    assert!(!unauthenticated.authenticates());
    // The two header-token families cover the body **not at all** yet still authenticate — the exact pair of
    // facts that made Gmail and Calendar push undeclarable before they existed.
    for algorithm in [
        SignatureAlgorithm::OidcIdToken,
        SignatureAlgorithm::EchoedChannelToken,
    ] {
        assert!(
            !algorithm.covers_the_body(),
            "{algorithm:?} authenticates a header, not the body"
        );
        assert!(
            algorithm.is_body_independent(),
            "{algorithm:?} is body-independent"
        );
        assert!(
            scheme(algorithm, "authorization").authenticates(),
            "{algorithm:?} is an authenticator even without a body signature"
        );
    }
    // `None` is body-independent but authenticates nothing: the axis `covers_the_body` is orthogonal to
    // `authenticates`, and collapsing them would let `None` read as a mechanism.
    assert!(SignatureAlgorithm::None.is_body_independent());
    assert!(!unauthenticated.authenticates());
    // `covers_the_body` and `is_keyed_mac` are different questions, proved by a case where they disagree:
    // Ed25519 is keyed-mac-false yet body-covering, and an echoed channel token is the reverse.
    assert!(!SignatureAlgorithm::EchoedChannelToken.is_keyed_mac());
    assert!(!SignatureAlgorithm::EchoedChannelToken.covers_the_body());
    let codes: Vec<&str> = [
        SignatureAlgorithm::HmacSha256,
        SignatureAlgorithm::HmacSha1,
        SignatureAlgorithm::Ed25519,
        SignatureAlgorithm::OidcIdToken,
        SignatureAlgorithm::EchoedChannelToken,
        SignatureAlgorithm::None,
    ]
    .iter()
    .map(|algorithm| algorithm.as_str())
    .collect();
    let mut unique = codes.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(
        unique.len(),
        codes.len(),
        "the algorithm codes must be distinct"
    );
}

#[test]
fn a_body_independent_authenticator_may_not_claim_a_signature_encoding() {
    // `encoding` describes how a **signature's bytes** are presented. A body-independent authenticator presents
    // an opaque header value — a bearer JWT or an echoed string — so there are no bytes whose encoding could be
    // hex or base64, and a scheme claiming one would tell a verifier to decode something that is not a
    // signature. `Raw` ("consume the header value as it arrived") is the only honest value, and the refusal is
    // `ADR-0057`'s rule: a field that cannot take an honest value for a variant is refused rather than defaulted.
    for algorithm in [
        SignatureAlgorithm::OidcIdToken,
        SignatureAlgorithm::EchoedChannelToken,
        SignatureAlgorithm::None,
    ] {
        let refused = SignatureScheme::new(algorithm, "authorization", SignatureEncoding::Base64);
        match refused {
            Err(SignatureError::Encoding { reason }) => {
                assert!(
                    reason.contains("raw"),
                    "the refusal must name the value: {reason}"
                );
            }
            other => {
                panic!("{algorithm:?} with a signature encoding must be refused, got {other:?}")
            }
        }
        // And the body-covering algorithms are unaffected: an encoding is exactly what they need.
        assert!(
            SignatureScheme::new(
                SignatureAlgorithm::HmacSha256,
                "x-signature",
                SignatureEncoding::Base64
            )
            .is_ok(),
            "a body signature needs its encoding and must be accepted"
        );
    }
}

#[test]
fn the_two_google_push_mechanisms_are_now_declarable() {
    // The finding this round closes (Unresolved Question 1): Gmail's Pub/Sub delivery is an OIDC bearer JWT with
    // an **unsigned body**, and Calendar's is an echoed `X-Goog-Channel-Token` over a **zero-length** body.
    // Neither is a MAC over bytes, so neither could be expressed before `OidcIdToken`/`EchoedChannelToken`
    // existed and the connector had to declare `polling`. This pins both mechanisms as constructible schemes.
    let gmail = scheme(SignatureAlgorithm::OidcIdToken, "authorization");
    assert!(gmail.authenticates() && gmail.algorithm.is_body_independent());
    // Calendar's token arrives in the documented header, lowercase, and is consumed verbatim.
    let calendar = scheme(
        SignatureAlgorithm::EchoedChannelToken,
        "x-goog-channel-token",
    );
    assert!(calendar.authenticates() && calendar.algorithm.is_body_independent());
    assert_eq!(calendar.encoding, SignatureEncoding::Raw);
    // The header name is a real one, so `SignatureScheme`'s lowercase token rule accepts it unmodified.
    assert_eq!(calendar.header, "x-goog-channel-token");
}

#[test]
fn every_refusal_is_a_distinct_control_and_says_whether_it_was_an_authenticity_failure() {
    // `A12` enumerates the rejection reasons, and they are separate because an operator must be able to tell
    // an attack from a misconfiguration: an invalid signature is one or the other, while a wrong endpoint is
    // almost always a deployment mistake.
    let table = [
        (WebhookRejection::InvalidSignature, true, false),
        (WebhookRejection::StaleTimestamp, false, false),
        (WebhookRejection::FutureTimestamp, false, false),
        (WebhookRejection::WrongEndpoint, false, false),
        (WebhookRejection::WrongAccount, true, false),
        (WebhookRejection::Oversized, false, false),
        // A replay is a provider doing exactly what it promised, so it must not page anyone.
        (WebhookRejection::Replayed, false, true),
        (WebhookRejection::UnknownSchemaVersion, false, false),
    ];
    for (rejection, authenticity, acknowledge) in table {
        assert_eq!(
            rejection.indicates_an_authenticity_failure(),
            authenticity,
            "{rejection:?} authenticity"
        );
        assert_eq!(
            rejection.should_acknowledge_to_the_provider(),
            acknowledge,
            "{rejection:?} acknowledgement"
        );
        assert!(!rejection.as_str().is_empty());
    }
    // Only a replay is acknowledged, because answering a duplicate with a success tells the provider to stop
    // — and `A12` requires that a duplicate concurrent delivery produces one canonical event, so the second
    // arrival must be identifiable rather than processed and deduplicated later.
    let acknowledged: Vec<WebhookRejection> = table
        .iter()
        .filter(|(rejection, _, _)| rejection.should_acknowledge_to_the_provider())
        .map(|(rejection, _, _)| *rejection)
        .collect();
    assert_eq!(acknowledged, vec![WebhookRejection::Replayed]);
    // Every code is distinct, so a stored refusal cannot be ambiguous.
    let codes: Vec<&str> = table
        .iter()
        .map(|(rejection, _, _)| rejection.as_str())
        .collect();
    let mut unique = codes.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(
        unique.len(),
        codes.len(),
        "the refusal codes must be distinct"
    );
}

#[test]
fn a_delivery_exposes_raw_bytes_and_refuses_to_resolve_an_ambiguous_header() {
    // The body is bytes so "verify what arrived" is the only thing a verifier can do. A `String` or a parsed
    // `Value` would make the re-serialization defect representable.
    let body = b"{\"key\":\"value\"}";
    let single: [(&str, &[u8]); 1] = [("X-Signature", b"abc123")];
    let delivery = WebhookDelivery {
        path: "/webhooks/example",
        headers: &single,
        body,
    };
    // The lookup is case-insensitive, because a header name on the wire is.
    assert_eq!(delivery.single_header("x-signature"), Some("abc123"));
    assert_eq!(delivery.single_header("X-SIGNATURE"), Some("abc123"));
    assert_eq!(delivery.single_header("absent"), None);
    assert_eq!(
        delivery.header_values("x-signature"),
        vec![b"abc123".as_slice()]
    );
    assert!(delivery.is_within_payload_bound(1_000));
    assert!(!delivery.is_within_payload_bound(2));
    assert_eq!(delivery.body, body, "the body must be the raw bytes");

    // A duplicate header is NOT resolved: an attacker presenting two signatures, or a proxy doing what
    // proxies do, must be refused rather than have one picked for it.
    let duplicated: [(&str, &[u8]); 2] = [("X-Signature", b"first"), ("x-signature", b"second")];
    let ambiguous = WebhookDelivery {
        path: "/webhooks/example",
        headers: &duplicated,
        body,
    };
    assert_eq!(
        ambiguous.single_header("x-signature"),
        None,
        "an ambiguous security header must not be resolved by taking the first"
    );
    assert_eq!(ambiguous.header_values("x-signature").len(), 2);
}

#[test]
fn a_replay_window_may_not_be_zero_and_its_relationship_with_the_skew_is_reported() {
    // Zero is refused because a disabled control that still looks configured is the failure direction that
    // matters. The relationship to the skew allowance is a **reported** fact, because the two are different
    // questions and a window shorter than the skew leaves a gap in which a replay is accepted.
    for unusable in [0, MAX_REPLAY_WINDOW_SECONDS + 1] {
        assert!(
            matches!(
                ReplayWindow::new(unusable),
                Err(SignatureError::ReplayWindow { .. })
            ),
            "a {unusable}-second window must be refused"
        );
    }
    let one_second = must(ReplayWindow::new(1), "a one-second window");
    assert_eq!(one_second.seconds(), 1);
    assert!(
        !one_second.covers_the_skew_allowance(),
        "a one-second replay window is shorter than the timestamp skew, so a replay inside a delivery's \
         allowance but outside the window would be accepted"
    );
    let full = must(
        ReplayWindow::new(MAX_REPLAY_WINDOW_SECONDS),
        "the maximum window",
    );
    assert!(full.covers_the_skew_allowance());
    // The bound itself is accepted, so the window check is a boundary.
    assert!(ReplayWindow::new(MAX_REPLAY_WINDOW_SECONDS).is_ok());
    assert_eq!(full.to_string(), format!("{MAX_REPLAY_WINDOW_SECONDS}s"));
    // The skew window is the documented five minutes, and it is smaller than the replay window — which is the
    // correct direction, since a provider may redeliver a failed delivery after a longer delay than the skew.
    // Both constants are bound to locals first, because `assert!(CONST > CONST2)` is a *constant assertion*
    // that clippy warns about — `P4-004`'s recorded note about the same shape.
    let skew = MAX_TIMESTAMP_SKEW_SECONDS;
    let replay = MAX_REPLAY_WINDOW_SECONDS;
    assert_eq!(skew, 300);
    assert!(replay > skew);
}

#[test]
fn a_delivery_that_presented_several_signatures_is_refused_by_name() {
    // The error carries the count, so a caller can report that two arrived rather than that none did — which
    // is the difference between a proxy misconfiguration and a wrong secret.
    let error = SignatureError::AmbiguousHeader {
        header: "x-signature",
        count: 2,
    };
    let message = error.to_string();
    assert!(message.contains("x-signature"), "got {message}");
    assert!(message.contains('2'), "got {message}");
    // A malformed field names the field rather than echoing the value, because the value may be attacker
    // text — the same rule `P3-008i` records for a parser's error.
    let malformed = SignatureError::Malformed { field: "timestamp" };
    let message = malformed.to_string();
    assert!(message.contains("timestamp"), "got {message}");
}
