//! What the model is told about the time.
//!
//! A model has no clock: asked "what day is it" or to schedule "tomorrow at nine" it guesses from its training data. Every
//! run therefore opens with the current date, the weekday and the local offset, so relative dates are right.
//!
//! The local offset is read **once, before the async runtime starts its threads**: the platform call that finds it is only
//! sound while a process has one thread (the `time` crate refuses otherwise), and the daemon starts single-threaded. The
//! cost is that a daemon left running across a daylight-saving change keeps the old offset until it restarts; the UTC time
//! is always given too, so the model is never wrong about the instant, only possibly an hour off about the wall clock.

use std::sync::OnceLock;

use jarvis_core::UtcTimestamp;
use time::{OffsetDateTime, UtcOffset};

static LOCAL_OFFSET: OnceLock<UtcOffset> = OnceLock::new();

/// Reads and remembers the machine's UTC offset. Call it before any thread is started; it does nothing useful after.
pub fn remember_local_offset() {
    if let Ok(offset) = UtcOffset::current_local_offset() {
        let _ = LOCAL_OFFSET.set(offset);
    }
}

/// The sentence a run is given about the present moment.
#[must_use]
pub fn clock_line(now: UtcTimestamp) -> String {
    describe(now, LOCAL_OFFSET.get().copied())
}

fn describe(now: UtcTimestamp, offset: Option<UtcOffset>) -> String {
    let Ok(utc) = OffsetDateTime::from_unix_timestamp_nanos(now.unix_nanos()) else {
        return String::new();
    };
    let utc_text = format!(
        "{:04}-{:02}-{:02} {:02}:{:02} UTC",
        utc.year(),
        u8::from(utc.month()),
        utc.day(),
        utc.hour(),
        utc.minute()
    );
    let Some(offset) = offset.filter(|offset| !offset.is_utc()) else {
        return format!("The current date and time is {} {utc_text}.", utc.weekday());
    };
    let local = utc.to_offset(offset);
    let (hours, minutes, _) = offset.as_hms();
    let sign = if offset.is_negative() { '-' } else { '+' };
    format!(
        "The current date and time is {} {:04}-{:02}-{:02} {:02}:{:02} (the user's local time, UTC{sign}{:02}:{:02}); that is {utc_text}.",
        local.weekday(),
        local.year(),
        u8::from(local.month()),
        local.day(),
        local.hour(),
        local.minute(),
        hours.unsigned_abs(),
        minutes.unsigned_abs()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> UtcTimestamp {
        text.parse()
            .unwrap_or_else(|error| panic!("a valid timestamp: {error}"))
    }

    /// **A run knows today's date, the weekday, and what the wall clock says where the user is.**
    #[test]
    fn the_line_names_the_weekday_the_local_time_and_the_instant() {
        let offset = UtcOffset::from_hms(2, 0, 0).unwrap_or_else(|error| panic!("{error}"));
        let line = describe(at("2026-10-09T23:30:00Z"), Some(offset));
        // 23:30 UTC on a Friday is already Saturday 01:30 in UTC+02:00: the date must follow the offset.
        assert!(
            line.contains("Saturday 2026-10-10 01:30") && line.contains("UTC+02:00"),
            "{line}"
        );
        assert!(line.contains("2026-10-09 23:30 UTC"), "{line}");
    }

    #[test]
    fn a_negative_offset_and_an_unknown_offset_are_both_stated_honestly() {
        let west = UtcOffset::from_hms(-5, -30, 0).unwrap_or_else(|error| panic!("{error}"));
        assert!(describe(at("2026-10-09T12:00:00Z"), Some(west)).contains("UTC-05:30"));
        let unknown = describe(at("2026-10-09T12:00:00Z"), None);
        assert_eq!(
            unknown,
            "The current date and time is Friday 2026-10-09 12:00 UTC."
        );
    }
}
