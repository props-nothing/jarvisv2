//! Tests for the watch lease.
//!
//! The load-bearing ones are about **units and the boundary**, because both fail silently: a watch expired a
//! thousand times too far in the future raises nothing, and a boundary treated as alive keeps a dead watch for
//! one more interval.
//!
//! Falsification record in `TODO.md`.

use super::*;

fn must<T, E: std::fmt::Display>(result: Result<T, E>, what: &str) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("{what}: {error}"),
    }
}

fn instant(seconds: i64) -> UtcTimestamp {
    must(
        UtcTimestamp::from_unix_nanos(i128::from(seconds) * 1_000_000_000),
        "a representable instant",
    )
}

#[test]
fn the_documented_expiration_is_millis_and_the_conversion_is_asserted() {
    // **The `users.watch` response the reference publishes, verbatim**: `{ "historyId": "1234567890",
    // "expiration": "1431990098200" }`. That value is epoch **milliseconds** — 1,431,990,098 seconds.
    //
    // A conversion that used the wrong scale does not fail, it produces a watch that dies in the year 47,000, so
    // the arithmetic is asserted against the documented number rather than left to inspection.
    let parsed = must(
        parse_watch_expiration(r#"{"historyId": "1234567890", "expiration": "1431990098200"}"#),
        "the documented watch response must parse",
    );
    assert_eq!(
        parsed.unix_nanos() / 1_000_000_000,
        1_431_990_098,
        "epoch millis must be scaled to seconds, not read as seconds or as nanos"
    );
    // And the same instant rendered, so a reader can see the unit is right rather than only the number. Google's
    // own example value is a Tuesday in May 2015, which is the check that the scale is seconds and not something
    // else. The fractional part has its trailing zeros **removed** by `Rfc3339` (`time`'s rule, also recorded in
    // `jarvis-core`'s timestamp tests), so `.200` renders as `.2` — asserted literally so a change of precision
    // would be visible here rather than silently accepted.
    assert_eq!(parsed.to_string(), "2015-05-18T23:01:38.2Z");
}

#[test]
fn the_expiration_is_a_string_and_a_number_is_refused() {
    // Google types it `string (int64 format)` because JSON cannot carry a 64-bit integer exactly as a number. A
    // parser reading a JSON number would refuse a **conforming** response, so both halves are asserted: the
    // string form parses and the number form is refused with the type error rather than silently accepted.
    assert!(parse_watch_expiration(r#"{"expiration": "1431990098200"}"#).is_ok());
    assert_eq!(
        parse_watch_expiration(r#"{"expiration": 1431990098200}"#),
        Err(WatchError::NotAString),
        "a JSON number is not the documented form, so it must be refused as the wrong type"
    );
}

#[test]
fn each_unreadable_expiration_has_its_own_error() {
    // Four distinct remedies, so four distinct errors. A single "malformed" variant would leave a caller unable
    // to tell a provider shape change from a bad unit.
    assert!(matches!(
        parse_watch_expiration("not json"),
        Err(WatchError::NotJson { .. })
    ));
    assert_eq!(
        parse_watch_expiration(r#"{"historyId": "1234567890"}"#),
        Err(WatchError::MissingExpiration),
        "a body with no expiration is a shape this connector does not understand"
    );
    assert_eq!(
        parse_watch_expiration(r#"{"expiration": null}"#),
        Err(WatchError::MissingExpiration),
        "a null expiration means the provider sent no value, which is the same remedy as an absent one"
    );
    assert_eq!(
        parse_watch_expiration(r#"{"expiration": "soon"}"#),
        Err(WatchError::NotAnInteger)
    );
    // A value large enough to overflow when scaled is a range failure rather than a wrapped instant.
    assert_eq!(
        parse_watch_expiration(r#"{"expiration": "9223372036854775807"}"#),
        Err(WatchError::OutOfRange)
    );
}

#[test]
fn a_lease_that_has_just_ended_counts_as_lapsed() {
    // **The boundary, and the direction it fails in.** The reference says the watch stops *at* `expiration`, so
    // treating the exact instant as alive would keep a dead watch for one more interval. Asserted at the edge,
    // one second either side, because a `>` / `>=` slip is invisible at any other distance.
    let expiration = instant(1_431_990_098);
    assert!(
        watch_lapse(expiration, expiration).is_lapsed(),
        "the expiry instant itself is not alive"
    );
    assert_eq!(
        watch_lapse(expiration, expiration),
        WatchLapse::Lapsed { for_seconds: 0 }
    );

    // A second before, one second of lease remains; a second after, it lapsed a second ago. Both directions, so
    // a sign error fails here rather than producing a plausible-looking report.
    assert_eq!(
        watch_lapse(expiration, instant(1_431_990_097)),
        WatchLapse::Alive { for_seconds: 1 }
    );
    assert_eq!(
        watch_lapse(expiration, instant(1_431_990_099)),
        WatchLapse::Lapsed { for_seconds: 1 }
    );
}

#[test]
fn a_lapsed_lease_reports_how_long_it_has_been_dead() {
    // The distinction a `bool` would erase: "not alive" does not say whether this just happened or whether
    // notifications have been missing for days, and those need different responses.
    let expired_two_days_ago = watch_lapse(instant(1_000_000), instant(1_000_000 + 2 * 86_400));
    assert_eq!(
        expired_two_days_ago,
        WatchLapse::Lapsed {
            for_seconds: 2 * 86_400
        }
    );
    assert_eq!(expired_two_days_ago.seconds_from_edge(), 2 * 86_400);
    assert!(expired_two_days_ago.is_lapsed());

    let alive = watch_lapse(instant(1_000_000 + 3_600), instant(1_000_000));
    assert_eq!(alive.seconds_from_edge(), 3_600);
    assert!(!alive.is_lapsed());
}

#[test]
fn the_renewal_bound_and_the_recommendation_are_two_figures() {
    // **Both from one sentence of the push guide**, and they answer different questions: "at least once every
    //  7 days" is when the watch dies, "We recommend calling `watch` once per day" is when to renew. Three
    // states, so three producers — the rule this repository applies to every enum variant.
    let last = instant(1_000_000);

    // Past the bound: notifications have stopped.
    assert_eq!(
        renewal_advice(last, instant(1_000_000 + WATCH_RENEWAL_BOUND_SECONDS)),
        RenewalAdvice::Overdue {
            since_seconds: WATCH_RENEWAL_BOUND_SECONDS
        }
    );
    // Past the recommendation, inside the bound: renew, but nothing has failed yet.
    assert_eq!(
        renewal_advice(last, instant(1_000_000 + WATCH_RENEWAL_RECOMMENDED_SECONDS)),
        RenewalAdvice::Recommended {
            since_seconds: WATCH_RENEWAL_RECOMMENDED_SECONDS
        }
    );
    // Inside the recommendation: leave it.
    assert_eq!(
        renewal_advice(last, instant(1_000_000 + 3_600)),
        RenewalAdvice::NotYet {
            since_seconds: 3_600
        }
    );

    // The three are distinct, which is the property that makes the enum worth having rather than a duration.
    let states = [
        renewal_advice(last, instant(1_000_000 + WATCH_RENEWAL_BOUND_SECONDS)),
        renewal_advice(last, instant(1_000_000 + WATCH_RENEWAL_RECOMMENDED_SECONDS)),
        renewal_advice(last, instant(1_000_000 + 3_600)),
    ];
    assert_ne!(states[0], states[1]);
    assert_ne!(states[1], states[2]);
    assert_ne!(states[0], states[2]);

    // And the two figures are pinned to the guide's own numbers, spelled out rather than derived from the same
    // expression so a typo in the arithmetic is caught rather than cancelled: 7 days and 1 day. These two
    // assertions also establish the ordering (`604_800 > 86_400`), which is what makes `Overdue` reachable —
    // a swapped pair of constants would make the first branch fire for a one-day-old watch.
    assert_eq!(WATCH_RENEWAL_BOUND_SECONDS, 604_800);
    assert_eq!(WATCH_RENEWAL_RECOMMENDED_SECONDS, 86_400);
}

#[test]
fn a_clock_skew_ahead_of_the_last_renewal_is_not_an_error() {
    // A renewal "in the future" is what a clock skew looks like, and refusing it would turn a healthy watch into
    // a failure. `NotYet` is the safe reading: the cost of being wrong is one wasted call, while the cost of
    // refusing is a caller that stops renewing. The negative elapsed time is carried rather than clamped, so a
    // dashboard can show that the clock is ahead instead of reporting zero elapsed.
    let advice = renewal_advice(instant(2_000_000), instant(1_000_000));
    assert_eq!(
        advice,
        RenewalAdvice::NotYet {
            since_seconds: -1_000_000
        }
    );
    assert!(
        !advice.should_renew(),
        "a watch renewed after now is not due for renewal"
    );
}

#[test]
fn every_state_except_not_yet_asks_for_a_renewal() {
    // `should_renew` is the one method a scheduler would call, so each state's answer is pinned — including that
    // `Recommended` renews, because a caller that only renewed on `Overdue` would run every watch to the edge of
    // its life for no benefit.
    let last = instant(1_000_000);
    assert!(
        renewal_advice(last, instant(1_000_000 + WATCH_RENEWAL_BOUND_SECONDS)).should_renew(),
        "an overdue watch must be renewed"
    );
    assert!(
        renewal_advice(last, instant(1_000_000 + WATCH_RENEWAL_RECOMMENDED_SECONDS)).should_renew(),
        "a recommended renewal must be taken, not deferred to the bound"
    );
    assert!(
        !renewal_advice(last, instant(1_000_000)).should_renew(),
        "a fresh watch must be left alone"
    );
}

#[test]
fn the_watch_response_anchors_the_first_sync_and_the_guide_uses_two_different_ids() {
    // **The load-bearing test for this slice.** The push guide's `watch` response is
    // `{ "historyId": "1234567890", "expiration": "1431990098200" }`, and two paragraphs later the guide's
    // worked example is: *"Pass `1234567890` as the `startHistoryId` to `history.list`. Afterward, you can
    // persist `9876543210` as the last known `historyId`"*.
    //
    // So the response's id is the **anchor** a first sync starts from (`1234567890`), and `9876543210` is a
    // *different* number: the position that sync ended at. A reader that returned the response's id as "the new
    // position" would store a value that never moves as the mailbox changes, and a sync from it would re-read
    // the same window forever. The two numbers in the guide are what let this test distinguish them, so both are
    // written out rather than one being derived from the other.
    let response = must(
        parse_watch_response(r#"{"historyId": "1234567890", "expiration": "1431990098200"}"#),
        "the documented watch response must parse as both fields",
    );
    assert_eq!(
        response.anchor, "1234567890",
        "the anchor is the response's historyId — the startHistoryId the guide passes to history.list"
    );
    assert_ne!(
        response.anchor, "9876543210",
        "the anchor must not be the position the resulting sync ends at; the guide uses a different number for \
         that, and conflating the two inverts the direction of a sync"
    );
    // And the expiration is read from the same body, so the struct is not one field read twice.
    assert_eq!(
        response.expires_at.unix_nanos() / 1_000_000_000,
        1_431_990_098
    );
}

#[test]
fn the_anchor_is_read_from_the_same_body_as_the_lease_and_each_missing_field_is_its_own_error() {
    // The response's two fields live in one body, so the combined reader must demand both — which is the whole
    // point of the slice: reading only the lease is what dropped the anchor. Each omission gets its own error,
    // because "the anchor is absent" and "the expiration is absent" are different problems with different
    // remedies, and a caller debugging one must not be pointed at the other.
    assert_eq!(
        parse_watch_response(r#"{"expiration": "1431990098200"}"#),
        Err(WatchError::MissingHistoryId),
        "a response with no historyId cannot anchor a sync, so it is refused rather than defaulted"
    );
    assert_eq!(
        parse_watch_response(r#"{"historyId": "1234567890"}"#),
        Err(WatchError::MissingExpiration),
        "a response with no expiration cannot be renewed by, so it is refused"
    );
    // Both absent: the expiration is reported first, because a caller that cannot tell when the lease ends
    // cannot use the anchor either.
    assert_eq!(
        parse_watch_response("{}"),
        Err(WatchError::MissingExpiration)
    );
    // A JSON null is absent, not a wrong type — the same reading the expiration reader takes, asserted here so
    // the two fields cannot drift apart on this decision.
    assert_eq!(
        parse_watch_response(r#"{"historyId": null, "expiration": "1431990098200"}"#),
        Err(WatchError::MissingHistoryId)
    );
}

#[test]
fn a_history_id_that_is_not_a_string_is_refused_rather_than_coerced() {
    // The reference types `historyId` as a string, and the trap is the mirror of the expiration's: a JSON number
    // is *not* the documented form. Coercing it would accept a response the reference says cannot occur, and it
    // would hide a provider shape change — which is the class of change this whole module exists to surface.
    assert_eq!(
        parse_watch_anchor(r#"{"historyId": 1234567890}"#),
        Err(WatchError::HistoryIdNotAString)
    );
    assert_eq!(
        parse_watch_anchor(r#"{"historyId": ["1"]}"#),
        Err(WatchError::HistoryIdNotAString)
    );
    // The control: the documented string form IS accepted, so the refusal above is about the type and not about
    // the reader refusing everything.
    assert_eq!(
        must(
            parse_watch_anchor(r#"{"historyId": "1234567890"}"#),
            "the documented string form must be accepted"
        ),
        "1234567890"
    );
    // A body that is not JSON at all is reported as a JSON problem rather than as a missing field, so the layer
    // to check is named.
    assert!(matches!(
        parse_watch_anchor("not json"),
        Err(WatchError::NotJson { .. })
    ));
}
