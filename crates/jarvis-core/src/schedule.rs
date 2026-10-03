//! When a scheduled task fires: the cadence rules, with no clock and no storage.
//!
//! # What this deliberately is not
//!
//! **There is no "daily at 08:00" here.** A wall-clock time needs a time zone, and a time zone needs daylight-saving
//! rules: a local 02:30 that does not exist on one night of the year, and a 01:30 that exists twice on another.
//! `P6-003` owns that, and a half-right version would fire a morning briefing an hour late for six months of the year
//! without anyone being told. So the two cadences here are the ones that mean the same thing everywhere:
//!
//! - **`Every`** — every N seconds from when it was created. "Every 24 hours" is honest about being 24 hours.
//! - **`Once`** — at one UTC instant.
//!
//! # Missed fires are skipped, not replayed
//!
//! A laptop that slept through six fires of an hourly task has **one** fire due when it wakes, not six. The next
//! fire is `now + interval`, never `previous + interval`: replaying the backlog would answer a question the user
//! asked about *then* with a flood of runs *now*, each possibly parked on an approval. Skipping is the product
//! decision, and [`Cadence::next_after`] is where it is made.

use thiserror::Error;

use crate::timestamp::UtcTimestamp;

/// The shortest interval a task may repeat at, in seconds.
///
/// A task is a model run, which is a paid and slow thing; a one-second loop is a runaway, not a schedule.
pub const MIN_INTERVAL_SECONDS: u64 = 60;

/// The longest interval, in seconds (366 days).
pub const MAX_INTERVAL_SECONDS: u64 = 366 * 24 * 60 * 60;

/// The longest objective a schedule may carry, in characters. Matches a conversation turn.
pub const MAX_SCHEDULE_OBJECTIVE_CHARS: usize = 4096;

/// How many schedules one workspace may hold.
pub const MAX_SCHEDULES_PER_WORKSPACE: usize = 50;

/// Why a schedule request was refused.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum InvalidSchedule {
    /// The interval text was not a whole number followed by `s`, `m`, `h`, or `d`.
    #[error("an interval is a whole number and a unit: 90s, 30m, 6h, or 2d")]
    IntervalSyntax,
    /// The interval is below the minimum.
    #[error("an interval must be at least {MIN_INTERVAL_SECONDS} seconds")]
    IntervalTooShort,
    /// The interval is above the maximum.
    #[error("an interval must be at most 366 days")]
    IntervalTooLong,
    /// A one-off time that has already passed.
    #[error("a one-off time must be in the future")]
    InThePast,
    /// Neither `every` nor `at`, or both.
    #[error("give exactly one of an interval (every) or a time (at)")]
    ExactlyOneCadence,
    /// The objective was blank.
    #[error("a scheduled task needs an objective")]
    EmptyObjective,
    /// The objective is longer than a turn may be.
    #[error("a scheduled objective is at most {MAX_SCHEDULE_OBJECTIVE_CHARS} characters")]
    ObjectiveTooLong,
}

/// When a task fires.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Cadence {
    /// Repeatedly, this many seconds apart.
    Every(u64),
    /// Once, at this instant.
    Once(UtcTimestamp),
}

impl Cadence {
    /// Builds a repeating cadence from validated seconds.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidSchedule`] when the interval is outside the supported range.
    pub fn every_seconds(seconds: u64) -> Result<Self, InvalidSchedule> {
        if seconds < MIN_INTERVAL_SECONDS {
            return Err(InvalidSchedule::IntervalTooShort);
        }
        if seconds > MAX_INTERVAL_SECONDS {
            return Err(InvalidSchedule::IntervalTooLong);
        }
        Ok(Self::Every(seconds))
    }

    /// Builds a one-off cadence, refusing an instant that is not after `now`.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidSchedule::InThePast`] for an instant that has passed.
    pub fn once_at(at: UtcTimestamp, now: UtcTimestamp) -> Result<Self, InvalidSchedule> {
        if at <= now {
            return Err(InvalidSchedule::InThePast);
        }
        Ok(Self::Once(at))
    }

    /// The first time the task fires, given when it was created.
    #[must_use]
    pub fn first_fire(self, created: UtcTimestamp) -> UtcTimestamp {
        match self {
            Self::Every(seconds) => advance(created, seconds),
            Self::Once(at) => at,
        }
    }

    /// The next fire after one that happened at `now`, or `None` when the task is finished.
    ///
    /// Always measured from **`now`**, never from the fire time that was due: a backlog is skipped, not replayed.
    #[must_use]
    pub fn next_after(self, now: UtcTimestamp) -> Option<UtcTimestamp> {
        match self {
            Self::Every(seconds) => Some(advance(now, seconds)),
            Self::Once(_) => None,
        }
    }
}

/// Adds whole seconds to an instant, saturating at the representable maximum rather than wrapping.
fn advance(from: UtcTimestamp, seconds: u64) -> UtcTimestamp {
    let nanos = from
        .unix_nanos()
        .saturating_add(i128::from(seconds).saturating_mul(1_000_000_000));
    UtcTimestamp::from_unix_nanos(nanos).unwrap_or(from)
}

/// Parses an interval such as `90s`, `30m`, `6h` or `2d` into seconds.
///
/// # Errors
///
/// Returns [`InvalidSchedule::IntervalSyntax`] for anything else, and the range errors for a value outside
/// [`MIN_INTERVAL_SECONDS`]..=[`MAX_INTERVAL_SECONDS`].
pub fn parse_interval(text: &str) -> Result<u64, InvalidSchedule> {
    let text = text.trim();
    let split = text
        .find(|character: char| !character.is_ascii_digit())
        .ok_or(InvalidSchedule::IntervalSyntax)?;
    let (digits, unit) = text.split_at(split);
    let amount: u64 = digits
        .parse()
        .map_err(|_| InvalidSchedule::IntervalSyntax)?;
    let multiplier = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86_400,
        _ => return Err(InvalidSchedule::IntervalSyntax),
    };
    let seconds = amount
        .checked_mul(multiplier)
        .ok_or(InvalidSchedule::IntervalTooLong)?;
    Cadence::every_seconds(seconds).map(|_| seconds)
}

/// Checks a scheduled objective and returns it trimmed.
///
/// # Errors
///
/// Returns [`InvalidSchedule::EmptyObjective`] or [`InvalidSchedule::ObjectiveTooLong`].
pub fn validate_objective(objective: &str) -> Result<String, InvalidSchedule> {
    let trimmed = objective.trim();
    if trimmed.is_empty() {
        return Err(InvalidSchedule::EmptyObjective);
    }
    if trimmed.chars().count() > MAX_SCHEDULE_OBJECTIVE_CHARS {
        return Err(InvalidSchedule::ObjectiveTooLong);
    }
    Ok(trimmed.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: i128) -> UtcTimestamp {
        UtcTimestamp::from_unix_nanos(1_774_000_000_000_000_000 + seconds * 1_000_000_000)
            .unwrap_or_else(|error| panic!("{error}"))
    }

    #[test]
    fn intervals_parse_in_each_unit_and_refuse_everything_else() {
        assert_eq!(parse_interval("90s"), Ok(90));
        assert_eq!(parse_interval("30m"), Ok(1800));
        assert_eq!(parse_interval(" 6h "), Ok(21_600));
        assert_eq!(parse_interval("2d"), Ok(172_800));
        for bad in ["", "m", "30", "30 m", "1.5h", "-5m", "5w", "0x10m", "30M"] {
            assert_eq!(
                parse_interval(bad),
                Err(InvalidSchedule::IntervalSyntax),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn the_interval_bounds_are_enforced_at_both_ends() {
        assert_eq!(
            parse_interval("59s"),
            Err(InvalidSchedule::IntervalTooShort)
        );
        assert_eq!(parse_interval("60s"), Ok(60));
        assert_eq!(parse_interval("366d"), Ok(MAX_INTERVAL_SECONDS));
        assert_eq!(
            parse_interval("367d"),
            Err(InvalidSchedule::IntervalTooLong)
        );
        assert_eq!(
            parse_interval("99999999999999999999d"),
            Err(InvalidSchedule::IntervalSyntax)
        );
        assert_eq!(
            parse_interval("18446744073709551615d"),
            Err(InvalidSchedule::IntervalTooLong)
        );
    }

    /// **A backlog is skipped, not replayed.** The next fire is measured from when this one *happened*.
    #[test]
    fn the_next_fire_is_measured_from_now_not_from_when_it_was_due() {
        let every_hour = Cadence::every_seconds(3600).unwrap_or_else(|error| panic!("{error}"));
        let created = at(0);
        assert_eq!(every_hour.first_fire(created), at(3600));
        // The daemon was down for ten hours; the task fires once, late, and the next one is an hour after *that*.
        let woke = at(36_000);
        assert_eq!(every_hour.next_after(woke), Some(at(39_600)));
    }

    #[test]
    fn a_one_off_fires_once_and_is_then_finished() {
        let once = Cadence::once_at(at(500), at(0)).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(once.first_fire(at(0)), at(500));
        assert_eq!(once.next_after(at(500)), None);
    }

    #[test]
    fn a_one_off_in_the_past_or_now_is_refused() {
        assert_eq!(
            Cadence::once_at(at(0), at(0)),
            Err(InvalidSchedule::InThePast)
        );
        assert_eq!(
            Cadence::once_at(at(-1), at(0)),
            Err(InvalidSchedule::InThePast)
        );
    }

    #[test]
    fn an_objective_is_trimmed_and_bounded() {
        assert_eq!(
            validate_objective("  check the weather \n"),
            Ok("check the weather".to_owned())
        );
        assert_eq!(
            validate_objective("   "),
            Err(InvalidSchedule::EmptyObjective)
        );
        assert_eq!(
            validate_objective(&"x".repeat(MAX_SCHEDULE_OBJECTIVE_CHARS + 1)),
            Err(InvalidSchedule::ObjectiveTooLong)
        );
        assert!(validate_objective(&"x".repeat(MAX_SCHEDULE_OBJECTIVE_CHARS)).is_ok());
    }
}
