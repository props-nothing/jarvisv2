//! Tests for the access-token boundary.
//!
//! The point of these is mostly **negative**: that the token cannot be rendered, serialized, or reached
//! except through a named exposure. An assertion that `Debug` does not contain a value proves a control that
//! every other assertion in this file depends on.
//!
//! Falsification record in `TODO.md`.

use super::*;
use crate::google::request::AUTHORIZATION_HEADER as REQUEST_HEADER;

/// A value long enough to satisfy the floor, and distinctive enough that a leak is greppable.
const TOKEN: &str = "ya29.a0AfH6SMBsecretvalue1234567890";

fn must<T, E: std::fmt::Display>(result: Result<T, E>, what: &str) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("{what}: {error}"),
    }
}

fn token() -> AccessToken {
    must(AccessToken::new(TOKEN), "a valid token")
}

#[test]
fn a_token_is_never_rendered_by_debug() {
    // The central control, and the reason `Debug` is hand-written. A derived impl would print the value, and
    // one `{:?}` in a log, an error, or a test failure message is enough to leak it.
    let rendered = format!("{:?}", token());
    assert!(
        !rendered.contains(TOKEN),
        "`Debug` must not render the token: {rendered}"
    );
    assert!(rendered.contains("[REDACTED]"), "{rendered}");
    // The length IS reported, because it is not the value and it is what distinguishes two credentials.
    assert!(rendered.contains(&TOKEN.len().to_string()), "{rendered}");
    // The `Debug` of an `Option<AccessToken>` and of a `Vec` go through the same impl, which is what a
    // containing struct's derived `Debug` would use — so a token nested in a struct is also safe.
    let nested = format!("{:?}", Some(token()));
    assert!(!nested.contains(TOKEN), "{nested}");
}

#[test]
fn the_token_is_unreachable_except_through_a_named_exposure() {
    // `with_exposed` is the only accessor, and its name is the warning. Asserted by the value being reachable
    // through it and the closure receiving only the text rather than the whole token.
    let exposed = token().with_exposed(str::to_owned);
    assert_eq!(exposed, TOKEN);
    // The closure's argument is a `&str`, so a caller cannot move the token out and store it where a later
    // `Debug` could reach it — the borrow cannot outlive the call.
    let length = token().with_exposed(str::len);
    assert_eq!(length, TOKEN.len());
}

#[test]
fn the_authorization_header_is_the_only_place_the_bytes_and_the_scheme_meet() {
    // A transport that built the header itself would be a second implementation of `"Bearer " + value`, which
    // is where a missing space or a doubled scheme comes from.
    let header = token().authorization_header_value();
    assert_eq!(header, format!("Bearer {TOKEN}"));
    assert!(header.starts_with("Bearer "));
    // The single space is the detail worth pinning: `Bearer  x` and `Bearerx` are both wrong and both
    // plausible typos.
    assert!(header.contains("Bearer "));
    assert!(!header.contains("Bearer  "), "one space, not two");
    assert_eq!(
        header.matches(' ').count(),
        1,
        "a token may not contain a space"
    );
}

#[test]
fn the_two_modules_state_one_header_name_and_one_scheme() {
    // The request module states where a credential goes and this module states what goes there. Two constants
    // for one fact would be the "two values that must agree, with nothing holding both" defect, so they are
    // asserted equal rather than trusted to stay so.
    assert_eq!(AUTHORIZATION_HEADER, REQUEST_HEADER);
    assert_eq!(BEARER_SCHEME, crate::google::request::BEARER_SCHEME);
    assert_eq!(AUTHORIZATION_HEADER, "Authorization");
}

#[test]
fn a_pasted_header_reports_removing_the_scheme_and_not_the_whitespace() {
    // The ordering decision, and the reason it is a decision: a pasted `Authorization` header contains BOTH a
    // scheme and a space, and reporting "contains whitespace" is technically true and useless — it sends a
    // reader hunting an invisible character. The scheme check runs first so the message says what to do.
    let pasted = format!("Bearer {TOKEN}");
    assert_eq!(
        AccessToken::new(pasted).err(),
        Some(AccessTokenError::IncludesScheme)
    );
    let lowercase = format!("bearer {TOKEN}");
    assert_eq!(
        AccessToken::new(lowercase).err(),
        Some(AccessTokenError::IncludesScheme),
        "a lowercase scheme is the same mistake"
    );
    // And a value with only a space inside still reports the whitespace, so the second message is reachable.
    assert_eq!(
        AccessToken::new("ya29.value with space here").err(),
        Some(AccessTokenError::ContainsWhitespace)
    );
}

#[test]
fn every_refusal_names_the_problem_and_none_renders_the_value() {
    // The property that matters for an error about a credential: it reaches a log, so an error that printed
    // the value it refused would leak the thing it was protecting. Asserted for every variant at once,
    // because a new variant is the case that would forget.
    let cases = [
        (String::new(), AccessTokenError::Empty),
        ("short".to_owned(), AccessTokenError::TooShort),
        (
            "a".repeat(MAX_ACCESS_TOKEN_CHARS + 1),
            AccessTokenError::TooLong,
        ),
        (
            "ya29.abc\ndef".to_owned(),
            AccessTokenError::ContainsControl,
        ),
        (
            "ya29.abc def".to_owned(),
            AccessTokenError::ContainsWhitespace,
        ),
        (format!("Bearer {TOKEN}"), AccessTokenError::IncludesScheme),
    ];
    for (value, expected) in cases {
        let Err(error) = AccessToken::new(value.clone()) else {
            panic!("`{expected:?}` must be refused");
        };
        assert_eq!(error, expected);
        let rendered = error.to_string();
        assert!(
            !rendered.contains(&value) || value.is_empty(),
            "the error for {expected:?} rendered the value it refused: {rendered}"
        );
        assert!(
            !rendered.contains(TOKEN),
            "an error must never render a token: {rendered}"
        );
    }
}

#[test]
fn the_length_floor_makes_a_paste_mistake_surface_where_it_was_made() {
    // A client id or a project number is much shorter than a token, so the floor catches the common
    // mistake at construction rather than at the provider, where a generic auth error sends a reader to
    // debug the credential's validity instead of its shape.
    assert_eq!(
        AccessToken::new("1234567890123456789").err(),
        Some(AccessTokenError::TooShort),
        "a 19-character value is below the floor"
    );
    let at_floor = "a".repeat(MIN_ACCESS_TOKEN_CHARS);
    assert!(
        AccessToken::new(&at_floor).is_ok(),
        "the floor must be reachable rather than one below it"
    );
    // And the ceiling is reachable too, so neither bound is an unreachable one — the defect `P5-001`
    // recorded for a limit that could not be hit.
    let at_ceiling = "a".repeat(MAX_ACCESS_TOKEN_CHARS);
    assert!(AccessToken::new(&at_ceiling).is_ok());
}

#[test]
fn two_tokens_of_different_lengths_are_distinguishable_without_either_being_printed() {
    // `char_len` is what a diagnostic uses to answer "is this the credential I think it is?" without the
    // credential being part of the record. Asserted on two values that share a PREFIX, so a comparison that
    // rendered a prefix could not pass.
    let first = must(
        AccessToken::new("ya29.aaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        "a valid token",
    );
    let second = must(
        AccessToken::new("ya29.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        "a valid token",
    );
    assert_ne!(first.char_len(), second.char_len());
    assert_ne!(format!("{first:?}"), format!("{second:?}"));
    assert!(!format!("{first:?}").contains("ya29."));
    assert!(!format!("{second:?}").contains("ya29."));
}

#[test]
fn a_token_can_say_which_credential_it_came_from_without_saying_what_it_is() {
    // The attribution a diagnostic needs: "the stored refresh exchange for this account failed" rather than
    // "some token is wrong". A `SecretRef` is metadata, so it is safe to render — and its own `Debug` redacts
    // its locator, which this asserts rather than assumes.
    let origin = must(
        SecretRef::new("os-keychain", "google-oauth-refresh", "google-account"),
        "a valid secret reference",
    );
    let attributed = token().with_origin(origin);
    let Some(origin) = attributed.origin() else {
        panic!("the origin must be recorded");
    };
    assert_eq!(origin.provider(), "os-keychain");
    let rendered = format!("{origin:?}");
    assert!(
        !rendered.contains("google-oauth-refresh"),
        "a secret reference must redact its locator: {rendered}"
    );
    // And a token with no origin is ordinary rather than an error: material obtained from an in-memory
    // exchange has no stored secret to name.
    let bare = token();
    assert!(bare.origin().is_none());
    assert_eq!(bare.with_exposed(str::to_owned), TOKEN);
}

#[test]
fn a_control_character_is_refused_because_it_would_forge_a_header() {
    // A newline in a header value is header injection, and in a log line it is a forged record. Asserted with
    // the specific characters rather than one, because `\r` and `\t` fail differently.
    for hazardous in ['\n', '\r', '\t', '\u{0}'] {
        let value = format!("ya29.abc{hazardous}defghijklmnop");
        assert_eq!(
            AccessToken::new(value).err(),
            Some(AccessTokenError::ContainsControl),
            "{hazardous:?} must be refused"
        );
    }
}
