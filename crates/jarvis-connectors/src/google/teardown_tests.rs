//! Tests for the account teardown order.
//!
//! The load-bearing ones are about **order**, because a reversed teardown does not fail where it is wrong: the
//! revoke succeeds, and the stop that follows is the call that fails. So each test asserts the pairing rather
//! than either step alone.
//!
//! Falsification record in `TODO.md`.

use super::*;

#[test]
fn stopping_notifications_needs_the_grant_that_revoking_destroys() {
    // **The one fact the whole module follows from**, asserted in both directions rather than one, because a
    // reader could otherwise conclude that the revoke also needs a live grant and that the order therefore does
    // not matter.
    assert!(
        TeardownStep::StopWatch.needs_a_live_grant(),
        "`users.stop` is an ordinary authenticated API call, so it needs a token that still works"
    );
    assert!(
        TeardownStep::RevokeGrant.withdraws_access(),
        "revocation is what removes the grant `stop` needs"
    );
    // And the asymmetry that makes the mistake easy: the revoke does NOT need a live grant, so a caller that
    // revoked first sees its revoke succeed and gets no signal that it has just blocked the next step.
    assert!(
        !TeardownStep::RevokeGrant.needs_a_live_grant(),
        "revocation accepts an already-dead token, which is why reversing the order fails silently at the \
         *next* call rather than at this one"
    );
    assert!(
        !TeardownStep::StopWatch.withdraws_access(),
        "turning off notifications changes nothing about the grant"
    );
}

#[test]
fn the_plan_orders_the_step_that_needs_the_grant_before_the_step_that_removes_it() {
    // The plan is derived from `may_precede`, and this asserts the two agree — so a plan edited into the wrong
    // order fails here rather than shipping, and a reader can see *why* the order is what it is.
    let steps: Vec<TeardownStep> = TEARDOWN_PLAN.iter().map(|planned| planned.step).collect();
    assert_eq!(
        steps,
        vec![TeardownStep::StopWatch, TeardownStep::RevokeGrant],
        "the stop must come first: it is the step that needs a live grant"
    );
    // In this order, every pair is safe.
    assert!(may_precede(steps[0], steps[1]), "stop then revoke is safe");
    // **The reversed order is refused, and this is the assertion the slice exists for.** Revoking first makes
    // the stop impossible: the call goes out with an invalidated token, fails, and leaves the watch registered.
    assert!(
        !may_precede(TeardownStep::RevokeGrant, TeardownStep::StopWatch),
        "revoke then stop must be refused: the stop would be sent with a token the revoke just invalidated"
    );
    // The rule is a pairing check, not a total order, so a step may precede itself — which is what makes it
    // usable when a third step is inserted rather than restated as a fixed sequence.
    assert!(may_precede(
        TeardownStep::StopWatch,
        TeardownStep::StopWatch
    ));
    assert!(may_precede(
        TeardownStep::RevokeGrant,
        TeardownStep::RevokeGrant
    ));
}

#[test]
fn a_failed_stop_must_not_leave_a_working_credential_in_place() {
    // The two halves are not equally important, and the policies say so. Asserted as a pair so a change that
    // made both `Required` — or both `BestEffort` — fails here rather than only changing what a caller does.
    let stop = TEARDOWN_PLAN[0];
    let revoke = TEARDOWN_PLAN[1];
    assert_eq!(stop.step, TeardownStep::StopWatch);
    assert_eq!(revoke.step, TeardownStep::RevokeGrant);
    assert_eq!(
        stop.policy,
        StepPolicy::BestEffort,
        "a failed stop must not abort the teardown: its failure leaves a bounded privacy window, while \
         aborting would leave a working credential"
    );
    assert_eq!(
        revoke.policy,
        StepPolicy::Required,
        "a failed revoke means the account is NOT disconnected, so the caller must be told"
    );
    assert!(stop.policy.may_be_skipped());
    assert!(!revoke.policy.may_be_skipped());
    // And the property that makes the asymmetry a decision rather than a preference: skipping the revoke leaves
    // access live (visible, actionable), while skipping the stop leaves something running that access control
    // cannot then stop.
    assert!(TeardownStep::StopWatch.skipping_leaves_a_residue());
    assert!(!TeardownStep::RevokeGrant.skipping_leaves_a_residue());
}

#[test]
fn a_skipped_stop_leaves_notifications_arriving_until_the_lease_lapses_not_for_a_few_minutes() {
    // **The consequence, and the reason a reversed teardown is a privacy leak rather than a failed call.** When
    // the stop happens, the push guide's "within a few minutes" applies. When it does not, that sentence is
    // irrelevant: nothing ends the stream but the lease running out, and nothing renews it because the grant is
    // gone. So the exposure is the lease's bound — up to seven days — and each delivery carries the mailbox
    // address, which is the value `PubsubNotification` exists to redact.
    assert_eq!(
        notification_exposure(true),
        NotificationExposure::SettlingWithinMinutes
    );
    // The figure is the lease's own, reused rather than restated, so the two cannot drift apart.
    assert_eq!(
        notification_exposure(false),
        NotificationExposure::UntilTheLeaseLapses {
            seconds: WATCH_RENEWAL_BOUND_SECONDS
        }
    );
    assert_eq!(
        notification_exposure(false),
        NotificationExposure::UntilTheLeaseLapses { seconds: 604_800 },
        "the exposure is the lease's seven-day bound, spelled out so a change to the constant is visible here"
    );
    // The two answers are not the same state with a different number, which is why this is an enum: one is a
    // qualitative provider statement with no figure in it, and the other is a figure this crate can state.
    assert_ne!(notification_exposure(true), notification_exposure(false));
}

#[test]
fn every_step_has_a_distinct_name_for_a_plan_a_log_and_a_test() {
    // A stable name per step, so a recorded plan is legible and a test names what it asserts. Asserted distinct
    // because two steps sharing a name would make a plan ambiguous in a log — the same reason `SyncCursorKind`
    // and `SecretKind` each pin their spellings.
    let names: Vec<&str> = [TeardownStep::StopWatch, TeardownStep::RevokeGrant]
        .into_iter()
        .map(TeardownStep::as_str)
        .collect();
    assert_eq!(names, vec!["stop_watch", "revoke_grant"]);
    assert_ne!(names[0], names[1]);
}
