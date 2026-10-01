//! Tests for the Calendar notification-channel push message.
//!
//! The load-bearing ones are: a `sync` message is **not** a change; an unknown resource state is **refused**
//! rather than guessed; an ambiguous header is refused rather than resolved; the echoed token is **surfaced
//! but redacted**; and the expiration is kept as **text** because it is not the Gmail lease's epoch millis.
//!
//! Falsification record in `TODO.md`.

use super::*;
use crate::webhook::{SignatureAlgorithm, SignatureEncoding, SignatureScheme};

fn must<T, E: std::fmt::Display>(result: Result<T, E>, what: &str) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("{what}: {error}"),
    }
}

/// The five headers the guide documents as always present, with a resource change.
///
/// Values are the guide's own examples where it gives one (`X-Goog-Message-Number: 10` for a change message,
/// the sample channel id and resource id), so a reader can compare the fixture against the page.
fn change_headers() -> Vec<(&'static str, &'static [u8])> {
    vec![
        (
            CHANNEL_ID_HEADER,
            b"4ba78bf0-6a47-11e2-bcfd-0800200c9a66".as_slice(),
        ),
        (RESOURCE_ID_HEADER, b"ret08u3rv24htgh289g".as_slice()),
        (
            RESOURCE_URI_HEADER,
            b"https://www.googleapis.com/calendar/v3/calendars/primary/events".as_slice(),
        ),
        (RESOURCE_STATE_HEADER, b"exists".as_slice()),
        (MESSAGE_NUMBER_HEADER, b"10".as_slice()),
    ]
}

/// Wraps a header list and a body into a delivery, at a path that names the endpoint.
fn delivery<'a>(headers: &'a [(&'a str, &'a [u8])], body: &'a [u8]) -> WebhookDelivery<'a> {
    WebhookDelivery {
        path: "/webhooks/google/calendar",
        headers,
        body,
    }
}

#[test]
fn the_sync_message_is_the_handshake_and_not_a_change() {
    // The one message on a channel that is **not** a resource update, and the reason `ResourceState` is an enum
    // rather than a bool: a caller must branch on it before doing any work, and `is_sync` is the predicate.
    // Acting on the sync message would start a read the moment a channel is created.
    let mut headers = change_headers();
    headers[3] = (RESOURCE_STATE_HEADER, b"sync".as_slice());
    // The guide states the sync message's number is always 1 — carried here so the fixture is the documented
    // shape, and asserted *below* that the detection does NOT depend on it.
    headers[4] = (MESSAGE_NUMBER_HEADER, b"1".as_slice());
    let message = must(
        parse_channel_message(&delivery(&headers, &[])),
        "the sync message is a documented shape",
    );
    assert!(message.is_sync(), "a sync message is the handshake");
    assert_eq!(message.resource_state, ResourceState::Sync);
    assert_eq!(message.message_number, 1);
    // **Detection is by state, not by the number.** A `sync` message whose number were not 1 would still be the
    // handshake, and an `exists` message numbered 1 would still be a change — so the property is the state.
    let mut numbered_like_sync = change_headers();
    numbered_like_sync[4] = (MESSAGE_NUMBER_HEADER, b"1".as_slice());
    let change = must(
        parse_channel_message(&delivery(&numbered_like_sync, &[])),
        "an exists message with number 1 is a change",
    );
    assert!(
        !change.is_sync(),
        "an `exists` message numbered 1 is a change, so the number is not the discriminator"
    );
}

#[test]
fn a_change_notification_is_not_a_handshake() {
    // The other direction, so `is_sync` cannot be a constant: an ordinary `exists` message is a change.
    let headers = change_headers();
    let message = must(
        parse_channel_message(&delivery(&headers, &[])),
        "a change message is a documented shape",
    );
    assert_eq!(message.resource_state, ResourceState::Exists);
    assert!(!message.is_sync());
    assert_eq!(message.channel_id, "4ba78bf0-6a47-11e2-bcfd-0800200c9a66");
    assert_eq!(message.resource_id, "ret08u3rv24htgh289g");
    assert_eq!(message.message_number, 10);
    // `not_exists` is a third state and is **not** the handshake either.
    let mut deleted = change_headers();
    deleted[3] = (RESOURCE_STATE_HEADER, b"not_exists".as_slice());
    let gone = must(
        parse_channel_message(&delivery(&deleted, &[])),
        "not_exists is a documented state",
    );
    assert_eq!(gone.resource_state, ResourceState::NotExists);
    assert!(!gone.is_sync(), "not_exists is a change, not a handshake");
}

#[test]
fn each_required_header_is_refused_by_name_when_absent() {
    // A missing header is the provider sending less than it promised, and the refusal must name the field so a
    // caller knows what to look at -- "a bad notification" would leave it guessing. Each of the five is removed
    // in turn, so a parser that quietly defaulted one would fail here.
    for missing in [
        CHANNEL_ID_HEADER,
        RESOURCE_ID_HEADER,
        RESOURCE_URI_HEADER,
        RESOURCE_STATE_HEADER,
        MESSAGE_NUMBER_HEADER,
    ] {
        let headers: Vec<(&str, &[u8])> = change_headers()
            .into_iter()
            .filter(|(name, _)| *name != missing)
            .collect();
        match parse_channel_message(&delivery(&headers, &[])) {
            Err(ChannelMessageError::Missing { header }) => {
                assert_eq!(header, missing, "the refusal must name the absent header");
            }
            other => panic!("removing `{missing}` must be refused, got {other:?}"),
        }
    }
}

#[test]
fn an_ambiguous_header_is_refused_rather_than_resolved() {
    // Two values for a security-relevant header is a proxy or an attacker, and picking one lets the wrong value
    // win -- the same refusal `WebhookDelivery::single_header` makes, reached through `read_header`.
    let mut headers = change_headers();
    headers.push((RESOURCE_STATE_HEADER, b"sync".as_slice()));
    match parse_channel_message(&delivery(&headers, &[])) {
        Err(ChannelMessageError::Ambiguous { header, count }) => {
            assert_eq!(header, RESOURCE_STATE_HEADER);
            assert_eq!(count, 2);
        }
        other => panic!("two resource-state headers must be refused, got {other:?}"),
    }
}

#[test]
fn an_unknown_resource_state_is_refused_rather_than_guessed() {
    // The state is how a caller tells a handshake from a change, so an unrecognised value is a message it
    // cannot decide how to act on -- and the fail-open direction (treat it as a change) would act on a message
    // the connector does not understand, while the fail-closed alternative of ignoring it could miss a change.
    // Refusing is the only answer that names the problem instead of choosing a direction.
    let mut headers = change_headers();
    headers[3] = (RESOURCE_STATE_HEADER, b"deleted".as_slice());
    match parse_channel_message(&delivery(&headers, &[])) {
        Err(ChannelMessageError::UnknownResourceState { value }) => assert_eq!(value, "deleted"),
        other => panic!("an unknown resource state must be refused, got {other:?}"),
    }
    // The three documented values are matched **exactly**, so a case variant is unknown rather than silently
    // accepted -- the provider's vocabulary is its own tokens, not a case-insensitive enum.
    for not_documented in ["Sync", "EXISTS", ""] {
        let mut headers = change_headers();
        headers[3] = (RESOURCE_STATE_HEADER, not_documented.as_bytes());
        assert!(
            matches!(
                parse_channel_message(&delivery(&headers, &[])),
                Err(ChannelMessageError::UnknownResourceState { .. })
            ),
            "`{not_documented}` is not a documented state and must be refused"
        );
    }
}

#[test]
fn the_message_number_is_read_and_is_not_a_position() {
    // The number increases but is not sequential, so it is read as an integer and nothing else -- a caller
    // must not treat it as a cursor. A non-numeric value is refused rather than defaulted.
    let headers = change_headers();
    let message = must(
        parse_channel_message(&delivery(&headers, &[])),
        "the message number parses",
    );
    assert_eq!(message.message_number, 10);
    let mut unreadable = change_headers();
    unreadable[4] = (MESSAGE_NUMBER_HEADER, b"ten".as_slice());
    match parse_channel_message(&delivery(&unreadable, &[])) {
        Err(ChannelMessageError::UnreadableMessageNumber { value }) => assert_eq!(value, "ten"),
        other => panic!("a non-numeric message number must be refused, got {other:?}"),
    }
}

#[test]
fn the_echoed_token_is_surfaced_but_redacted_in_debug() {
    // The channel token is the anti-spoofing control, so it must be reachable (a verifier needs it) and must
    // never be printed (a log line is where an attacker would read it to forge a delivery). `ADR-0091`'s shape.
    let mut headers = change_headers();
    headers.push((
        CHANNEL_TOKEN_HEADER,
        b"target=myApp-myChannelDest".as_slice(),
    ));
    let message = must(
        parse_channel_message(&delivery(&headers, &[])),
        "a token-bearing message parses",
    );
    let token = "target=myApp-myChannelDest";
    assert_eq!(message.channel_token(), Some(token));
    let rendered = format!("{message:?}");
    assert!(
        !rendered.contains(token),
        "the token must not appear in a `Debug` rendering: {rendered}"
    );
    assert!(
        rendered.contains("REDACTED"),
        "the redaction must be visible, not the value: {rendered}"
    );
    // The length is derived from the token rather than hardcoded, so the assertion cannot drift from a
    // mistyped constant -- which is exactly the trap a literal here sets.
    assert!(
        rendered.contains(&format!("{} chars", token.len())),
        "the length is the useful diagnostic and must be shown: {rendered}"
    );
    // A delivery with no token surfaces `None` rather than an empty string, because "no token was sent" and "a
    // zero-length token" are different facts.
    let message = must(
        parse_channel_message(&delivery(&change_headers(), &[])),
        "a message without a token parses",
    );
    assert_eq!(message.channel_token(), None);
}

#[test]
fn the_expiration_is_kept_as_text_because_it_is_not_epoch_millis() {
    // The guide types this header "in human-readable format" -- a date string -- which is the **opposite** of
    // the Gmail watch lease's `expiration`, an epoch-millis *string*. So this module keeps the value verbatim
    // rather than parsing it as a number, and a shared `parse` would have to guess which encoding it held.
    let mut headers = change_headers();
    headers.push((
        CHANNEL_EXPIRATION_HEADER,
        b"Tue, 19 Nov 2013 01:13:52 GMT".as_slice(),
    ));
    let message = must(
        parse_channel_message(&delivery(&headers, &[])),
        "an expiring channel's message parses",
    );
    assert_eq!(
        message.expiration.as_deref(),
        Some("Tue, 19 Nov 2013 01:13:52 GMT"),
        "the value is a human-readable date, kept verbatim"
    );
    // And the absence of an expiration is `None`, because the header is present only while the channel expires.
    let message = must(
        parse_channel_message(&delivery(&change_headers(), &[])),
        "a non-expiring channel's message parses",
    );
    assert_eq!(message.expiration, None);
}

#[test]
fn the_body_is_not_read_so_a_nonempty_body_changes_nothing() {
    // The delivery has a zero-length body, and this is the property that makes an echoed channel token the
    // authenticator rather than a MAC. The parser reads headers only, so a body -- of any content -- cannot
    // change the result, which is what "nothing to sign" means in code.
    let headers = change_headers();
    let empty = must(
        parse_channel_message(&delivery(&headers, &[])),
        "the documented zero-length body parses",
    );
    let with_body = must(
        parse_channel_message(&delivery(&headers, b"{\"not\":\"read\"}")),
        "a body is ignored, not refused",
    );
    assert_eq!(
        empty, with_body,
        "the body is not read, so it cannot change the message"
    );
}

#[test]
fn the_channel_token_header_is_the_one_the_echoed_authenticator_names() {
    // The join between this module and `ADR-0099`: the contract names an `EchoedChannelToken` authenticator
    // arriving in a header, and this is the header it arrives in. The two are asserted **equal** so a verifier
    // built on one cannot read the other's header and find nothing.
    let scheme = must(
        SignatureScheme::new(
            SignatureAlgorithm::EchoedChannelToken,
            CHANNEL_TOKEN_HEADER,
            // A header-token authenticator presents an opaque value, so its encoding is `raw` -- a non-raw
            // encoding is refused by `SignatureScheme::new`.
            SignatureEncoding::Raw,
        ),
        "the channel token header is a valid scheme header",
    );
    assert_eq!(scheme.header, CHANNEL_TOKEN_HEADER);
    assert!(
        scheme.algorithm.is_body_independent(),
        "the echoed token is body-independent, which is why a Calendar delivery has nothing to MAC"
    );
    assert!(scheme.authenticates());
}
