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
//! **The delivery is not authenticated, and this record says so.** `P5-004`'s Finding 1 establishes that
//! neither Google mechanism fits [`crate::manifest::WebhookSupport::Push`]: Gmail's push is an **OIDC bearer
//! JWT** rather than an HMAC over the body, and Calendar's is an **echoed channel token over a zero-length
//! body** — while `SignatureScheme` is HMAC-family only. So the connector currently has **no way to verify that
//! a delivery came from Google at all**, and that is recorded as an unresolved question rather than papered
//! over.
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
use crate::google::pubsub::{DeliveryAck, PubsubNotification};

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

#[cfg(test)]
#[path = "routing_tests.rs"]
mod tests;
