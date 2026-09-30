//! Tests for the push notification envelope.
//!
//! The load-bearing ones are about the **alphabet**, because the two official pages disagree about it and the
//! guide's own example cannot tell the two apart — so a convenient test proves nothing.
//!
//! Falsification record in `TODO.md`.

use super::*;
use crate::base64::url_safe_no_pad;

/// The Gmail push guide's own example value, verbatim.
///
/// Captured from the guide's envelope sample. It decodes to
/// `{"emailAddress": "user@example.com", "historyId": "1234567890"}` — asserted below, because a fixture whose
/// expected value is computed rather than stated would prove nothing about the encoding.
const GUIDE_EXAMPLE: &str =
    "eyJlbWFpbEFkZHJlc3MiOiAidXNlckBleGFtcGxlLmNvbSIsICJoaXN0b3J5SWQiOiAiMTIzNDU2Nzg5MCJ9";

/// A payload spelling that **forces** the two alphabets to differ, in both spellings.
///
/// The JSON is `{"emailAddress":"a@example.invalid","historyId":"0>"}`. A sweep of all 95 printable ASCII
/// characters at all four base64 alignments found only three — `>`, `?`, `~`, each after a one-character offset
/// — whose standard encoding contains `+` or `/`, so this is one of the few Gmail-shaped payloads that can
/// distinguish RFC 4648 §4 from §5. Both spellings are literals rather than computed, because a computed
/// fixture would test the encoder that produced it.
const FORCING_URL_SAFE: &str =
    "eyJlbWFpbEFkZHJlc3MiOiJhQGV4YW1wbGUuaW52YWxpZCIsImhpc3RvcnlJZCI6IjA-In0";

/// The **standard**-alphabet spelling of [`FORCING_URL_SAFE`]'s bytes, unpadded.
const FORCING_STANDARD: &str =
    "eyJlbWFpbEFkZHJlc3MiOiJhQGV4YW1wbGUuaW52YWxpZCIsImhpc3RvcnlJZCI6IjA+In0";

fn must<T, E: std::fmt::Display>(result: Result<T, E>, what: &str) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("{what}: {error}"),
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
    // The example contains **no** `-`, `_`, `+` or `/`, so it decodes identically under either alphabet. The
    // reported encoding is therefore the URL-safe attempt, which is the one made first — asserted so the
    // ordering is pinned rather than incidental.
    assert_eq!(
        encoding,
        PubsubData::UrlSafe,
        "the URL-safe alphabet is tried first, so a value that suits both reports UrlSafe"
    );
}

#[test]
fn both_declared_alphabets_decode_because_two_pages_disagree() {
    // **The finding this module exists for.** The Gmail guide says `message.data` is Base64URL; the
    // `PubsubMessage` reference types the same field `string (bytes format)`, "A base64-encoded string". The two
    // differ in `+`/`/` versus `-`/`_`, and `GUIDE_EXAMPLE` cannot tell them apart — it uses only `A-Za-z0-9`.
    //
    // **And that is true of almost every Gmail notification.** A sweep of all 95 printable ASCII characters at
    // all four base64 alignments found only **three** (`>`, `?`, `~`, each after a one-character offset) whose
    // standard encoding contains `+` or `/`. So in practice the alphabets agree on nearly every real payload,
    // which is exactly why the two pages' disagreement is worth recording rather than assuming it cannot
    // matter: it is invisible until the one value that differs, and that value is a **missed change** if the
    // decoder refuses it.
    //
    // The fixture is therefore one of those three, spelled out here rather than generated, so a reader can see
    // that it genuinely forces the difference.
    const FORCING_STANDARD: &str =
        "eyJlbWFpbEFkZHJlc3MiOiJhQGV4YW1wbGUuaW52YWxpZCIsImhpc3RvcnlJZCI6IjA+In0=";
    const FORCING_URL_SAFE: &str =
        "eyJlbWFpbEFkZHJlc3MiOiJhQGV4YW1wbGUuaW52YWxpZCIsImhpc3RvcnlJZCI6IjA-In0";
    assert!(
        FORCING_STANDARD.contains('+') && FORCING_URL_SAFE.contains('-'),
        "the fixture must force the alphabet difference, or the test proves nothing"
    );

    // The URL-safe spelling, which is what the Gmail guide declares.
    let (notification, encoding) = must(
        decode_notification(FORCING_URL_SAFE),
        "a URL-safe payload must decode",
    );
    assert_eq!(notification.email_address, "a@example.invalid");
    assert_eq!(notification.history_id, "0>");
    assert_eq!(encoding, PubsubData::UrlSafe);

    // And the **standard** spelling of the same bytes, which the `PubsubMessage` reference declares. Padding is
    // trimmed because the decoder accepts only the unpadded form, which is asserted separately below.
    let (notification, encoding) = must(
        decode_notification(FORCING_STANDARD.trim_end_matches('=')),
        "a standard-alphabet payload must decode too, because the PubsubMessage reference declares it",
    );
    assert_eq!(notification.email_address, "a@example.invalid");
    assert_eq!(notification.history_id, "0>");
    assert_eq!(
        encoding,
        PubsubData::Standard,
        "a value that is not URL-safe decodes under the standard alphabet and must say so"
    );
}

#[test]
fn a_padded_value_is_refused_rather_than_quietly_stripped() {
    // Neither form this crate accepts is padded (the URL-safe one because RFC 7636 requires padding omitted, the
    // standard one because the caller passes what the encoder produced). A decoder that silently stripped `=`
    // would accept two spellings of one value and could not then say which it received.
    //
    // The fixture is `{"emailAddress":"a@example.invalid","historyId":"0>"}` in standard base64, which **does**
    // end in padding. An earlier version of this test used a two-byte payload whose base64 happened to need no
    // padding at all, so it asserted that an unpadded value decoded — proving nothing about padding. A
    // fixture that does not contain the thing under test is the trap this repository keeps recording.
    const PADDED: &str = "eyJlbWFpbEFkZHJlc3MiOiJhQGV4YW1wbGUuaW52YWxpZCIsImhpc3RvcnlJZCI6IjA+In0=";
    assert!(
        PADDED.ends_with('='),
        "the fixture must be padded: {PADDED}"
    );
    assert_eq!(
        decode_notification(PADDED),
        Err(PubsubNotificationError::Base64 {
            source: crate::base64::Base64Error::Padded
        }),
        "a padded value must be refused, naming the padding"
    );
    // The unpadded spelling of the same bytes decodes, so the refusal is about the padding and not the value.
    assert!(
        decode_notification(PADDED.trim_end_matches('=')).is_ok(),
        "the same bytes without padding must decode"
    );
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
    // `"____"` is valid base64 under **both** alphabets (`_` is the URL-safe 63 and the standard 63 is `/`, so
    // under the standard alphabet these four characters are out of it — the URL-safe attempt therefore succeeds
    // and yields three bytes that are not UTF-8, which is the case distinguishing `NotUtf8` from
    // `NotTheDocumentedShape`).
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
fn an_encoded_value_and_its_decode_are_inverses_in_both_alphabets() {
    // A round-trip pins the decoder against the encoder, which is otherwise an independent implementation. Run
    // over lengths that exercise every remainder, because the trailing partial group is where an off-by-one
    // lives: a 1-byte, 2-byte and 3-byte tail each flush differently.
    //
    // The **standard** form is derived from the URL-safe one by substituting the two characters that differ
    // (RFC 4648 §5's transformation read backwards: `-`→`+`, `_`→`/`), so both decoders are exercised against
    // one encoder — a second encoder would be a second thing that could be wrong.
    let payload = br#"{"emailAddress":"user@example.com","historyId":"9876543210"}"# as &[u8];
    for length in 1..=payload.len() {
        let slice = &payload[..length];
        let url_safe = url_safe_no_pad(slice);
        assert_eq!(
            crate::base64::decode_url_safe(&url_safe),
            Ok(slice.to_vec()),
            "url-safe round trip failed at {length} bytes"
        );
        let standard = url_safe.replace('-', "+").replace('_', "/");
        assert_eq!(
            crate::base64::decode_standard(&standard),
            Ok(slice.to_vec()),
            "standard round trip failed at {length} bytes"
        );
    }

    // **And the same round trip over a value the two alphabets actually spell differently.** The sweep above
    // may substitute nothing at all — as the ASCII survey in the sibling test established, almost no Gmail-shaped
    // payload produces the difference — so the substitution is checked to have *had* an effect by decoding the
    // forcing fixture, whose URL-safe form contains `-` and whose standard form contains `+`.
    assert_eq!(
        FORCING_URL_SAFE.replace('-', "+").replace('_', "/"),
        FORCING_STANDARD,
        "the substitution must produce the standard spelling of the forcing fixture, or the alphabets never \
         differ in this test"
    );
    let bytes = must(
        crate::base64::decode_url_safe(FORCING_URL_SAFE),
        "the forcing fixture must decode as URL-safe",
    );
    assert_eq!(
        url_safe_no_pad(&bytes),
        FORCING_URL_SAFE,
        "re-encoding must reproduce the URL-safe spelling"
    );
    assert_eq!(
        crate::base64::decode_standard(FORCING_STANDARD),
        Ok(bytes),
        "the standard spelling must decode to the same bytes"
    );
}
