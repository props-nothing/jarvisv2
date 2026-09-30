//! The Gmail push notification envelope, and the two encodings Google declares for it.
//!
//! # What arrives, and what it is for
//!
//! A Gmail push delivery is a `POST` from Cloud Pub/Sub whose JSON body is a `PubsubMessage` wrapper:
//!
//! ```json
//! {
//!   "message": {
//!     "data": "eyJlbWFpbEFkZHJlc3MiOiAidXNlckBleGFtcGxlLmNvbSIsICJoaXN0b3J5SWQiOiAiMTIzNDU2Nzg5MCJ9",
//!     "messageId": "2070443601311540",
//!     "publishTime": "2021-02-26T19:13:55.749Z"
//!   },
//!   "subscription": "projects/myproject/subscriptions/mysubscription"
//! }
//! ```
//!
//! The useful content is **not** in `data` directly — `data` is base64 text whose decoded bytes are a small JSON
//! object naming the mailbox and its new position:
//!
//! ```json
//! {"emailAddress": "user@example.com", "historyId": "9876543210"}
//! ```
//!
//! So a delivery says *which mailbox changed and roughly how far*, and the caller then reads `history.list` from
//! its stored `historyId` up to the notified one. **The envelope carries no message content and no change
//! detail**, which is the property that makes it safe to log and cheap to handle.
//!
//! # The finding: Google declares this field's encoding two different ways
//!
//! Two official pages describe `message.data`, and they do not agree:
//!
//! - The **Gmail push guide**: *"The `message.data` field is a Base64URL-encoded string that decodes to a
//!   JSON object containing the email address and the new mailbox history ID."*
//! - The **`PubsubMessage` reference** (the type the same guide links to): `data | string (bytes format) | …
//!   A base64-encoded string.`
//!
//! RFC 4648 §4 and §5 differ in exactly two characters (`+`/`/` versus `-`/`_`), so the disagreement is
//! invisible on any value that happens to contain neither — including **the guide's own example**, which uses
//! only `A-Za-z0-9` and therefore decodes identically under both. That is why the discrepancy is worth
//! recording rather than resolving by preference: every convenient test would pass under either reading, and
//! the difference would surface on the first real notification whose payload needs the difference.
//!
//! [`PubsubData::Encoding`] exists to make the outcome visible — the decoder tries the URL-safe alphabet first,
//! because the Gmail guide is the more specific statement and the surface this connector implements, and reports
//! which one matched so a caller (and a test) can see what actually happened.
//!
//! # What is not verified
//!
//! **No delivery has been received.** The shapes are transcribed from the dated sources in
//! `docs/research/integrations/google.md`, and the guide's own example value is used as a test fixture, so the
//! tests prove this module implements the *record* and the record's example — not that a live subscription
//! delivers what the guide says.

use crate::base64::{Alphabet, Base64Error, Padding, decode_with};

/// The field a Gmail push payload carries its mailbox and position in.
const EMAIL_FIELD: &str = "emailAddress";

/// The field a Gmail push payload carries the mailbox's new position in.
const HISTORY_ID_FIELD: &str = "historyId";

/// The two alphabets a `message.data` value can be written in, in the order this module tries them.
///
/// **Only the alphabet needs searching**, because padding is *observable* — it is the `=` at the end — while the
/// alphabet leaves no marker: a value using neither `+`/`/` nor `-`/`_` decodes identically either way. So the
/// decoder is told which alphabet to try and reads the padding for itself (`ADR-0089`).
///
/// **Ordered from most to least specific**, so the first success is the most informative report:
///
/// 1. **URL-safe** — what the Gmail push guide says the field is ("a Base64URL-encoded string"), and what
///    RFC 7636 §4.2 means by base64url.
/// 2. **Standard** — what the `PubsubMessage` reference's `string (bytes format)` means, and what **Cloud
///    Pub/Sub's own push example** actually contains. This alphabet's padded form is the one an earlier version
///    of this code refused outright, on the strength of an OAuth rule that does not govern Pub/Sub
///    (`ADR-0088`'s finding, corrected here).
const ALPHABETS: [Alphabet; 2] = [Alphabet::UrlSafe, Alphabet::Standard];

/// Which combination of alphabet and padding a value was written in.
///
/// **Two axes rather than four variants**, because they *are* two independent facts and a caller asking "was
/// this padded?" should not have to match on four spellings of one answer. The forms this module *documents* —
/// the Gmail guide's and the Pub/Sub reference's — are predicates over this type rather than variants of it, so
/// a document that changes one axis does not need a new variant.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PubsubData {
    /// The alphabet the value was written in.
    pub alphabet: Alphabet,
    /// Whether the value carried `=` padding.
    pub padding: Padding,
}

impl PubsubData {
    /// The form the **Gmail push guide** describes: "a Base64URL-encoded string".
    pub const GMAIL_GUIDE: Self = Self {
        alphabet: Alphabet::UrlSafe,
        padding: Padding::Absent,
    };

    /// The form the **`PubsubMessage` reference** describes through `string (bytes format)`, which is Google's
    /// convention for standard base64 carrying its padding — and what the push page's own example is.
    pub const PUBSUB_FIELD_TYPE: Self = Self {
        alphabet: Alphabet::Standard,
        padding: Padding::Present,
    };

    /// Returns a two-axis name, so a log line names both decisions rather than one word that conflates them.
    #[must_use]
    pub fn as_str(self) -> String {
        format!("{}/{}", self.alphabet.as_str(), self.padding.as_str())
    }
}

/// The decoded Gmail push payload: which mailbox changed, and how far.
///
/// Two fields, both provider-supplied, and **no message content** — which is the whole point of the envelope.
/// The values are kept as text rather than converted: `history_id` becomes a cursor through
/// `SyncCursor::new`, which applies its own bound, and `email_address` is an identity the caller matches against
/// its connected accounts rather than a value this module validates as an address.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PubsubNotification {
    /// The mailbox that changed, as the provider named it.
    pub email_address: String,
    /// The mailbox's **new** position, which is where an incremental sync advances to.
    pub history_id: String,
}

/// Why a push notification's `data` could not be read.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum PubsubNotificationError {
    /// The base64 text was not decodable under **either** declared alphabet.
    ///
    /// Carries the URL-safe attempt's error because that is the encoding the Gmail guide names, and the two
    /// alphabets overlap on every character but two — so the first attempt's failure is the one that is
    /// meaningful, and the second's would differ only on the two characters, which the message already covers.
    #[error("a notification payload is not base64 in either declared alphabet: {source}")]
    Base64 {
        /// The URL-safe attempt's failure.
        source: Base64Error,
    },
    /// The decoded bytes were not UTF-8.
    #[error(
        "a notification payload decodes to bytes that are not UTF-8, so the JSON cannot be read"
    )]
    NotUtf8,
    /// The decoded payload was not JSON of the documented shape.
    #[error(
        "a notification payload must decode to a JSON object with `emailAddress` and `historyId`"
    )]
    NotTheDocumentedShape,
}

/// Decodes a push notification's `message.data`, reporting which alphabet it used.
///
/// # The order of the two attempts, and why it is this way round
///
/// **URL-safe first**, because the Gmail push guide is the more specific statement — it describes *this*
/// notification payload — while the `PubsubMessage` reference describes a general Cloud Pub/Sub field that Gmail
/// happens to reuse. Preferring the specific statement is the reading that matches the surface being
/// implemented.
///
/// **Both are tried, because the contradiction is the provider's and neither reading is provably wrong.** A
/// decoder that accepted only URL-safe would refuse a delivery encoded the other way, and a notification the
/// connector refuses is a **missed change** — the silent failure this whole path exists to avoid. Accepting
/// both costs one extra attempt on a value whose alphabet was already wrong, and the returned
/// [`PubsubData`] says which matched, so the ambiguity stays visible instead of being silently resolved.
///
/// # Errors
///
/// Returns [`PubsubNotificationError`] for text that decodes under neither alphabet, bytes that are not UTF-8,
/// or JSON that is not the documented object. Each is a distinct variant because the remedies differ: a
/// non-base64 body is a delivery this connector misunderstands, while a base64 body that decodes to the wrong
/// JSON is a payload shape that changed.
pub fn decode_notification(
    data: &str,
) -> Result<(PubsubNotification, PubsubData), PubsubNotificationError> {
    // The alphabets are tried in order and the first that works wins. The **first** attempt's error is the one
    // reported on total failure, because it is the alphabet the Gmail guide names and therefore the one whose
    // refusal is most informative.
    let mut first_error: Option<Base64Error> = None;
    let mut decoded: Option<(Vec<u8>, PubsubData)> = None;
    for alphabet in ALPHABETS {
        match decode_with(data, alphabet) {
            Ok(bytes) => {
                decoded = Some((
                    bytes,
                    PubsubData {
                        alphabet,
                        padding: Padding::of(data),
                    },
                ));
                break;
            }
            Err(error) => first_error = first_error.or(Some(error)),
        }
    }
    let Some((bytes, encoding)) = decoded else {
        return Err(PubsubNotificationError::Base64 {
            source: first_error.unwrap_or(Base64Error::Alphabet),
        });
    };
    let text = String::from_utf8(bytes).map_err(|_| PubsubNotificationError::NotUtf8)?;
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|_| PubsubNotificationError::NotTheDocumentedShape)?;
    let email_address = value
        .get(EMAIL_FIELD)
        .and_then(serde_json::Value::as_str)
        .ok_or(PubsubNotificationError::NotTheDocumentedShape)?
        .to_owned();
    let history_id = value
        .get(HISTORY_ID_FIELD)
        .and_then(serde_json::Value::as_str)
        .ok_or(PubsubNotificationError::NotTheDocumentedShape)?
        .to_owned();
    Ok((
        PubsubNotification {
            email_address,
            history_id,
        },
        encoding,
    ))
}

/// The status codes that **acknowledge** a push delivery.
///
/// From the push page: *"To acknowledge the message, return one of the following status codes: `102`, `200`,
/// `201`, `202`, `204`."* Any other code is a negative acknowledgement and Pub/Sub resends.
///
/// # Why this is a list and not "2xx"
///
/// Because the page gives a list, and the two differ: a `203` or a `206` is a success by the HTTP classification
/// and **not** by this one, so a handler that answered two-hundred-and-something would be silently
/// negative-acknowledging every delivery — a redelivery loop with no error in it. The list is also why the
/// acknowledgement is a decision a caller has to make deliberately rather than inherit from a framework's
/// default.
pub const ACKNOWLEDGING_STATUSES: [u16; 5] = [102, 200, 201, 202, 204];

/// Returns whether a status code acknowledges a push delivery.
///
/// **Fails closed**: an unrecognised code is treated as a negative acknowledgement, which is the direction that
/// causes a *redelivery* rather than a lost notification. The opposite choice — acknowledging everything not
/// obviously wrong — would silently drop a message the connector never processed.
#[must_use]
pub const fn acknowledges_delivery(status: u16) -> bool {
    let mut index = 0;
    while index < ACKNOWLEDGING_STATUSES.len() {
        if ACKNOWLEDGING_STATUSES[index] == status {
            return true;
        }
        index += 1;
    }
    false
}

/// One push delivery, as the `POST` body carries it.
///
/// # At-least-once, which is why `message_id` is here
///
/// The push page: *"A non-success response indicates that Pub/Sub must resend the messages"* and *"If you send
/// a negative acknowledgment or the acknowledgment deadline expires, Pub/Sub resends the message."* So a
/// delivery **repeats**, and `messageId` — *"Guaranteed to be unique within the topic"* — is the only field that
/// can tell a redelivery from a new change. Without it a repeated Gmail notification would be processed twice,
/// which for a sync is wasted work and for any future effect would be a second effect.
///
/// # The two spellings, and why both are accepted
///
/// The push page's own examples show **both** `messageId` and `message_id`, and both `publishTime` and
/// `publish_time`. The page presents the second of each pair without explanation, so which one a subscription
/// sends is not established — and a parser that read only one spelling would find `None` for the other while
/// reporting no error at all. [`Self::message_id`] is therefore populated from either, with the camelCase
/// preferred because it is the spelling the `PubsubMessage` reference documents.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize)]
pub struct PubsubDelivery {
    /// The subscription that delivered this, as a full resource name.
    #[serde(default)]
    pub subscription: Option<String>,
    /// How many times delivery of this message has been attempted, when the subscription reports it.
    ///
    /// **Top-level**, unlike the message's own fields — the push page's maximum-value example puts
    /// `deliveryAttempt` beside `message` rather than inside it. It is the field that makes a redelivery
    /// visible, so reading it from the wrong place would lose the only signal that a delivery repeated.
    #[serde(default, rename = "deliveryAttempt", alias = "delivery_attempt")]
    pub delivery_attempt: Option<u32>,
    /// The wrapped message, whose `data` is the Gmail payload.
    #[serde(default)]
    pub message: Option<PubsubMessageBody>,
}

impl PubsubDelivery {
    /// Returns the message's server-assigned identifier, which is the deduplication key.
    ///
    /// A method rather than a field, because the value lives **inside** `message` and a caller should not have
    /// to know that. `None` when the delivery carried no message or the message carried no identifier.
    #[must_use]
    pub fn message_id(&self) -> Option<&str> {
        self.message.as_ref()?.message_id.as_deref()
    }

    /// Returns the message's wrapped base64 payload.
    #[must_use]
    pub fn data(&self) -> Option<&str> {
        self.message.as_ref()?.data.as_deref()
    }
}

/// The `message` object inside a push delivery.
///
/// # Where these fields live, and why the shape matters
///
/// The push page's own examples put `messageId`, `publishTime` and `attributes` **inside `message`**, while
/// `subscription` and `deliveryAttempt` sit **beside** it. An earlier version of this struct had the identifier
/// and the publish time at the top level, which made every lookup return `None` — and `None` from a
/// `#[serde(default)]` field is **silent**, so the deduplication key would simply have been absent on every
/// delivery with nothing reporting it. The test that caught it asserts the field is `Some` for the provider's
/// own example rather than merely that the body parses.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize)]
pub struct PubsubMessageBody {
    /// The base64 payload, in either alphabet [`ALPHABETS`] covers.
    #[serde(default)]
    pub data: Option<String>,
    /// The message's server-assigned identifier, unique within the topic. The deduplication key.
    #[serde(default, rename = "messageId", alias = "message_id")]
    pub message_id: Option<String>,
    /// When the message was published, in the provider's RFC 3339 form. **Not parsed here**, because nothing
    /// downstream reads it yet and a timestamp type in the envelope would invite ordering logic the provider
    /// explicitly does not guarantee (`orderingKey` exists precisely because order is opt-in).
    #[serde(default, rename = "publishTime", alias = "publish_time")]
    pub publish_time: Option<String>,
}

/// Why a delivery body could not be read.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum PubsubDeliveryError {
    /// The body was not JSON.
    #[error("a push delivery body must be JSON: {reason}")]
    NotJson {
        /// What went wrong.
        reason: &'static str,
    },
    /// The delivery carried no `message.data`.
    ///
    /// **A distinct case, and the one worth naming**: Pub/Sub can deliver **unwrapped** (the push page's
    /// `payload-unwrapping` option), in which case the raw payload is the whole body and no `data` field exists.
    /// So this is either an unwrapped subscription or a delivery with attributes but no data — both of which
    /// mean *this connector does not understand the delivery*, and reading either as "no change" is the silent
    /// direction.
    #[error(
        "a push delivery carried no `message.data`; this connector reads the wrapped form, so an unwrapped \
         subscription is not supported"
    )]
    NoWrappedPayload,
    /// The delivery's payload could not be read.
    ///
    /// **Kept as a nested variant rather than flattened into [`Self::NotJson`]**, because the two are different
    /// layers: the envelope is Pub/Sub's and the payload inside it is Gmail's. A caller debugging a refusal needs
    /// to know which of the two to look at, and a single "bad body" message would send it to the wrong one —
    /// the same argument every error type in this crate is built on.
    #[error(transparent)]
    Payload(#[from] PubsubNotificationError),
}

/// Parses a push delivery body and decodes its Gmail payload.
///
/// # Errors
///
/// Returns [`PubsubDeliveryError`] for a body that is not the wrapped shape, and
/// [`PubsubNotificationError`] for a payload that is not decodable. The two are separate types because they
/// name different layers: the delivery envelope is Pub/Sub's, and the payload inside it is Gmail's.
pub fn parse_delivery(
    body: &str,
) -> Result<(PubsubDelivery, PubsubNotification, PubsubData), PubsubDeliveryError> {
    let delivery: PubsubDelivery =
        serde_json::from_str(body).map_err(|_| PubsubDeliveryError::NotJson {
            reason: "the body did not parse as a push delivery",
        })?;
    let data = delivery
        .data()
        .ok_or(PubsubDeliveryError::NoWrappedPayload)?;
    let (notification, encoding) = decode_notification(data)?;
    Ok((delivery, notification, encoding))
}

#[cfg(test)]
#[path = "pubsub_tests.rs"]
mod tests;
