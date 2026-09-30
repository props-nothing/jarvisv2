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

use crate::base64::{Base64Error, decode_standard, decode_url_safe};

/// The field a Gmail push payload carries its mailbox and position in.
const EMAIL_FIELD: &str = "emailAddress";

/// The field a Gmail push payload carries the mailbox's new position in.
const HISTORY_ID_FIELD: &str = "historyId";

/// Which of the two declared alphabets a value decoded under.
///
/// Reported rather than discarded, because the two pages disagree (`ADR-0088`) and a caller that logged the
/// answer could settle it from a real delivery. A `bool` would stand for "one of two encodings" without saying
/// which, which is exactly the information worth having.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PubsubData {
    /// The value decoded as RFC 4648 §5 URL-safe base64, which is what the Gmail guide calls it.
    UrlSafe,
    /// The value decoded as RFC 4648 §4 standard base64, which is what the `PubsubMessage` reference types it
    /// as.
    Standard,
}

impl PubsubData {
    /// Returns the encoding's stable name, for a log line or a diagnostic.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UrlSafe => "base64url",
            Self::Standard => "base64",
        }
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
    let (bytes, encoding) = match decode_url_safe(data) {
        Ok(bytes) => (bytes, PubsubData::UrlSafe),
        Err(url_safe_error) => match decode_standard(data) {
            Ok(bytes) => (bytes, PubsubData::Standard),
            // The URL-safe error is reported, and the reason is above: on any value whose failure is about the
            // alphabet at all, the two errors differ only in which two characters are permitted, and the
            // URL-safe attempt is the one the Gmail guide's wording predicts.
            Err(_) => {
                return Err(PubsubNotificationError::Base64 {
                    source: url_safe_error,
                });
            }
        },
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

#[cfg(test)]
#[path = "pubsub_tests.rs"]
mod tests;
