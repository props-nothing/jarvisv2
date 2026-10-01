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

/// A registered channel token, as the connector would hold it.
fn stored(value: &str) -> crate::auth::SecretValue {
    must(
        crate::auth::SecretValue::new(value),
        "a valid registered channel token",
    )
}

/// A parsed message whose delivery carries the given channel token (or none).
fn message_with_token(token: Option<&str>) -> ChannelMessage {
    let mut headers = change_headers();
    if let Some(token) = token {
        headers.push((CHANNEL_TOKEN_HEADER, token.as_bytes()));
    }
    must(
        parse_channel_message(&delivery(&headers, &[])),
        "a message with the given token parses",
    )
}

#[test]
fn a_registered_token_verifies_only_against_the_exact_echoed_value() {
    // The positive case and the security-relevant one: a token that matches verifies, and any *other* value is
    // a mismatch -- not merely "not verified", because a mismatch is a forged or misrouted delivery.
    let expected = stored("target=myApp-myChannelDest");
    let exact = message_with_token(Some("target=myApp-myChannelDest"));
    assert_eq!(
        verify_channel_token(Some(&expected), &exact),
        ChannelTokenCheck::Verified
    );
    // Five near-misses, each one a rule a looser comparison would accept. The token is a shared secret, so the
    // comparison is exact: a prefix, a superstring, a case difference, a trailing space, and an empty value are
    // all mismatches.
    for near_miss in [
        "target=myApp-myChannelDes",   // prefix
        "target=myApp-myChannelDestX", // superstring
        "TARGET=MYAPp-mychanneldest",  // case differs
        "target=myApp-myChannelDest ", // trailing space
        "",                            // empty
    ] {
        assert_eq!(
            verify_channel_token(Some(&expected), &message_with_token(Some(near_miss))),
            ChannelTokenCheck::Mismatch,
            "`{near_miss}` must not verify against a different stored token"
        );
    }
    // And a **different but well-formed** token is a mismatch, so `Verified` is not a constant.
    let other = stored("target=myApp-otherChannel");
    assert_eq!(
        verify_channel_token(Some(&other), &exact),
        ChannelTokenCheck::Mismatch,
        "the same delivery must not verify against a different channel's token"
    );
}

#[test]
fn a_missing_token_is_required_or_absent_depending_on_what_was_registered() {
    // **The pair that a `bool` would collapse.** A delivery with no token is the documented shape when the
    // channel was registered without one (`Absent`, and it may be acted on), and a refusal when one was
    // registered (`TokenRequired`). Same delivery, opposite answers -- so the stored value is what decides, and
    // neither state is reachable from the other.
    let without = message_with_token(None);
    assert_eq!(
        verify_channel_token(None, &without),
        ChannelTokenCheck::Absent,
        "no stored token means there is no control to check"
    );
    assert!(
        ChannelTokenCheck::Absent.may_be_acted_on(),
        "an absent control is not a failed one"
    );
    let expected = stored("target=myApp-myChannelDest");
    assert_eq!(
        verify_channel_token(Some(&expected), &without),
        ChannelTokenCheck::TokenRequired,
        "a registered token with no presented one cannot prove the channel"
    );
    assert!(
        !ChannelTokenCheck::TokenRequired.may_be_acted_on(),
        "a missing token against a registered one is a refusal"
    );
    // **`Absent` and `TokenRequired` are distinguishable**, which is the reason this is an enum: read as a
    // `bool` both would be "no token", and an operator would not know whether a control was configured.
    assert_ne!(ChannelTokenCheck::Absent, ChannelTokenCheck::TokenRequired);
}

#[test]
fn only_the_two_refusal_states_are_rejections_and_the_pair_reads_the_same_both_ways() {
    // The mapping a caller wires to an alert: the two refusals are rejections, the two non-refusals are not,
    // and `is_rejection` is exactly the complement of `may_be_acted_on`. Asserted over **every** variant, so a
    // future variant that forgot one accessor fails here rather than defaulting to "accepted".
    let table = [
        (ChannelTokenCheck::Verified, true),
        (ChannelTokenCheck::Absent, true),
        (ChannelTokenCheck::Mismatch, false),
        (ChannelTokenCheck::TokenRequired, false),
    ];
    for (check, may_be_acted_on) in table {
        assert_eq!(check.may_be_acted_on(), may_be_acted_on, "{check:?}");
        assert_eq!(
            check.is_rejection(),
            !may_be_acted_on,
            "{check:?} must be the exact complement"
        );
    }
    // **An `Absent` must NOT page anyone.** A channel the operator registered without a token is a deliberate
    // choice, not an attack -- the same "a replay must not page" rule `WebhookRejection` draws -- so treating
    // every non-`Verified` answer as suspicious would alert on a documented configuration.
    assert!(!ChannelTokenCheck::Absent.is_rejection());
}

#[test]
fn a_candidate_of_a_different_length_is_refused_without_a_bound_check() {
    // The comparison is constant-time with respect to **content** and refuses a different-length candidate
    // immediately, so there is deliberately **no separate bound check** (`ADR-0101`): a guard that rejected an
    // over-long candidate "before the comparison" would itself walk the attacker-supplied value and decide
    // nothing the comparison has not already decided. What this proves is the observable behaviour that makes
    // such a guard unnecessary — an oversized candidate is a mismatch, and the check is not what refuses it.
    let at_bound = "y".repeat(MAX_CHANNEL_TOKEN_BYTES);
    let expected = stored(&at_bound);
    // A candidate ONE byte longer than the bound, sharing the stored token as a prefix: refused as a mismatch.
    let over = format!("{at_bound}y");
    assert_eq!(
        verify_channel_token(Some(&expected), &message_with_token(Some(&over))),
        ChannelTokenCheck::Mismatch,
        "a longer candidate cannot verify, because its length differs from the stored token's"
    );
    // The control, and the assertion that shows there is no hidden bound: the token exactly at the bound, with
    // identical content, **verifies** — so the refusal above is the length, not a 256-character ceiling.
    assert_eq!(
        verify_channel_token(Some(&expected), &message_with_token(Some(&at_bound))),
        ChannelTokenCheck::Verified,
        "a value at the documented maximum is accepted, so nothing enforces the maximum as a guard"
    );
    assert_eq!(MAX_CHANNEL_TOKEN_BYTES, 256, "the guide's stated maximum");
}

/// A message whose delivery names the given channel id.
fn message_for_channel(channel_id: &str) -> ChannelMessage {
    // The header list is typed `&'static [u8]`, so the dynamic id is leaked — a test-only leak, and the
    // clearest way to keep the fixture's slice types rather than parameterising them.
    let leaked: &'static [u8] = Box::leak(channel_id.to_owned().into_boxed_str()).as_bytes();
    let mut headers = change_headers();
    headers[0] = (CHANNEL_ID_HEADER, leaked);
    must(
        parse_channel_message(&delivery(&headers, &[])),
        "a message for the given channel parses",
    )
}

/// A registration for a channel id and an account.
fn registration(channel_id: &str, account: &str) -> ChannelRegistration {
    ChannelRegistration::new(
        channel_id.to_owned(),
        must(
            crate::account::AccountReference::new(account),
            "a valid account reference",
        ),
        None,
    )
}

#[test]
fn a_delivery_routes_to_the_account_whose_registration_names_its_channel() {
    // The join the push path needs: a Calendar delivery names the **channel** and never the account, so the
    // sync it triggers learns whose credential to use only from what the connector registered. Two
    // registrations, and the delivery's channel selects exactly one.
    let registrations = [
        registration("channel-alpha", "acct-alpha"),
        registration("channel-beta", "acct-beta"),
    ];
    let route = route_channel(&message_for_channel("channel-beta"), &registrations);
    assert_eq!(
        route
            .account()
            .map(crate::account::AccountReference::as_str),
        Some("acct-beta"),
        "the delivery must route to the account that registered that exact channel"
    );
    assert!(route.may_be_applied_automatically());
    // A channel nobody registered routes nowhere -- and this is not an error, because a delivery for an
    // unknown channel is answered by acknowledging and recording.
    assert_eq!(
        route_channel(&message_for_channel("channel-gamma"), &registrations),
        ChannelRoute::Unknown
    );
    // And a connector with no registrations routes nothing, which is the same answer as a non-matching channel.
    assert_eq!(
        route_channel(&message_for_channel("channel-beta"), &[]),
        ChannelRoute::Unknown
    );
}

#[test]
fn a_channel_id_is_matched_exactly_and_a_case_variant_is_not_the_same_channel() {
    // The channel id is the connector's **own** value, compared against the value it stored, so the comparison
    // is byte-exact. Unlike the `emailAddress` near-match `ADR-0097` reports, a channel id has no case
    // ambiguity to report: it is an identifier, and a case variant is a different id. None of these is the
    // stored id, so each routes `Unknown` rather than to the wrong account.
    let registrations = [registration("channel-alpha", "acct-alpha")];
    for near_miss in [
        "Channel-Alpha",  // case differs
        "channel-alph",   // prefix
        "channel-alphaa", // superstring
        "channel-alpha ", // trailing space
        " channel-alpha", // leading space
    ] {
        assert_eq!(
            route_channel(&message_for_channel(near_miss), &registrations),
            ChannelRoute::Unknown,
            "`{near_miss}` is not a registered channel id"
        );
    }
    // The control: the exact id DOES route, so the refusals above are about the comparison and not a route
    // function that matches nothing.
    assert!(
        route_channel(&message_for_channel("channel-alpha"), &registrations)
            .may_be_applied_automatically()
    );
}

#[test]
fn two_registrations_for_one_channel_id_are_ambiguous_and_never_a_pick() {
    // Reachable when a channel id is reused -- which the guide's UUID recommendation exists to prevent, but a
    // recommendation is not an enforcement. Two registrations for one id make the route **undecidable**, so it
    // reports a **count** and carries no reference: picking the first, oldest, or most recent would sync one
    // account under another's identity.
    let registrations = [
        registration("channel-alpha", "acct-one"),
        registration("channel-alpha", "acct-two"),
    ];
    let route = route_channel(&message_for_channel("channel-alpha"), &registrations);
    assert_eq!(route, ChannelRoute::Ambiguous { accounts: 2 });
    assert_eq!(
        route.account(),
        None,
        "an ambiguous route must not expose an arbitrarily chosen account"
    );
    assert!(!route.may_be_applied_automatically());
}

#[test]
fn a_registration_redacts_its_token_and_the_route_uses_only_the_id() {
    // The registration holds the channel's anti-spoofing token, so a `{:?}` must not print it (`ADR-0091`). And
    // the route is decided by the **id** alone -- the token is a separate control -- so a registration whose
    // token differs still routes, because routing and verification answer different questions.
    let with_token = ChannelRegistration::new(
        "channel-alpha".to_owned(),
        must(
            crate::account::AccountReference::new("acct-alpha"),
            "a valid account reference",
        ),
        Some(stored("target=myApp-myChannelDest")),
    );
    let rendered = format!("{with_token:?}");
    assert!(
        !rendered.contains("target=myApp-myChannelDest"),
        "the registration must not print its token: {rendered}"
    );
    assert!(
        rendered.contains("REDACTED"),
        "the redaction must be visible: {rendered}"
    );
    assert_eq!(
        with_token.token().map(crate::auth::SecretValue::expose),
        Some("target=myApp-myChannelDest"),
        "the token is reachable for verification"
    );
    // Routing ignores the token entirely: a registration with a token still routes by id.
    assert_eq!(
        route_channel(&message_for_channel("channel-alpha"), &[with_token])
            .account()
            .map(crate::account::AccountReference::as_str),
        Some("acct-alpha")
    );
    // And a registration with NO token is a recorded choice, surfaced as `None` rather than a sentinel.
    assert_eq!(registration("channel-beta", "acct-beta").token(), None);
}
