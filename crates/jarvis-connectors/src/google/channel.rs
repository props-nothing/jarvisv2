//! The Calendar notification-channel push message: a header-only delivery, and the handshake that is not a
//! change.
//!
//! # Why this is a separate module from `pubsub`
//!
//! Google's two push mechanisms arrive in **completely different shapes**, and one module per mechanism is what
//! keeps each honest:
//!
//! | | Gmail (Cloud Pub/Sub) | Calendar (notification channels) |
//! | --- | --- | --- |
//! | body | a JSON `PubsubMessage` envelope | **zero length** (`Content-Length: 0`) |
//! | what names the resource | inside `message.data`, base64 | the `X-Goog-*` **headers** |
//! | authenticator | an RS256 JWT in `Authorization` | an echoed `X-Goog-Channel-Token` |
//!
//! [`crate::google::pubsub`] models the first. **Nothing modelled the second**, so a Calendar notification
//! could not be read at all — and that is the more surprising gap because `ADR-0099` has just taught the webhook
//! contract to *name* an echoed channel token as an authenticator while nothing read the header the token
//! arrives in. **A named authenticator with no reader is `ADR-0092`'s "a value with no reader" one level up**:
//! the contract can describe a control whose input the connector cannot yet parse.
//!
//! # The headers, and the one state that is not a change
//!
//! A notification is a `POST` with a **zero-length body** and these headers:
//!
//! | header | always? | meaning |
//! | --- | --- | --- |
//! | `X-Goog-Channel-ID` | yes | the channel id **you chose**, echoed back |
//! | `X-Goog-Resource-ID` | yes | an opaque, version-stable id for the watched resource |
//! | `X-Goog-Resource-URI` | yes | the version-specific URI of that resource |
//! | `X-Goog-Resource-State` | yes | `sync` \| `exists` \| `not_exists` |
//! | `X-Goog-Message-Number` | yes | an integer; **not sequential** |
//! | `X-Goog-Channel-Token` | **sometimes** | the value **you set**, echoed — the anti-spoofing control |
//! | `X-Goog-Channel-Expiration` | sometimes | a **human-readable** date, when the channel expires |
//!
//! **`sync` is not a change.** The guide says the API *"sends a `sync` message to indicate that notifications
//! are starting"* and that *"It's safe to ignore the `sync` notification"*. So the **first** message on a
//! channel is a handshake, not a resource update — the Calendar counterpart of the Gmail rule that a successful
//! `watch` *"immediately sends a notification, so the first delivery is not a change"* (`ADR-0092`). A caller
//! that acted on every delivery would therefore do one spurious read the moment it started watching, and
//! [`ChannelMessage::is_sync`] is the predicate that prevents it. The `sync` message can also arrive **before**
//! the `watch` response, so a caller must not assume it holds the channel metadata when the first message lands.
//!
//! # Two fields that look interchangeable and are not
//!
//! - **`X-Goog-Message-Number` is not a position.** The guide: numbers *"increase for each subsequent message on
//!   the channel, but they're not sequential"*, so it is a redelivery hint at best and never a cursor — the same
//!   restraint `crate::google::client` records for an opaque `nextSyncToken`. The `sync` message's number **is**
//!   always `1`, but that is a fact *about the sync message* and not a way to detect one: the state is the
//!   declared discriminator, and a caller keying on the number would be inferring from an accident.
//! - **`X-Goog-Channel-Expiration` is not epoch millis.** It is *"expressed in human-readable format"* (for
//!   example `Tue, 19 Nov 2013 01:13:52 GMT`) — the **opposite** of the Gmail watch lease, whose `expiration` is
//!   an epoch-millis **string** ([`crate::google::watch`]). So this module keeps the value as text rather than
//!   inventing a number, and the two expirations are deliberately read by different code: a shared `parse`
//!   would have to guess which of two contradictory encodings a value is in.
//!
//! # What is not verified
//!
//! **No notification has been received.** The header names, the three states, the zero-length body and the
//! message rules are transcribed from the dated Calendar push guide in `docs/research/integrations/google.md`.
//! And the `X-Goog-Channel-Token` is **surfaced, not verified**: comparing it against the value the connector
//! stored is a *verifier*, and building one is `P5-010`'s work (`ADR-0099`). This module's job is to make the
//! value readable, which is the precondition for any verifier at all.

use std::fmt;

use crate::webhook::WebhookDelivery;

/// The header carrying the channel id the connector chose. Always present.
pub const CHANNEL_ID_HEADER: &str = "x-goog-channel-id";

/// The header carrying the echoed channel token. Present only if the connector set one.
pub const CHANNEL_TOKEN_HEADER: &str = "x-goog-channel-token";

/// The header carrying the channel's expiration as a **human-readable date**. Present only if the channel
/// expires.
pub const CHANNEL_EXPIRATION_HEADER: &str = "x-goog-channel-expiration";

/// The header carrying an opaque, version-stable id for the watched resource. Always present.
pub const RESOURCE_ID_HEADER: &str = "x-goog-resource-id";

/// The header carrying the version-specific URI of the watched resource. Always present.
pub const RESOURCE_URI_HEADER: &str = "x-goog-resource-uri";

/// The header carrying the resource state that triggered the notification. Always present.
pub const RESOURCE_STATE_HEADER: &str = "x-goog-resource-state";

/// The header carrying the message number. Always present.
pub const MESSAGE_NUMBER_HEADER: &str = "x-goog-message-number";

/// What triggered a Calendar notification.
///
/// The three values the guide names, and the reason this is an enum rather than a free string: **the state is
/// how a caller tells a handshake from a change**, and a typo'd or unknown state must not be silently treated
/// as one or the other. [`ResourceState::Sync`] is the value that is *not* a change.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResourceState {
    /// The channel was created and notifications are starting. **Not a resource change**, and safe to ignore.
    Sync,
    /// The watched resource changed — *"the creation of a new resource, or the modification or deletion of an
    /// existing resource"*.
    Exists,
    /// The watched resource is reported as no longer existing.
    ///
    /// **A documented value whose precise semantics were not established by the page read.** It is neither the
    /// handshake nor an ordinary `exists`, so a caller that must choose acts on it rather than ignoring it: the
    /// fail-safe direction is a read that turns out to be unnecessary, not a change that is missed.
    NotExists,
}

impl ResourceState {
    /// Returns the stable wire spelling, as the header carries it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sync => "sync",
            Self::Exists => "exists",
            Self::NotExists => "not_exists",
        }
    }

    /// Parses the header's value.
    ///
    /// **Exact, case-sensitive matching**, because the three values are the provider's own tokens and a
    /// case-insensitive match would accept a spelling the guide never uses while silently tolerating a change
    /// in the provider's vocabulary. `None` for anything else, which the caller refuses rather than guessing a
    /// state.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "sync" => Some(Self::Sync),
            "exists" => Some(Self::Exists),
            "not_exists" => Some(Self::NotExists),
            _ => None,
        }
    }
}

/// Why a channel notification's headers could not be read.
///
/// Five variants, because the remedies differ and each points at a different layer: a missing header is the
/// provider sending less than it promised, an ambiguous one is a proxy or an attacker, a non-UTF-8 one is a
/// wire problem, and the two unreadable values point at *this connector's* vocabulary or parsing rather than
/// at the delivery's shape.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ChannelMessageError {
    /// A header the guide documents as always present was absent.
    #[error("a channel notification must carry the `{header}` header")]
    Missing {
        /// The header that was absent, so the refusal names the field to look at.
        header: &'static str,
    },
    /// A header arrived more than once.
    ///
    /// Refused rather than resolved, for the reason [`crate::webhook::WebhookDelivery::single_header`] refuses
    /// it: a delivery carrying two resource states or two channel ids is either a proxy doing what proxies do
    /// or an attacker trying to get one of them read, and picking one is the decision that lets the wrong value
    /// win.
    #[error("the delivery presented {count} values for `{header}`, and exactly one is required")]
    Ambiguous {
        /// The header.
        header: &'static str,
        /// How many values arrived.
        count: usize,
    },
    /// A header value was not valid UTF-8.
    #[error("the `{header}` header is not valid UTF-8")]
    NotUtf8 {
        /// The header.
        header: &'static str,
    },
    /// The resource state was not one this connector understands.
    #[error(
        "`{value}` is not a resource state this connector understands, so it cannot tell a handshake from a \
         change"
    )]
    UnknownResourceState {
        /// The value that arrived.
        value: String,
    },
    /// The message number was not a whole number.
    #[error("the message number `{value}` is not a whole number")]
    UnreadableMessageNumber {
        /// The value that arrived.
        value: String,
    },
}

/// A Calendar notification-channel message, read from its headers.
///
/// # The token and the expiration are private, and the `Debug` redacts the token
///
/// `X-Goog-Channel-Token` is a value the **connector** chose and the provider echoes, and it is the
/// anti-spoofing control for the channel — so a value that reaches a log line is a value an attacker could
/// replay into a forged delivery. It is therefore reached through [`Self::channel_token`] rather than a public
/// field, and the hand-written `Debug` prints a marker where a derived one would print it (`ADR-0091`). The
/// `expiration` is a date and names no secret, so it is a public field.
#[derive(Clone, Eq, PartialEq)]
pub struct ChannelMessage {
    /// The channel id the connector chose when it registered the watch.
    pub channel_id: String,
    /// An opaque, version-stable id for the watched resource.
    pub resource_id: String,
    /// The version-specific URI of the watched resource.
    pub resource_uri: String,
    /// What triggered the notification.
    pub resource_state: ResourceState,
    /// The message number. **Not sequential, and not a position** — see the module doc.
    pub message_number: u64,
    /// The channel's expiration, as the provider wrote it, when the channel expires.
    pub expiration: Option<String>,
    /// The echoed channel token, when the connector set one.
    channel_token: Option<String>,
}

impl ChannelMessage {
    /// Returns whether this is the channel's `sync` handshake rather than a resource change.
    ///
    /// The predicate a caller branches on before doing any work, and it is true only for
    /// [`ResourceState::Sync`] — so the one message that is *not* a change is not the default a caller falls
    /// into. The guide says it is safe to ignore, and the cost of not ignoring it is a read that starts a
    /// mailbox-worth of work the moment a channel is created.
    #[must_use]
    pub const fn is_sync(&self) -> bool {
        matches!(self.resource_state, ResourceState::Sync)
    }

    /// Returns the echoed channel token, when the delivery carried one.
    ///
    /// **Surfaced rather than compared**, deliberately: comparing it against the stored value is a *verifier*,
    /// and a verifier needs the value the connector registered, which this crate does not hold. Returning it
    /// makes a verifier possible; it does not pretend one exists (`ADR-0099`).
    #[must_use]
    pub fn channel_token(&self) -> Option<&str> {
        self.channel_token.as_deref()
    }
}

impl fmt::Debug for ChannelMessage {
    /// Redacts `channel_token`, which authenticates the channel, and keeps the rest.
    ///
    /// The token is the anti-spoofing control, so a `{:?}` that printed it would put a value an attacker could
    /// replay into a log line — the `ADR-0091` shape. The **length** is printed instead, because "a token
    /// arrived, of this size" is the useful part of a diagnostic and names nothing an attacker can use.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ChannelMessage")
            .field("channel_id", &self.channel_id)
            .field("resource_id", &self.resource_id)
            .field("resource_uri", &self.resource_uri)
            .field("resource_state", &self.resource_state)
            .field("message_number", &self.message_number)
            .field("expiration", &self.expiration)
            .field(
                "channel_token",
                &self
                    .channel_token
                    .as_ref()
                    .map(|token| format!("[REDACTED], {} chars", token.len())),
            )
            .finish()
    }
}

/// Reads a header the guide documents as always present.
fn required_header<'a>(
    delivery: &WebhookDelivery<'a>,
    header: &'static str,
) -> Result<&'a str, ChannelMessageError> {
    read_header(delivery, header)?.ok_or(ChannelMessageError::Missing { header })
}

/// Reads a header that may be absent, returning `None` when it is.
fn optional_header<'a>(
    delivery: &WebhookDelivery<'a>,
    header: &'static str,
) -> Result<Option<&'a str>, ChannelMessageError> {
    read_header(delivery, header)
}

/// Reads exactly one UTF-8 value for a header.
///
/// # Why this does not use [`WebhookDelivery::single_header`]
///
/// `single_header` returns `None` for **absent, ambiguous and non-UTF-8** alike, which collapses three
/// different defects into one answer. A caller here must tell them apart — an absent header is a provider
/// sending less than documented, an ambiguous one is a wire-level attack, and a non-UTF-8 one is an encoding
/// problem — so this reads the raw values and separates the cases, which is the same reason
/// [`crate::webhook::SignatureError::AmbiguousHeader`] exists beside `single_header`.
fn read_header<'a>(
    delivery: &WebhookDelivery<'a>,
    header: &'static str,
) -> Result<Option<&'a str>, ChannelMessageError> {
    let values = delivery.header_values(header);
    match values.as_slice() {
        [] => Ok(None),
        // A single value: decode it, or report an encoding fault rather than an absence.
        [value] => std::str::from_utf8(value)
            .map(Some)
            .map_err(|_| ChannelMessageError::NotUtf8 { header }),
        // Two or more: refused, because picking one is the decision that lets a wrong value win.
        many => Err(ChannelMessageError::Ambiguous {
            header,
            count: many.len(),
        }),
    }
}

/// Reads a Calendar notification-channel message from a delivery's headers.
///
/// # The body is not read, and that is the message
///
/// The delivery has a **zero-length body**, so there is nothing to parse and nothing to MAC — which is exactly
/// why `ADR-0099` had to add an authenticator that does not cover the body. This function takes the
/// [`WebhookDelivery`] (so a caller can pass a body of any length) and reads **only headers**, so an
/// accidental body cannot change the result.
///
/// # Errors
///
/// Returns [`ChannelMessageError`] for a missing or ambiguous header, a non-UTF-8 value, an unknown resource
/// state, or an unreadable message number. Each names the specific defect, because a single "bad notification"
/// would leave a caller unable to tell a provider shape change from a wire attack from a local parsing bug.
pub fn parse_channel_message(
    delivery: &WebhookDelivery<'_>,
) -> Result<ChannelMessage, ChannelMessageError> {
    let channel_id = required_header(delivery, CHANNEL_ID_HEADER)?.to_owned();
    let resource_id = required_header(delivery, RESOURCE_ID_HEADER)?.to_owned();
    let resource_uri = required_header(delivery, RESOURCE_URI_HEADER)?.to_owned();
    let state_text = required_header(delivery, RESOURCE_STATE_HEADER)?;
    // The state is matched to the closed vocabulary **before** the message is built, so an unknown state is a
    // refusal rather than a value a caller has to interpret — a state this connector cannot name is a message
    // it cannot decide whether to act on, and guessing would be the fail-open direction.
    let resource_state = ResourceState::parse(state_text).ok_or_else(|| {
        ChannelMessageError::UnknownResourceState {
            value: state_text.to_owned(),
        }
    })?;
    let number_text = required_header(delivery, MESSAGE_NUMBER_HEADER)?;
    let message_number =
        number_text
            .parse::<u64>()
            .map_err(|_| ChannelMessageError::UnreadableMessageNumber {
                value: number_text.to_owned(),
            })?;
    // The two optional headers: present only when the connector set a token and only while the channel has an
    // expiration. Absent is a normal shape, which is why it is not an error here.
    let channel_token = optional_header(delivery, CHANNEL_TOKEN_HEADER)?.map(str::to_owned);
    let expiration = optional_header(delivery, CHANNEL_EXPIRATION_HEADER)?.map(str::to_owned);
    Ok(ChannelMessage {
        channel_id,
        resource_id,
        resource_uri,
        resource_state,
        message_number,
        expiration,
        channel_token,
    })
}

#[cfg(test)]
#[path = "channel_tests.rs"]
mod tests;
