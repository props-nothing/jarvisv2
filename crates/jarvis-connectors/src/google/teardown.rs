//! Account teardown: the halves with a forced order, and one of them destroys the other's authority.
//!
//! # The finding this module exists for
//!
//! Disconnecting an account is **at least two** operations on Google, not one:
//!
//! | step | what it stops | what it needs |
//! | --- | --- | --- |
//! | `users.stop` | Gmail push notification *delivery* | a **valid** OAuth grant |
//! | `channels.stop` | one Calendar notification channel | a **valid** OAuth grant, *and* the channel's two identifiers |
//! | revocation | all access and refresh tokens | the token itself, valid or not |
//!
//! `users.stop` is a normal API call: `POST …/users/{userId}/stop`, and the reference lists the same four
//! authorization scopes `users.watch` requires — `mail.google.com/`, `gmail.modify`, `gmail.readonly`,
//! `gmail.metadata`. Revocation **removes those scopes**, per the identity page this crate already records:
//!
//! > "Revocation removes all OAuth 2.0 scopes previously granted to a project, invalidating any issued access
//! > or refresh tokens for all clients registered under that project."
//!
//! So **stopping notifications requires an authorization that revocation destroys**, and the order is not a
//! matter of taste:
//!
//! - **Stop, then revoke** — both succeed. Delivery stops, then access is withdrawn.
//! - **Revoke, then stop** — the second call is made with a token the first call invalidated, so it fails. The
//!   watch (or the channel) is **still registered**, and it stays registered until its lease lapses.
//!
//! # The two stops are not one step, and the difference is arity
//!
//! `users.stop` ends **the** mailbox watch: one resource, one call. `channels.stop` ends **a** channel: the
//! guide is explicit that a channel `"is associated both with a particular user and a particular resource (or
//! set of resources)"` and that `"there's only one `stop` method"` — so an account watching three calendars
//! needs **three** calls, and `channels.stop` has no per-user form at all. Hence two variants: a caller that
//! performed one stop and reported "notifications stopped" would be wrong for whichever mechanism it skipped,
//! and the two mechanisms' leases are not even the same length (see [`notification_exposure`]).
//!
//! # Why that ordering mistake costs more than a failed call
//!
//! The push guide says of `stop`: *"All new notifications should stop within a few minutes."* If the stop never
//! happened, that sentence is irrelevant — nothing ends the stream but **the lease running out**, and nothing
//! renews it because the grant is gone. For Gmail the lease's bound is
//! [`crate::google::watch::WATCH_RENEWAL_BOUND_SECONDS`] — **seven days** — so a reversed teardown leaves a
//! mailbox's notifications arriving at the subscription endpoint for up to a week:
//!
//! - each delivery carries the mailbox address, which is **a person's identity** and the exact value
//!   [`crate::google::pubsub::PubsubNotification`] redacts; and
//! - the connector **cannot stop it**, because the only call that would have stopped it needs the credential it
//!   just destroyed — and the user believes they have disconnected.
//!
//! So the reversal is not "an extra failed request". It converts a teardown into a **privacy leak of up to
//! seven days**, invisible to the person who asked to disconnect, and unrecoverable without going back to them
//! for a fresh consent. That is why the order is enforced as a **value** rather than documented as a
//! caution.
//!
//! # The second decision: a failed stop must not block the revoke
//!
//! The halves are not equally important. Revocation is the **security** half — it is what makes the tokens
//! stop working — while the stops are the **privacy** half. If a stop fails (a `429`, a `403` because an
//! administrator disabled the app), running the revoke anyway leaves notifications arriving for up to the lease
//! but **withdraws access**, which is the failure a user can act on. The opposite policy — abort the teardown
//! when the stop fails — leaves a working credential in place because a *notification preference* could not be
//! changed, which trades the larger harm for the smaller one.
//!
//! So each step carries its own policy, and the plan says plainly which half may be skipped.
//!
//! # What is not built
//!
//! **No request is sent.** There is no caller for this plan: the teardown executor does not exist, in the same
//! sense that the push handler and the sync loop do not. What is here is the ordering rule and the failure
//! policy, which are the two things that are wrong if they are wrong — and both are checkable without a socket.
//! The **count** of channels is also a caller's fact rather than this module's: a plan says which effects to
//! perform, not how many times, so a caller with three channels performs [`TeardownStep::StopCalendarChannel`]
//! three times and no plan is needed per channel.

use crate::google::channel::ChannelLease;
use crate::google::watch::WATCH_RENEWAL_BOUND_SECONDS;

/// One half of an account teardown.
///
/// Three variants, and they are the three *effects* a disconnect has on the provider — not the HTTP calls,
/// which would be a fact about the wire rather than about what the user asked for. The split between the two
/// stop variants is the finding (`ADR-0107`): Google's two push mechanisms are ended by calls of **different
/// arity**, so "stop notifications" is not one effect.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TeardownStep {
    /// `users.stop`: turn off push delivery for this mailbox.
    ///
    /// **One call, per mailbox**, because the resource being watched is the mailbox itself and there is exactly
    /// one of those per account.
    StopWatch,
    /// `channels.stop`: turn off push delivery for **one** Calendar channel.
    ///
    /// **One call per channel, and there may be several**, because a channel *"is associated both with a
    /// particular user and a particular resource (or set of resources)"* and *"there's only one `stop`
    /// method"* — so an account watching three calendars needs three calls, and this variant names the effect
    /// while the caller supplies the count. That is why it is distinct from [`Self::StopWatch`]: the two are not
    /// interchangeable, and a teardown that performed one of them and reported "notifications stopped" would be
    /// wrong for whichever mechanism it skipped.
    StopCalendarChannel,
    /// Revocation: withdraw the grant and invalidate every token minted from it.
    RevokeGrant,
}

impl TeardownStep {
    /// Returns the stable name used in a plan, a log line and a test.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::StopWatch => "stop_watch",
            Self::StopCalendarChannel => "stop_calendar_channel",
            Self::RevokeGrant => "revoke_grant",
        }
    }

    /// Returns whether this step needs the OAuth grant to **still be valid** when it runs.
    ///
    /// **The single fact the whole ordering follows from.** Both stops are ordinary authenticated API calls, so
    /// they need a live grant; `RevokeGrant` presents the token to the revocation endpoint, which accepts a
    /// token that is already dead (RFC 7009 §2.2 makes a `200` cover "the client submitted an invalid token"),
    /// so it needs nothing to be valid.
    #[must_use]
    pub const fn needs_a_live_grant(self) -> bool {
        match self {
            Self::StopWatch | Self::StopCalendarChannel => true,
            // **`false`, and it is the reason reversing the order is so easy to do by accident.** Revocation
            // works on a dead token, so a caller that revoked first sees a *successful* revocation and has no
            // signal that it has just made the next step impossible.
            Self::RevokeGrant => false,
        }
    }

    /// Returns whether this step **destroys** the authority the other steps may need.
    #[must_use]
    pub const fn withdraws_access(self) -> bool {
        match self {
            // Turning off notifications changes nothing about the grant.
            Self::StopWatch | Self::StopCalendarChannel => false,
            Self::RevokeGrant => true,
        }
    }

    /// Returns whether skipping this step leaves something **running** that access control cannot stop.
    ///
    /// True for both stops: a skipped stop leaves a watch or a channel registered, and after the revoke there is
    /// no credential left to turn it off. Skipping the revoke leaves access live, which is visible — every call
    /// still works — and is the failure a user can act on.
    #[must_use]
    pub const fn skipping_leaves_a_residue(self) -> bool {
        match self {
            Self::StopWatch | Self::StopCalendarChannel => true,
            Self::RevokeGrant => false,
        }
    }
}

/// Whether `first` may be performed **before** `second`.
///
/// The rule, in one line: **a step that withdraws access may not precede a step that needs it.** Every other
/// ordering is permitted, including a step preceding itself, so the function answers "is this pair safe" rather
/// than "is this the one true order" — which is what makes it usable on a plan that grows.
///
/// **It was written as a function so a future third step would be checked by the same rule instead of by a
/// reader remembering a comment, and that is exactly what happened** (`ADR-0107`):
/// [`TeardownStep::StopCalendarChannel`] was added, and the rule admitted it **unchanged and unedited** — two
/// stops before the revoke, no pair refused. So `ADR-0095`'s revisit condition ("the first real test of whether
/// `may_precede` generalises or needs a per-API argument") is answered: it generalises, because the property it
/// tests is *authority*, which neither stop carries and revocation does. A fourth step that also needs a live
/// grant and withdraws nothing would be admitted the same way, and one that withdrew access would be refused —
/// which is the answer a reader wants without having to re-derive it.
#[must_use]
pub const fn may_precede(first: TeardownStep, second: TeardownStep) -> bool {
    !(first.withdraws_access() && second.needs_a_live_grant())
}

/// What a step's **failure** must do to the rest of the teardown.
///
/// Two variants because the halves are not equally important, and the difference is what a caller does next
/// rather than how loudly it logs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StepPolicy {
    /// A failure is recorded and the teardown continues.
    ///
    /// For the privacy half: its failure must not leave a working credential in place.
    BestEffort,
    /// A failure is surfaced to the caller as a failed disconnect.
    ///
    /// For the security half: if access is not withdrawn, the user has not disconnected and must be told.
    Required,
}

impl StepPolicy {
    /// Returns whether a failed step may be passed over.
    #[must_use]
    pub const fn may_be_skipped(self) -> bool {
        matches!(self, Self::BestEffort)
    }
}

/// One step of the plan, with what a failure of it means.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlannedStep {
    /// Which effect to perform.
    pub step: TeardownStep,
    /// What a failure must do to the rest of the plan.
    pub policy: StepPolicy,
}

/// The teardown plan, in the only order that works.
///
/// # Why this is derived from the steps rather than chosen
///
/// [`may_precede`] says what is safe, and this constant is the safe order. A test asserts the rule admits every
/// **adjacent** pair here, so a plan edited into the wrong order fails rather than quietly shipping — and when
/// [`TeardownStep::StopCalendarChannel`] was added it was appended by inserting it here and finding out whether
/// the rule admitted it, which is the procedure this doc described before the step existed (`ADR-0107`).
///
/// # And why the policies are two `BestEffort` stops then a `Required` revoke
///
/// Both stops may be skipped; the revoke may not. The asymmetry is the argument at the top of this module: a
/// failed stop costs a bounded privacy window and is recoverable by a person re-connecting, while a failed
/// revoke means the account is **not** disconnected and the user has been told nothing. Sharing one policy
/// between the two stops is deliberate and is not a conflation: each ends one mechanism, each has the same
/// failure cost, and neither is a precondition of the other, so a caller may run them in either order or skip
/// either one.
///
/// # Why the count is not in the plan
///
/// A plan is a **sequence of effects**, and how many channels an account has is a fact about the account rather
/// than about the effect. An account watching three calendars performs [`TeardownStep::StopCalendarChannel`]
/// three times against three `(channel_id, resource_id)` pairs; encoding the count here would make one plan per
/// account and make the shared ordering rule harder to check.
pub const TEARDOWN_PLAN: [PlannedStep; 3] = [
    PlannedStep {
        step: TeardownStep::StopWatch,
        policy: StepPolicy::BestEffort,
    },
    PlannedStep {
        step: TeardownStep::StopCalendarChannel,
        policy: StepPolicy::BestEffort,
    },
    PlannedStep {
        step: TeardownStep::RevokeGrant,
        policy: StepPolicy::Required,
    },
];

/// How long notifications can keep arriving after a teardown, by whether the stop worked.
///
/// An enum rather than a number, because the answers are of **different kinds** and only one of them is a figure
/// this crate can honestly state:
///
/// - After an accepted stop, the push guide says *"All new notifications should stop within a few minutes"* —
///   a qualitative statement with no number in it, so inventing seconds here would be fabricating a provider
///   rule.
/// - After a skipped or failed stop, `stop`'s timing is irrelevant: nothing ends the stream but **the lease
///   lapsing**.
/// - And when the lease had **already** ended before the teardown, nothing is exposed at all — a case that a
///   single "seconds until the lease lapses" number would report as *some* exposure, overstating a teardown
///   that is in fact already clean.
///
/// # Why the mechanism is not a parameter here
///
/// Gmail's exposure and a Calendar channel's are computed by **[
/// `gmail_exposure`]** and **[
/// `calendar_exposure`]** rather than by one function taking a mechanism flag, because the two need *different
/// inputs* — a stated constant versus a lease. A flag would have to be paired with an `Option` lease that is
/// meaningless in one branch, which is the shape that lets a caller pass a Calendar lease to a Gmail figure. A
/// `PushMechanism` enum was drafted for this and **removed**, because its only use would have been an equality
/// assertion in its own test: naming the mechanism is what the *function choice* already does, and a value
/// nothing consumes is the defect `ADR-0092` records.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NotificationExposure {
    /// The stop was accepted. The provider says new notifications stop within a few minutes, and states no
    /// number — so none is claimed here.
    SettlingWithinMinutes,
    /// The stop did **not** happen, so the watch or channel is still registered and nothing renews it.
    ///
    /// The seconds value is the lease's remaining time, not `users.stop`'s settle time: with no stop, only the
    /// lease's expiry ends the stream.
    UntilTheLeaseLapses {
        /// How long the watch or channel can keep notifying, in seconds.
        seconds: i64,
    },
    /// The lease had **already** ended, so a stop that failed costs nothing: there is nothing left to silence.
    ///
    /// Reachable only for [`PushMechanism::CalendarChannel`], because a Calendar channel's expiry is known from
    /// its own `watch` response and can therefore be compared to a clock before the teardown runs. Gmail's
    /// exposure cannot reach this state, and the reason is a real asymmetry rather than an omission: the mailbox
    /// watch's expiry is read from a `watch` response too, but this function is given the **documented bound**
    /// rather than a lease, so it has no instant to compare — see the note on [`gmail_exposure`].
    AlreadyEnded {
        /// How long ago the lease ended, in seconds. At least zero.
        ended_seconds_ago: i64,
    },
}

/// Returns what a teardown of a **Gmail** mailbox watch leaves exposed for notifications.
///
/// Takes the documented bound rather than a lease, and that is a recorded limit rather than a design choice:
/// a caller *does* hold the watch's `expiration` (`crate::google::watch::parse_watch_response`), so a lease-aware
/// form could report [`NotificationExposure::AlreadyEnded`] for Gmail too. It is not built because nothing
/// calls this yet and because the two mechanisms' figures come from different documents — so the Calendar form
/// is written where the lease is unavoidable (a channel has **no** stated bound, only its own expiry) and this
/// one keeps the provider's stated constant.
#[must_use]
pub const fn gmail_exposure(stop_succeeded: bool) -> NotificationExposure {
    if stop_succeeded {
        NotificationExposure::SettlingWithinMinutes
    } else {
        NotificationExposure::UntilTheLeaseLapses {
            seconds: WATCH_RENEWAL_BOUND_SECONDS,
        }
    }
}

/// Returns what a teardown of one **Calendar** channel leaves exposed, from that channel's lease.
///
/// # Why this takes a lease and not a constant
///
/// Gmail's watch has a stated bound ([`WATCH_RENEWAL_BOUND_SECONDS`]) because Google publishes one for the
/// mechanism. **Calendar publishes no equivalent bound for a channel** — a channel's life is *"determined
/// either by your request or by any Google Calendar API internal limits or defaults"* — so the only honest
/// figure is the one the channel's own `watch` response reported, which is exactly the
/// [`crate::google::channel::ChannelWatchResponse::expires_at`] a lease is computed from.
///
/// **A lease that has already ended outranks a successful stop**, because the answer then does not depend on
/// the call at all: nothing can arrive from a channel that has expired, whether or not a stop was attempted.
///
/// **That property comes from the match being on the lease variant, not from the order of the arms** — and the
/// distinction was established by trying to falsify the order. Swapping the `Lapsed` arm below the
/// `Alive if stop_succeeded` one changed **nothing**: `Lapsed` and `Alive` are different variants, so the arms
/// are mutually exclusive and no arm can shadow another. A doc that had claimed "the arms are ordered
/// lease-first" would have been asserting a fact about the source's layout as if it were a fact about the
/// behaviour, which is unfalsifiable and therefore not checkable. The load-bearing part is the guard on the
/// **`stop_succeeded`** arm instead: removing that guard makes a live, unstopped channel report
/// [`NotificationExposure::SettlingWithinMinutes`], which the test detects.
#[must_use]
pub fn calendar_exposure(lease: ChannelLease, stop_succeeded: bool) -> NotificationExposure {
    match lease {
        ChannelLease::Lapsed { for_seconds } => NotificationExposure::AlreadyEnded {
            ended_seconds_ago: for_seconds,
        },
        ChannelLease::Alive { .. } if stop_succeeded => NotificationExposure::SettlingWithinMinutes,
        ChannelLease::Alive { for_seconds } => NotificationExposure::UntilTheLeaseLapses {
            seconds: for_seconds,
        },
    }
}

#[cfg(test)]
#[path = "teardown_tests.rs"]
mod tests;
