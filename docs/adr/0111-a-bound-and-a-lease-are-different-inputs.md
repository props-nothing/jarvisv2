# ADR-0111: A bound and a lease are different inputs, so the exposure that took one could not report the state the other reaches

- **Status:** Accepted
- **Date:** 2026-10-01
- **Slice:** `P5-005` (the Google connector — ending a Gmail push watch). Closes the wiring gap `ADR-0107`
  recorded as its own third revisit condition.
- **Relates to:** `ADR-0107` (which named this gap and argued why the two mechanisms' figures are computed by
  two functions), `ADR-0106` (the channel lease this mirrors, and the `watch` response it is read from),
  `ADR-0092` (a value whose consumer is asserted in prose — here a **lease** the caller holds and the figure
  cannot accept), `ADR-0087` (the Gmail watch lease's unit, which is what makes a lease comparable to a clock
  at all), and `ADR-0035` (an enum rather than a `bool`, applied here to *which input is held* rather than to
  the state).

## Context

`ADR-0107` built `gmail_exposure(stop_succeeded) -> NotificationExposure` from Google's **mechanism bound** —
*"You must call the `watch` method at least once every 7 days"*, `WATCH_RENEWAL_BOUND_SECONDS` — and built
`calendar_exposure(lease, stop_succeeded)` from a channel's **own lease**, because a channel has no stated
bound. It then recorded, in the "Conditions that would justify revisiting" section, that the Gmail figure was
**one-sided**:

> "Gmail's exposure is computed from a **lease** rather than the documented bound, which would let
> `gmail_exposure` report `AlreadyEnded` too — the caller already holds the watch's `expiration`, so this is a
> wiring gap rather than a missing fact."

Three facts made that gap cost something, and all three were already in this crate:

1. **The caller really does hold the lease.** `crate::google::watch::parse_watch_response` reads the `watch`
   response's `expiration` into a `UtcTimestamp`, and `watch_lapse(expiration, now)` turns it into a
   `WatchLapse { Lapsed | Alive }`. So the value the answer needs is parsed, typed and one call away.
2. **`NotificationExposure` already had the third state.** `AlreadyEnded { ended_seconds_ago }` existed and was
   reachable **only** from Calendar, while its own doc had to explain which mechanism could not reach it.
3. **The bound is an overstatement when a lease is held.** After a watch whose lease has already lapsed, the
   bound-only figure reports `UntilTheLeaseLapses { seconds: 604_800 }` — *"up to seven days of exposure"* — for
   a teardown with **nothing left to silence**. That is the same overstatement `ADR-0107` fixed for Calendar by
   adding `AlreadyEnded`, left standing for Gmail.

The question was never whether Gmail *has* the fact. It was what shape carries it, and the answer is not a
parameter: a **bound** and a **lease** are different kinds of input, not two encodings of one.

## Decision

**1. A second Gmail function takes the lease; the bound-only one is unchanged and keeps its caller.**

```rust
// The mechanism's stated limit — for a caller that holds the bound and no particular watch.
pub const fn gmail_exposure(stop_succeeded: bool) -> NotificationExposure;

// This watch's own lease — for a caller disconnecting an account whose expiry it just read.
pub fn gmail_watch_exposure(lease: WatchLapse, stop_succeeded: bool) -> NotificationExposure;
```

The new function's answers:

| lease | `stop_succeeded` | exposure |
| --- | --- | --- |
| `Lapsed { s }` | either | `AlreadyEnded { ended_seconds_ago: s }` |
| `Alive { s }` | `true` | `SettlingWithinMinutes` |
| `Alive { s }` | `false` | `UntilTheLeaseLapses { seconds: s }` — **the lease's own seconds**, not the bound |

**2. Why a second function and not `gmail_exposure(lease: Option<WatchLapse>, stop_succeeded)`.**

The two calls answer the same question from **different inputs**, and the existing caller keeps the input it
has. An `Option` parameter would give every existing call site a second argument to pass `None`, and would make
"the mechanism's limit" and "this watch's expiry" two spellings of one call — the flag-plus-a-meaningless-
`Option` shape `NotificationExposure`'s own doc already rejects for the mechanism itself. `ADR-0107` removed a
`PushMechanism` enum for the same reason: **the function choice names the mechanism, so a value that only
distinguishes the two is a consumerless value.** The same argument now applies to the *input*: the function
choice names which input is held.

**3. The bound-only figure is kept rather than replaced, because it still answers a real question.**

A scheduler deciding when to renew holds the mechanism's limit and **no** particular watch — that is precisely
the caller `WATCH_RENEWAL_BOUND_SECONDS` exists for. So `gmail_exposure` is not made dead: the two functions
serve two callers, and a test asserts both remain reachable and give their own answers.

**4. The live arm reports the lease's own seconds, not the bound.**

When the lease is live and the stop did not happen, the remaining time is **this watch's**, and the reference
warns the actual expiry *"may return shorter than requested"*. Falling back to the bound here would overstate a
nearly-expired watch in exactly the direction the bound-only figure was already wrong. A falsification replaces
the lease's seconds with `WATCH_RENEWAL_BOUND_SECONDS` and the test fails.

## Consequences

- **Gmail's exposure can now reach every state `NotificationExposure` has**, so the enum's variants are all
  reachable from both mechanisms' code and none carries a "this mechanism cannot reach me" caveat. The
  `AlreadyEnded` doc was corrected to describe which **input** reaches it rather than which **mechanism** cannot.
- **A caller holding a `WatchLapse` is no longer told the bound.** The overstated figure — "up to seven days"
  for an already-dead watch — is unreachable through the function such a caller would use.
- **A dangling doc link was fixed in the same slice, and it was the same class as the defect.** The
  `AlreadyEnded` doc linked `[PushMechanism::CalendarChannel]` — an enum the *same doc paragraph* says was
  **removed** in `ADR-0107`. Nothing compiled it because `cargo doc` warnings are not denied; the link is now
  `calendar_exposure`/`gmail_exposure`/`gmail_watch_exposure`, and the same edit made the removed enum's
  surviving mention consistent with its own note.
- **Two guards falsified A-B-A**, both compiling: (1) inverting `gmail_watch_exposure`'s `stop_succeeded` guard →
  **detected** (`left: SettlingWithinMinutes`, `right: UntilTheLeaseLapses { seconds: 120000 }`); (2) replacing
  the live arm's `for_seconds` with `WATCH_RENEWAL_BOUND_SECONDS` → **detected** (`left: UntilTheLeaseLapses {
  seconds: 604800 }`, `right: … { seconds: 120000 }`). A third mutation — reordering the arms — changes nothing,
  because `Lapsed` and `Alive` are disjoint variants; that is `ADR-0107`'s lesson applied again rather than
  re-learned, and the test says so rather than re-falsifying it.
- **A sibling test asymmetry was closed.** `ADR-0109` recorded that `calendar_signal`'s 200 arm had no unit test
  while Gmail's did, and that a mutant routing a **page token through as the sync position** survived the whole
  `--lib` suite as a result. `CalendarContinuation::storable` and `page_token` were exercised only through
  `calendar_signal`; a test now asserts directly that the two accessors are **disjoint** (no value offers both a
  page to fetch and a position to store) and that `Rejected` yields neither. The same mutant — `storable`
  returning the page token — is now **detected in `--lib`**, confirmed by mutation.
- **A defect in the research record was found while appending to its Verification Log.** The findings were
  numbered `…20, 22, 23`: `ADR-0108`'s commit renamed the then-existing Finding 21 to 23 and inserted a new
  Finding 22, so **no Finding 21 existed** and two subsequent slices appended without noticing. Renumbered to
  `21 → historyId storing rule` and `22 → two stops of different arity`, which is **contiguous** with the
  `1..=20` that already preceded them. No other file referenced a Finding by number, so the renumber is
  confined to the record.

## Alternatives considered

- **`gmail_exposure(lease: Option<WatchLapse>, stop_succeeded: bool)`.** Rejected (§2): a `None` at every
  existing call site, and two inputs made to look like one parameterised call. The function choice already
  names which input is held, which is what the removed `PushMechanism` would have restated as a value.
- **Replace `gmail_exposure` entirely with the lease-aware form.** Rejected: a scheduler holds the mechanism
  bound and no watch, so deleting the bound-only form removes the answer for its only current caller. Two
  inputs, two functions, both reachable.
- **Return `UntilTheLeaseLapses { seconds: WATCH_RENEWAL_BOUND_SECONDS }` for a live lease, since the bound
  bounds it too.** Rejected: the bound is an **upper** bound on every watch, so reporting it for one live watch
  overstates that watch — and it is the exact overstatement the calendar function's `AlreadyEnded` exists to
  avoid at the other end of the lease.
- **Make `AlreadyEnded` Calendar-only and document Gmail's absence.** Rejected: the state is a fact about the
  **lease**, not about the mechanism, and `gmail_watch_exposure` reaches it from the same `WatchLapse` a channel
  reaches it from. Restricting it would keep the enum's doc explaining a limitation the types do not have.
- **Rename the findings rather than renumber, to avoid touching the record.** Rejected: a finding's number is
  how the record's prose and this repository's prose cross-reference it, and a gap in a numbered series reads as
  a **deleted** finding. Renumbering to contiguous is what makes the series auditable, and nothing outside the
  record cites a Finding by number.

## Conditions that would justify revisiting

- **A teardown executor is written**, at which point a caller actually computes the `WatchLapse` and passes it,
  and the "no caller" limit is removed for this pair of functions.
- **A caller holds a `WatchResponse` and asks for the exposure**, at which point a convenience that reads the
  `expiration` into the `WatchLapse` and calls `gmail_watch_exposure` may be worth having — it is not built now
  because it would be a second reader of a value that has exactly one caller, in the same sense the record
  refuses a consumerless value.
- **Google publishes a bound for Calendar channels**, which would make a `calendar`-side bound-only figure
  possible and would give the two mechanisms the *same pair* of functions rather than a matched pair with a
  documented divergence.
- **`users.stop` is built** (still deliberately absent: an empty body, a third request shape), at which point
  the exposure figure gains its first acting caller and the ordering and the figure are exercised together.
