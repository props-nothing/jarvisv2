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
/// # Which entry point to use
///
/// This one **does not anchor** the first sync, and that is a deliberate limit rather than an oversight: a
/// caller that connected by reading `users.getProfile` holds a `historyId` that *can* seed a first sync (see
/// [`establish_account_with_anchor`]), and one that has neither a profile anchor nor a watch has nothing to
/// anchor from — so this function's answer is `FullSync` and it never claims otherwise. The distinction is the
/// same one the two resume functions draw, at the point where the account is created.
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
    establish_account_inner(
        reference,
        profile,
        method,
        granted_scopes,
        verified_at,
        existing,
        None,
    )
}

/// Builds the [`VerifiedAccount`] for a mailbox, **keeping the profile's `historyId` as a first-sync anchor**.
///
/// # Why the same profile is read two ways
///
/// [`GmailProfile`] carries `historyId` — *"The ID of the mailbox's current history record"* — and the sync
/// guide's rule is that a first sync may start from *"the `historyId` of the most recent message"*, so a profile
/// read yields an anchor **without consuming a message** and for **one quota unit**. The same value is therefore
/// two things depending on what the caller wants: a cheap way to seed an incremental first sync, and something
/// to ignore entirely when the mailbox's existing contents are wanted.
///
/// The distinction is not representable in the account — [`crate::account::VerifiedAccount`] holds an identity
/// and a grant, not a sync position — so it is expressed where it is used, by which of these two functions
/// produced the account. **A caller that calls this one and then syncs from `FullSync` would discard the anchor
/// it just kept**, which is why the two functions exist rather than a flag: a flag would be a second place the
/// same decision is made, and `ADR-0035`'s rule applies to a `bool` standing for two different first syncs.
///
/// # Errors
///
/// The same as [`establish_account`], and for the same reasons — the anchor is carried, not validated here.
pub fn establish_account_with_anchor(
    reference: AccountReference,
    profile: &GmailProfile,
    method: AuthMethod,
    granted_scopes: &[String],
    verified_at: UtcTimestamp,
    existing: &[VerifiedAccount],
) -> Result<AnchoredAccount, ConnectError> {
    let account = establish_account_inner(
        reference,
        profile,
        method,
        granted_scopes,
        verified_at,
        existing,
        Some(SyncOrigin::Profile),
    )?;
    // The anchor is `profile.history_id`, which `parse_profile` has already bounded and filtered — an empty or
    // whitespace-only id arrives as `None` rather than as an empty anchor, so a profile that stated no position
    // yields an account whose first sync is a full one rather than one anchored at nothing.
    let anchor = profile.history_id.clone();
    Ok(AnchoredAccount {
        account,
        anchor,
        origin: SyncOrigin::Profile,
    })
}

/// A connected account and the anchor its first sync may start from.
///
/// Returned only by [`establish_account_with_anchor`] and [`establish_account_from_watch`]. The anchor is
/// **optional**, because a profile or a watch response can state no `historyId` — and an `Option` here is the
/// honest shape rather than a defaulted string: `None` means the first sync has nothing to start from, which is
/// [`ResumePoint::FullSync`], and a caller that treated a missing anchor as present would ask the provider to
/// list history from a position nobody stated.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnchoredAccount {
    /// The account, for storing.
    pub account: VerifiedAccount,
    /// The `historyId` a first `history.list` may start from, when the response stated one.
    pub anchor: Option<String>,
    /// Which response the anchor came from.
    pub origin: SyncOrigin,
}

impl AnchoredAccount {
    /// Decides where this account's first sync begins.
    ///
    /// The bridge between connecting and syncing, and the one place the two halves meet: an anchor with a `Start`
    /// cursor becomes [`ResumePoint::FromAnchor`], and no anchor stays [`ResumePoint::FullSync`]. A caller that
    /// wants the mailbox's existing contents uses [`resume_ignoring_anchor`] directly instead, which is a
    /// visible choice rather than a flag.
    #[must_use]
    pub fn resume(&self, cursor: &SyncCursor) -> ResumePoint {
        match self.anchor.as_deref() {
            Some(anchor) => resume_anchored(cursor, anchor, self.origin),
            None => ResumePoint::FullSync,
        }
    }
}

/// Builds the [`VerifiedAccount`] for a mailbox whose `watch` has just been established.
///
/// The second anchor source: the `users.watch` response's `historyId`, which the push guide says means *"Your
/// client receives notifications for all changes **after** that `historyId`"* — so it anchors a first sync at the
/// moment notifications begin, which is exactly the point an incremental sync should start from.
///
/// # Errors
///
/// The same as [`establish_account`]. The watch response is assumed to have been read already; a caller that has
/// not read one has no anchor and should use [`establish_account`].
pub fn establish_account_from_watch(
    reference: AccountReference,
    profile: &GmailProfile,
    method: AuthMethod,
    granted_scopes: &[String],
    verified_at: UtcTimestamp,
    existing: &[VerifiedAccount],
    watch: &crate::google::watch::WatchResponse,
) -> Result<AnchoredAccount, ConnectError> {
    let account = establish_account_inner(
        reference,
        profile,
        method,
        granted_scopes,
        verified_at,
        existing,
        Some(SyncOrigin::WatchResponse),
    )?;
    Ok(AnchoredAccount {
        account,
        anchor: Some(watch.anchor.clone()),
        origin: SyncOrigin::WatchResponse,
    })
}

/// The shared body of the three ways to connect an account.
///
/// `origin` is taken and used only to make the three call sites state which anchor they are working from —
/// including the one that has none — so a reader of any entry point can see the branch without following it.
fn establish_account_inner(
    reference: AccountReference,
    profile: &GmailProfile,
    method: AuthMethod,
    granted_scopes: &[String],
    verified_at: UtcTimestamp,
    existing: &[VerifiedAccount],
    origin: Option<SyncOrigin>,
) -> Result<VerifiedAccount, ConnectError> {
    // Recorded so that the three entry points differ in one visible place rather than in three copies of the
    // duplicate check. It is not used to alter the account: an anchor is a sync fact, not an identity one, and
    // `VerifiedAccount` deliberately holds no position (`ADR-0098`).
    let _ = origin;
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
    /// The account has **no stored position**, and the connector holds an **anchor** it has never synced from.
    ///
    /// This is the state a just-connected account is in, and it is **not** a full sync: `history.list` from the
    /// anchor returns exactly the changes since the anchor — typically none — rather than the whole mailbox.
    /// Reaching for a full sync here would download the mailbox's entire contents to discover that nothing had
    /// happened since the `watch`, and the guide's own worked example makes the opposite direction explicit.
    ///
    /// [`Self::FullSync`] is kept for the case the anchor cannot serve — see [`SyncOrigin`] — because collapsing
    /// the two would make "we know where to start" and "we must read everything" the same answer.
    FromAnchor {
        /// The token to send, and always a [`SyncCursorKind::MonotonicMarker`]: every Gmail anchor the connector
        /// can obtain is a `historyId`.
        anchor: String,
        /// Which **source** the anchor came from, so a caller can say where its starting point came from rather
        /// than only what it is.
        origin: SyncOrigin,
    },
    /// The account has no position **and no anchor**: the sync must read the mailbox from the beginning.
    FullSync,
}

/// Where an unsynced account's **anchor** came from.
///
/// Two sources, and the guide offers them for different moments: a `watch` response anchors *now* (the mailbox's
/// position at the moment the lease began), while `users.getProfile` anchors *now* as well but costs one quota
/// unit and no watch. The distinction is kept because the two are obtainable at different times — a profile read
/// needs only a credential, while a watch also needs a Pub/Sub topic — so a caller choosing between them is
/// choosing a cost rather than a value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyncOrigin {
    /// The `historyId` a `users.watch` response returned ([`crate::google::watch::WatchResponse`]).
    WatchResponse,
    /// The `historyId` a `users.getProfile` response returned ([`crate::google::request::GmailProfile`]).
    Profile,
}

impl SyncOrigin {
    /// Returns the stable snake-case code, for a log line and a test.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WatchResponse => "watch_response",
            Self::Profile => "profile",
        }
    }
}

impl ResumePoint {
    /// Returns the position, when there is one.
    ///
    /// **`None` for [`Self::FromAnchor`] as well as for [`Self::FullSync`]**, and the naming is what makes that
    /// correct rather than surprising: an anchor is not a *position* — nothing has been synced from it yet — it
    /// is where a sync should *begin*. A caller that treated it as a position would have the cursor it is about
    /// to create and the value that seeds it confused, which is the conflation the two ids in the guide's worked
    /// example invite. Use [`Self::anchor`] for the anchor.
    #[must_use]
    pub fn position(&self) -> Option<&str> {
        match self {
            Self::FromPosition { position, .. } => Some(position),
            Self::FromAnchor { .. } | Self::FullSync => None,
        }
    }

    /// Returns the anchor to start from, when the account has one and no position.
    #[must_use]
    pub fn anchor(&self) -> Option<&str> {
        match self {
            Self::FromAnchor { anchor, .. } => Some(anchor),
            Self::FromPosition { .. } | Self::FullSync => None,
        }
    }

    /// Returns whether this is a full sync.
    ///
    /// The predicate a scheduler calls, and it is **true only for a cursor with no position and no anchor** — so
    /// a full sync is never the default a caller falls into, which is the property [`SyncCursorKind::Start`]
    /// exists to make representable. Both an anchored start and a stored position answer `false`, because
    /// neither reads the mailbox from the beginning.
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

/// Decides where a **first** sync begins for an account whose `watch` has just been established.
///
/// # Why this exists: the anchor had a reader and no consumer
///
/// [`parse_watch_response`](crate::google::watch::parse_watch_response) reads the `watch` response's `historyId`
/// and calls it *"the anchor a first sync starts from"*, and **nothing ever started a sync from it** — the only
/// way to begin was [`ResumePoint::FullSync`], so a just-connected mailbox was read end to end. The guide's own
/// example is explicit that this is unnecessary:
///
/// > "Pass `1234567890` as the `startHistoryId` to `history.list`. Afterward, you can persist `9876543210` as the
/// > last known `historyId`."
///
/// — where `1234567890` **is** the watch response's `historyId`. So the first sync is `history.list` *from the
/// anchor*, which returns the changes since the watch was set (typically none) rather than the mailbox.
///
/// A caller that wants the mailbox's **existing** contents uses [`resume_ignoring_anchor`] instead, which is a
/// separate function because the two differ in cost by orders of magnitude: the guide says *"If you need to
/// process changes before this `historyId`, refer to Synchronize clients with Gmail"* — a second branch, and
/// choosing it should be a visible decision rather than a default.
///
/// A **`Start` cursor** is the precondition, and any other is a contract error rather than an input: an account
/// that already has a position resumes from it via [`resume_from`], and an anchored start is only meaningful for
/// a mailbox nothing has synced from yet. Rather than add an error variant for a state the caller cannot reach
/// without ignoring [`resume_from`]'s own answer, this function is written to be called on a `Start` cursor —
/// and a non-`Start` one is reported as a full sync, which is the conservative direction: it can never silently
/// start an incremental sync from an anchor it was not meant to use.
#[must_use]
pub fn resume_anchored(cursor: &SyncCursor, anchor: &str, origin: SyncOrigin) -> ResumePoint {
    if cursor.token().is_some() {
        // A stored position outranks an anchor: the account has been synced, so the anchor is historical. Using
        // it would re-read the window between the watch and the first stored position — a duplicate rather than
        // a miss, but a silent one, and the position is the better answer in every case.
        return resume_from(cursor);
    }
    ResumePoint::FromAnchor {
        anchor: anchor.to_owned(),
        origin,
    }
}

/// Decides where a synced-outcome **first** sync begins when the mailbox's existing contents are wanted.
///
/// The deliberate branch: `history.list` from an anchor reports only what changed **after** it, so a caller that
/// needs the mailbox as it stands must sync from the beginning and take the anchor only as the point at which to
/// stop — which is what the guide means by *"If you need to process changes before this `historyId`"*.
///
/// It takes the same inputs and returns [`ResumePoint::FullSync`] unconditionally, including when the account
/// **does** have a stored position: a caller that asked for the mailbox is asking for a full read, and quietly
/// returning a stored position instead would answer a different question. That is why it is a function rather
/// than "call [`resume_from`] and ignore the anchor" — the two answers are not interchangeable, and the name
/// says which question is being asked.
#[must_use]
pub fn resume_ignoring_anchor(
    cursor: &SyncCursor,
    anchor: &str,
    origin: SyncOrigin,
) -> ResumePoint {
    // The arguments are taken and deliberately unused: a caller has them because it just read the response that
    // produced them, and dropping them from the signature would make the two functions unalike in a way that
    // invites calling the wrong one. The anchor is not *forgotten* — a caller that wants it stores it as the
    // point where the full sync ends, which is the position `history.list` will report.
    let _ = (cursor, anchor, origin);
    ResumePoint::FullSync
}

#[cfg(test)]
#[path = "connection_tests.rs"]
mod tests;
