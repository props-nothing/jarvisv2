//! Connecting an account: the step that mints a `VerifiedAccount`, and the duplicate that would break routing.
//!
//! # Where this sits
//!
//! `ADR-0096` gave the connector an operation that reads a mailbox's **verified identity**
//! ([`crate::google::request::GmailProfile`], via `gmail_profile_read`) and named the connect-time flow that
//! would use it as unbuilt. `ADR-0097` gave the push path a **router** that attributes a delivery to one of the
//! connector's accounts, and named `DeliveryRoute::Ambiguous` — several account records carrying one address —
//! as a reachable state whose cause is "a reconnect mints a new [`AccountReference`] without retiring the old
//! row". This module is the third step, and it sits exactly between those two: it turns a profile into a
//! stored account, and it is where `Ambiguous` is either **prevented** or **created**.
//!
//! # The rule, and why it is a refusal rather than a deduplication
//!
//! **One address, one account.** Connecting a mailbox whose address another account already holds is
//! **refused** rather than allowed, and the reason is what a duplicate costs rather than tidiness:
//!
//! - **Routing stops being decidable.** `route_delivery` answers [`DeliveryRoute::Ambiguous`] once two accounts
//!   carry one address, and that route may not be applied without a person — so every notification for that
//!   mailbox stops being acted on until someone resolves it. A connector that could still *read* the mailbox
//!   on demand would silently stop receiving its **changes**, which is the degradation a push path exists to
//!   prevent.
//! - **And the duplicate is invisible in the meantime.** Two accounts for one mailbox look exactly like two
//!   mailboxes: two cursors, two sync schedules, and a quota budget paid twice for one mailbox's traffic.
//!
//! # The comparison is case-insensitive, and that is the *opposite* direction from routing
//!
//! `ADR-0097` refuses to **act** on a case-only near-match, because Google publishes no canonicalisation rule
//! for `emailAddress` and promoting the near-match could read the wrong mailbox. This module refuses to
//! **create** one, for the same underlying reason: if the two spellings are one mailbox, allowing both mints the
//! duplicate above; and if they are genuinely two mailboxes, the cost of the refusal is that a person is asked
//! — which is recoverable, where a wrong automatic action is not.
//!
//! So the two conservative choices point in opposite directions and they are consistent: **neither acts on an
//! uncertain case-match.** A test asserts both halves against one pair of spellings, so a later change that
//! loosened either one would show up as a contradiction rather than as a silent policy drift.
//!
//! # What is not verified
//!
//! **No profile has been read and no account stored.** The inputs are the crate's own types
//! ([`GmailProfile`], [`VerifiedAccount`], [`AccountReference`]) and every rule here is a decision about them
//! rather than a provider fact — which is why each is argued above rather than cited.

use crate::account::{AccountReference, VerifiedAccount};
use crate::auth::AuthMethod;
use crate::cursor::{SyncCursor, SyncCursorKind};
use crate::google::request::GmailProfile;
use jarvis_core::UtcTimestamp;

/// Why an account could not be connected.
///
/// Two variants, and they are different **subjects**: one is about the account set and one about the identity
/// itself. The first names the **owner** as well as the address so an operator can see which existing account
/// holds it, which is what makes the refusal actionable — "this address is taken" without saying by what leaves
/// a person searching.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ConnectError {
    /// Another account already holds this mailbox address.
    #[error(
        "the mailbox address is already held by the account `{holder}`; one address may belong to one account, \
         because two accounts for one mailbox make a push delivery unroutable and pay its quota twice"
    )]
    AddressAlreadyConnected {
        /// The account that already holds it, so the operator knows what to retire or reuse.
        holder: AccountReference,
    },
    /// The provider's identity could not be **stored**, so the account cannot be represented.
    ///
    /// A separate variant rather than a re-use of the duplicate refusal, because the remedies are unalike: a
    /// duplicate is fixed by retiring an account, while an unusable identifier is fixed by nothing the operator
    /// can do and points at a provider response — an oversized address, or one carrying a control character.
    /// Reporting the second as the first would send a person to look for a duplicate that does not exist.
    #[error("the provider's identity cannot be stored as an account identifier: {reason}")]
    IdentityUnusable {
        /// What is wrong, as the **static bound** that refused it.
        ///
        /// **`&'static str`, not the failing error's rendering, and that is the whole safety property.**
        /// `ConnectorError::Identifier`'s own `Display` interpolates the offending `value`, so carrying
        /// `error.to_string()` here would print the mailbox address in full — inside a refusal whose very
        /// purpose is to say an identity could not be *stored*, and while [`crate::account::VerifiedAccount`]'s
        /// `Debug` redacts that same value (`ADR-0091`). Taking the variant's `reason` field instead makes the
        /// leak **unrepresentable**: a `&'static str` has nowhere to put a runtime value.
        reason: &'static str,
    },
}

impl ConnectError {
    /// Returns the account holding the address, when the refusal is a duplicate.
    ///
    /// `None` for [`Self::IdentityUnusable`], because there is no holder — and returning a fabricated one to
    /// make the signature uniform would be the very thing the accessor exists to avoid saying.
    #[must_use]
    pub fn holder(&self) -> Option<&AccountReference> {
        match self {
            Self::AddressAlreadyConnected { holder } => Some(holder),
            Self::IdentityUnusable { .. } => None,
        }
    }
}

/// Builds the [`VerifiedAccount`] for a mailbox that has just been authorised, refusing a duplicate.
///
/// # Every argument is a value, and none is read from anywhere
///
/// Same reasoning as every other decision in this module family: a connector or a store would make the rule
/// untestable without one, and there is no store in this crate. `granted_scopes` and `verified_at` come from
/// the caller because only it has them — the scope list from the grant
/// ([`crate::token::TokenSet`]) and the instant from its clock.
///
/// # Why `existing` is `&[VerifiedAccount]` rather than a list of addresses
///
/// For the reason `route_delivery` takes the same shape (`ADR-0097`): the address and the reference must belong
/// to the same account, and a `&[(AccountReference, String)]` would let a caller pair one account's reference
/// with another's address — here that would report the **wrong holder** in the refusal, which is worse than no
/// report because it sends an operator to retire the wrong account.
///
/// # Errors
///
/// Returns [`ConnectError::AddressAlreadyConnected`] when any existing account's provider identifier matches
/// the profile's address, **ignoring ASCII case**. The comparison is deliberately the looser of the two
/// `ADR-0097` distinguishes, and in this direction: refusing is recoverable (a person is asked) where creating
/// a duplicate is not.
pub fn establish_account(
    reference: AccountReference,
    profile: &GmailProfile,
    method: AuthMethod,
    granted_scopes: &[String],
    verified_at: UtcTimestamp,
    existing: &[VerifiedAccount],
) -> Result<VerifiedAccount, ConnectError> {
    for account in existing {
        // A **case-insensitive** comparison, and the direction is argued in the module doc: routing refuses to
        // *act* on a case-only near-match, and this refuses to *create* one.
        if account
            .provider_account_id()
            .eq_ignore_ascii_case(&profile.email_address)
        {
            return Err(ConnectError::AddressAlreadyConnected {
                holder: account.reference().clone(),
            });
        }
    }
    // The provider's address becomes the account's `provider_account_id`, which is the value a push delivery is
    // later matched against — so this call is what makes `route_delivery` able to find the account at all, and
    // the guard above is what keeps that lookup single-valued.
    //
    // `display_name` is `None` because `users.getProfile` returns **no** display name: the reference gives
    // `emailAddress`, `messagesTotal`, `threadsTotal` and `historyId`. Passing the address as a display name
    // would be inventing a provider statement, which is the field `VerifiedAccount` exists to keep honest.
    VerifiedAccount::new(
        reference,
        profile.email_address.clone(),
        None,
        method,
        granted_scopes.to_vec(),
        verified_at,
    )
    .map_err(|error| {
        // `VerifiedAccount::new` refuses an identifier that is empty, oversized or control-bearing. The address
        // has already been through `parse_profile`, which refuses an empty or whitespace-only one — so the
        // reachable remainder is oversized or control-bearing.
        //
        // **The variant's `reason` is taken and its `Display` is not.** `ConnectorError::Identifier`'s Display
        // interpolates the offending `value`, so rendering it would print the mailbox address — see
        // `ConnectError::IdentityUnusable`.
        let reason = match error {
            crate::manifest::ConnectorError::Identifier { reason, .. } => reason,
            // `VerifiedAccount::new` produces `Identifier` for every refusal it can make, so this arm is a
            // contract change rather than an input. A fixed sentence is used rather than an unwrap, because
            // `expect` is denied in this crate and a refusal naming the wrong subject is still better than an
            // abort.
            _ => "the provider's identity is unusable as an account identifier",
        };
        ConnectError::IdentityUnusable { reason }
    })
}

/// Where a sync should begin for an account that is already connected.
///
/// # Why `FullSync` carries no reason, and why that is the honest shape
///
/// [`SyncCursorKind::Start`]'s own doc says it means *"the connector has never synced, or its position was
/// discarded"* — **two causes behind one value**, and the cursor carries nothing that separates them. A caller
/// that wants the reason has it in hand already (it is the component that just received
/// [`crate::google::client::SyncAdvance::HistoryPruned`] or `TokenInvalidated`), so this function reports the
/// **action** and avoids inventing a discrimination the input cannot support — the same restraint `ADR-0067`
/// records for a `404` whose cause Google publishes no code for.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResumePoint {
    /// The account has a position: continue incrementally from it.
    FromPosition {
        /// The token to send — `startHistoryId` for Gmail, the sync token for Calendar.
        position: String,
        /// Which kind of position this is, so a caller knows what may be concluded from it.
        ///
        /// Carried because the kinds differ in exactly the way that matters here: a
        /// [`SyncCursorKind::MonotonicMarker`] has detectable staleness and a defined recovery, while an
        /// [`SyncCursorKind::OpaqueToken`] may not be validated at all and only the provider's refusal is
        /// authoritative.
        kind: SyncCursorKind,
    },
    /// The account has no position, so the sync must be a **full** one.
    FullSync,
}

impl ResumePoint {
    /// Returns the position, when there is one.
    #[must_use]
    pub fn position(&self) -> Option<&str> {
        match self {
            Self::FromPosition { position, .. } => Some(position),
            Self::FullSync => None,
        }
    }

    /// Returns whether this is a full sync.
    ///
    /// The predicate a scheduler calls, and it is **true only for a cursor with no position** — so a full sync
    /// is never the default a caller falls into, which is the property [`SyncCursorKind::Start`] exists to
    /// make representable.
    #[must_use]
    pub const fn requires_full_sync(&self) -> bool {
        matches!(self, Self::FullSync)
    }
}

/// Decides where a sync should begin from an account's stored cursor.
///
/// # The two inputs a cursor has, and neither is inferred
///
/// [`SyncCursor::kind`] says what *shape* the position has, and [`SyncCursor::token`] is the position itself.
/// `SyncCursor::new` already refuses the combinations that cannot occur — a `Start` cursor carrying a token, or
/// any other kind carrying none — so this function does not re-check them: a `Start` with a token is
/// unrepresentable, and re-asserting it here would be a refusal that can never fire, the defect `ADR-0066` and
/// `P5-003` each record.
///
/// # Why a `Start` cursor is read as `FullSync` rather than as an error
///
/// `SyncCursorKind::Start`'s doc is explicit that a full sync "is a decision with consequences (cost, time,
/// possibly a rate-limit budget), and an absent value would make it the default a caller stumbles into." So the
/// kind is reported as the decision it is, and no caller has to interpret a missing token.
#[must_use]
pub fn resume_from(cursor: &SyncCursor) -> ResumePoint {
    match (cursor.kind(), cursor.token()) {
        // The position and the kind travel together, so a caller cannot take the token without seeing what may
        // be concluded from it.
        (kind, Some(position)) => ResumePoint::FromPosition {
            position: position.to_owned(),
            kind,
        },
        // The remaining cases all report a full sync, and they are collapsed deliberately rather than by
        // oversight: `SyncCursor::new` guarantees the only reachable one is `Start` with no token, and the
        // `(_, None)` arm for a non-`Start` kind would be a contract change. Splitting them would give two arms
        // with identical bodies, which clippy correctly flags as one arm doing two jobs — and the property that
        // matters is the same in both: **a cursor with no position cannot resume incrementally.**
        //
        // `Start`'s two documented causes — never synced, and position discarded — are not separated here either,
        // because the cursor carries nothing that distinguishes them and the caller that discarded the position
        // already knows why (`ADR-0067`).
        _ => ResumePoint::FullSync,
    }
}

#[cfg(test)]
#[path = "connection_tests.rs"]
mod tests;
