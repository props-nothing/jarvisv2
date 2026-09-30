# ADR-0087: A lease that lapses silently needs a code that can say so

- **Status:** Accepted
- **Date:** 2026-09-30
- **Slice:** `P5-005` (the Google connector contract).
- **Relates to:** `ADR-0077` (a bound documented but not applied is not a bound), `ADR-0080` (a limit and a
  recommendation are two facts), `ADR-0035` (a boolean standing for more than two situations is an enum),
  `ADR-0037` (an admitted request is a type).

## Context

The connector's manifest has always **linked** Gmail's seven-day watch bound — the Webhooks documentation link's
stated purpose is *"Gmail push notifications through Cloud Pub/Sub: `users.watch`, the seven-day renewal bound,
and the one-event-per-second cap"* — but **nothing in the crate read a watch response or could decide whether a
watch was still alive**. The bound was documented and unenforced, which is the defect `ADR-0077` records for a
bound that is not applied.

That matters more here than for an ordinary limit, because **a Gmail watch fails silently**. The push guide,
verbatim:

> "You must call the `watch` method at least once every 7 days or you'll stop receiving updates for the user. We
> recommend calling `watch` once per day."

No error is raised when the lease ends. No notification arrives to announce that notifications have stopped. For
a machine with no other view of the mailbox, a lapsed watch is **indistinguishable from a quiet day** — the
failure is not that something goes wrong but that nothing does.

Three facts from the sources shape the implementation, and each is a trap:

**1. `expiration` is epoch *milliseconds*, carried as a JSON *string*.** The `users.watch` reference gives
`{ "historyId": string, "expiration": string (int64 format) }` and describes `expiration` as *"When Gmail will
stop sending notifications for mailbox updates (epoch millis)."* A parser that read a JSON number would refuse a
conforming response; a parser that scaled millis as if they were seconds would put the watch's death a **thousand
times too far in the future**, which raises nothing and produces exactly the silent state this slice exists to
remove.

**2. The bound and the recommendation are two figures.** "At least once every 7 days" is when a watch *dies*;
"once per day" is when to *renew*. Reporting one as the other either renews six days late or hides the cadence a
caller should use.

**3. The bound and the `expiration` are different kinds of things.** The bound is a *policy* figure; the
`expiration` is a *value this watch returned*. A module that stored one in the other's place would renew on the
wrong schedule.

## Decision

A new `google::watch` module holds the watch lease as three pure decisions.

1. **`parse_watch_expiration` reads the documented shape, string millis, and scales to the timestamp's
   nanoseconds.** The multiplication is `1_000_000` (millis → nanos), not `1_000`. Because getting this wrong is
   **invisible** — a wrong scale is still a valid instant — the test asserts a real documented value
   (`"1431990098200"`) maps to 1,431,990,098 **seconds** *and* renders as a Tuesday in May 2015, so the unit is
   checked by both arithmetic and a human-readable instant.

2. **`WatchError` distinguishes four unreadable shapes** rather than one "malformed": not JSON, field absent (or
   `null`, which carries the same remedy), present but not a string, and not a whole number. The remedies differ
   — response, provider shape, type, value — and a single variant would send a reader to the wrong layer.

3. **`WatchLapse` is `Lapsed { for_seconds } | Alive { for_seconds }`, decided in *nanoseconds*.** A lease ends
   at an *instant*, so the boundary is decided on the nanosecond difference and the whole-second field is the
   magnitude computed **after** the direction. Comparing truncated seconds would call a watch with half a second
   left either alive (keeping a dead watch) or lapsed (renewing early), and neither is a statement about the
   lease. `Lapsed { 0 }` is the exact expiry instant — **which counts as lapsed**, because the reference says the
   watch stops *at* that time and the safe direction is to treat the boundary as dead.

4. **`RenewalAdvice` is `Overdue | Recommended | NotYet`** — three states, ordered by *urgency* rather than by
   elapsed time, which is why it is an enum and not a duration: a caller wants "renew now", "renew soon" or
   "leave it", and a bare number would make it re-derive both thresholds here. `should_renew` is true for the
   first two, because a caller that only renewed on `Overdue` would run every watch to the edge of its life for
   no benefit.

5. **A renewal "in the future" is `NotYet`, not an error.** That is what a system clock behind the renewal looks
   like, and refusing it would turn a healthy watch into a failure. The negative elapsed time is **carried**
   rather than clamped, so a caller can see that the clock is ahead instead of being told zero elapsed.

## Consequences

- The seven-day bound is now **enforced rather than linked**: a caller can read a watch's `expiration`, ask
  whether it is alive, and be told when to renew — none of which was possible before.
- **The silent-failure mode is representable and visible.** `WatchLapse::Lapsed { for_seconds }` distinguishes
  "this just ended" from "notifications have been missing for two days", which a `bool` would have erased.
- **Two mutants were falsified A-B-A**, both compiling:
  - scaling millis by `1_000` instead of `1_000_000` (detected by the documented-value test — the unit is now
    pinned by a test rather than by inspection);
  - treating the exact expiry instant as alive (`>` → `>=`, the boundary direction: detected by the lease test).
- **A limit remains:** the module is **pure decisions with no caller**. Nothing sends a `watch` request, so there
  is no response to parse and no scheduler that would call `renewal_advice` — the same pipeline-side gap recorded
  for `provider_request_id`, `batch_plan`, `calendar_signal` and `RetryDecision`. It is also **not verified
  against Google**: `expiration` being epoch millis, and the boundary being inclusive, are both taken from the
  reference pages and not observed. And `nanos_to_seconds` **truncates**, so a difference of 1.9 seconds reads as
  1 — deliberate (the direction is already decided in nanoseconds) but a caller wanting more precision than whole
  seconds would need the timestamps themselves, which it has.

## Alternatives considered

- **Return a `bool` for "is this watch alive".** Rejected: the two values would stand for three situations —
  alive, just expired and long expired — and the elapsed time is what tells a caller whether this is a fresh
  problem or a mailbox that has been unwatched for days. Same reasoning `ADR-0035` records.
- **Return the remaining duration and let the caller compare it to a threshold.** Rejected: the thresholds are
  provider figures the connector already owns (the seven-day bound and the daily recommendation), so pushing the
  comparison out would duplicate them at every call site — and the first caller to use the wrong one would renew
  on the wrong schedule.
- **Compare whole seconds rather than nanoseconds.** Rejected: the boundary would move by up to a second in a
  direction that depends on the rounding, so a watch could be reported alive for a fraction of a second after it
  stopped. The lease ends at an instant; the comparison belongs at the instant.
- **Treat a missing `expiration` as "never expires".** Rejected: the reference documents the field on a
  successful `watch`, so its absence is a shape this connector does not understand — and "never expires" would
  build a lease that never renews, which is the failure being fixed.
- **Clamp a negative elapsed time to zero.** Rejected: it would hide a clock skew, reporting "renewed now" for a
  renewal that is *ahead* of the system clock — a caller looking at that would see a healthy watch and a wrong
  reason for it.
- **Put the renewal decision in the manifest's declared webhook purpose string.** Rejected: a string is not a
  decision. The link's purpose describes what the documentation says; the code has to be able to act on it.

## Conditions that would justify revisiting

- A live `watch` call returns an `expiration` that is not epoch millis, which would falsify the conversion and
  the test that pins it.
- Google changes the seven-day bound or the daily recommendation, which would change both constants and the
  three-state advice.
- A scheduler is built, which would give `renewal_advice` a caller and turn "nothing consumes this" from a limit
  into tested behaviour.
- Calendar's channel `expiration` is added, which is an **RFC 3339 string** rather than epoch millis (the
  Calendar push page documents it as a date-time) — a different unit and type for the same concept, and the
  comparison between the two would be worth recording beside this ADR.
