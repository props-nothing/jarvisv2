//! Tests for routing a push delivery to a known account.
//!
//! The load-bearing ones are the **three ways routing fails**, because each has a different cause and only one
//! of them is fixed by waiting. And every failure must acknowledge rather than refuse, because a refusal is
//! charged to the whole subscription.
//!
//! Falsification record in `TODO.md`.

use super::*;
use crate::auth::AuthMethod;
use jarvis_core::UtcTimestamp;

fn must<T, E: std::fmt::Display>(result: Result<T, E>, what: &str) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("{what}: {error}"),
    }
}

fn reference(value: &str) -> AccountReference {
    must(AccountReference::new(value), "a valid account reference")
}

fn instant(seconds: i64) -> UtcTimestamp {
    must(
        UtcTimestamp::from_unix_nanos(i128::from(seconds) * 1_000_000_000),
        "a representable instant",
    )
}

fn account(reference_value: &str, address: &str) -> VerifiedAccount {
    must(
        VerifiedAccount::new(
            reference(reference_value),
            address,
            None,
            AuthMethod::OAuthPkce,
            vec!["https://www.googleapis.com/auth/gmail.readonly".to_owned()],
            instant(1_700_000_000),
        ),
        "a verified account",
    )
}

fn notification(address: &str) -> PubsubNotification {
    PubsubNotification {
        email_address: address.to_owned(),
        history_id: "1234567890".to_owned(),
    }
}

#[test]
fn an_exact_match_is_the_only_route_applied_without_a_person() {
    // The one route that needs no human decision, and `account()` returns it so the caller does not search
    // again — a second lookup would be a second place the comparison is decided.
    let accounts = [
        account("acct-1", "person@example.invalid"),
        account("acct-2", "other@example.invalid"),
    ];
    let route = route_delivery(&notification("other@example.invalid"), &accounts);
    assert_eq!(route.account(), Some(&reference("acct-2")));
    assert!(route.may_be_applied_automatically());
    assert_eq!(route.matched_accounts(), Some(1));
    // And a routable delivery has NO pre-emptive acknowledgement: an exact route means it *can* be processed,
    // not that it was, so answering here would claim a sync that has not happened.
    assert_eq!(route.unroutable_acknowledgement(), None);
}

#[test]
fn the_comparison_is_byte_exact_so_a_neighbouring_address_is_not_a_match() {
    // **The security-relevant property.** The address is untrusted input from an endpoint this connector has
    // **no verifier for** (Finding 1: neither Google mechanism is a body MAC; the contract can now *name* the
    // two header-token schemes, but nothing compares them), so a comparison that matched loosely would let one
    // delivery choose another mailbox's account. None of these is the stored address.
    let accounts = [account("acct-1", "person@example.invalid")];
    for near_miss in [
        // A different local part on the same domain.
        "other@example.invalid",
        // A different domain with the same local part.
        "person@example.invalid.evil",
        // A **prefix**, which a `starts_with` comparison would accept.
        "person@example.inval",
        // A **superstring**, which a `contains` comparison would accept.
        "x-person@example.invalid",
        // Leading or trailing whitespace, which a trimming comparison would accept.
        " person@example.invalid",
        "person@example.invalid ",
    ] {
        let route = route_delivery(&notification(near_miss), &accounts);
        assert_eq!(
            route,
            DeliveryRoute::Unknown,
            "`{near_miss}` is not the stored address and must not route to its account"
        );
    }
    // The control: the exact value DOES route, so the refusals above are about the comparison and not about
    // the lookup being broken.
    assert!(
        route_delivery(&notification("person@example.invalid"), &accounts)
            .may_be_applied_automatically()
    );
}

#[test]
fn a_case_differing_address_is_reported_rather_than_taken() {
    // **A near-match is not a match, and this is the decision rather than an oversight.** Addresses are
    // case-insensitive in practice, so `Person@example.invalid` is *probably* the same mailbox — but Google
    // publishes no canonicalisation statement for `emailAddress` in a push payload, and if the two spellings
    // were genuinely two accounts then applying the route would read the wrong mailbox. So it is reported.
    let accounts = [account("acct-1", "person@example.invalid")];
    let route = route_delivery(&notification("Person@Example.Invalid"), &accounts);
    assert_eq!(route, DeliveryRoute::CaseDiffers { accounts: 1 });
    assert!(
        !route.may_be_applied_automatically(),
        "a case-only near-match must not be applied without a person"
    );
    // `account()` is `None` rather than returning the near-match, so no accessor can hand out an arbitrarily
    // chosen account from this variant.
    assert_eq!(route.account(), None);
    assert_eq!(route.matched_accounts(), Some(1));
}

#[test]
fn several_accounts_with_one_address_are_ambiguous_and_no_account_is_chosen() {
    // Reachable when a reconnect mints a new reference without retiring the old row. Picking one — first,
    // oldest, most recently verified — would be an arbitrary choice that decides which mailbox is read, and a
    // wrong pick syncs one mailbox's changes under another account's identity. So only a count is reported.
    let accounts = [
        account("acct-1", "person@example.invalid"),
        account("acct-2", "person@example.invalid"),
        account("acct-3", "other@example.invalid"),
    ];
    let route = route_delivery(&notification("person@example.invalid"), &accounts);
    assert_eq!(route, DeliveryRoute::Ambiguous { accounts: 2 });
    assert_eq!(
        route.account(),
        None,
        "no account may be selected from an ambiguous route: {route:?}"
    );
    assert!(!route.may_be_applied_automatically());
    assert_eq!(route.matched_accounts(), Some(2));
}

#[test]
fn every_unroutable_delivery_is_acknowledged_rather_than_refused() {
    // **The conclusion `ADR-0094` forces.** None of the three unroutable cases is repaired by another attempt —
    // the account set is a local fact — so refusing would be a negative acknowledgement that never becomes a
    // positive one. And the push page says a negative acknowledgement triggers a **subscription-global**
    // backoff of up to 60 seconds, so that refusal would slow every other mailbox on the subscription.
    let accounts = [account("acct-1", "person@example.invalid")];
    let duplicate = [
        account("acct-1", "person@example.invalid"),
        account("acct-2", "person@example.invalid"),
    ];
    let unroutable = [
        route_delivery(&notification("nobody@example.invalid"), &accounts),
        route_delivery(&notification("person@example.invalid"), &duplicate),
        route_delivery(&notification("Person@example.invalid"), &accounts),
    ];
    for route in &unroutable {
        assert_eq!(
            route.unroutable_acknowledgement(),
            Some(DeliveryAck::AbandonAndAcknowledge),
            "{route:?} must be acknowledged and recorded, not refused"
        );
        assert!(!route.may_be_applied_automatically());
    }
    // And the acknowledgement refuses nothing, which is the property the whole arm exists for: `Retry` is the
    // only answer that costs the subscription, and no unroutable case produces it.
    for route in &unroutable {
        assert!(
            route
                .unroutable_acknowledgement()
                .is_some_and(DeliveryAck::acknowledges),
            "{route:?} must not refuse the delivery"
        );
    }
}

#[test]
fn no_connected_accounts_and_an_unknown_address_are_the_same_route() {
    // Both mean "nothing here to route to", and the caller's action is identical — acknowledge and record. A
    // separate variant for "not configured" would be a state the caller cannot act on differently, so they are
    // deliberately the same answer rather than two.
    assert_eq!(
        route_delivery(&notification("person@example.invalid"), &[]),
        DeliveryRoute::Unknown
    );
    assert_eq!(
        route_delivery(
            &notification("person@example.invalid"),
            &[account("acct-1", "other@example.invalid")]
        ),
        DeliveryRoute::Unknown
    );
    // `matched_accounts` is `None` rather than `Some(0)`: zero is the absence of a count, not a count of zero.
    assert_eq!(DeliveryRoute::Unknown.matched_accounts(), None);
}

#[test]
fn routing_names_an_account_to_read_and_never_a_credential() {
    // **The property that bounds what a forged delivery can do.** What this module returns is an
    // `AccountReference` — JARVIS's own local identifier — and nothing here holds or returns a token. A forged
    // delivery can therefore only make the connector *read* a mailbox it was already authorised to read, and
    // authorization was settled when the account was connected. Asserted as a type-level fact through the
    // accessor's return type, and as the reason the module has no field that could hold a credential.
    let accounts = [account("acct-1", "person@example.invalid")];
    let route = route_delivery(&notification("person@example.invalid"), &accounts);
    let named: &AccountReference = route
        .account()
        .unwrap_or_else(|| panic!("an exact route names an account"));
    assert_eq!(named.as_str(), "acct-1");
    // And the account's own address is the untrusted value being MATCHED, never produced as an output — the
    // reference is what travels, which is why the reference is opaque and case-constrained.
    assert!(!named.as_str().contains('@'));
}

/// Builds a Pub/Sub push body whose `message.data` decodes to a Gmail notification.
///
/// Uses the crate's own base64url encoder rather than a literal, so the fixture is the documented shape without
/// a hand-encoded blob that could drift from it.
fn delivery_body(address: &str, history_id: &str, message_id: Option<&str>) -> String {
    let payload = format!(r#"{{"emailAddress":"{address}","historyId":"{history_id}"}}"#);
    let data = crate::base64::url_safe_no_pad(payload.as_bytes());
    match message_id {
        Some(id) => format!(
            r#"{{"message":{{"data":"{data}","messageId":"{id}"}},"subscription":"projects/p/subscriptions/s"}}"#
        ),
        None => format!(
            r#"{{"message":{{"data":"{data}"}},"subscription":"projects/p/subscriptions/s"}}"#
        ),
    }
}

#[test]
fn a_routable_delivery_is_the_one_gmail_outcome_that_syncs() {
    // The happy path: the body parses, the address matches exactly, and the outcome carries the account, the
    // stated position, and the dedupe key.
    let accounts = [account("acct-1", "person@example.invalid")];
    let body = delivery_body(
        "person@example.invalid",
        "9876543210",
        Some("2070443601311540"),
    );
    let outcome = ingest_gmail_delivery(&body, &accounts);
    assert_eq!(
        outcome,
        GmailIngest::Changed {
            account: reference("acct-1"),
            history_id: "9876543210".to_owned(),
            message_id: Some("2070443601311540".to_owned()),
        }
    );
    assert_eq!(
        outcome.account_to_sync().map(AccountReference::as_str),
        Some("acct-1")
    );
    // A routed delivery is **accepted**, so the subscription stops redelivering it.
    assert_eq!(outcome.acknowledgement(), DeliveryAck::Accept);
}

#[test]
fn an_unroutable_delivery_carries_its_reason_and_is_acknowledged() {
    // The three unroutable routes. All acknowledge because the account set is a **local** fact — another attempt
    // changes nothing — and a negative ack is subscription-global (`ADR-0094`). But the route is carried, because
    // `CaseDiffers` and `Ambiguous` are actionable signals that must not be silently dropped.
    let accounts = [account("acct-1", "person@example.invalid")];
    // No account holds this address at all.
    let unknown = ingest_gmail_delivery(
        &delivery_body("other@example.invalid", "1", None),
        &accounts,
    );
    assert_eq!(unknown, GmailIngest::Unroutable(DeliveryRoute::Unknown));
    assert_eq!(
        unknown.acknowledgement(),
        DeliveryAck::AbandonAndAcknowledge
    );
    assert_eq!(unknown.account_to_sync(), None);
    // A case-only near-match: reported, never applied automatically.
    let case_differs = ingest_gmail_delivery(
        &delivery_body("Person@Example.Invalid", "1", None),
        &accounts,
    );
    assert_eq!(
        case_differs,
        GmailIngest::Unroutable(DeliveryRoute::CaseDiffers { accounts: 1 })
    );
    assert_eq!(
        case_differs.acknowledgement(),
        DeliveryAck::AbandonAndAcknowledge
    );
    // Two accounts for one address: a registration defect, undecidable without a person.
    let ambiguous = ingest_gmail_delivery(
        &delivery_body("person@example.invalid", "1", None),
        &[
            account("acct-1", "person@example.invalid"),
            account("acct-2", "person@example.invalid"),
        ],
    );
    assert_eq!(
        ambiguous,
        GmailIngest::Unroutable(DeliveryRoute::Ambiguous { accounts: 2 })
    );
    assert_eq!(
        ambiguous.acknowledgement(),
        DeliveryAck::AbandonAndAcknowledge
    );
}

#[test]
fn an_unreadable_body_is_distinct_by_layer_and_acknowledged() {
    // A body that cannot be read stops at the first step, and the **layer** that failed is preserved: a broken
    // Pub/Sub envelope points at the wire, while a broken Gmail payload inside a good envelope points at the
    // payload. A single "bad body" would send a caller to the wrong one.
    let accounts = [account("acct-1", "person@example.invalid")];
    // Not JSON at all — the envelope.
    let not_json = ingest_gmail_delivery("not json", &accounts);
    assert!(matches!(
        not_json,
        GmailIngest::Unreadable(GmailBodyError::Envelope(
            PubsubDeliveryError::NotJson { .. }
        ))
    ));
    // JSON, but not the wrapped shape — the envelope.
    let no_payload = ingest_gmail_delivery("{}", &accounts);
    assert_eq!(
        no_payload,
        GmailIngest::Unreadable(GmailBodyError::Envelope(
            PubsubDeliveryError::NoWrappedPayload
        ))
    );
    // A wrapped body whose `data` decodes to something that is not the Gmail payload — the payload layer.
    let bad_payload = format!(
        r#"{{"message":{{"data":"{}"}}}}"#,
        crate::base64::url_safe_no_pad(b"not the documented shape")
    );
    let payload = ingest_gmail_delivery(&bad_payload, &accounts);
    assert_eq!(
        payload,
        GmailIngest::Unreadable(GmailBodyError::Payload(
            PubsubNotificationError::NotTheDocumentedShape
        )),
        "a good envelope with a bad payload must report the payload layer, not the envelope"
    );
    // Both acknowledge, so a body this connector cannot read is not redelivered forever.
    for outcome in [not_json, no_payload, payload] {
        assert_eq!(
            outcome.acknowledgement(),
            DeliveryAck::AbandonAndAcknowledge
        );
    }
}

#[test]
fn the_message_id_is_carried_because_pubsub_delivers_at_least_once() {
    // `messageId` is the only field that tells a redelivery from a new change, so it is carried for
    // deduplication. A delivery that omits it is `None` rather than a fabricated id — and `None` means "this may
    // be a repeat I cannot detect", which a caller must handle differently from a real id.
    let accounts = [account("acct-1", "person@example.invalid")];
    let with_id = ingest_gmail_delivery(
        &delivery_body("person@example.invalid", "5", Some("2070443601311540")),
        &accounts,
    );
    let without_id = ingest_gmail_delivery(
        &delivery_body("person@example.invalid", "5", None),
        &accounts,
    );
    let id_of = |outcome: &GmailIngest| match outcome {
        GmailIngest::Changed { message_id, .. } => message_id.clone(),
        other => panic!("expected a routed change, got {other:?}"),
    };
    assert_eq!(
        id_of(&with_id),
        Some("2070443601311540".to_owned()),
        "the provider's message id is carried verbatim"
    );
    assert_eq!(
        id_of(&without_id),
        None,
        "an absent id is absent, not invented"
    );
}

#[test]
fn gmail_has_no_handshake_outcome_because_its_opening_notification_is_unmarked() {
    // **The divergence from `ChannelIngest`, asserted rather than assumed.** The Calendar ingest has a
    // `Handshake` outcome because `X-Goog-Resource-State: sync` marks its opening message (`ADR-0100`). Gmail's
    // guide says a successful `watch` *"also immediately sends a notification"* — but that notification is an
    // **ordinary** one, the same `{emailAddress, historyId}` payload a real change produces, with no marker. So
    // Gmail's start-of-notifications message is **indistinguishable from a change**: it ingests as `Changed`, and
    // a `Handshake` variant would claim a distinction the wire does not carry.
    let accounts = [account("acct-1", "person@example.invalid")];
    let opening = ingest_gmail_delivery(
        &delivery_body("person@example.invalid", "1234567890", Some("1")),
        &accounts,
    );
    assert!(
        matches!(opening, GmailIngest::Changed { .. }),
        "with no marker on the wire, the opening notification IS a change to the connector"
    );
    // The outcome space is exactly the three that exist — read failure, routing failure, routed change — so a
    // reader cannot find a handshake state here to branch on.
    let outcomes = [
        GmailIngest::Unreadable(GmailBodyError::Envelope(
            PubsubDeliveryError::NoWrappedPayload,
        )),
        GmailIngest::Unroutable(DeliveryRoute::Unknown),
        GmailIngest::Changed {
            account: reference("acct-1"),
            history_id: "1".to_owned(),
            message_id: None,
        },
    ];
    let syncing = outcomes
        .iter()
        .filter(|outcome| outcome.account_to_sync().is_some())
        .count();
    assert_eq!(syncing, 1, "exactly one of the three outcomes syncs");
    // And none is a `Retry`: this function decides what the delivery IS, not whether acting on it succeeded, so
    // the transient-failure answer belongs to the caller that acts.
    for outcome in &outcomes {
        assert_ne!(outcome.acknowledgement(), DeliveryAck::Retry);
    }
}

#[test]
fn the_two_ingest_paths_agree_that_a_routable_delivery_is_accept() {
    // **The asymmetry this slice fixes, asserted as a property rather than left to a comment.** The Gmail ingest
    // returned a three-state `DeliveryAck` while the Calendar ingest returned a `bool` that was always `true`,
    // so the two siblings answered one question with two vocabularies — and a caller of the `bool` could not
    // tell a delivery it **acted on** from one it **deliberately dropped**. This drives the accepted outcome of
    // **both** mechanisms and asserts they classify identically, so a future change that made one path say
    // `Accept` where the other says `AbandonAndAcknowledge` fails here rather than silently.
    use crate::google::channel::ingest_channel_delivery;

    let registration = registration_for("channel-alpha", None);
    let accepted = ingest_channel_delivery(
        &delivery(&calendar_headers("channel-alpha", None, "exists", "10")),
        std::slice::from_ref(&registration),
    );
    assert_eq!(accepted.acknowledgement(), DeliveryAck::Accept);
    let gmail_change = ingest_gmail_delivery(
        &delivery_body("person@example.invalid", "1", None),
        &[account("acct-1", "person@example.invalid")],
    );
    assert_eq!(gmail_change.acknowledgement(), DeliveryAck::Accept);
    assert_eq!(
        accepted.acknowledgement(),
        gmail_change.acknowledgement(),
        "both mechanisms must agree that an accepted delivery is `Accept`"
    );
    // Neither ever answers `Retry` from an ingest decision: the transient-failure answer belongs to the caller
    // that acted, and ingest decides what the delivery IS.
    assert_ne!(accepted.acknowledgement(), DeliveryAck::Retry);
}

#[test]
fn the_two_ingest_paths_agree_that_a_dropped_delivery_is_abandoned() {
    // The other half of the parity property: every outcome **neither** mechanism acted on is
    // `AbandonAndAcknowledge`, and the two agree case for case — a `bool` returned `true` for these too, which
    // is why it could not distinguish a success from a deliberate drop.
    use crate::google::channel::{ChannelIngest, ingest_channel_delivery};

    // A Calendar delivery for a channel nothing registered — the counterpart of Gmail's unknown address.
    let registered = registration_for("channel-alpha", None);
    let unroutable = ingest_channel_delivery(
        &delivery(&calendar_headers("channel-unknown", None, "exists", "11")),
        std::slice::from_ref(&registered),
    );
    let gmail_missing = ingest_gmail_delivery(
        &delivery_body("stranger@example.invalid", "1", None),
        &[account("acct-1", "person@example.invalid")],
    );
    assert_eq!(
        unroutable.acknowledgement(),
        DeliveryAck::AbandonAndAcknowledge
    );
    assert_eq!(
        gmail_missing.acknowledgement(),
        DeliveryAck::AbandonAndAcknowledge
    );
    assert_eq!(
        unroutable.acknowledgement(),
        gmail_missing.acknowledgement(),
        "both mechanisms must agree that an unroutable delivery is `AbandonAndAcknowledge`"
    );

    // A Calendar delivery that fails its token control — the counterpart of Gmail's unreadable body.
    let tokened = registration_for("channel-beta", Some("registered-token"));
    let rejected = ingest_channel_delivery(
        &delivery(&calendar_headers(
            "channel-beta",
            Some("forged"),
            "exists",
            "12",
        )),
        std::slice::from_ref(&tokened),
    );
    let gmail_unreadable = ingest_gmail_delivery("not json", &[]);
    assert_eq!(
        rejected.acknowledgement(),
        DeliveryAck::AbandonAndAcknowledge
    );
    assert_eq!(
        gmail_unreadable.acknowledgement(),
        DeliveryAck::AbandonAndAcknowledge
    );
    assert_eq!(
        rejected.acknowledgement(),
        gmail_unreadable.acknowledgement(),
        "both mechanisms must agree that a refused delivery is `AbandonAndAcknowledge`"
    );

    // The two types expose the **same** vocabulary, which is the finding: a caller that handles one handles the
    // other, and neither collapses "done" and "dropped" into one value.
    let _: DeliveryAck = ChannelIngest::Handshake.acknowledgement();
    let _: DeliveryAck = gmail_missing.acknowledgement();
}

/// A registration for a channel, with an optional token. Fixture helper for the two parity tests.
fn registration_for(
    channel_id: &str,
    token: Option<&str>,
) -> crate::google::channel::ChannelRegistration {
    crate::google::channel::ChannelRegistration::new(
        channel_id.to_owned(),
        "ret08u3rv24htgh289g".to_owned(),
        reference("acct-1"),
        token.map(|value| must(crate::auth::SecretValue::new(value), "a token")),
        instant(1_426_325_213),
    )
}

/// The full required Calendar header set for a delivery naming a channel. Fixture helper.
///
/// Dynamic values are leaked (test-only) so the header slice keeps its `&'static` element type, the pattern the
/// channel tests use.
fn calendar_headers(
    channel_id: &str,
    token: Option<&str>,
    state: &str,
    number: &str,
) -> Vec<(&'static str, &'static [u8])> {
    use crate::google::channel::{
        CHANNEL_ID_HEADER, CHANNEL_TOKEN_HEADER, MESSAGE_NUMBER_HEADER, RESOURCE_ID_HEADER,
        RESOURCE_STATE_HEADER, RESOURCE_URI_HEADER,
    };
    let leak =
        |value: &str| -> &'static [u8] { Box::leak(value.to_owned().into_boxed_str()).as_bytes() };
    let mut headers = vec![
        (CHANNEL_ID_HEADER, leak(channel_id)),
        (RESOURCE_ID_HEADER, b"ret08u3rv24htgh289g".as_slice()),
        (
            RESOURCE_URI_HEADER,
            b"https://www.googleapis.com/calendar/v3/calendars/primary/events".as_slice(),
        ),
        (RESOURCE_STATE_HEADER, leak(state)),
        (MESSAGE_NUMBER_HEADER, leak(number)),
    ];
    if let Some(token) = token {
        headers.push((CHANNEL_TOKEN_HEADER, leak(token)));
    }
    headers
}

/// Wraps a header slice in a `WebhookDelivery` with a zero-length body. Fixture helper.
fn delivery<'a>(
    headers: &'a [(&'static str, &'static [u8])],
) -> crate::webhook::WebhookDelivery<'a> {
    crate::webhook::WebhookDelivery {
        path: "/webhooks/google/calendar",
        headers,
        body: b"",
    }
}
