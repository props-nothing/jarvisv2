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
///
/// The resource id is the guide's own example value, because it is the provider's that the connector only ever
/// echoes back — the stop call's second identifier.
fn registration(channel_id: &str, account: &str) -> ChannelRegistration {
    ChannelRegistration::new(
        channel_id.to_owned(),
        "ret08u3rv24htgh289g".to_owned(),
        must(
            crate::account::AccountReference::new(account),
            "a valid account reference",
        ),
        None,
        // An arbitrary but representable expiry: routing and verification do not read it, so its value is
        // irrelevant to the tests that use this helper.
        instant(1_426_325_213),
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
        "o3hgv1538sdjfh".to_owned(),
        must(
            crate::account::AccountReference::new("acct-alpha"),
            "a valid account reference",
        ),
        Some(stored("target=myApp-myChannelDest")),
        instant(1_426_325_213),
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

#[test]
fn a_registration_carries_both_identifiers_the_stop_call_needs_and_they_reach_it_in_order() {
    // **The finding this field exists for.** `parse_channel_watch_response` read the `watch` response's
    // `resourceId` and there was **nowhere to put it**, so the value the stop call needs was dropped one
    // function after it was obtained -- while `ADR-0106`'s own note said `resourceId` is *"what the
    // `channels.stop` call needs"*. A registration is the only place that survives between the `watch` and the
    // teardown, so it is where the provider's identifier has to live.
    let registration = registration("channel-alpha", "acct-alpha");
    assert_eq!(registration.resource_id, "ret08u3rv24htgh289g");
    // The two identifiers are OPAQUE STRINGS OF SIMILAR SHAPE, so a transposition would be well-formed and
    // would silently stop the wrong channel -- or none. This asserts they reach the body in the fields the
    // reference names, with `id` holding the channel and `resourceId` holding the resource.
    let request = crate::google::request::calendar_channel_stop(
        &registration.channel_id,
        registration.resource_id(),
    )
    .unwrap_or_else(|error| panic!("a registered channel must be stoppable: {error}"));
    assert!(
        request.url().ends_with("/calendar/v3/channels/stop"),
        "the stop is addressed on the Calendar base, not a per-user path: {}",
        request.url()
    );
    let body = request.rendered_body();
    assert!(
        body.contains(r#""id":"channel-alpha""#),
        "the channel id must be the `id` field: {body}"
    );
    assert!(
        body.contains(r#""resourceId":"ret08u3rv24htgh289g""#),
        "the watched resource must be the `resourceId` field: {body}"
    );
    // And the pair is genuinely required: an empty resource id is refused rather than sent as a blank field,
    // which would name no resource and leave that channel notifying.
    assert!(crate::google::request::calendar_channel_stop("channel-alpha", "  ").is_err());
}

#[test]
fn a_registration_carries_the_channels_expiry_so_renewal_is_reachable_from_the_value_that_survives()
{
    // **The finding this field exists for.** `parse_channel_watch_response` read the response's `expiration`
    // into `ChannelWatchResponse::expires_at`, whose own doc says it can *"drive a renewal decision"* — and the
    // renewal decision `renewal_decision` takes that expiry as its **only** input. But the registration, the
    // value that survives from the `watch` to the teardown, had **no field for it**, so nothing that survived
    // held the input and the decision was reachable only from a test. This is `ADR-0107`'s dropped `resourceId`
    // one round later, from the same omission shape: a value read and then dropped.
    let response = must(
        parse_channel_watch_response(
            r#"{"kind":"api#channel","id":"01234567-89ab-cdef-0123456789ab","resourceId":"o3hgv1538sdjfh","resourceUri":"https://www.googleapis.com/calendar/v3/calendars/primary/events","expiration":1426325213000}"#,
        ),
        "the guide's own watch response must parse",
    );
    let registration = must(
        ChannelRegistration::from_watch_response(
            &response,
            must(
                crate::account::AccountReference::new("acct-alpha"),
                "a valid account reference",
            ),
            None,
        ),
        "a conforming response must register",
    );
    // The expiry the response reported is the expiry the registration holds — the same instant from the
    // provider, not a recomputation from the request (which could differ: the guide says the actual value is
    // "the more restrictive" of the request and Google's own limits).
    assert_eq!(registration.expires_at(), response.expires_at);
    // And the decision is reachable **from the registration alone**, at three instants straddling the
    // replacement lead (`CHANNEL_REPLACE_LEAD_SECONDS` = 86_400). Before the fix there was no way to ask a
    // stored channel this question at all.
    let expiry = instant(1_426_325_213);
    assert!(matches!(
        registration.renewal(instant(1_426_325_213 - 86_401)),
        ChannelRenewal::NotYet { .. }
    ));
    assert!(matches!(
        registration.renewal(instant(1_426_325_213 - 86_400)),
        ChannelRenewal::ReplaceSoon { .. }
    ));
    assert!(matches!(
        registration.renewal(expiry),
        ChannelRenewal::ReplaceNow { .. }
    ));
    // The registration's answer and the free function agree, so `renewal` is a bridge and not a second
    // opinion: two implementations of one decision would be the defect this repository records for every
    // duplicated rule.
    assert_eq!(
        registration.renewal(expiry),
        renewal_decision(expiry, expiry)
    );
}

#[test]
fn a_registration_surfaces_its_expiry_and_refuses_a_blank_resource_id_from_a_response() {
    // The expiry is a stored **fact** and not a secret, so it is surfaced and shown in a diagnostic.
    let built = registration("channel-alpha", "acct-alpha");
    assert_eq!(built.expires_at(), instant(1_426_325_213));
    assert!(
        format!("{built:?}").contains("expires_at"),
        "a diagnostic about a stale channel needs its expiry: {built:?}"
    );
    // And a response whose `resourceId` is **blank** is refused rather than registered: the parser reads a
    // string as-is, so usability is the constructor's question, and an empty second stop identifier would build
    // a registration that cannot end its own channel.
    let response = must(
        parse_channel_watch_response(r#"{"id":"c","resourceId":"   ","expiration":1426325213000}"#),
        "a blank resource id still parses; parsing checks presence and type, not usability",
    );
    assert_eq!(
        ChannelRegistration::from_watch_response(
            &response,
            must(
                crate::account::AccountReference::new("acct-alpha"),
                "a valid account reference",
            ),
            None,
        ),
        Err(ChannelWatchError::Missing {
            field: "resourceId"
        })
    );
}

/// Full header set for a delivery naming a channel, in a given state, with an optional token.
///
/// Dynamic values are leaked (test-only) so the header slice keeps its `&'static` element type rather than
/// parameterising the fixture helpers.
fn headers_for(
    channel_id: &str,
    token: Option<&str>,
    state: &str,
) -> Vec<(&'static str, &'static [u8])> {
    let id: &'static [u8] = Box::leak(channel_id.to_owned().into_boxed_str()).as_bytes();
    let state: &'static [u8] = Box::leak(state.to_owned().into_boxed_str()).as_bytes();
    let mut headers = vec![
        (CHANNEL_ID_HEADER, id),
        (RESOURCE_ID_HEADER, b"ret08u3rv24htgh289g".as_slice()),
        (
            RESOURCE_URI_HEADER,
            b"https://www.googleapis.com/calendar/v3/calendars/primary/events".as_slice(),
        ),
        (RESOURCE_STATE_HEADER, state),
        (MESSAGE_NUMBER_HEADER, b"10".as_slice()),
    ];
    if let Some(token) = token {
        headers.push((
            CHANNEL_TOKEN_HEADER,
            Box::leak(token.to_owned().into_boxed_str()).as_bytes(),
        ));
    }
    headers
}

/// Ingests a delivery for a channel, in a state, with optional token.
fn ingest(
    channel_id: &str,
    token: Option<&str>,
    state: &str,
    registrations: &[ChannelRegistration],
) -> ChannelIngest {
    let headers = headers_for(channel_id, token, state);
    ingest_channel_delivery(&delivery(&headers, &[]), registrations)
}

/// A registration with a token.
fn registration_with_token(channel_id: &str, account: &str, token: &str) -> ChannelRegistration {
    ChannelRegistration::new(
        channel_id.to_owned(),
        "o3hgv1538sdjfh".to_owned(),
        must(
            crate::account::AccountReference::new(account),
            "a valid account reference",
        ),
        Some(stored(token)),
        instant(1_426_325_213),
    )
}

#[test]
fn a_verified_change_on_a_registered_channel_is_the_one_outcome_that_syncs() {
    // The happy path, and the only variant that starts work: the delivery is well-formed, its channel is
    // registered, its token matches, and it reports a change.
    let registrations = [registration_with_token(
        "channel-alpha",
        "acct-alpha",
        "target=myApp-myChannelDest",
    )];
    let outcome = ingest(
        "channel-alpha",
        Some("target=myApp-myChannelDest"),
        "exists",
        &registrations,
    );
    assert!(outcome.is_accepted(), "a verified change is accepted");
    assert_eq!(
        outcome
            .account_to_sync()
            .map(crate::account::AccountReference::as_str),
        Some("acct-alpha"),
        "the sync must run under the account that registered the channel"
    );
    assert!(outcome.acknowledges());
}

#[test]
fn a_verified_sync_handshake_is_accepted_but_syncs_nothing() {
    // The `sync` message is accepted -- it is a well-formed, verified delivery -- yet it starts no work,
    // because the first message on a channel reports that notifications are starting, not a change (`ADR-0100`).
    // `is_accepted` and `account_to_sync` are therefore **different questions**: acceptance tells the sender to
    // keep the message; `account_to_sync` tells the caller whether to do work.
    let registrations = [registration("channel-alpha", "acct-alpha")];
    let outcome = ingest("channel-alpha", None, "sync", &registrations);
    assert_eq!(outcome, ChannelIngest::Handshake);
    assert!(outcome.is_accepted(), "a handshake is accepted");
    assert_eq!(
        outcome.account_to_sync(),
        None,
        "a handshake reports no change, so nothing syncs"
    );
    assert!(outcome.acknowledges(), "accepted, so not retried");
}

#[test]
fn the_seam_is_where_the_composition_was_found_and_it_rejects_a_failed_token() {
    // **The gap composing these three functions exposed.** Read, verify and route were each correct alone and
    // nothing joined them; joining them showed a caller could not get one answer meaning "the channel verified
    // and this delivery failed it". Here the channel routes, the token does not match, and the outcome is
    // `Rejected` rather than `Changed` -- the branch that did not exist before this function.
    let registrations = [registration_with_token(
        "channel-alpha",
        "acct-alpha",
        "target=myApp-myChannelDest",
    )];
    let outcome = ingest(
        "channel-alpha",
        Some("target=myApp-forged"),
        "exists",
        &registrations,
    );
    assert_eq!(
        outcome,
        ChannelIngest::Rejected(ChannelTokenCheck::Mismatch)
    );
    assert!(!outcome.is_accepted(), "a failed control is not accepted");
    assert_eq!(
        outcome.account_to_sync(),
        None,
        "a rejected delivery must not sync, even though its channel routes"
    );
    assert!(outcome.acknowledges(), "recorded and dropped, not retried");
}

#[test]
fn a_registered_channel_whose_delivery_carries_no_token_is_rejected_not_accepted() {
    // The other refusal: a token was registered and the delivery presented none. Distinct from a mismatch (no
    // value vs a wrong value) but the same operational reading -- refuse -- and the two are told apart by the
    // carried `ChannelTokenCheck`.
    let registrations = [registration_with_token(
        "channel-alpha",
        "acct-alpha",
        "target=myApp-myChannelDest",
    )];
    let outcome = ingest("channel-alpha", None, "exists", &registrations);
    assert_eq!(
        outcome,
        ChannelIngest::Rejected(ChannelTokenCheck::TokenRequired)
    );
    assert!(!outcome.is_accepted());
    assert_eq!(outcome.account_to_sync(), None);
}

#[test]
fn an_unregistered_or_ambiguous_channel_is_unroutable_and_carries_which() {
    // Routing runs **before** verification, because a token cannot be checked without the registration that
    // holds it. So a channel that is not registered is `Unroutable`, never `Rejected` -- reporting "rejected"
    // would imply a comparison happened when there was no value to compare against. And the two unroutable
    // cases are told apart by the carried route, because a collision is a defect here while a stray delivery is
    // not.
    let registrations = [registration("channel-alpha", "acct-alpha")];
    let unknown = ingest("channel-gamma", None, "exists", &registrations);
    assert_eq!(unknown, ChannelIngest::Unroutable(ChannelRoute::Unknown));
    assert!(!unknown.is_accepted());
    assert_eq!(unknown.account_to_sync(), None);
    // A collision: two registrations for the same channel id.
    let collided = [
        registration("channel-alpha", "acct-one"),
        registration("channel-alpha", "acct-two"),
    ];
    assert_eq!(
        ingest("channel-alpha", None, "exists", &collided),
        ChannelIngest::Unroutable(ChannelRoute::Ambiguous { accounts: 2 })
    );
}

#[test]
fn an_unreadable_delivery_is_distinct_from_an_unroutable_one() {
    // A delivery whose headers do not parse stops at the **first** step, so it is `Unreadable` -- carrying the
    // specific refusal -- not `Unroutable`, which would imply the channel was read and simply not found. The two
    // are different layers (the wire shape vs this connector's records) and a diagnostic must not conflate them.
    // A delivery with no channel id at all is the minimal unreadable shape.
    let headers: Vec<(&str, &[u8])> = headers_for("channel-alpha", None, "exists")
        .into_iter()
        .filter(|(name, _)| *name != CHANNEL_ID_HEADER)
        .collect();
    let outcome = ingest_channel_delivery(&delivery(&headers, &[]), &[]);
    assert_eq!(
        outcome,
        ChannelIngest::Unreadable(ChannelMessageError::Missing {
            header: CHANNEL_ID_HEADER
        })
    );
    assert!(!outcome.is_accepted());
    assert_eq!(outcome.account_to_sync(), None);
}

#[test]
fn an_untokened_channel_still_syncs_on_a_real_change() {
    // The guide makes the token optional, so a channel registered **without** one reports `Absent` -- which
    // **may be acted on** -- and a real change still syncs. This is the case that would be lost if `Absent` were
    // treated as a refusal: a correctly configured, un-tokened channel would never sync.
    let registrations = [registration("channel-alpha", "acct-alpha")];
    assert_eq!(registrations[0].token(), None, "registered without a token");
    let outcome = ingest("channel-alpha", None, "exists", &registrations);
    assert_eq!(
        outcome,
        ChannelIngest::Changed {
            account: must(
                crate::account::AccountReference::new("acct-alpha"),
                "a valid account reference"
            )
        }
    );
    assert!(outcome.account_to_sync().is_some());
    // The control: a **handshake** on the same un-tokened channel still does not sync, so `Absent` accepting a
    // change does not mean it accepts everything.
    assert_eq!(
        ingest("channel-alpha", None, "sync", &registrations),
        ChannelIngest::Handshake
    );
}

#[test]
fn the_account_acted_on_and_the_token_verified_come_from_one_registration() {
    // **The property `ChannelRoute::Exact` carrying the whole registration exists for.** Two registrations, each
    // with its **own** token. A delivery for channel B carrying B's token routes to B and verifies against
    // **B's** token. A mutant that fetched the token with a second scan -- or verified against A's -- would
    // reject this, which is exactly the "two lookups that merely happen to agree" hazard.
    let registrations = [
        registration_with_token("channel-alpha", "acct-alpha", "token-alpha"),
        registration_with_token("channel-beta", "acct-beta", "token-beta"),
    ];
    // B's delivery with B's token: accepted, and routes to B.
    let accepted = ingest("channel-beta", Some("token-beta"), "exists", &registrations);
    assert_eq!(
        accepted
            .account_to_sync()
            .map(crate::account::AccountReference::as_str),
        Some("acct-beta")
    );
    // B's delivery carrying **A's** token: the route still selects B (by id), and verification runs against
    // B's token, so A's token is a mismatch -- proving the token checked is the matched registration's.
    assert_eq!(
        ingest(
            "channel-beta",
            Some("token-alpha"),
            "exists",
            &registrations
        ),
        ChannelIngest::Rejected(ChannelTokenCheck::Mismatch),
        "the token checked must be the matched registration's, not another channel's"
    );
}

#[test]
fn every_ingest_outcome_acknowledges_because_none_is_repaired_by_retrying() {
    // The acknowledgement rule, over **every** variant: a malformed body, an unregistered channel, a failed
    // token and a handshake all fail or repeat identically, and a negative acknowledgement is subscription-
    // global (`ADR-0094`) -- so refusing would slow every other channel for a message that can never become
    // actionable. Asserted as a table so a future variant that *should* be retried has to say so explicitly.
    let registrations = [registration_with_token(
        "channel-alpha",
        "acct-alpha",
        "token-alpha",
    )];
    let all = [
        ingest(
            "channel-alpha",
            Some("token-alpha"),
            "exists",
            &registrations,
        ),
        ingest("channel-alpha", Some("token-alpha"), "sync", &registrations),
        ingest("channel-alpha", Some("wrong"), "exists", &registrations),
        ingest("channel-missing", None, "exists", &registrations),
    ];
    for outcome in &all {
        assert!(outcome.acknowledges(), "{outcome:?} must be acknowledged");
    }
    // And only `Changed` starts work, so acceptance is not a synonym for "act".
    let syncing = all
        .iter()
        .filter(|outcome| outcome.account_to_sync().is_some())
        .count();
    assert_eq!(syncing, 1, "exactly one of the four outcomes syncs");
}

/// A representable instant from whole seconds, for the renewal tests.
fn instant(seconds: i64) -> jarvis_core::UtcTimestamp {
    must(
        jarvis_core::UtcTimestamp::from_unix_nanos(i128::from(seconds) * 1_000_000_000),
        "a representable instant",
    )
}

#[test]
fn the_watch_responses_expiration_is_epoch_millis_and_the_conversion_is_asserted() {
    // The guide's own `watch` response example, verbatim. Its `expiration` is a **JSON number** of
    // **milliseconds** (1426325213000), the opposite of the human-readable header AND the opposite of the Gmail
    // lease's millisecond **string** — three encodings across two mechanisms. `UtcTimestamp` takes nanos, so
    // the value is scaled by one million; a wrong factor is not an error, it is an instant in the year 1970 or
    // 47,000, so the conversion is pinned against the guide's number rather than inspected.
    let body = r#"{
      "kind": "api#channel",
      "id": "01234567-89ab-cdef-0123456789ab",
      "resourceId": "o3hgv1538sdjfh",
      "resourceUri": "https://www.googleapis.com/calendar/v3/calendars/primary/events",
      "expiration": 1426325213000
    }"#;
    let response = must(
        parse_channel_watch_response(body),
        "the guide's own watch response must parse",
    );
    assert_eq!(response.channel_id, "01234567-89ab-cdef-0123456789ab");
    assert_eq!(response.resource_id, "o3hgv1538sdjfh");
    // 1426325213000 ms == 1426325213 s. If the parser used the number as seconds the instant would be
    // 1426325213000 s, and if it multiplied by 1000 instead of 1_000_000 it would be 1000x too early — so this
    // exact value is what distinguishes the three scalings.
    assert_eq!(
        response.expires_at.unix_nanos(),
        i128::from(1_426_325_213_i64) * 1_000_000_000,
        "the value is milliseconds and must be scaled by one million to nanos"
    );
}

#[test]
fn an_unreadable_watch_response_names_the_field_and_the_layer() {
    // Each refusal is a distinct variant pointing at a distinct defect — the body, the field, the type, or the
    // range — because "the response is unreadable" would leave a caller guessing which to look at.
    // Not JSON at all.
    assert!(matches!(
        parse_channel_watch_response("not json"),
        Err(ChannelWatchError::NotJson { .. })
    ));
    // A missing field, named.
    match parse_channel_watch_response(r#"{"id":"c","resourceId":"r"}"#) {
        Err(ChannelWatchError::Missing { field }) => assert_eq!(field, RESPONSE_EXPIRATION_FIELD),
        other => panic!("a missing expiration must be refused by name, got {other:?}"),
    }
    // A wrong type: the field is documented as a millisecond number, and a string is a different encoding (the
    // header's, or the Gmail lease's) that this field is not.
    match parse_channel_watch_response(
        r#"{"id":"c","resourceId":"r","expiration":"1426325213000"}"#,
    ) {
        Err(ChannelWatchError::WrongType { field, .. }) => {
            assert_eq!(field, RESPONSE_EXPIRATION_FIELD);
        }
        other => panic!("a string expiration must be refused as the wrong type, got {other:?}"),
    }
    // A value too large for an instant is refused rather than wrapped.
    assert_eq!(
        parse_channel_watch_response(
            r#"{"id":"c","resourceId":"r","expiration":9223372036854775807}"#
        ),
        Err(ChannelWatchError::OutOfRange)
    );
    // And a `null` is treated as absent rather than as a wrong type — the provider sent no value.
    assert!(matches!(
        parse_channel_watch_response(r#"{"id":"c","resourceId":"r","expiration":null}"#),
        Err(ChannelWatchError::Missing { .. })
    ));
}

#[test]
fn a_channel_lease_reports_direction_and_the_exact_boundary_is_lapsed() {
    // The boundary is decided in nanoseconds and the **exact expiry instant counts as lapsed**, the same rule
    // `watch_lapse` uses and for the same reason: treating it as alive keeps a dead channel one interval
    // longer, which is the silent failure this path removes.
    let expiry = instant(1_000_000 + 600);
    assert!(!channel_lease(expiry, instant(1_000_000)).is_lapsed());
    assert_eq!(
        channel_lease(expiry, instant(1_000_000)).seconds_from_edge(),
        600
    );
    assert_eq!(
        channel_lease(expiry, expiry),
        ChannelLease::Lapsed { for_seconds: 0 },
        "the expiry instant itself is lapsed, not alive"
    );
    assert_eq!(
        channel_lease(expiry, instant(1_000_000 + 900)),
        ChannelLease::Lapsed { for_seconds: 300 }
    );
}

#[test]
fn the_renewal_decision_replaces_a_lapsed_channel_a_nearly_lapsed_one_and_leaves_the_rest() {
    // Three states, because a lapsed channel and a nearly-lapsed one need the same action but mean different
    // things: one is already **losing** notifications, the other must be replaced **before** it does.
    let now = instant(1_000_000);
    // A channel well inside its lease: nothing to do.
    let comfortable = instant(1_000_000 + CHANNEL_REPLACE_LEAD_SECONDS + 3_600);
    assert_eq!(
        renewal_decision(comfortable, now),
        ChannelRenewal::NotYet {
            remaining_seconds: CHANNEL_REPLACE_LEAD_SECONDS + 3_600
        }
    );
    assert!(!renewal_decision(comfortable, now).should_replace());
    // Exactly at the lead time: replace now (the boundary is inclusive on the "soon" side).
    let at_lead = instant(1_000_000 + CHANNEL_REPLACE_LEAD_SECONDS);
    assert_eq!(
        renewal_decision(at_lead, now),
        ChannelRenewal::ReplaceSoon {
            remaining_seconds: CHANNEL_REPLACE_LEAD_SECONDS
        }
    );
    assert!(renewal_decision(at_lead, now).should_replace());
    // Just inside the lead time.
    assert!(matches!(
        renewal_decision(instant(1_000_000 + 60), now),
        ChannelRenewal::ReplaceSoon {
            remaining_seconds: 60
        }
    ));
    // Lapsed: already losing notifications, and the variant says how long ago it stopped.
    assert_eq!(
        renewal_decision(instant(1_000_000 - 120), now),
        ChannelRenewal::ReplaceNow {
            lapsed_for_seconds: 120
        }
    );
    // The two "replace" states are distinct, so a caller cannot mistake one for the other.
    assert_ne!(
        renewal_decision(at_lead, now),
        renewal_decision(instant(1_000_000 - 120), now)
    );
    assert_eq!(
        CHANNEL_REPLACE_LEAD_SECONDS, 86_400,
        "one day, a stated JARVIS figure"
    );
}
