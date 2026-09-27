# ADR-0077: A bound that is documented but not applied is not a bound

- **Status:** Accepted
- **Date:** 2026-09-27
- **Slice:** `P5-005` (the Google refusal path).
- **Relates to:** `ADR-0076` (the `RetryAfter` type whose stated delay this bounds), `ADR-0075` (the
  classification table that first consumed a stated delay), `ADR-0054` (`RetryGuidance` travels with its class),
  `ADR-0058` (a provider's decisions belong in the crate with no socket), `P5-005`.

## Context

`ratelimit.rs` declares a ceiling:

```rust
pub const MAX_RETRY_AFTER_SECONDS: u32 = 3_600;
```

Its doc states a **rule**, not a figure:

> One hour. A provider asking for longer is describing a quota window rather than a transient limit, and
> honouring it inside a retry loop would hold a worker for the whole window — which is why the bound exists
> and why a longer value is refused rather than clamped: a clamp would silently retry sooner than the provider
> asked.

Nothing enforced it. A repository-wide search found the identifier in exactly three places: its definition,
a `pub use` re-export, and a doc link on an error variant. **No code read it.** The companion error variant made
the same claim and was equally inert:

```rust
/// A retry-after value is longer than the caller may hold.
#[error("a provider asked to wait {requested} seconds, above the {maximum} a caller may hold")]
RetryAfterTooLong { requested: u32, maximum: u32 },
```

A variant whose doc said the bound was "enforced by the constructor a caller would use" — and there was no
constructor. Its only construction in the tree was inside a test:

```rust
let error = RateLimitError::RetryAfterTooLong {
    requested: MAX_RETRY_AFTER_SECONDS + 1,
    maximum: MAX_RETRY_AFTER_SECONDS,
};
```

That test asserted the variant's **message formatting**, so it passed for as long as the type existed and
proved nothing about any production path. A construction that exists only in a test is the signature of this
defect: the type can hold the value; no code produces it.

`ADR-0076` then made the gap load-bearing. It routed the provider's stated delay into a `u32`-carrying variant:

```rust
Some(RetryAfter::Seconds(seconds)) => RetryGuidance::RetryAfterSeconds(seconds),
```

`seconds` is whatever the provider sent, so a `429` answering `Retry-After: 18000` produced a reason reading
`… (throttled); retry after 18000s` — a **five-hour wait presented as an ordinary retry delay**. Google
documents exactly this response: a daily-limit `429` "might result in these errors for multiple hours". So the
input is realistic, and the outcome was the one the ceiling's own doc says the bound exists to prevent.

This is a new form of the phase's recurring defect. `ADR-0067`, `0068`, `0073`, `0075` and `0076` each found a
**type** that could hold a value nothing produced. This one is a **constant whose stated rule nothing applied**:
the declaration and the behaviour were one comment apart, and the comment was the only thing that knew.

## Decision

The refusal becomes a **value** the type can already hold, and one constructor applies the bound.

```rust
pub enum RetryGuidance {
    RetryAfterSeconds(u32),
    BackoffSeconds(u32),
    BackoffAfterUnreadableDelay(u32),
    /// Do **not** retry in this loop: the provider asked for a longer wait than a caller may hold.
    DeferSeconds(u32),
    DoNotRetry,
    Reauthenticate,
    Reconcile,
}

impl RetryGuidance {
    #[must_use]
    pub const fn for_stated_delay(seconds: u32) -> Self {
        if seconds > MAX_RETRY_AFTER_SECONDS {
            return Self::DeferSeconds(seconds);
        }
        Self::RetryAfterSeconds(seconds)
    }

    pub const fn deferred_seconds(self) -> Option<u32> {
        match self {
            Self::DeferSeconds(seconds) => Some(seconds),
            _ => None,
        }
    }
}
```

Five properties are deliberate:

1. **The bound is applied in exactly one place.** `classify`'s `429` arm calls `for_stated_delay` rather than
   constructing `RetryAfterSeconds` directly, so there is one site to test and one place a reader must check.
   Both sides of the ceiling are asserted.

2. **`DeferSeconds` answers `None` from `delay_seconds()`.** That accessor answers *"how long before the
   automatic retry"*, and this guidance forbids the automatic retry. Returning the number there would make
   `delay_seconds().is_some()` mean "retryable" and reintroduce the hazard the variant exists to remove, so the
   provider's number moves to a **separate accessor**, `deferred_seconds()`, which is the one whoever schedules
   the deferral reads. The two accessors are disjoint, and a test asserts that for every variant.

3. **The refusal is a variant, not an error.** A provider asking to wait five hours is a **real response** that
   means *defer the work*, which is a state this vocabulary already names (`BudgetOutcome::Exhausted` is the
   budget-side twin). A `Result` whose every caller immediately converted `Err` into `DeferSeconds` would be
   ceremony — and worse, a caller could `?` a plan to wait into a hard failure. So `for_stated_delay` is total.

4. **`RateLimitError::RetryAfterTooLong` was removed, with the reason recorded in place.** Once the refusal is a
   variant that is produced, the error had nothing left to mean, and keeping it would leave a second declaration
   of the same rule that nothing constructs — the defect this ADR is about. A comment where it was names the
   test-only construction that hid the problem.

5. **No clamp, and no wait inside the loop.** Clamping would retry sooner than the provider asked (the direction
   that gets a caller blocked); honouring it in the loop would hold a worker for a whole quota window. The
   remedy is the one already established: defer.

The reason renders the distinction, and checks the deferral **before** `delay_seconds` because that accessor
answers `None` for it:

```
… the provider refused the call: rateLimitExceeded (throttled); retry after 30s
… the provider refused the call: rateLimitExceeded (throttled); defer for 18000s (above the 3600s this caller will hold)
```

## Consequences

- The ceiling's doc is now a statement about behaviour rather than an intention. `MAX_RETRY_AFTER_SECONDS` has a
  reader, and `RetryGuidance::for_stated_delay` is the only path from a provider's number to retry guidance.
- A `429` stating a multi-hour delay no longer reads as an ordinary retry, so an operator or a scheduler sees
  the difference between "wait thirty seconds" and "this quota window is hours long".
- The class is unchanged (`Throttled`) — the **remedy** changes, not the diagnosis — which is the same
  separation `ADR-0054` established for `RetryClass` and `RetryGuidance`.
- **Two mutants were falsified A-B-A**, both compiling:
  - removing the bound from `classify` (passing the provider's number straight through: detected by the
    end-to-end deferral test);
  - moving the comparison to `>=` (detected by both the unit test and the end-to-end test, because a delay
    exactly at the ceiling is one the caller may hold).
- **A limit remains:** no consumer reads `RetryDecision` outside `jarvis-connectors` yet. A `DeferSeconds`
  reaching an operator's eyes works today (it is in the bounded reason), but nothing schedules the deferred
  work. That is the pipeline-side half, and it is the same gap `ADR-0075` recorded for `provider_request_id`:
  a run-record consumer rather than a connector fix.

## Alternatives considered

- **Clamp to `MAX_RETRY_AFTER_SECONDS`.** Rejected, and this is the alternative the constant's own doc already
  rejected: a clamp retries **sooner** than the provider asked, which is the direction that gets a caller blocked.
- **Return `Err(RateLimitError::RetryAfterTooLong)` from a fallible constructor.** Rejected: the value is not an
  error, it is a deferral, and a `Result` a caller can `?` would turn "plan to wait" into "fail".
- **Keep a number in `delay_seconds()` for the deferral too.** Rejected: that accessor is the retry loop's, and a
  number there reads as permission to retry.
- **Leave the constant and delete the doc's "refused rather than clamped" sentence.** Rejected: the sentence is
  the correct rule; the code was the part that was wrong.
- **Keep `RetryAfterTooLong` for completeness.** Rejected: an error variant nothing constructs is a second
  declaration of the same rule, which is the class of defect this ADR removes.

## Conditions that would justify revisiting

- A JARVIS-side scheduler is built that consumes `deferred_seconds()` — then the deferral has a producer *and* a
  consumer, and its interaction with `BudgetOutcome::Exhausted` should be stated explicitly.
- A provider is added whose documented delays are routinely above the ceiling, which would make the ceiling a
  per-provider figure rather than a platform constant.
- A measured run shows the ceiling rejecting a delay the scheduler would have been happy to hold, which would
  move the bound to a policy value rather than a constant.
