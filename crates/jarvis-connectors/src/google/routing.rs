//! Routing a push delivery to the account it names — and the trust that limits it.
//!
//! # The gap this module closes
//!
//! A decoded [`PubsubNotification`] carries two fields: an **`emailAddress`** and a `historyId`. The push path
//! can decode it, [`crate::google::watch`] can read the lease it renews, and
//! [`crate::google::pubsub::decide_acknowledgement`] can decide how to answer it. **Nothing connected the
//! address to one of the connector's own accounts**, so a delivery said "a mailbox changed" with no way to
//! learn *which of yours* — and the sync it triggers has to run through a specific account's stored credential.
//! A notification that cannot be attributed is a notification that cannot be acted on.
//!
//! # The trust that bounds it, and why the address selects rather than authorises
//!
//! **The delivery is not authenticated *by this crate*, and this record says so.** `P5-004`'s Finding 1
//! establishes that neither Google mechanism is a MAC over the body: Gmail's push is an **OIDC bearer JWT**
//! and Calendar's is an **echoed channel token** over a zero-length body. The webhook contract can now **name**
//! both ([`crate::webhook::SignatureAlgorithm::OidcIdToken`] and
//! [`crate::webhook::SignatureAlgorithm::EchoedChannelToken`]), which was the contract gap Finding 1 recorded —
//! but naming an authenticator is not verifying one, and **no verifier is built**: there is no JWKS reader for
//! the JWT and no stored channel token to compare against. So a delivery reaching this function has not been
//! authenticated, and that is why the rules below are shaped the way they are rather than because the type
//! could not describe the scheme.
//!
//! **A consequence that must not be skipped: the address in the payload is untrusted input.** It is a string a
//! caller of the endpoint supplies, so a routing decision that treated it as authority would let a forged
//! delivery choose which account is synced. Two properties keep that from being the whole story, and both are
//! load-bearing:
//!
//! 1. **The route selects a mailbox to *read*, never a credential to use.** The sync runs through the account's
//!    own stored token, so a forged delivery cannot reach a mailbox the connector was not already authorised to
//!    read. Its worst case is a **spurious sync of an account the attacker already knew about**.
//! 2. **The notified `historyId` is not a position the sync trusts.** The read that follows is
//!    `history.list` from the connector's **stored** cursor, and `advance_gmail_history` refuses a marker that
//!    moves backwards — so a forged id that is too *high* cannot skip changes, because `history.list` returns
//!    everything after the stored position regardless of the id the notification named. A forged id is
//!    therefore not a way to make the connector *miss* mail, which is the failure that would matter.
//!
//! So the honest statement is: routing decides **which known account to read**, and authorization was settled
//! when the account was connected. That is why [`DeliveryRoute::Exact`] is the only route applied without a
//! person, and why nothing here returns a credential.
//!
//! # The three ways a delivery does not route, and none of them is retried
//!
//! [`DeliveryRoute::Unknown`] and [`DeliveryRoute::Ambiguous`] are the cases worth naming:
//!
//! - **`Unknown`** — no connected account has this address. A delivery for a mailbox that was disconnected, or
//!   one forged for an address the connector never held.
//! - **`Ambiguous`** — more than one account record carries the same address. Reachable when a reconnect mints
//!   a new [`AccountReference`] without retiring the old row.
//! - **`CaseDiffers`** — no byte-exact match, but some account's address matches when case is ignored.
//!
//! None is repaired by another attempt, so all three acknowledge the delivery rather than refusing it — and
//! that is not a shrug: `crate::google::pubsub`'s finding is that a negative acknowledgement triggers a
//! **subscription-global** backoff for up to 60 seconds, so refusing a delivery that will never become
//! routable would slow **every other account on the subscription** ([`crate::google::pubsub::DeliveryAck`],
//! `ADR-0094`). [`Self::unroutable_acknowledgement`] is that conclusion applied.
//!
//! # What is not verified
//!
//! **No delivery has been received and no account has been connected.** The types are the connector's own
//! (`AccountReference`, `VerifiedAccount`); every rule here is a decision about them rather than a transcription
//! of a provider fact, which is why each one is argued rather than cited.

use crate::account::{AccountReference, VerifiedAccount};
use crate::google::pubsub::{
    DeliveryAck, PubsubDeliveryError, PubsubNotification, PubsubNotificationError, parse_delivery,
};

/// Which of the connector's known accounts a push delivery names.
///
/// # Why this is four variants rather than an `Option`
///
/// `Option<AccountReference>` has two states and this decision has **four**: exactly one match, several, a
/// near-match that differs only in case, and none. Collapsing the middle two into `None` is the defect this
/// type removes — a reader of `None` could not tell "this is not my account" from "this looks like my account
/// but the spelling differs", and the two call for different operator action.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeliveryRoute {
    /// **Exactly one** known account's address matches the notification's **byte for byte**.
    ///
    /// The only route that may be applied without a person, and the reference is carried so the caller does not
    /// have to search again — a second lookup would be a second place the comparison is decided.
    Exact(AccountReference),
    /// More than one known account record carries this address.
    ///
    /// **A count and not a selection, deliberately.** Picking one — the first, the oldest, the most recently
    /// verified — would be an arbitrary choice that decides which mailbox is read, and a wrong pick syncs one
    /// mailbox's changes under another account's identity. The count is what a caller needs to report the
    /// condition; the choice is a person's.
    Ambiguous {
        /// How many accounts carry this address.
        accounts: usize,
    },
    /// No account matches byte for byte, and some account matches when case is ignored.
    ///
    /// **Not promoted to a match, and that is the decision.** Email addresses are case-insensitive in their
    /// domain and, in practice, in their local part too — so a provider that rendered `User@example.com` where
    /// the stored address is `user@example.com` is *probably* naming the same mailbox. But "probably" is not a
    /// rule this connector can verify: Google publishes no canonicalisation statement for the `emailAddress` in
    /// a push payload, and if the two spellings are genuinely two accounts then auto-applying would read the
    /// wrong mailbox. So the near-match is **reported rather than taken**, and a person decides.
    ///
    /// A count rather than a reference, for the same reason as [`Self::Ambiguous`]: several accounts could
    /// differ from the notification only in case, and choosing one would be the arbitrary pick.
    CaseDiffers {
        /// How many accounts match when case is ignored.
        accounts: usize,
    },
    /// No known account carries this address in any spelling.
    ///
    /// **Not an error**, and the distinction matters: a delivery for a mailbox that was disconnected is a
    /// normal consequence of a teardown, and one for an address the connector never held is what a forged
    /// delivery looks like. Both are answered the same way — acknowledge and record — and neither is a fault in
    /// this module.
    Unknown,
}

impl DeliveryRoute {
    /// Returns the account this route names, when it names exactly one.
    ///
    /// `Some` for [`Self::Exact`] alone. Both other informative variants are counts, so there is deliberately
    /// no accessor that could return an arbitrarily chosen account from them.
    #[must_use]
    pub const fn account(&self) -> Option<&AccountReference> {
        match self {
            Self::Exact(reference) => Some(reference),
            Self::Ambiguous { .. } | Self::CaseDiffers { .. } | Self::Unknown => None,
        }
    }

    /// Returns whether this route may be applied **without a person's decision**.
    ///
    /// True only for [`Self::Exact`]. The predicate a scheduler calls before acting on a delivery, named for
    /// the authority it grants rather than for a match quality, so a caller reads "may I act" rather than "did
    /// it match".
    #[must_use]
    pub const fn may_be_applied_automatically(&self) -> bool {
        matches!(self, Self::Exact(_))
    }

    /// Returns how many accounts this route found, when it found any.
    ///
    /// `None` for [`Self::Unknown`], because zero is the absence of a count rather than a count of zero — the
    /// same distinction `Option` exists for, and the one a caller reporting "which accounts matched" needs.
    #[must_use]
    pub const fn matched_accounts(&self) -> Option<usize> {
        match self {
            Self::Exact(_) => Some(1),
            Self::Ambiguous { accounts } | Self::CaseDiffers { accounts } => Some(*accounts),
            Self::Unknown => None,
        }
    }

    /// Returns what to answer the delivery **when routing is the failure**, and `None` when routing succeeded.
    ///
    /// # Why `None` for `Exact` rather than `Some(Accept)`
    ///
    /// An exact route means the delivery *can* be processed, not that it *was*. This function reports the
    /// answer for a delivery this module has rejected; answering `Accept` for a routable one would claim the
    /// sync had already happened. The caller answers `Accept` after its own work, through
    /// [`crate::google::pubsub::decide_acknowledgement`].
    ///
    /// # Why every unroutable case abandons rather than refuses
    ///
    /// None of the three is repaired by another attempt: the account set is a local fact, so an address that
    /// matches nothing today matches nothing on the next delivery either. Refusing would therefore be a
    /// negative acknowledgement that never becomes a positive one, and `crate::google::pubsub`'s own finding is
    /// that the cost of that lands on **every other mailbox on the subscription** — up to 60 seconds of
    /// push backoff per refusal, which the subscriber cannot disable. So the delivery is acknowledged and the
    /// drop recorded, which is [`DeliveryAck::AbandonAndAcknowledge`]'s exact purpose (`ADR-0094`).
    #[must_use]
    pub const fn unroutable_acknowledgement(&self) -> Option<DeliveryAck> {
        match self {
            Self::Exact(_) => None,
            Self::Ambiguous { .. } | Self::CaseDiffers { .. } | Self::Unknown => {
                Some(DeliveryAck::AbandonAndAcknowledge)
            }
        }
    }
}

/// Routes a push delivery to one of the connector's known accounts by the address it names.
///
/// # The comparison, in order, and each step's reason
///
/// 1. **Byte-exact against every account's provider identifier.** This is the only comparison that needs no
///    assumption about the provider: the stored value and the notified value are compared as the strings they
///    are. One match routes; more than one is [`DeliveryRoute::Ambiguous`].
/// 2. **Case-insensitive, only if step 1 found nothing.** Reported as [`DeliveryRoute::CaseDiffers`] and never
///    promoted to a match, because Google publishes no canonicalisation statement for this field — see
///    [`DeliveryRoute::CaseDiffers`].
/// 3. **[`DeliveryRoute::Unknown`]** otherwise.
///
/// # Why `accounts` is a slice of `VerifiedAccount` rather than addresses
///
/// The address and the reference must belong to **the same account**, and `VerifiedAccount` is the type that
/// holds both — a `&[(AccountReference, String)]` argument would let a caller pair one account's reference with
/// another's address, which would route a delivery to the wrong mailbox with nothing able to notice. The type
/// that already binds them is the argument.
///
/// # Why the empty case is `Unknown` rather than a refusal
///
/// A connector with no connected accounts has nothing to route to, which is the same answer as an address
/// matching none of them — and the caller's action is the same for both: acknowledge and record. A separate
/// variant for "not configured" would be a state the caller cannot act on differently.
#[must_use]
pub fn route_delivery(
    notification: &PubsubNotification,
    accounts: &[VerifiedAccount],
) -> DeliveryRoute {
    let notified = notification.email_address.as_str();
    let mut exact: Option<&AccountReference> = None;
    let mut exact_count: usize = 0;
    let mut case_matches: usize = 0;
    for account in accounts {
        let known = account.provider_account_id();
        if known == notified {
            exact_count += 1;
            // Kept only for the single-match case; a second match makes the route `Ambiguous`, so retaining
            // the first would be a selection nothing uses.
            if exact.is_none() {
                exact = Some(account.reference());
            }
        }
        // Counted regardless of whether an exact match was found, because the case-differing count is only
        // consulted when there was none — and computing it during the same pass keeps one loop rather than two.
        if known.eq_ignore_ascii_case(notified) {
            case_matches += 1;
        }
    }
    match (exact_count, exact) {
        (1, Some(reference)) => DeliveryRoute::Exact(reference.clone()),
        (n, _) if n > 1 => DeliveryRoute::Ambiguous { accounts: n },
        // No exact match: report a case-only near-match if there is one, without applying it.
        (_, _) if case_matches > 0 => DeliveryRoute::CaseDiffers {
            accounts: case_matches,
        },
        (_, _) => DeliveryRoute::Unknown,
    }
}

/// The two ways a push body could not be read, kept apart because they name different layers.
///
/// [`crate::google::pubsub::parse_delivery`] already separates the Pub/Sub **envelope** from the Gmail
/// **payload** inside it, and this type preserves that split rather than flattening it into one string: a
/// caller debugging a refusal needs to know whether the delivery's wrapper or the payload it carried was
/// wrong, and a single "bad body" message would send it to the wrong one.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GmailBodyError {
    /// The Pub/Sub envelope was not the documented wrapped shape.
    Envelope(PubsubDeliveryError),
    /// The envelope was fine and the Gmail payload inside it was not.
    Payload(PubsubNotificationError),
}

/// What ingesting one Gmail notification concluded.
///
/// # Why there is no `Handshake` here, and why that is the finding
///
/// The Calendar mechanism has a `Handshake` outcome (`ADR-0103`) because its opening message is **marked**: the
/// guide defines a `sync` state a caller may *"safely ignore"* (`ADR-0100`). **Gmail has no such marker.** Its
/// guide says a successful `watch` *"immediately sends a notification"*, and that notification is an ordinary
/// one — the same `{emailAddress, historyId}` payload a real change produces. So **Gmail's start-of-notifications
/// notification is indistinguishable from a change**, and a type with a `Handshake` variant would claim a
/// distinction the wire does not carry. This is the deliberate divergence from [`ChannelIngest`]: a value shaped
/// to have a handshake would invite a caller to detect one.
///
/// # Why this is not a `bool`
///
/// A `bool` ("acknowledged or not") erases the difference between **a delivery that failed to parse** and **a
/// delivery routed to a mailbox this connector does not hold**, which need different diagnostics and different
/// operator action — a broken envelope is a wire fault, while an unknown address is a stray or forged delivery.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GmailIngest {
    /// The body could not be read into a Pub/Sub delivery carrying a Gmail payload.
    ///
    /// Carries the layer that failed. A delivery this connector cannot read is **acknowledged and recorded**, so
    /// it is not redelivered forever.
    Unreadable(GmailBodyError),
    /// The payload named an address none of the connector's accounts hold, or several do.
    ///
    /// The route is carried because [`DeliveryRoute::CaseDiffers`] and [`DeliveryRoute::Ambiguous`] are
    /// **actionable signals** a caller must surface — two accounts for one address is a registration defect, and
    /// a case-only near-match may be one mailbox, so **neither may be applied automatically**.
    Unroutable(DeliveryRoute),
    /// The payload named exactly one account, so a sync is warranted.
    ///
    /// Carries the `historyId` the delivery stated (which a sync uses as an **upper bound**, never as a
    /// position it trusts) and the **message id** because Pub/Sub is **at-least-once**: the same message can
    /// arrive twice, and `messageId` — the only field that distinguishes a redelivery from a new change — is
    /// what a caller deduplicates on (`ADR-0094`).
    Changed {
        /// The account whose stored credential syncs the mailbox.
        account: AccountReference,
        /// The mailbox position the delivery stated, as an upper bound for the sync.
        history_id: String,
        /// The Pub/Sub message id: the **deduplication key** for an at-least-once delivery.
        ///
        /// `None` when the delivery carried no `messageId`. **Absent is not the same as absent** — a caller that
        /// needs a dedupe key must treat a missing one as *"this delivery may be a repeat I cannot detect"*,
        /// which is why the field is an `Option` rather than a defaulted string.
        message_id: Option<String>,
    },
}

impl GmailIngest {
    /// The answer to send the subscription, so the delivery is acknowledged or redelivered.
    ///
    /// # Why `Unreadable` and `Unroutable` acknowledge, and the one case that does not
    ///
    /// A malformed body and an address this connector does not hold are **never repaired by another attempt** —
    /// the payload is what it is and the account set is a **local** fact — so both are acknowledged and recorded,
    /// because a negative acknowledgement triggers a **subscription-global** backoff for up to 60 seconds
    /// (`ADR-0094`) and would slow **every other mailbox** for a message that can never become routable. This is
    /// the same conclusion [`DeliveryRoute::unroutable_acknowledgement`] reaches.
    ///
    /// **`Changed` acknowledges too**, and for a different and stronger reason: the delivery routed, so the
    /// work is *accepted* — acknowledging is exactly what tells Pub/Sub to stop redelivering it. A routable
    /// delivery is therefore `Accept`, not the `Retry` reserved for a *transient* failure to act on a
    /// **verified** delivery (a held store, a token refresh in flight), which this function cannot see because
    /// it decides *what* the delivery is, not whether acting on it succeeded.
    #[must_use]
    pub const fn acknowledgement(&self) -> DeliveryAck {
        match self {
            Self::Unreadable(_) | Self::Unroutable(_) => DeliveryAck::AbandonAndAcknowledge,
            Self::Changed { .. } => DeliveryAck::Accept,
        }
    }

    /// Returns the account whose credential should sync, when a sync is warranted.
    ///
    /// `Some` for [`Self::Changed`] **alone** — the question a caller asks before doing work, named so a missed
    /// arm reads as "nothing to sync" rather than as a silent default.
    #[must_use]
    pub const fn account_to_sync(&self) -> Option<&AccountReference> {
        match self {
            Self::Changed { account, .. } => Some(account),
            Self::Unreadable(_) | Self::Unroutable(_) => None,
        }
    }
}

/// Ingests one Gmail push notification: reads the body, routes it, and decides what to do.
///
/// # The composition, and the finding that shaped it
///
/// This joins [`parse_delivery`] ([`crate::google::pubsub`]) with [`route_delivery`], the Gmail counterpart of
/// `ingest_channel_delivery` (`ADR-0103`). Composing them is where the **mechanism difference** becomes explicit
/// rather than assumed: the Calendar ingest has five outcomes including a **`Handshake`**, and this one has
/// **three with none**, because Gmail's opening notification is not marked and so cannot be told from a change.
/// A caller that reused `ChannelIngest`'s shape here would be looking for a marker that does not exist.
///
/// # The order: read, then route
///
/// A body that does not parse into a notification has no address to route, so reading is first. Unlike the
/// Calendar path there is **no verification step between** them: Gmail's delivery is authenticated by an OIDC
/// bearer JWT (`ADR-0099`), and **that verifier is not built** — there is no JWKS reader — so this function
/// decides attribution and *records* that the authentication is outstanding rather than pretending to perform
/// it. That is the same limit `docs/research/integrations/google.md` records as Unresolved Question 9, and it is
/// why `GmailIngest` has no `Rejected` variant: nothing can reject on a control that does not exist.
///
/// # What it does not do
///
/// It does not receive the delivery, verify the JWT, sync anything, or record the drop. It decides, and hands
/// back the account whose credential the sync must use.
#[must_use]
pub fn ingest_gmail_delivery(body: &str, accounts: &[VerifiedAccount]) -> GmailIngest {
    // 1. Read. `parse_delivery` splits the envelope from the payload, and this preserves which layer failed.
    let (delivery, notification, _encoding) = match parse_delivery(body) {
        Ok(parsed) => parsed,
        Err(error) => return GmailIngest::Unreadable(read_error(error)),
    };
    // 2. Route by the address the payload named.
    let route = route_delivery(&notification, accounts);
    let Some(account) = route.account() else {
        // `Unknown`, `Ambiguous` or `CaseDiffers` — all carried, because the last two are actionable signals a
        // caller must surface rather than silently dropping.
        return GmailIngest::Unroutable(route);
    };
    GmailIngest::Changed {
        account: account.clone(),
        history_id: notification.history_id,
        message_id: delivery.message_id().map(str::to_owned),
    }
}

/// Classifies a `parse_delivery` failure into the layer it came from.
///
/// `parse_delivery` returns `PubsubDeliveryError`, whose `Payload` variant **wraps** a
/// [`PubsubNotificationError`]. `#[from]` makes that wrap a one-way conversion, so this destructures it back
/// into [`GmailBodyError`] to keep the two layers distinguishable to the caller — the same reason
/// `parse_delivery`'s own error type nests rather than flattens.
fn read_error(error: PubsubDeliveryError) -> GmailBodyError {
    match error {
        PubsubDeliveryError::Payload(payload) => GmailBodyError::Payload(payload),
        envelope => GmailBodyError::Envelope(envelope),
    }
}

#[cfg(test)]
#[path = "routing_tests.rs"]
mod tests;
