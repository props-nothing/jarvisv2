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
    for stop in [TeardownStep::StopWatch, TeardownStep::StopCalendarChannel] {
        assert!(
            stop.needs_a_live_grant(),
            "{stop:?} is an ordinary authenticated API call, so it needs a token that still works"
        );
        assert!(
            !stop.withdraws_access(),
            "turning off notifications changes nothing about the grant"
        );
    }
    assert!(
        TeardownStep::RevokeGrant.withdraws_access(),
        "revocation is what removes the grant every stop needs"
    );
    // And the asymmetry that makes the mistake easy: the revoke does NOT need a live grant, so a caller that
    // revoked first sees its revoke succeed and gets no signal that it has just blocked the next step.
    assert!(
        !TeardownStep::RevokeGrant.needs_a_live_grant(),
        "revocation accepts an already-dead token, which is why reversing the order fails silently at the \
         *next* call rather than at this one"
    );
}

#[test]
fn the_plan_orders_the_steps_that_need_the_grant_before_the_step_that_removes_it() {
    // The plan is derived from `may_precede`, and this asserts the rule admits every adjacent pair — so a plan
    // edited into the wrong order fails here rather than shipping, and a reader can see *why* the order is what
    // it is.
    let steps: Vec<TeardownStep> = TEARDOWN_PLAN.iter().map(|planned| planned.step).collect();
    assert_eq!(
        steps,
        vec![
            TeardownStep::StopWatch,
            TeardownStep::StopCalendarChannel,
            TeardownStep::RevokeGrant
        ],
        "both stops must come first: each is a step that needs a live grant"
    );
    // Every adjacent pair is safe, checked through the rule rather than restated as an expected order — which
    // is what makes adding a fourth step a mechanical exercise instead of a re-derivation.
    for pair in steps.windows(2) {
        assert!(
            may_precede(pair[0], pair[1]),
            "`may_precede` must admit {:?} then {:?}",
            pair[0],
            pair[1]
        );
    }
    assert!(may_precede(steps[0], steps[2]), "stop then revoke is safe");
    assert!(may_precede(steps[1], steps[2]), "stop then revoke is safe");
    // **The reversed order is refused, and this is the assertion the slice exists for.** Revoking first makes
    // every stop impossible: the call goes out with an invalidated token, fails, and leaves the watch and each
    // channel registered.
    assert!(
        !may_precede(TeardownStep::RevokeGrant, TeardownStep::StopWatch),
        "revoke then stop must be refused: the stop would be sent with a token the revoke just invalidated"
    );
    assert!(
        !may_precede(TeardownStep::RevokeGrant, TeardownStep::StopCalendarChannel),
        "and the same refusal covers the Calendar channel stop, which was added without editing the rule"
    );
    // A Calendar stop may run before or after the mailbox watch's, because neither needs the other: they end
    // **different mechanisms**, so a caller with no Gmail watch may still stop its channels.
    assert!(may_precede(
        TeardownStep::StopWatch,
        TeardownStep::StopCalendarChannel
    ));
    assert!(may_precede(
        TeardownStep::StopCalendarChannel,
        TeardownStep::StopWatch
    ));
    // The rule is a pairing check, not a total order, so a step may precede itself — which is what makes it
    // usable when a third step is inserted rather than restated as a fixed sequence.
    assert!(may_precede(
        TeardownStep::StopWatch,
        TeardownStep::StopWatch
    ));
    assert!(may_precede(
        TeardownStep::StopCalendarChannel,
        TeardownStep::StopCalendarChannel
    ));
    assert!(may_precede(
        TeardownStep::RevokeGrant,
        TeardownStep::RevokeGrant
    ));
}

#[test]
fn the_two_stops_are_distinct_effects_because_one_is_a_call_per_resource() {
    // **The finding `ADR-0107` records, asserted as properties rather than as prose.** Google's two push
    // mechanisms are ended by calls of different arity: `users.stop` ends *the* mailbox watch (one resource,
    // one call), while `channels.stop` ends *a* channel and has no per-user form, so an account watching three
    // calendars needs three calls. A single `StopWatch` variant would therefore have been a claim about the two
    // mechanisms that is false for one of them.
    //
    // They share every *authority* property — which is why `may_precede` admitted the new one unchanged...
    assert_eq!(
        TeardownStep::StopWatch.needs_a_live_grant(),
        TeardownStep::StopCalendarChannel.needs_a_live_grant()
    );
    assert_eq!(
        TeardownStep::StopWatch.withdraws_access(),
        TeardownStep::StopCalendarChannel.withdraws_access()
    );
    assert_eq!(
        TeardownStep::StopWatch.skipping_leaves_a_residue(),
        TeardownStep::StopCalendarChannel.skipping_leaves_a_residue()
    );
    // ...and yet they are distinct values with distinct names, because the difference is in *what they act on*:
    // every other variant's name is a distinct effect, and a plan or a log that said "stop_watch" after stopping
    // a Calendar channel would misreport which mechanism was silenced.
    assert_ne!(TeardownStep::StopWatch, TeardownStep::StopCalendarChannel);
    assert_ne!(
        TeardownStep::StopWatch.as_str(),
        TeardownStep::StopCalendarChannel.as_str()
    );
    // And the two mechanisms cannot share one exposure figure: Gmail's is a stated bound, Calendar's is the
    // channel's own lease — see the exposure tests below, which is where that difference has teeth.
    assert_ne!(
        TeardownStep::StopWatch.as_str(),
        TeardownStep::StopCalendarChannel.as_str()
    );
}

#[test]
fn a_failed_stop_must_not_leave_a_working_credential_in_place() {
    // The halves are not equally important, and the policies say so. Asserted over the whole plan rather than
    // at two indices, so the arrival of a third step cannot quietly change what a caller does.
    let stops: Vec<PlannedStep> = TEARDOWN_PLAN
        .iter()
        .filter(|planned| planned.step.needs_a_live_grant())
        .copied()
        .collect();
    let revoke = TEARDOWN_PLAN[TEARDOWN_PLAN.len() - 1];
    assert_eq!(revoke.step, TeardownStep::RevokeGrant);
    // Every stop is best-effort, and the revoke is not — the asymmetry the module argues for.
    for stop in &stops {
        assert_eq!(
            stop.policy,
            StepPolicy::BestEffort,
            "{:?} must be best-effort: its failure leaves a bounded privacy window, while aborting would 
             leave a working credential",
            stop.step
        );
        assert!(stop.policy.may_be_skipped());
        assert!(stop.step.skipping_leaves_a_residue());
    }
    assert_eq!(
        revoke.policy,
        StepPolicy::Required,
        "a failed revoke means the account is NOT disconnected, so the caller must be told"
    );
    assert!(!revoke.policy.may_be_skipped());
    // And the property that makes the asymmetry a decision rather than a preference: skipping the revoke leaves
    // access live (visible, actionable), while skipping a stop leaves something running that access control
    // cannot then stop.
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
        gmail_exposure(true),
        NotificationExposure::SettlingWithinMinutes
    );
    // The figure is the lease's own, reused rather than restated, so the two cannot drift apart.
    assert_eq!(
        gmail_exposure(false),
        NotificationExposure::UntilTheLeaseLapses {
            seconds: WATCH_RENEWAL_BOUND_SECONDS
        }
    );
    assert_eq!(
        gmail_exposure(false),
        NotificationExposure::UntilTheLeaseLapses { seconds: 604_800 },
        "the exposure is the lease's seven-day bound, spelled out so a change to the constant is visible here"
    );
    // The two answers are not the same state with a different number, which is why this is an enum: one is a
    // qualitative provider statement with no figure in it, and the other is a figure this crate can state.
    assert_ne!(gmail_exposure(true), gmail_exposure(false));
}

#[test]
fn a_calendar_channels_exposure_is_its_own_lease_and_an_ended_channel_exposes_nothing() {
    // **The second finding, and the one a shared function could not express.** Gmail's exposure is a stated
    // constant; a Calendar channel has **no** stated bound (its life is "determined either by your request or by
    // any Google Calendar API internal limits or defaults"), so the only honest figure is the lease its own
    // `watch` response reported. A function taking one `stop_succeeded` flag could not compute this.
    let alive = |for_seconds| ChannelLease::Alive { for_seconds };
    let lapsed = |for_seconds| ChannelLease::Lapsed { for_seconds };
    assert_eq!(
        calendar_exposure(alive(90_000), true),
        NotificationExposure::SettlingWithinMinutes,
        "a stopped channel that still had lease left settles within minutes, and states no figure"
    );
    assert_eq!(
        calendar_exposure(alive(90_000), false),
        NotificationExposure::UntilTheLeaseLapses { seconds: 90_000 },
        "an unstopped channel exposes exactly ITS remaining lease, not Gmail's seven-day bound"
    );
    // A channel that had already ended exposes **nothing**, whether or not the stop was attempted. This is the
    // third state, and a `bool` function reporting `UntilTheLeaseLapses { seconds: 0 }` would have overstated a
    // teardown that is already clean.
    assert_eq!(
        calendar_exposure(lapsed(120), false),
        NotificationExposure::AlreadyEnded {
            ended_seconds_ago: 120
        }
    );
    assert_eq!(
        calendar_exposure(lapsed(120), true),
        calendar_exposure(lapsed(120), false),
        "an ended channel outranks a successful stop: it does not depend on the call at all"
    );
    // **The `stop_succeeded` guard on the live arm is what decides, and this is the falsification that showed
    // it.** Removing that guard (making every live channel report `SettlingWithinMinutes`) fails the assertion
    // above. Reordering the arms does **not** — `Lapsed` and `Alive` are different variants, so they cannot
    // shadow each other and no order changes an answer. A test that only reordered would therefore have been
    // checking the source's layout rather than the behaviour.
    assert_ne!(
        calendar_exposure(alive(1), false),
        NotificationExposure::SettlingWithinMinutes,
        "a live channel that was never stopped must not be reported as settling: the stop is what shortens it"
    );
    // The two mechanisms genuinely diverge, which is why the functions are separate rather than one with a flag.
    assert_ne!(
        calendar_exposure(alive(90_000), false),
        gmail_exposure(false)
    );
}

#[test]
fn every_step_has_a_distinct_name_for_a_plan_a_log_and_a_test() {
    // A stable name per step, so a recorded plan is legible and a test names what it asserts. Asserted distinct
    // because two steps sharing a name would make a plan ambiguous in a log — the same reason `SyncCursorKind`
    // and `SecretKind` each pin their spellings.
    let names: Vec<&str> = [
        TeardownStep::StopWatch,
        TeardownStep::StopCalendarChannel,
        TeardownStep::RevokeGrant,
    ]
    .into_iter()
    .map(TeardownStep::as_str)
    .collect();
    assert_eq!(
        names,
        vec!["stop_watch", "stop_calendar_channel", "revoke_grant"]
    );
    // Pairwise distinct, asserted over all pairs so a third step cannot be added by copying a name.
    for (index, name) in names.iter().enumerate() {
        for (other_index, other) in names.iter().enumerate() {
            if index != other_index {
                assert_ne!(name, other, "two steps must not share the name {name}");
            }
        }
    }
}
