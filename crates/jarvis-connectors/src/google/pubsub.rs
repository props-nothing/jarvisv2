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

use std::fmt;

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
#[derive(Clone, Eq, PartialEq)]
pub struct PubsubNotification {
    /// The mailbox that changed, as the provider named it.
    pub email_address: String,
    /// The mailbox's **new** position, which is where an incremental sync advances to.
    pub history_id: String,
}

impl fmt::Debug for PubsubNotification {
    /// Redacts the mailbox address, which is a person's.
    ///
    /// **Hand-written rather than derived, and the crate is of two minds about this field otherwise.**
    /// `VerifiedAccount` already redacts its provider account identifier with the note that it "is usually an
    /// email address and a diagnostic that printed it would leak the account's owner", and `AccessToken` and
    /// `FormRequest` follow the same pattern — while this type, which holds an address the provider sent over a
    /// public endpoint, printed it. A `{:?}` reaches a log line, and `security.md` forbids token material and
    /// account content there (`ADR-0091`).
    ///
    /// **The history id is kept**, and the asymmetry is deliberate: it is a mailbox *position*, so it names no
    /// person, and it is exactly what a diagnostic about a stuck sync needs to show.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PubsubNotification")
            .field("email_address", &"[REDACTED]")
            .field("history_id", &self.history_id)
            .finish()
    }
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

/// How a handler answered one delivery, and therefore what the *subscription* does next.
///
/// # Why this is not a `bool`, and why it is not the status code either
///
/// [`acknowledges_delivery`] answers "was this code in the list", which is a fact about one response. It does
/// **not** say what the answer costs, and the push page makes that the operative question: a negative
/// acknowledgement does not merely cause this one message to be redelivered, it is also an input to a
/// **subscription-global** delay.
///
/// > "If a push subscriber sends too many negative acknowledgments, Pub/Sub might start delivering messages
/// > using a push backoff. When Pub/Sub uses a push backoff, it stops delivering messages for a predetermined
/// > amount of time. This time span can range between 100 milliseconds to 60 seconds."
///
/// and, among the considerations the page lists:
///
/// > "• Push backoff can't be turned on or off. You also can't modify the values used to calculate the delay.
/// > • Push backoff triggers on the following actions: When a negative acknowledgment is received. When the
/// > acknowledgment deadline of a message expires. • **Push backoff applies to all the messages in a
/// > subscription (global).**"
///
/// So one delivery's answer is paid for by **every** delivery on the subscription, for up to a minute, and the
/// subscriber cannot opt out. That is the fact a bare `bool` erases: `false` from [`acknowledges_delivery`]
/// reads as "this one message will be retried" while it also means "and nothing else will be delivered for a
/// while". A caller deciding what to answer a delivery it cannot process needs the second half, and it is the
/// half that decides between "refuse again" and "record it somewhere and accept it".
///
/// # Why a decision rather than the raw status
///
/// A `u16` would let a caller invent a code and re-derive [`acknowledges_delivery`] at the call site — the
/// duplication this type removes. It also keeps the *reason* legible: [`Self::Retry`] and
/// [`Self::AbandonAndAcknowledge`] may both be sent as a negative code and an acknowledging one respectively,
/// but they mean opposite things about the message, and a reader of a call site should not have to infer which
/// from an integer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeliveryAck {
    /// The delivery was processed. Answer with a code that acknowledges it.
    Accept,
    /// The delivery **could not be processed** and should come back, and the handler can afford the backoff.
    ///
    /// For a transient cause — the store is locked, a token refresh is in flight — where a redelivery is the
    /// documented remedy.
    Retry,
    /// The delivery can **never** be processed by this handler, so it is acknowledged rather than refused.
    ///
    /// # The decision this variant exists for, and its cost
    ///
    /// A negative acknowledgement is the one answer that is charged to the whole subscription: it triggers a
    /// backoff that "applies to all the messages in a subscription (global)" and that cannot be turned off. So
    /// for a delivery the handler will *never* accept — a payload shape this connector does not read, a mailbox
    /// that is no longer connected — refusing it does not merely redeliver a message that will fail again, it
    /// **slows delivery for every other account on the subscription** for up to a minute, and keeps doing so
    /// for as long as the bad message is retried. The redelivery is not bounded by the handler; it is bounded by
    /// the subscription's retry policy, and the page notes that a push subscriber "can't modify the
    /// acknowledgment deadline of individual messages".
    ///
    /// So the honest answer to a permanently unprocessable delivery is to acknowledge it and **record that it
    /// was dropped**, rather than to refuse it forever. That is a choice with a real downside — the message is
    /// gone from the queue — which is exactly why it is a named variant with the reasoning attached instead of a
    /// default a caller falls into. `security.md`'s "missing or stale evidence fails closed" does not decide
    /// this one: both answers are closed against *acting* on the delivery, and the question is only whether the
    /// cost lands on this message or on the subscription.
    AbandonAndAcknowledge,
}

impl DeliveryAck {
    /// Returns whether this answer acknowledges the delivery.
    ///
    /// `Accept` and `AbandonAndAcknowledge` both do, for opposite reasons — one because the work is done, one
    /// because the work can never be done. **Refusing is the exception**, and it is the only arm that costs the
    /// subscription.
    #[must_use]
    pub const fn acknowledges(self) -> bool {
        match self {
            Self::Accept | Self::AbandonAndAcknowledge => true,
            Self::Retry => false,
        }
    }
}

/// Decides what to answer a delivery this handler could not process, from the failure's **retryability** and
/// the provider's own attempt count.
///
/// # The two inputs, and why the caller supplies the first
///
/// `retryable` is the caller's reading of *why* the delivery failed, because only the caller knows: a store
/// error is transient while a payload this connector does not decode is not, and this module sees neither. It
/// is the same inference/decision split `ADR-0066` establishes for a cursor — the component with the evidence
/// reads it, and the component with the policy decides.
///
/// `delivery_attempt` is the provider's [`PubsubDelivery::delivery_attempt`] value **verbatim**, which is the
/// field the push page increments per attempt (`"deliveryAttempt": 5`), and **`0` when the provider did not
/// report one**. Absent is not the same as first, and the direction matters: `0` is below
/// [`MAX_RETRY_ATTEMPTS`], so an unreported attempt still gets a retry — the reading that keeps a possibly-new
/// delivery alive, where treating absent as "already retried too often" would abandon a first delivery that
/// merely arrived without the optional field.
///
/// # Why a bound on attempts rather than on time
///
/// There is no per-message deadline to read: the push page says a subscriber "can't modify the acknowledgment
/// deadline of individual messages that you receive from push subscriptions". What *is* readable is
/// `deliveryAttempt`, so the count is the only per-message fact available.
#[must_use]
pub const fn decide_acknowledgement(retryable: bool, delivery_attempt: u32) -> DeliveryAck {
    // **Not retryable is not the same as retryable-but-out-of-budget**, and the two arms are kept apart on
    // purpose: the first says the delivery can never be processed, the second says it might have been and did
    // not within the budget. Both acknowledge, but a reader should see which is which from the reason.
    if !retryable {
        return DeliveryAck::AbandonAndAcknowledge;
    }
    if delivery_attempt >= MAX_RETRY_ATTEMPTS {
        return DeliveryAck::AbandonAndAcknowledge;
    }
    DeliveryAck::Retry
}

/// The `deliveryAttempt` value at which a **retryable** failure stops being refused, and why it is small.
///
/// **A JARVIS bound, not a provider figure**, and named as such: the page publishes the backoff range and the
/// global scope but no retry count a subscriber should use. It is compared against the provider's
/// `deliveryAttempt` verbatim, so a value of `3` means the third attempt is the last refused and the fourth is
/// abandoned — one comparison with no off-by-one to re-derive.
///
/// Three, because the page's own backoff reaches 60 seconds and grows with the count of negative
/// acknowledgements: a handful of attempts spans the transient causes (a held store, an in-flight token
/// refresh) without holding the whole subscription down through a full backoff cycle for each. The figure is
/// deliberately small and stated, because the alternative — refusing without a bound — is not a policy but the
/// absence of one, and its cost is paid by every other mailbox on the subscription.
pub const MAX_RETRY_ATTEMPTS: u32 = 3;

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
///
/// # Why the derived `Debug` is safe here
///
/// The only sensitive value this type holds is inside [`PubsubMessageBody::data`], whose own `Debug` is
/// **hand-written to redact it** — so the derive prints a redaction rather than a mailbox. That is the shape to
/// check when a struct's `Debug` is derived: not "does this struct hold a secret" but "does every field it
/// holds print one", and the answer changes when a neighbour's `Debug` changes (`ADR-0091`).
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
#[derive(Clone, Eq, PartialEq, serde::Deserialize)]
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

impl fmt::Debug for PubsubMessageBody {
    /// Redacts `data`, which **is** the address: its bytes decode to `{"emailAddress":…}`.
    ///
    /// This is the subtler of the two redactions in this module, because the field looks like an opaque base64
    /// blob rather than like a person's address — so a reader could reasonably assume it is safe to print. It is
    /// not: the value carries no credential, but it names a mailbox, which is the same disclosure
    /// [`PubsubNotification`] redacts. The **length** is printed instead, because a diagnostic that shows the
    /// payload arrived at all, and roughly how large it was, is the useful part (`ADR-0091`).
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PubsubMessageBody")
            .field(
                "data",
                &self
                    .data
                    .as_ref()
                    .map(|data| format!("[REDACTED], {} chars", data.len())),
            )
            .field("message_id", &self.message_id)
            .field("publish_time", &self.publish_time)
            .finish()
    }
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
