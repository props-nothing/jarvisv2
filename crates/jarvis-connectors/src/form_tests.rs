//! Tests for the form codec: the one rule both directions share.
//!
//! The load-bearing assertion is the **difference from RFC 3986 encoding**, because that is the mistake the
//! module exists to prevent: a caller reaching for the request side's encoder would produce `%20` for a space
//! and the server would read a different value.

use super::*;
use crate::google::request::percent_encode;

#[test]
fn a_space_is_plus_and_the_unreserved_set_is_narrower_than_rfc_3986s() {
    // HTML 4.01 §17.13.4, which RFC 6749 Appendix B points at: "Space characters are replaced by `+', and then
    // reserved characters are escaped ... Non-alphanumeric characters are replaced by `%HH'." So the alphabet
    // that survives is **alphanumerics only** — narrower than RFC 3986's `ALPHA / DIGIT / - . _ ~`.
    for character in "abcXYZ019".chars() {
        assert_eq!(
            encode_component(&character.to_string()),
            character.to_string(),
            "`{character}` is alphanumeric and must not be encoded"
        );
    }
    // The four characters RFC 3986 leaves alone and this encoding escapes. Escaping more is interoperable in
    // both directions; escaping less is what produces a value the server reads differently.
    assert_eq!(encode_component(" "), "+");
    assert_eq!(encode_component("~"), "%7E");
    assert_eq!(encode_component("-"), "%2D");
    assert_eq!(encode_component("."), "%2E");
    assert_eq!(encode_component("_"), "%5F");
    // Uppercase hex, so a reader comparing this with the RFC 3986 encoder does not have to decide whether the
    // casing difference means anything.
    assert_eq!(encode_component("/"), "%2F");
    assert_eq!(encode_component("a b"), "a+b");
}

#[test]
fn the_form_encoder_and_the_query_encoder_are_not_interchangeable() {
    // **This is the assertion the module exists for.** The same input produces two different strings, and a
    // caller must not substitute one for the other: a query parameter and a form body follow different rules
    // about the space and about `~ - . _`. If a future refactor made them one function, this fails.
    assert_ne!(
        encode_component("a b~c"),
        percent_encode("a b~c"),
        "the form and query encodings must stay distinguishable"
    );
    assert_eq!(encode_component("a b"), "a+b");
    assert_eq!(percent_encode("a b"), "a%20b");
    // Both escape a literal `+`, but by the same route, so the two agree there rather than differing.
    assert_eq!(encode_component("a+b"), "a%2Bb");
    assert_eq!(percent_encode("a+b"), "a%2Bb");
}

#[test]
fn a_non_ascii_value_is_encoded_per_utf8_byte() {
    // RFC 6749 §4.1.3: the body is "with a character encoding of UTF-8". So a multi-byte character becomes one
    // `%XX` per byte, and the round trip must return the original string.
    let encoded = encode_component("£");
    assert_eq!(encoded, "%C2%A3");
    assert_eq!(encoded.len(), 6);
    assert_eq!(decode_component(&encoded).unwrap_or_default(), "£");
    // And a two-byte character that is not the pound sign, so the assertion is not about one byte pattern.
    assert_eq!(encode_component("é"), "%C3%A9");
}

#[test]
fn decoding_reverses_the_encoding_for_every_byte_it_can_produce() {
    // A round trip over a value containing every class the encoder treats differently: alphanumerics (passed
    // through), a space (`+`), RFC 3986-unreserved characters this encoding escapes, a literal `+` (`%2B`,
    // which must NOT come back as a space), and a non-ASCII character.
    let original = "a b~-._+/:£é";
    let encoded = encode_component(original);
    assert_eq!(
        decode_component(&encoded).unwrap_or_default(),
        original,
        "the round trip must be lossless: {encoded}"
    );
    // The specific confusion the two-way rule creates: `+` decodes to a space while `%2B` decodes to a `+`.
    assert_eq!(decode_component("a+b").unwrap_or_default(), "a b");
    assert_eq!(decode_component("a%2Bb").unwrap_or_default(), "a+b");
}

#[test]
fn a_malformed_escape_or_invalid_utf8_is_refused_rather_than_lossily_decoded() {
    // Appendix B says a parsed value is "an octet sequence, to be decoded using the UTF-8 character encoding
    // scheme". A lossy decode would turn a corrupted value into a *different* string that merely fails to
    // match, hiding a transport fault behind what looks like a security refusal.
    for (input, expected) in [
        ("%2", FormError::MalformedEscape),
        ("%zz", FormError::MalformedEscape),
        ("%F", FormError::MalformedEscape),
        ("%FF%FE", FormError::NotUtf8),
    ] {
        assert_eq!(
            decode_component(input),
            Err(expected),
            "`{input}` must be refused"
        );
        assert!(!expected.reason().is_empty());
    }
    // A bare `%` at the very end is the truncation case, and a `%` with a valid first digit but no second.
    assert_eq!(decode_component("%"), Err(FormError::MalformedEscape));
    assert_eq!(decode_component("%0"), Err(FormError::MalformedEscape));
}

#[test]
fn a_body_is_the_ordered_pairs_joined_with_ampersands() {
    // The shape a transport puts in an HTTP body. Order is preserved rather than sorted, so the same logical
    // request produces the same bytes on every run — which is what makes a body comparable across runs.
    //
    // **The names are escaped too**, and `grant_type` shows it: the underscore becomes `%5F`. That is correct
    // rather than a defect — HTML 4.01 §17.13.4 says non-alphanumeric characters become `%HH`, and it says so
    // about "control names AND values". A server decodes `%5F` back to `_`, and Google accepts the escaped
    // form of its own parameter names. My first version of this assertion wrote the names unescaped, which is
    // the value-looks-reasonable mistake the module's own test for names exists to catch.
    let parameters = vec![
        ("grant_type".to_owned(), "authorization_code".to_owned()),
        ("code".to_owned(), "a b".to_owned()),
    ];
    assert_eq!(
        encode_body(&parameters),
        "grant%5Ftype=authorization%5Fcode&code=a+b"
    );
    // An alphanumeric name is passed through unchanged, so the escaping is not blanket.
    assert_eq!(
        encode_body(&[("code".to_owned(), "x".to_owned())]),
        "code=x"
    );
    assert_eq!(encode_body(&[]), "");
}

#[test]
fn a_parameter_name_is_encoded_too_not_only_its_value() {
    // An `=` or `&` in a NAME restructures the body exactly as it would in a query string, so encoding only
    // the value is the habit that lets a caller-supplied name through unescaped.
    let parameters = vec![("a&b=c".to_owned(), "v".to_owned())];
    let body = encode_body(&parameters);
    assert_eq!(body, "a%26b%3Dc=v");
    assert_eq!(
        body.split('&').count(),
        1,
        "an encoded name must not start a new pair: {body}"
    );
}

#[test]
fn the_content_type_is_the_one_the_token_endpoint_requires() {
    // RFC 6749 §4.1.3 requires this media type for the token request's body, and a transport that guessed or
    // omitted it produces a request the server cannot parse — which reads as an auth failure, not a header.
    assert_eq!(CONTENT_TYPE, "application/x-www-form-urlencoded");
    assert_eq!(SPACE_ENCODED, '+');
}
