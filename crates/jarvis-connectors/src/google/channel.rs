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
//! The `X-Goog-Channel-Token` is **verified as well as surfaced** ([`verify_channel_token`], `ADR-0101`): the
//! connector stores the value it registered, [`verify_channel_token`] compares it in constant time, and the
//! four situations an absent or mismatched token can produce are **named** rather than collapsed into a
//! `bool`. And a verified delivery is **routed to the account** whose channel it names
//! ([`route_channel`], `ADR-0102`), so the sync it triggers knows whose credential to use. What stays
//! unverified is anything *live* — no channel has been registered and no notification received — so the
//! comparison and the route are proved against values this crate builds.

use std::fmt;

use crate::account::AccountReference;
use crate::auth::SecretValue;
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

/// The maximum channel token **Google** accepts, in octets, stated rather than enforced.
///
/// The guide gives *"Maximum length: 256 characters"* for the `token` property a `watch` request carries, so a
/// conforming provider never sends a longer value back. **Nothing in this crate needs to enforce it, which is
/// why this is a stated fact and not a guard:** the verification below compares a candidate against the stored
/// token, and that comparison already rejects a different-length candidate in constant time — so no input can
/// reach a walk of an attacker-sized value. A check that rejected an over-long value "before the comparison"
/// would be redundant *and* would itself be an unbounded walk, which is why there is none (`ADR-0101`).
pub const MAX_CHANNEL_TOKEN_BYTES: usize = 256;

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
    /// **The reader, not the check.** This returns whatever the delivery carried, so it is deliberately *not* a
    /// statement that the value is the one the connector registered — that comparison is [`verify_channel_token`],
    /// which a caller must run before acting. This accessor exists so the verifier (and a test) can reach the
    /// value without the field being public, and the value can still be presented for comparison without ever
    /// being printed (`ADR-0091`).
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

/// What verifying a delivery's channel token concluded.
///
/// # Why four variants and not a `bool`, and why the pair is not two
///
/// A `bool` could answer "did it match", but the four situations differ in what they mean and what a caller
/// should do — and, read as a `bool`, two of them (`Absent` and `Mismatch`) collapse into "false" even though
/// one is a missing control and one is a failed comparison.
///
/// **The four are two pairs, and only one member of each pair is a refusal**, which is the distinction worth
/// naming rather than a `bool`:
///
/// - [`Verified`](Self::Verified) — the delivery carried a token that matches the stored one.
/// - [`Absent`](Self::Absent) — **not a refusal**: the connector registered the channel **without** a token, so
///   the guide's *"Only present if defined"* applies and a delivery without one is the documented shape. There
///   is simply no control to check.
/// - [`Mismatch`](Self::Mismatch) — **a refusal**: the channel is watched with a token, and this delivery's
///   token does not match it, which is what a forged or misrouted delivery looks like.
/// - [`TokenRequired`](Self::TokenRequired) — **a refusal**: the channel is watched with a token and the
///   delivery presented none, so it cannot prove it is for this channel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChannelTokenCheck {
    /// The delivery carried a token, and it matches the value the connector stored for the channel.
    Verified,
    /// The connector registered the channel with no token, so there is no control to check.
    ///
    /// **Not a refusal**, and reachable only because the guide makes the token optional: *"Only present if
    /// defined"*. A delivery that carries no token for such a channel is the documented shape, not a failure.
    Absent,
    /// The channel is watched with a token, and this delivery's token does not match it.
    Mismatch,
    /// The channel is watched with a token, and the delivery presented none.
    ///
    /// Distinct from [`Self::Mismatch`] because the two describe different wire facts — a wrong value versus
    /// no value — and from [`Self::Absent`] because a control **is** configured here. The remedy is the same
    /// as a mismatch (refuse), which is why both are refusals, but a diagnostic that said "mismatch" for a
    /// delivery carrying no token at all would send an operator to look for a wrong value that was never sent.
    TokenRequired,
}

impl ChannelTokenCheck {
    /// Returns whether the delivery may be acted on.
    ///
    /// True for [`Self::Verified`] and [`Self::Absent`] **only** — the two states in which the delivery did
    /// not fail a control: one passed it, and the other had none to fail. Both refusals answer `false`.
    #[must_use]
    pub const fn may_be_acted_on(self) -> bool {
        matches!(self, Self::Verified | Self::Absent)
    }

    /// Returns whether this is a refusal rather than an accepted delivery.
    ///
    /// The complement of [`Self::may_be_acted_on`], named because a caller wiring an alert needs the negative
    /// reading: a [`Self::Mismatch`] or a [`Self::TokenRequired`] is either an attacker or a broken deployment
    /// and deserves attention, while [`Self::Absent`] is a channel the operator chose not to protect and must
    /// not page anyone — the distinction `WebhookRejection::indicates_an_authenticity_failure` draws.
    #[must_use]
    pub const fn is_rejection(self) -> bool {
        !self.may_be_acted_on()
    }
}

/// Verifies a delivery's echoed channel token against the value the connector stored for its channel.
///
/// # The comparison is constant-time, and it is the one control on this path
///
/// A Calendar delivery has a **zero-length body**, so there is no MAC to check — the same fact that made
/// `ADR-0099` widen the webhook contract for a body-independent authenticator. The echoed token is therefore
/// the **only** thing that distinguishes a delivery Google sent for a channel this connector created from one
/// anyone who knows the endpoint could post. So the comparison must not short-circuit: a byte-at-a-time early
/// return leaks the stored token's prefix, and the stored token *is* the secret. This uses
/// [`SecretValue::matches`], the same constant-time routine the OAuth `state` is compared with (`P5-002`),
/// rather than a `==` — one implementation, one place to audit.
///
/// # The bound is Google's, and it is not enforced here on purpose
///
/// The guide documents a maximum channel-token length ([`MAX_CHANNEL_TOKEN_BYTES`]), but this function does
/// **not** check it — because the comparison below already refuses any candidate of a different length, in
/// constant time, before walking it. A separate "is the candidate too long" guard would be **redundant** and
/// would itself scan the attacker-supplied value, so it would add work without adding a refusal. This is the
/// `ADR-0066` rule the other way round: not a guard that can never fire, but a guard that can never *decide*
/// anything the comparison has not already decided.
///
/// # What this does not check
///
/// **Nothing about the resource.** That a delivery names a resource this channel watches — and not merely a
/// channel this connector registered — is a separate control (`security.md`'s "wrong endpoint" row), and this
/// function answers only the token question. [`ChannelTokenCheck::Absent`] says *no control was present*, not
/// *the resource was verified*.
#[must_use]
pub fn verify_channel_token(
    stored: Option<&SecretValue>,
    delivery: &ChannelMessage,
) -> ChannelTokenCheck {
    match stored {
        // No token was registered for the channel, so there is nothing to compare. The guide makes the token
        // optional, so this is the documented case rather than a failure to authenticate.
        None => ChannelTokenCheck::Absent,
        Some(expected) => match delivery.channel_token() {
            // A token was registered but the delivery carried none: it cannot prove it is for this channel, so
            // it is refused rather than read as "no control". This is the arm a `bool` would have hidden.
            None => ChannelTokenCheck::TokenRequired,
            // `matches` is constant-time with respect to content and refuses a different-length candidate
            // immediately, so an over-long delivery is a `Mismatch` without a walk of the candidate. See the
            // doc above for why there is no separate bound check.
            Some(presented) if expected.matches(presented) => ChannelTokenCheck::Verified,
            Some(_) => ChannelTokenCheck::Mismatch,
        },
    }
}

/// A notification channel a connector **registered for one account**, and the token it set.
///
/// # Why the account and the channel are one value and not two
///
/// A Calendar notification names the **channel** (`X-Goog-Channel-ID`) and never the account — the account is
/// whatever the `watch` call was authenticated as. So attributing a delivery to an account is a **join**
/// against what the connector registered, and that join must not be reconstructable from parts: a caller
/// holding a channel id and an account reference *separately* could pair one account with another's channel
/// and route a delivery to the wrong mailbox with nothing able to notice. This type is the pair, so a route
/// cannot pair them wrongly — the same argument `ADR-0097` makes for taking `&[VerifiedAccount]` rather than a
/// list of `(reference, address)` tuples.
///
/// # The token is optional, and its absence is a *recorded choice*
///
/// The guide makes the channel token optional, so `token: None` means the connector registered this channel
/// **without** one — a deliberate configuration, not a missing value. That is why the field is an `Option`
/// rather than a [`SecretValue`] with a sentinel: [`verify_channel_token`] reads `None` as
/// [`ChannelTokenCheck::Absent`] (not a refusal), and a caller cannot express "I forgot the token" as distinct
/// from "there is none".
#[derive(Clone)]
pub struct ChannelRegistration {
    /// The channel id the connector chose for this watch. The value a delivery echoes back.
    pub channel_id: String,
    /// The account whose credential created the channel.
    pub account: AccountReference,
    /// The token the connector set, when it set one. Redacted by the hand-written `Debug`.
    token: Option<SecretValue>,
}

impl ChannelRegistration {
    /// Records a registered channel.
    #[must_use]
    pub const fn new(
        channel_id: String,
        account: AccountReference,
        token: Option<SecretValue>,
    ) -> Self {
        Self {
            channel_id,
            account,
            token,
        }
    }

    /// Returns the token the connector set, when it set one.
    ///
    /// `None` means the channel was registered **without** a token — the guide's optional case — so this is
    /// the value [`verify_channel_token`] reads to decide between a control to check and none. Reached through
    /// an accessor rather than a public field so the token never sits in a caller's `{:?}` of the registration.
    #[must_use]
    pub const fn token(&self) -> Option<&SecretValue> {
        self.token.as_ref()
    }
}

impl fmt::Debug for ChannelRegistration {
    /// Redacts `token`, which is the channel's anti-spoofing control (`ADR-0091`).
    ///
    /// The channel id and the account are kept, because they name no secret and are what a diagnostic about a
    /// stray delivery needs to show; a token would be a value an attacker could replay.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ChannelRegistration")
            .field("channel_id", &self.channel_id)
            .field("account", &self.account)
            .field(
                "token",
                &self
                    .token
                    .as_ref()
                    .map(|token| format!("[REDACTED], {} chars", token.expose().len())),
            )
            .finish()
    }
}

/// Which account a Calendar notification's channel belongs to.
///
/// # Why this is an enum and not `Option<AccountReference>`
///
/// `Option` has two states and this decision has **three**, and the two non-matches call for different
/// operator action — the same reason [`crate::google::routing::DeliveryRoute`] is an enum rather than an
/// `Option` (`ADR-0097`). Collapsing [`Self::Unknown`] and [`Self::Ambiguous`] into `None` would make a
/// channel the connector **never registered** indistinguishable from a **registration collision**, and only
/// one of those is a defect in this connector.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ChannelRoute {
    /// Exactly one registration names this channel.
    ///
    /// The only route that may be applied without a person, and the reference is carried so the caller does not
    /// have to search again — a second lookup would be a second place the comparison is decided.
    Exact(AccountReference),
    /// More than one registration carries this channel id.
    ///
    /// Reachable when a channel id is reused — the guide *recommends* a UUID precisely so it is unique — and
    /// **undecidable** without a person: picking one registration would sync one account under another's
    /// identity. A count rather than a selection, for the reason [`crate::google::routing::DeliveryRoute`]
    /// carries counts.
    Ambiguous {
        /// How many registrations carry this channel id.
        accounts: usize,
    },
    /// No registration names this channel.
    ///
    /// **Not an error.** A delivery for a channel this connector never registered — a watch established by a
    /// previous profile, or one forged by anyone who knows the endpoint — is answered by acknowledging and
    /// recording, not by a fault, and the token cannot even be checked because there is no stored value to
    /// check it against.
    Unknown,
}

impl ChannelRoute {
    /// Returns the account this route names, when it names exactly one.
    ///
    /// `Some` for [`Self::Exact`] alone, so no accessor returns an arbitrarily chosen account from an ambiguous
    /// route.
    #[must_use]
    pub const fn account(&self) -> Option<&AccountReference> {
        match self {
            Self::Exact(reference) => Some(reference),
            Self::Ambiguous { .. } | Self::Unknown => None,
        }
    }

    /// Returns whether this route may be applied **without a person's decision**.
    ///
    /// True only for [`Self::Exact`], named for the authority it grants rather than for a match quality.
    #[must_use]
    pub const fn may_be_applied_automatically(&self) -> bool {
        matches!(self, Self::Exact(_))
    }
}

/// Routes a Calendar notification to the account whose registration names its channel.
///
/// # The binding is by channel **id**, which is the connector's own value
///
/// A Calendar delivery carries no account — the account is whatever the `watch` call was authenticated as — so
/// the only stable link from a delivery back to an account is the **channel id**, which the connector
/// *chose*. That is the opposite of [`crate::google::routing::route_delivery`], which matches a
/// **provider-supplied** `emailAddress`, and the difference is worth stating: an id the connector generated is
/// its own value, so a match is a lookup in its own records rather than a comparison against untrusted text.
/// (The id is still *echoed* through an unauthenticated channel and so remains untrusted input — it selects,
/// and never authorises: the credential it selects is the account's own, and the token check is separate.)
///
/// # Why `registrations` is a slice of [`ChannelRegistration`] and not a `(id, account)` list
///
/// For the reason [`ChannelRegistration`] itself exists: the id and the account must belong to the same
/// registration, and separate lists would let a caller pair them wrongly. The type that already binds them is
/// the argument.
///
/// # Why the empty case is `Unknown`
///
/// A connector with no registered channels has nothing to route to, which is the same answer as a channel
/// matching none — and the caller's action is the same for both: acknowledge and record. A separate "not
/// configured" variant would be a state the caller cannot act on differently, the same restraint
/// `route_delivery` records.
#[must_use]
pub fn route_channel(
    message: &ChannelMessage,
    registrations: &[ChannelRegistration],
) -> ChannelRoute {
    let named = message.channel_id.as_str();
    let mut matched: Option<&AccountReference> = None;
    let mut count: usize = 0;
    for registration in registrations {
        // **Byte-exact**, because the channel id is the connector's own value and it is compared against the
        // value it stored. There is no case-folding: a channel id is an identifier, not an address with an
        // ambiguous case rule, so a case variant is a different id — the opposite of the `emailAddress`
        // near-match `ADR-0097` reports, and both are "compare the field as the provider sent it".
        if registration.channel_id == named {
            count += 1;
            // Kept only for the single-match case; a second match makes the route `Ambiguous`, so retaining a
            // second reference would be a selection nothing uses.
            if matched.is_none() {
                matched = Some(&registration.account);
            }
        }
    }
    match (count, matched) {
        (1, Some(reference)) => ChannelRoute::Exact(reference.clone()),
        (n, _) if n > 1 => ChannelRoute::Ambiguous { accounts: n },
        (_, _) => ChannelRoute::Unknown,
    }
}

#[cfg(test)]
#[path = "channel_tests.rs"]
mod tests;
