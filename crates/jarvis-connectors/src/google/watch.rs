//! The Gmail watch lease: when a watch stops, and when to renew it.
//!
//! # Why this module exists
//!
//! A Gmail push watch **lapses silently**. The push guide is explicit: *"You must call the `watch` method at
//! least once every 7 days or you'll stop receiving updates for the user."* Nothing fails, no error is raised,
//! and no notification arrives to say so — updates simply stop. For a machine that has no other view of the
//! mailbox, that is indistinguishable from a quiet day, so the connector must decide for itself when a lease
//! is near its end and, harder, must be able to tell a **live** watch from a **lapsed** one.
//!
//! The manifest has always *linked* the seven-day bound
//! ([`DocumentationLink`](crate::manifest::DocumentationLink) with [`LinkKind::Webhooks`](crate::manifest::LinkKind)),
//! but a link is not a decision: nothing in this crate could read a watch response or answer "is this still
//! alive", so the bound was documented and unenforced — the defect `ADR-0077` records for a bound that is not
//! applied.
//!
//! # The three facts this module is built from
//!
//! 1. **`expiration` is epoch milliseconds, carried as a JSON *string*.** The `users.watch` reference gives the
//!    response as `{ "historyId": string, "expiration": string (int64 format) }` and describes `expiration` as
//!    *"When Gmail will stop sending notifications for mailbox updates (epoch millis)."* Both halves are traps:
//!    it is a string rather than a number, and it is **milliseconds** rather than seconds. A seconds value read
//!    as milliseconds lands a watch's death a thousand times too far in the future, which is the direction that
//!    produces a **silently dead watch** rather than a visible error.
//! 2. **The renewal bound and the renewal recommendation are two figures, not one.** The bound is "at least once
//!    every 7 days"; the advice is "We recommend calling `watch` once per day". Reporting only the bound would
//!    hide the cadence a caller should actually use, and reporting only the recommendation would make the hard
//!    limit unreachable — the two-facts split `ADR-0080` records for a limit and a recommendation.
//! 3. **The bound and the `expiration` are different kinds of thing.** The bound is a *policy* figure — how often
//!    a caller should renew — while `expiration` is a *value this watch returned* — when this particular lease
//!    ends. A module that stored one as the other would renew on the wrong schedule, so both are here and the
//!    types keep them apart.
//! 4. **The response carries two facts, and the guide's own worked example uses a different number for each.**
//!    The push guide quotes `{"historyId": "1234567890", "expiration": "1431990098200"}` and then, two
//!    paragraphs later: *"Pass `1234567890` as the `startHistoryId` to `history.list`. Afterward, you can
//!    persist `9876543210` as the last known `historyId`"* — **two different ids**, where `1234567890` is the
//!    watch response's anchor and `9876543210` is the position the resulting sync ends at. So the anchor a fresh
//!    sync starts from *is* the watch response's `historyId`, and the number a renewal must return is the one
//!    the *sync* produced. Reading the response's id as the new position, or the sync's end as the anchor,
//!    inverts the direction of a sync — silently, because both are plausible values of the same type.
//!
//! # What is not verified
//!
//! **No request has been sent and no watch response has been observed.** As everywhere in this crate, the
//! behaviour is implemented from the dated record in `docs/research/integrations/google.md` and tested against
//! values built from it, so the tests prove this module implements the *record*.

use jarvis_core::UtcTimestamp;

/// The field a `users.watch` response carries the anchor for a first sync in.
const HISTORY_ID_FIELD: &str = "historyId";

/// How long Gmail guarantees a watch without renewal, in seconds.
///
/// Seven days, from the push guide: *"You must call the `watch` method at least once every 7 days or you'll stop
/// receiving updates for the user."* This is the **bound** — the interval beyond which notifications stop — and
/// it is a provider figure rather than a JARVIS choice, which is why it is stated here in the provider's own
/// unit (days) and spelled in seconds once.
pub const WATCH_RENEWAL_BOUND_SECONDS: i64 = 7 * 24 * 60 * 60;

/// The renewal cadence Google **recommends**, in seconds.
///
/// One day, from the same sentence: *"We recommend calling `watch` once per day."* Kept separate from
/// [`WATCH_RENEWAL_BOUND_SECONDS`] because they answer different questions — the bound says when a watch
/// *dies*, the recommendation says when to *renew* — and a caller that treated one as the other would either
/// renew six days too late or waste six calls. `ADR-0080` records the same split for a rate limit and a
/// recommendation.
pub const WATCH_RENEWAL_RECOMMENDED_SECONDS: i64 = 24 * 60 * 60;

/// Why a watch response's `expiration` could not be read.
///
/// Four distinct variants rather than one "malformed" error, because the remedies differ: a body that is not
/// JSON points at the response, a missing field points at the provider's shape, and a value that is not a string
/// or not an integer points at the type — and a caller that conflated them would be debugging the wrong layer.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum WatchError {
    /// The body was not JSON of the documented shape.
    #[error("a watch response body must be JSON: {reason}")]
    NotJson {
        /// What went wrong.
        reason: &'static str,
    },
    /// The body carried no `expiration` field.
    ///
    /// **Distinct from a wrong type, and it is the case worth naming.** The reference documents `expiration` on
    /// a successful `watch`, so a response without one is a shape the connector does not understand rather than
    /// a watch with no end — and treating "absent" as "never expires" would build a lease that never renews.
    #[error("a watch response must carry an `expiration` field, which is when the watch stops")]
    MissingExpiration,
    /// The body carried no `historyId` field.
    ///
    /// **The other half of the response, and the one that is easy to drop.** The push guide says the response
    /// "contains the current mailbox `historyId`" and the reference lists `historyId` and `expiration` as the
    /// response's two fields. That id is the **anchor a first sync starts from** — the guide's worked example
    /// passes the response's `1234567890` as `startHistoryId` — so a response without one is a response whose
    /// sync cannot be anchored. Refusing is the fail-closed reading: a caller that instead defaulted to "no
    /// anchor" would sync from the beginning of the mailbox, which is the expensive case rather than the safe
    /// one.
    #[error("a watch response must carry a `historyId`, which anchors the first sync")]
    MissingHistoryId,
    /// The `historyId` was present but not a string.
    #[error(
        "a watch `historyId` is documented as a string, but this response carried a different type"
    )]
    HistoryIdNotAString,
    /// The `expiration` was present but not the documented `string (int64 format)`.
    #[error(
        "a watch `expiration` is documented as a string containing an integer, but this response carried a \
             different type"
    )]
    NotAString,
    /// The `expiration` was a string that did not contain a whole number.
    #[error("a watch `expiration` must be a whole number of milliseconds, but this one was not")]
    NotAnInteger,
    /// The `expiration` was a whole number outside the representable range.
    ///
    /// **The variant that catches a seconds/milliseconds confusion.** A value in the wrong unit is still a
    /// number, so no type check refuses it; what refuses it is that the range differs. Epoch **milliseconds**
    /// around now are about `1.7e12`, while a seconds value is about `1.7e9` — and `1.7e9` milliseconds is a
    /// 1970 instant, which is representable and therefore *not* caught here. So this variant catches the
    /// opposite mistake (a millis value used where nanos were expected, or an absurd value), and the unit is
    /// pinned by the test rather than by this range check — a distinction stated because it is easy to believe a
    /// range check covers a unit confusion when it does not.
    #[error("a watch `expiration` is outside the range of instants this platform represents")]
    OutOfRange,
}

/// The document's `expiration` field name, spelled once.
const EXPIRATION_FIELD: &str = "expiration";

/// Reads a `users.watch` response's `expiration` as an instant.
///
/// # The two traps, both handled here
///
/// **It is a string.** The reference types it `string (int64 format)`, which is Google's convention for a 64-bit
/// integer that JSON cannot carry exactly as a number — so a response gives `"1431990098200"` and **not**
/// `1431990098200`. A parser reading a JSON number would refuse a conforming response.
///
/// **It is milliseconds.** `UtcTimestamp` is nanosecond-precision and its constructor takes **nanos**, so the
/// value is scaled by one million and not by one thousand. Getting this wrong does not produce an error — it
/// produces a watch that appears to expire in the year 47,000, so the multiplication is asserted in a test
/// against a real documented value rather than left to inspection.
///
/// # Errors
///
/// Returns a [`WatchError`] for a body that is not the documented shape. Each variant names the specific defect,
/// because "the expiration is unreadable" would not tell a caller whether to check the response, the field, or
/// the unit.
pub fn parse_watch_expiration(body: &str) -> Result<UtcTimestamp, WatchError> {
    let value: serde_json::Value = serde_json::from_str(body).map_err(|_| WatchError::NotJson {
        reason: "the body did not parse",
    })?;
    let Some(field) = value.get(EXPIRATION_FIELD) else {
        return Err(WatchError::MissingExpiration);
    };
    // A JSON `null` is treated as absent rather than as a wrong type, because `null` means "the provider sent no
    // value" and the remedy is the same as a missing field: the watch's end is unknown.
    if field.is_null() {
        return Err(WatchError::MissingExpiration);
    }
    let Some(text) = field.as_str() else {
        return Err(WatchError::NotAString);
    };
    let millis: i64 = text.parse().map_err(|_| WatchError::NotAnInteger)?;
    // A negative instant is rejected by the constructor below; the multiplication is checked so that a value
    // large enough to overflow i64 does not silently wrap into a plausible-looking instant.
    let nanos = i64::checked_mul(millis, 1_000_000).ok_or(WatchError::OutOfRange)?;
    UtcTimestamp::from_unix_nanos(i128::from(nanos)).map_err(|_| WatchError::OutOfRange)
}

/// Reads a `users.watch` response's `historyId`: the anchor a first sync starts from.
///
/// # Why this is not the same number as the position a sync ends at
///
/// The push guide states it plainly, and the guide's own example is the proof:
///
/// > "The response contains the current mailbox `historyId` for the user. Your client receives notifications
/// > for all changes **after** that `historyId`."
///
/// and, two paragraphs later, with a *different* number:
///
/// > "Pass `1234567890` as the `startHistoryId` to `history.list`. Afterward, you can persist `9876543210` as
/// > the last known `historyId` for future use cases."
///
/// Here `1234567890` is the watch response's `historyId` — the anchor — and `9876543210` is where the sync that
/// followed ended. **Two ids in one worked example, at opposite ends of the same operation.** So this value is
/// the *from* of a first sync; the *to* is whatever `history.list` reports, which
/// [`crate::google::client::advance_gmail_history`] already carries as
/// [`crate::google::client::SyncSignal::Advanced`]. Returning the anchor here and letting that function compute
/// the successor is what keeps the two from being confused: a caller that stored this id as the new position
/// would re-read the mailbox from a fixed point forever, because the anchor does not move when the mailbox does.
///
/// # Why the value is kept as text rather than converted
///
/// `historyId` is compared and passed through, never arithmetically combined — the ordering check in
/// `advance_gmail_history` parses it only to decide direction — and `SyncCursor::new` applies its own bound to a
/// token. Converting here would invent a numeric type for a value whose only operations are "equal", "greater"
/// and "put in a query string".
///
/// # Errors
///
/// Returns [`WatchError::MissingHistoryId`] for a response with no `historyId`, or
/// [`WatchError::HistoryIdNotAString`] for one whose `historyId` is another JSON type. Both are distinct from
/// [`WatchError::MissingExpiration`]: the remedies differ — an anchor that is absent makes the first sync
/// unanchorable, while an expiration that is absent makes the lease unreadable, and a caller debugging one
/// should not be pointed at the other.
pub fn parse_watch_anchor(body: &str) -> Result<String, WatchError> {
    let value: serde_json::Value = serde_json::from_str(body).map_err(|_| WatchError::NotJson {
        reason: "the body did not parse",
    })?;
    let Some(field) = value.get(HISTORY_ID_FIELD) else {
        return Err(WatchError::MissingHistoryId);
    };
    // `null` is treated as absent, matching the expiration reader: `null` means the provider sent no value, and
    // the remedy is the same as a field that was not there.
    if field.is_null() {
        return Err(WatchError::MissingHistoryId);
    }
    let Some(text) = field.as_str() else {
        return Err(WatchError::HistoryIdNotAString);
    };
    Ok(text.to_owned())
}

/// A `users.watch` response: both of its documented fields, read together.
///
/// # Why the two travel together
///
/// The response carries exactly two values — the reference gives it as
/// `{ "historyId": string, "expiration": string (int64 format) }` — and a caller needs **both**: the anchor to
/// start a sync from, and the instant to renew the lease by. Until this type existed only the second was read,
/// so a caller that consumed the connector's one watch reader got a lease with **no anchor**, and the first
/// sync after a `watch` had nowhere documented to start. It is the "a value with a producer and no consumer"
/// shape from the other direction: an output the response supplies and nothing read (`ADR-0092`).
///
/// **[`parse_watch_response`] is the reader to use, and the two field readers are its parts.** They are public
/// because each has its own traps worth testing in isolation — a unit confusion and a wrong JSON type — but a
/// caller holding this struct cannot have read one field and forgotten the other, which is precisely the defect
/// that made the anchor worth a slice.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WatchResponse {
    /// The mailbox's `historyId` at the moment the watch was set: the `startHistoryId` for a first sync.
    ///
    /// **Not the position a sync ends at.** See [`parse_watch_anchor`] — the guide's own example uses two
    /// different ids, and this is the *from*.
    pub anchor: String,
    /// When the watch stops sending notifications.
    pub expires_at: UtcTimestamp,
}

/// Reads a `users.watch` response into **both** of its fields.
///
/// # Errors
///
/// Returns the first [`WatchError`] either field reader produces, so the caller sees the specific defect — a
/// missing anchor and a missing expiration are different problems with different remedies. The `expiration` is
/// read first because a caller that cannot tell when the lease ends cannot use the anchor either, and a
/// response that fails both should report the fact that stops the watch working at all.
pub fn parse_watch_response(body: &str) -> Result<WatchResponse, WatchError> {
    let expires_at = parse_watch_expiration(body)?;
    let anchor = parse_watch_anchor(body)?;
    Ok(WatchResponse { anchor, expires_at })
}

/// Whether a watch is still alive, and how much of its lease remains.
///
/// Two variants and not a `bool`, because the two states carry different information: a live watch has time
/// left, and a lapsed one has been lapsed for some period — and the second is what tells a caller whether this
/// just happened or whether notifications have been missing for days. `ADR-0035`'s "a boolean standing for more
/// than two situations is an enum".
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WatchLapse {
    /// The lease has ended: the watch is no longer delivering notifications.
    Lapsed {
        /// How long ago the lease ended, in seconds. At least zero.
        for_seconds: i64,
    },
    /// The lease is still running.
    Alive {
        /// How long remains, in seconds. At least zero, and zero only at the exact expiry instant.
        for_seconds: i64,
    },
}

impl WatchLapse {
    /// Returns whether the watch has stopped delivering notifications.
    #[must_use]
    pub const fn is_lapsed(self) -> bool {
        matches!(self, Self::Lapsed { .. })
    }

    /// Returns the magnitude of the interval, in seconds, whichever state this is.
    ///
    /// A helper for a caller that only wants "how far from the edge am I", which is what a dashboard shows. It
    /// deliberately does **not** flatten the state: the two variants remain distinguishable through
    /// [`Self::is_lapsed`], so a caller that needs the direction still has it.
    #[must_use]
    pub const fn seconds_from_edge(self) -> i64 {
        match self {
            Self::Lapsed { for_seconds } | Self::Alive { for_seconds } => for_seconds,
        }
    }
}

/// Decides whether a watch whose lease ends at `expiration` is still delivering, as of `now`.
///
/// # The comparison is made in nanoseconds, and that is the point
///
/// A lease ends at an **instant**, so the boundary is decided on the nanosecond difference and not on whole
/// seconds. Comparing truncated seconds would call a watch with half a second left either alive (a rounding that
/// keeps a dead watch) or lapsed (a rounding that renews early), and neither is a statement about the lease.
/// The `for_seconds` field is the whole-second *magnitude*, computed after the direction is decided, so it is
/// allowed to be zero in **either** variant — `Lapsed { 0 }` is the exact expiry instant and `Alive { 0 }` is a
/// fraction of a second of remaining lease.
///
/// **The exact expiry instant counts as lapsed.** The reference says the watch stops *at* that time, and the
/// direction matters: treating the boundary as alive would keep a dead watch for one more interval, which is the
/// silent failure this module exists to remove.
#[must_use]
pub fn watch_lapse(expiration: UtcTimestamp, now: UtcTimestamp) -> WatchLapse {
    let difference = expiration.unix_nanos() - now.unix_nanos();
    if difference > 0 {
        WatchLapse::Alive {
            for_seconds: nanos_to_seconds(difference),
        }
    } else {
        // Negated explicitly rather than taken as an absolute value, so a caller reading the source sees that
        // `for_seconds` is never negative in either variant.
        WatchLapse::Lapsed {
            for_seconds: nanos_to_seconds(-difference),
        }
    }
}

/// Converts a nanosecond difference to whole seconds, saturating at the `i64` bounds.
///
/// **Saturating rather than wrapping**, and the direction is chosen: a difference that large means the two
/// instants are centuries apart, so any large value gives the same answer to a caller asking "is this watch
/// alive" — while a wrap could flip the sign and call a lapsed watch alive, which is the silent failure the
/// module exists to remove.
///
/// **Signed, because one caller needs a negative.** [`renewal_advice`] computes an elapsed time that may be
/// negative when the system clock is behind the renewal, and it reports that rather than refusing — so the
/// saturation is asymmetric and a negative value keeps its sign. Truncation toward zero keeps the magnitude
/// exact for a sub-second difference, which is represented as zero seconds after the direction has already been
/// decided (by [`watch_lapse`], in nanoseconds).
fn nanos_to_seconds(nanos: i128) -> i64 {
    i64::try_from(nanos / 1_000_000_000).unwrap_or(if nanos < 0 { i64::MIN } else { i64::MAX })
}

/// What a caller should do about a watch, by when it was last renewed.
///
/// Three variants, and they are **ordered by urgency** rather than by elapsed time: `Overdue` past the
/// provider's bound, `Recommended` past the provider's advice, `NotYet` before either. The order is why this is
/// an enum and not a duration — a caller wants "renew now", "renew soon" or "leave it", and a number would leave
/// it to re-derive the thresholds here, which is the duplication this module removes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenewalAdvice {
    /// Past the seven-day bound. Notifications have stopped or are about to.
    Overdue {
        /// How long since the last renewal, in seconds.
        since_seconds: i64,
    },
    /// Past the recommended daily cadence but still inside the bound.
    Recommended {
        /// How long since the last renewal, in seconds.
        since_seconds: i64,
    },
    /// Inside the recommended cadence. Nothing to do.
    NotYet {
        /// How long since the last renewal, in seconds.
        since_seconds: i64,
    },
}

impl RenewalAdvice {
    /// Returns whether a renewal should be attempted now.
    ///
    /// True for [`Self::Overdue`] and [`Self::Recommended`], because both are past a figure the provider named —
    /// one a bound and one a recommendation — and a caller that only renews on `Overdue` is running a watch to
    /// the edge of its life for no benefit. `NotYet` is the only state that means "leave it".
    #[must_use]
    pub const fn should_renew(self) -> bool {
        !matches!(self, Self::NotYet { .. })
    }
}

/// Decides what to do about a watch last renewed at `last_renewed`, as of `now`.
///
/// **A renewal in the future is `NotYet`, not an error.** A clock skew between JARVIS and the provider would
/// otherwise turn a healthy watch into a refusal, and "renew later" is the safe reading — the opposite mistake,
/// renewing early, costs one call.
#[must_use]
pub fn renewal_advice(last_renewed: UtcTimestamp, now: UtcTimestamp) -> RenewalAdvice {
    let elapsed = nanos_to_seconds(now.unix_nanos() - last_renewed.unix_nanos());
    if elapsed >= WATCH_RENEWAL_BOUND_SECONDS {
        RenewalAdvice::Overdue {
            since_seconds: elapsed,
        }
    } else if elapsed >= WATCH_RENEWAL_RECOMMENDED_SECONDS {
        RenewalAdvice::Recommended {
            since_seconds: elapsed,
        }
    } else {
        RenewalAdvice::NotYet {
            since_seconds: elapsed,
        }
    }
}

#[cfg(test)]
#[path = "watch_tests.rs"]
mod tests;
