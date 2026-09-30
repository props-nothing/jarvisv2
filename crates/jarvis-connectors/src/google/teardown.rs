//! Account teardown: two halves with a forced order, and one of them destroys the other's authority.
//!
//! # The finding this module exists for
//!
//! Disconnecting an account is **two** operations on Google, not one:
//!
//! | step | what it stops | what it needs |
//! | --- | --- | --- |
//! | `users.stop` | push notification *delivery* | a **valid** OAuth grant |
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
//!   watch is **still registered**, and it stays registered until its lease lapses.
//!
//! # Why that ordering mistake costs more than a failed call
//!
//! The push guide says of `stop`: *"All new notifications should stop within a few minutes."* If the stop never
//! happened, that sentence is irrelevant — nothing ends the stream but **the lease running out**, and nothing
//! renews it because the grant is gone. The lease's bound is
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
//! The two halves are not equally important. Revocation is the **security** half — it is what makes the tokens
//! stop working — while the stop is the **privacy** half. If the stop fails (a `429`, a `403` because an
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

use crate::google::watch::WATCH_RENEWAL_BOUND_SECONDS;

/// One half of an account teardown.
///
/// Two variants, and they are the two *effects* a disconnect has on the provider — not the two HTTP calls, which
/// would be a fact about the wire rather than about what the user asked for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TeardownStep {
    /// `users.stop`: turn off push delivery for this mailbox.
    StopWatch,
    /// Revocation: withdraw the grant and invalidate every token minted from it.
    RevokeGrant,
}

impl TeardownStep {
    /// Returns the stable name used in a plan, a log line and a test.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::StopWatch => "stop_watch",
            Self::RevokeGrant => "revoke_grant",
        }
    }

    /// Returns whether this step needs the OAuth grant to **still be valid** when it runs.
    ///
    /// **The single fact the whole ordering follows from.** `StopWatch` is an ordinary authenticated API call,
    /// so it needs a live grant; `RevokeGrant` presents the token to the revocation endpoint, which accepts a
    /// token that is already dead (RFC 7009 §2.2 makes a `200` cover "the client submitted an invalid token"),
    /// so it needs nothing to be valid.
    #[must_use]
    pub const fn needs_a_live_grant(self) -> bool {
        match self {
            Self::StopWatch => true,
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
            Self::StopWatch => false,
            Self::RevokeGrant => true,
        }
    }

    /// Returns whether skipping this step leaves something **running** that access control cannot stop.
    ///
    /// True for `StopWatch` alone: a skipped stop leaves the watch registered, and after the revoke there is no
    /// credential left to turn it off. Skipping the revoke leaves access live, which is visible — every call
    /// still works — and is the failure a user can act on.
    #[must_use]
    pub const fn skipping_leaves_a_residue(self) -> bool {
        match self {
            Self::StopWatch => true,
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
/// It is a function rather than a sentence so a future third step (Calendar's `channels.stop`, a Cloud
/// Pub/Sub subscription deletion) is checked by the same rule instead of by a reader remembering this comment.
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
/// [`may_precede`] says what is safe, and this constant is the safe order. A test asserts the two agree —
/// `may_precede(plan[0], plan[1])` is true and the reverse is false — so a plan edited into the wrong order
/// fails rather than quietly shipping, and a **third** step added later is checked by inserting it here and
/// finding out whether the rule admits it.
///
/// # And why the policies are `BestEffort` then `Required`
///
/// The stop may be skipped; the revoke may not. The asymmetry is the argument at the top of this module: a
/// failed stop costs a bounded privacy window and is recoverable by a person re-connecting, while a failed
/// revoke means the account is **not** disconnected and the user has been told nothing.
pub const TEARDOWN_PLAN: [PlannedStep; 2] = [
    PlannedStep {
        step: TeardownStep::StopWatch,
        policy: StepPolicy::BestEffort,
    },
    PlannedStep {
        step: TeardownStep::RevokeGrant,
        policy: StepPolicy::Required,
    },
];

/// How long notifications can keep arriving after a teardown, by whether the stop worked.
///
/// An enum rather than a number, because the two answers are of **different kinds** and only one of them is a
/// figure this crate can honestly state:
///
/// - After an accepted stop, the push guide says *"All new notifications should stop within a few minutes"* —
///   a qualitative statement with no number in it, so inventing seconds here would be fabricating a provider
///   rule.
/// - After a skipped or failed stop, `stop`'s timing is irrelevant: nothing ends the stream but **the lease
///   lapsing**, which is [`WATCH_RENEWAL_BOUND_SECONDS`] — a figure Google does state, reused from the watch
///   module rather than restated, so the two cannot drift.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NotificationExposure {
    /// The stop was accepted. The provider says new notifications stop within a few minutes, and states no
    /// number — so none is claimed here.
    SettlingWithinMinutes,
    /// The stop did **not** happen, so the watch is still registered and nothing renews it.
    ///
    /// The seconds value is the lease's bound, not `users.stop`'s settle time: with no stop, only the lease's
    /// expiry ends the stream.
    UntilTheLeaseLapses {
        /// How long the watch can keep notifying, in seconds.
        seconds: i64,
    },
}

/// Returns what a teardown leaves exposed for notifications.
#[must_use]
pub const fn notification_exposure(stop_succeeded: bool) -> NotificationExposure {
    if stop_succeeded {
        NotificationExposure::SettlingWithinMinutes
    } else {
        NotificationExposure::UntilTheLeaseLapses {
            seconds: WATCH_RENEWAL_BOUND_SECONDS,
        }
    }
}

#[cfg(test)]
#[path = "teardown_tests.rs"]
mod tests;
