//! Tests for the push notification envelope.
//!
//! The load-bearing ones are about the **alphabet and the padding**, because the two official pages disagree
//! about the first, the push page's own example contradicts the second, and a convenient payload cannot
//! distinguish either.
//!
//! Falsification record in `TODO.md`.

use super::*;
use crate::base64::{Alphabet, Padding, decode_with, url_safe_no_pad};

/// The Gmail push guide's own example value, verbatim.
///
/// Captured from the guide's envelope sample. It decodes to
/// `{"emailAddress": "user@example.com", "historyId": "1234567890"}` — asserted below, because a fixture whose
/// expected value is computed rather than stated would prove nothing about the encoding.
const GUIDE_EXAMPLE: &str =
    "eyJlbWFpbEFkZHJlc3MiOiAidXNlckBleGFtcGxlLmNvbSIsICJoaXN0b3J5SWQiOiAiMTIzNDU2Nzg5MCJ9";

/// Cloud Pub/Sub's **own** push-page example of `message.data`, verbatim.
///
/// The push page shows this as the minimum-value example and says the field is "base64-encoded". It decodes to
/// `Hello Cloud Pub/Sub! Here is my message!` and — the point of keeping it — it **ends in `=`**, i.e. it is
/// **padded standard** base64. The Gmail guide describes the same field as `Base64URL`, so this one value is
/// enough to show that padding is not an OAuth rule and that the two pages' descriptions are both partial
/// (`ADR-0089`).
const PUBSUB_PAGE_EXAMPLE: &str = "SGVsbG8gQ2xvdWQgUHViL1N1YiEgSGVyZSBpcyBteSBtZXNzYWdlIQ==";

/// A payload spelling that **forces** the two alphabets to differ, in both spellings.
///
/// The JSON is `{"emailAddress":"a@example.invalid","historyId":"0>"}`. A sweep of all 95 printable ASCII
/// characters at all four base64 alignments found only three — `>`, `?`, `~`, each after a one-character offset
/// — whose standard encoding contains `+` or `/`, so this is one of the few Gmail-shaped payloads that can
/// distinguish RFC 4648 §4 from §5. Both spellings are literals rather than computed, because a computed
/// fixture would test the encoder that produced it.
const FORCING_URL_SAFE: &str =
    "eyJlbWFpbEFkZHJlc3MiOiJhQGV4YW1wbGUuaW52YWxpZCIsImhpc3RvcnlJZCI6IjA-In0";

/// The **standard**-alphabet, padded spelling of [`FORCING_URL_SAFE`]'s bytes.
const FORCING_STANDARD: &str =
    "eyJlbWFpbEFkZHJlc3MiOiJhQGV4YW1wbGUuaW52YWxpZCIsImhpc3RvcnlJZCI6IjA+In0=";

fn must<T, E: std::fmt::Display>(result: Result<T, E>, what: &str) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("{what}: {error}"),
    }
}

fn url_safe_absent() -> PubsubData {
    PubsubData {
        alphabet: Alphabet::UrlSafe,
        padding: Padding::Absent,
    }
}

#[test]
fn the_guides_own_example_decodes_to_the_two_documented_fields() {
    // The documented end-to-end: the guide says this value "decodes to a JSON object containing the email
    // address and the new mailbox history ID", and both fields are asserted **literally** so a change in the
    // decoding would be visible here rather than showing up as a plausible-looking mailbox.
    let (notification, encoding) = must(
        decode_notification(GUIDE_EXAMPLE),
        "the guide's own example must decode",
    );
    assert_eq!(notification.email_address, "user@example.com");
    assert_eq!(notification.history_id, "1234567890");
    // The example contains **no** `-`, `_`, `+` or `/` and carries **no** padding, so it decodes identically
    // under three of the four forms. The reported form is therefore the first in `DECODE_ORDER` — asserted so
    // the ordering is pinned rather than incidental.
    assert_eq!(
        encoding,
        url_safe_absent(),
        "the URL-safe unpadded form is tried first, so a value that suits several reports it"
    );
}

#[test]
fn the_providers_own_pubsub_example_decodes_even_though_it_is_padded() {
    // **The regression test for the bug `ADR-0088` introduced.** That ADR refused `=` padding outright, on the
    // strength of RFC 7636 — an **OAuth** rule about the PKCE verifier. But `message.data` is a **Pub/Sub** field,
    // and the Pub/Sub push page's own example of it (`PUBSUB_PAGE_EXAMPLE`) ends in `==`. So the connector would
    // have refused the provider's own published example: a delivery the connector does not understand, which for
    // a notification path means a **missed change**.
    //
    // A rule was applied from a context that does not govern the value it was applied to — the same conflation
    // this crate keeps finding, one level up from `ADR-0088`'s own finding.
    assert!(
        PUBSUB_PAGE_EXAMPLE.ends_with("=="),
        "the fixture must be padded, or it cannot catch the bug: {PUBSUB_PAGE_EXAMPLE}"
    );
    let decoded = must(
        decode_with(PUBSUB_PAGE_EXAMPLE, Alphabet::Standard),
        "the provider's own example must decode as standard base64",
    );
    assert_eq!(
        String::from_utf8(decoded).unwrap_or_default(),
        "Hello Cloud Pub/Sub! Here is my message!",
        "the example is the push page's, so its plaintext is known"
    );
    // Through the notification decoder, which is where the refusal was: a value that is not JSON of the Gmail
    // shape still gets *past* the base64 layer, so the failure it reports is about the payload and not the
    // padding. That is the assertion distinguishing "the padding was accepted" from "the padding was refused".
    assert_eq!(
        decode_notification(PUBSUB_PAGE_EXAMPLE),
        Err(PubsubNotificationError::NotTheDocumentedShape),
        "the padding must be accepted so the fault reported is the payload's, not the encoding's"
    );
    // **But decoding it is not enough, because this example cannot distinguish the alphabets** — it uses only
    // `A-Za-z0-9`, so the URL-safe attempt succeeds on it too. A mutation that dropped the standard alphabet
    // entirely therefore still passed the assertions above, which is what these two do not: the padding is
    // reported as present, and a value that *does* force the alphabet resolves to Standard.
    let (_, form) = must(
        decode_notification(&format!(
            "{}{}",
            url_safe_no_pad(br#"{"emailAddress":"a@example.invalid","historyId":"0>"}"#),
            "="
        )),
        "a URL-safe payload must decode too",
    );
    assert_eq!(form.padding, Padding::Present, "`=` at the end is observed");
    let (_, form) = must(
        decode_notification(FORCING_STANDARD),
        "the padded standard fixture must decode",
    );
    assert_eq!(
        form.alphabet,
        Alphabet::Standard,
        "the standard alphabet must be attempted, or a `+`-bearing payload is refused"
    );
    assert_eq!(form, PubsubData::PUBSUB_FIELD_TYPE);
}

#[test]
fn a_payload_in_any_combination_of_the_two_axes_decodes_and_reports_which() {
    // **Both axes, every combination.** They are independent: the alphabet needs a parameter because it leaves
    // no marker, and the padding is read from the value because it *is* the marker. A decoder that conflated
    // them would accept one combination and refuse another while looking correct wherever both agree — and the
    // forcing fixture is used because it is the only kind that discriminates the alphabets.
    assert!(FORCING_STANDARD.contains('+'), "{FORCING_STANDARD}");
    assert!(FORCING_URL_SAFE.contains('-'), "{FORCING_URL_SAFE}");
    let forcing_unpadded_standard = FORCING_STANDARD.trim_end_matches('=');

    let cases = [
        (FORCING_URL_SAFE, Alphabet::UrlSafe, Padding::Absent),
        (FORCING_STANDARD, Alphabet::Standard, Padding::Present),
        (
            forcing_unpadded_standard,
            Alphabet::Standard,
            Padding::Absent,
        ),
    ];
    for (value, alphabet, padding) in cases {
        let (notification, reported) =
            must(decode_notification(value), "every combination must decode");
        assert_eq!(notification.email_address, "a@example.invalid");
        assert_eq!(notification.history_id, "0>");
        assert_eq!(
            reported,
            PubsubData { alphabet, padding },
            "`{value}` must be reported as the combination it is"
        );
    }

    // The fourth combination — **URL-safe with padding** — is documented nowhere, so it is built here from the
    // encoder plus padding rather than taken from a fixture. It exists because the two axes are independent, and
    // this is the assertion that proves they are rather than four spellings of one choice.
    let padded_url_safe = format!("{FORCING_URL_SAFE}=");
    assert!(
        padded_url_safe.len().is_multiple_of(4),
        "a padded value's length is a multiple of four"
    );
    assert_eq!(
        decode_notification(&padded_url_safe)
            .ok()
            .map(|(_, form)| form),
        Some(PubsubData {
            alphabet: Alphabet::UrlSafe,
            padding: Padding::Present
        }),
        "a padded URL-safe value must decode and report both axes"
    );
}

#[test]
fn the_two_documented_forms_are_predicates_rather_than_variants() {
    // The Gmail guide's form and the Pub/Sub field type are **two points in one space**, not two cases. Asserted
    // directly so a future reader can see why `PubsubData` is a pair of enums rather than a four-variant enum:
    // the documents describe combinations, and a document changing one axis should not need a new variant.
    assert_eq!(
        PubsubData::GMAIL_GUIDE,
        PubsubData {
            alphabet: Alphabet::UrlSafe,
            padding: Padding::Absent
        }
    );
    assert_eq!(
        PubsubData::PUBSUB_FIELD_TYPE,
        PubsubData {
            alphabet: Alphabet::Standard,
            padding: Padding::Present
        }
    );
    assert_ne!(PubsubData::GMAIL_GUIDE, PubsubData::PUBSUB_FIELD_TYPE);
    assert_eq!(PubsubData::GMAIL_GUIDE.as_str(), "base64url/unpadded");
    assert_eq!(PubsubData::PUBSUB_FIELD_TYPE.as_str(), "base64/padded");
}

#[test]
fn each_unreadable_payload_has_its_own_error() {
    // Four distinct remedies: a non-base64 body is a delivery this connector misunderstands, a base64 body that
    // decodes to non-UTF-8 bytes is a payload that is not JSON at all, and JSON of the wrong shape is a payload
    // contract that changed. A single "malformed" variant would send a reader to the wrong layer.
    assert!(matches!(
        decode_notification("not base64!!"),
        Err(PubsubNotificationError::Base64 { .. })
    ));
    // `"____"` is valid **URL-safe** base64 (four `_` are four 63s) and decodes to three bytes that are not
    // UTF-8, which is the case distinguishing `NotUtf8` from `NotTheDocumentedShape`.
    assert_eq!(
        decode_notification("____"),
        Err(PubsubNotificationError::NotUtf8),
        "base64 that decodes to non-UTF-8 bytes is not a JSON payload"
    );
    // Valid UTF-8 that is not JSON, and JSON that is not the documented object.
    assert_eq!(
        decode_notification(&url_safe_no_pad(b"not json")),
        Err(PubsubNotificationError::NotTheDocumentedShape)
    );
    assert_eq!(
        decode_notification(&url_safe_no_pad(br#"{"emailAddress":"a@example.invalid"}"#)),
        Err(PubsubNotificationError::NotTheDocumentedShape),
        "a payload missing `historyId` is not the documented shape, because the position is what it is for"
    );
    assert_eq!(
        decode_notification(&url_safe_no_pad(br#"{"historyId":"42"}"#)),
        Err(PubsubNotificationError::NotTheDocumentedShape)
    );
    assert_eq!(
        decode_notification(&url_safe_no_pad(br#"["a","b"]"#)),
        Err(PubsubNotificationError::NotTheDocumentedShape),
        "an array is not the documented object"
    );
}

#[test]
fn an_empty_payload_is_refused_as_a_shape_and_not_read_as_no_change() {
    // An empty `data` is legal for a `PubsubMessage` — the reference says "If this field is empty, the message
    // must contain at least one attribute" — but a *Gmail* notification with no field is not an empty change
    // set, it is a delivery this connector does not understand. Reading it as "nothing changed" would advance
    // nothing and say nothing, which is the silent direction.
    assert_eq!(
        decode_notification(""),
        Err(PubsubNotificationError::NotTheDocumentedShape),
        "empty base64 decodes to no bytes, which is not JSON"
    );
}

#[test]
fn an_encoded_value_and_its_decode_are_inverses_in_every_form() {
    // A round-trip pins the decoder against the encoder, which is otherwise an independent implementation. Run
    // over lengths that exercise every remainder, because the trailing partial group is where an off-by-one
    // lives: a 1-byte, 2-byte and 3-byte tail each flush differently.
    let payload = br#"{"emailAddress":"user@example.com","historyId":"9876543210"}"# as &[u8];
    for length in 1..=payload.len() {
        let slice = &payload[..length];
        let url_safe = url_safe_no_pad(slice);
        assert_eq!(
            decode_with(&url_safe, Alphabet::UrlSafe),
            Ok(slice.to_vec()),
            "url-safe round trip failed at {length} bytes"
        );
        // The same bytes with the padding the encoder omits, so the padded path is exercised over every
        // remainder too — the two axes are independent, and this is the form that proves the padding is read
        // rather than assumed.
        let padded_url_safe = format!("{url_safe}{}", "=".repeat((4 - url_safe.len() % 4) % 4));
        assert_eq!(
            decode_with(&padded_url_safe, Alphabet::UrlSafe),
            Ok(slice.to_vec()),
            "padded round trip failed at {length} bytes"
        );
    }

    // **And one round trip over a value the two alphabets actually spell differently.** The sweep above may
    // substitute nothing at all — almost no Gmail-shaped payload produces the difference — so this asserts the
    // forcing fixture's two spellings decode to the same bytes, and that the second is what the RFC 4648 §5
    // substitution produces.
    assert_eq!(
        FORCING_URL_SAFE.replace('-', "+").replace('_', "/") + "=",
        FORCING_STANDARD,
        "the substitution must produce the standard padded spelling, or the alphabets never differ in this test"
    );
    let bytes = must(
        decode_with(FORCING_URL_SAFE, Alphabet::UrlSafe),
        "the forcing fixture must decode as URL-safe",
    );
    assert_eq!(url_safe_no_pad(&bytes), FORCING_URL_SAFE);
    assert_eq!(
        decode_with(FORCING_STANDARD, Alphabet::Standard),
        Ok(bytes),
        "the standard padded spelling must decode to the same bytes"
    );
}

#[test]
fn a_delivery_body_parses_with_either_spelling_of_its_metadata_fields() {
    // **The push page's own examples show both spellings.** It gives `messageId` and `message_id`, and
    // `publishTime` and `publish_time`, without explaining the choice. A parser that read only one would find
    // `None` for the other and report **no error** — so the deduplication key would silently be absent exactly
    // when it is needed, which is on a redelivery.
    let camel = format!(
        r#"{{"message":{{"data":"{GUIDE_EXAMPLE}","messageId":"2070443601311540","publishTime":"2021-02-26T19:13:55.749Z"}},"subscription":"projects/p/subscriptions/s"}}"#
    );
    let snake = format!(
        r#"{{"message":{{"data":"{GUIDE_EXAMPLE}","message_id":"2070443601311540","publish_time":"2021-02-26T19:13:55.749Z"}},"subscription":"projects/p/subscriptions/s"}}"#
    );
    for (label, body) in [("camelCase", camel), ("snake_case", snake)] {
        let (delivery, notification, encoding) =
            must(parse_delivery(&body), "a wrapped delivery must parse");
        assert_eq!(
            delivery.message_id(),
            Some("2070443601311540"),
            "the {label} spelling must populate the deduplication key, which is inside `message`"
        );
        assert_eq!(
            delivery
                .message
                .as_ref()
                .and_then(|message| message.publish_time.as_deref()),
            Some("2021-02-26T19:13:55.749Z"),
            "the {label} spelling must populate the publish time, which is also inside `message`"
        );
        assert_eq!(
            delivery.subscription.as_deref(),
            Some("projects/p/subscriptions/s"),
            "`subscription` is beside `message`, not inside it"
        );
        assert_eq!(notification.email_address, "user@example.com");
        assert_eq!(notification.history_id, "1234567890");
        assert_eq!(encoding, url_safe_absent());
    }

    // The maximum-value example from the same page adds `deliveryAttempt` at the **top level**, not inside
    // `message`, and an `attributes` map. Asserted because a parser that looked in the wrong place would report
    // no attempt number and no error.
    let with_attempt = format!(
        r#"{{"deliveryAttempt":5,"message":{{"attributes":{{"key":"value"}},"data":"{GUIDE_EXAMPLE}"}},"subscription":"projects/p/subscriptions/s"}}"#
    );
    let (delivery, _, _) = must(
        parse_delivery(&with_attempt),
        "a delivery with an attempt count must parse",
    );
    assert_eq!(
        delivery.delivery_attempt,
        Some(5),
        "`deliveryAttempt` is top-level, beside `message` rather than inside it — and a redelivery is what it \
         counts"
    );
    // The same example carries an `attributes` map, which this connector does not read. Asserted **absent from
    // the type** rather than merely unread, so a future reader knows the omission is deliberate: nothing in the
    // Gmail push path filters on an attribute, and a field added for one would need a use for it.
    assert!(
        delivery.message_id().is_none(),
        "the maximum-value example has no messageId, so the dedupe key must be absent rather than invented"
    );
}

#[test]
fn an_unwrapped_delivery_is_refused_rather_than_read_as_no_change() {
    // Pub/Sub can deliver **unwrapped** — the raw payload as the whole body — and this connector reads only the
    // wrapped form. That is a real, documented subscription option, so it is a delivery this connector does not
    // understand rather than a malformed one; reading it as "nothing changed" would be the silent direction.
    assert_eq!(
        parse_delivery(r#"{"message":{"messageId":"1"}}"#).err(),
        Some(PubsubDeliveryError::NoWrappedPayload)
    );
    assert_eq!(
        parse_delivery("{}").err(),
        Some(PubsubDeliveryError::NoWrappedPayload)
    );
    // A body that is not JSON at all is a different fault, reported as one.
    assert!(matches!(
        parse_delivery("not json").err(),
        Some(PubsubDeliveryError::NotJson { .. })
    ));
    // And a **payload** fault stays a payload fault rather than being flattened into the envelope's: the two
    // layers have different remedies, and a caller debugging one should not be sent to the other (`ADR-0086`).
    assert!(matches!(
        parse_delivery(r#"{"message":{"data":"bm90IGpzb24"}}"#).err(),
        Some(PubsubDeliveryError::Payload(
            PubsubNotificationError::NotTheDocumentedShape
        ))
    ));
}

#[test]
fn only_the_five_documented_statuses_acknowledge_a_delivery() {
    // **A list rather than "2xx".** The push page names exactly `102`, `200`, `201`, `202` and `204`, and the
    // difference decides behaviour: a `203` or `206` is a success by HTTP's classification and a **negative
    // acknowledgement** by Pub/Sub's, so a handler that returned two-hundred-and-something would silently
    // request redelivery of every message.
    for status in ACKNOWLEDGING_STATUSES {
        assert!(
            acknowledges_delivery(status),
            "{status} is on the page's list and must acknowledge"
        );
    }
    for status in [0, 100, 101, 203, 206, 301, 400, 500] {
        assert!(
            !acknowledges_delivery(status),
            "{status} is not on the list, so it is a negative acknowledgement"
        );
    }
    // The two that make the point: a success status the page omits, and the one 1xx status it names.
    assert!(!acknowledges_delivery(203));
    assert!(acknowledges_delivery(102));
    assert_eq!(ACKNOWLEDGING_STATUSES.len(), 5);
}
