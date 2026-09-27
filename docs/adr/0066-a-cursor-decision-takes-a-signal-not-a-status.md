# ADR-0066: A cursor decision takes a signal, not a status, because only the caller knows the method

- **Status:** Accepted
- **Date:** 2026-09-27
- **Slice:** `P5-005` (Gmail and Calendar incremental sync).
- **Relates to:** `ADR-0048` (semantic similarity is one conditional signal), `ADR-0062` (an unanswered request
  is classified by whether it may have reached the provider), `ADR-0065` (a protocol type stays correct when a
  provider disagrees with it).

## Context

`P5-001` built `SyncCursor`, `SyncAdvance`, and `CursorOutcome`. `P5-005`'s client added the two functions that
advance a cursor, and a predicate that names Gmail's staleness signal:

```rust
pub fn advance_gmail_history(
    previous: &SyncCursor,
    next_history_id: Option<&str>,
    ...
) -> Result<CursorOutcome, CursorError>

pub const fn gmail_history_status_is_pruned(status: u16) -> bool { status == 404 }
```

Three defects were in that shape, and only the third was visible from the code alone.

**First, the documented remedy was unreachable.** `SyncAdvance` has a `HistoryPruned` variant whose own doc says
it is "the finding this module was written around" — and **no input could produce it**, because
`advance_gmail_history` never received a status or any other failure signal. A caller with a `404` had no way to
reach the resync path at all. `TokenInvalidated` was unreachable the same way for Calendar. This is the
"refusal that can never fire" pattern `P5-001` and `P5-003` each recorded, in its inverse form: a *remedy* that
can never be produced.

**Second, the predicate asserted a distinction the provider does not make.** `gmail_history_status_is_pruned`
returned `status == 404`. Finding 2 in `docs/research/integrations/google.md` says a `404` on `history.list` is
indistinguishable from a `404` for an absent mailbox. The name claimed a resolution the code had not achieved,
and a caller reading it would believe two cases had been distinguished when they had not.

**Third, researching it properly made the second defect worse rather than better.** The sync guide says a
`startHistoryId` outside the retained range returns "an `HTTP 404` error response", and the error guide
(`handle-errors`, last updated **2026-09-15**) lists `404 - Not Found — The requested resource couldn't be
found` in its status summary and then **has no 404 subsection at all**: its sections are 400, 401, 403, 429 and
5xx. **So Google publishes no `reason` code for a 404.** The ambiguity is not merely documented; it is
**irreducible from the response**, and no predicate can resolve it.

## Decision

**1. The advance functions take a `SyncSignal`, and the caller produces it.**

```rust
pub enum SyncSignal {
    Advanced { history_id: Option<String> },
    CursorUnusable,
    Refused(RetryDecision),
}
```

The signal exists because **the inference from a status to a cursor verdict depends on which method was
called**, and only the caller knows that. A `404` on `users.history.list` may mean pruned history; a `404` on
`users.messages.get` means the message does not exist, and mapping that to a resync would discard a whole sync
over one missing message. Requiring the signal makes a caller's inference an explicit act instead of a
comparison hidden inside a function that never saw the request.

**2. `SyncAdvance::HistoryPruned` and `TokenInvalidated` are now reachable.**

`SyncSignal::CursorUnusable` produces each function's own variant, so one shared signal does not erase the
difference: Gmail prunes history and Calendar invalidates a token, and each reason is named where it is known.

**3. A dead cursor carries no cursor forward.**

`CursorUnusable` returns `cursor: None`. Carrying the previous cursor would invite a caller to resume from the
position the provider just rejected, which is the exact failure the arm exists to prevent — and `CursorOutcome`
holds both fields precisely so "no new position" is representable rather than implied.

**4. A `404` may be read as "cannot prove usable", because the wrong reading is self-correcting.**

This is the argument that makes the decision safe rather than merely convenient. **Both** documented causes of a
`404` begin with a full sync: if the history was pruned, that is the documented remedy; if the account is gone,
the full sync's own first call fails and surfaces *that*. So the wrong reading costs **one extra
`messages.list` call**, while the alternative — resuming from a position the provider rejected — produces a store
that reports itself in sync while missing everything. The asymmetry of the costs decides the direction, which is
the same reasoning `ADR-0062` uses for an ambiguous transport failure.

**5. The predicate is removed and replaced by one that names what is readable.**

`gmail_history_status_is_pruned` claimed "pruned". `gmail_history_status_cannot_prove_usable` claims only "this
status carries no distinguishing information", which is the whole truth about a `404` here. The resync still
happens, but through an explicit `SyncSignal::CursorUnusable` rather than through a name that implied the
question was settled.

**6. `429` and `5xx` are not read as dead cursors.**

`gmail_history_status_cannot_prove_usable` is true for `404` alone, and a test asserts it is false for the
retryable family — because a resync on a transient failure would discard a working store, which is the opposite
mistake and a much more expensive one.

**7. The test that the Verification Plan asked for cannot be written as stated, and the record says so.**

The plan's "404-is-staleness test" required a fixture showing the two causes produce opposite outcomes. That
comparison needs information the response does not carry. So the item is marked **WRITTEN** and then
**corrected**: the fixture (`gmail_history_404_no_reason.json`) asserts the **absence of an `errors` array and
therefore of a `reason`**, and the record explains that the discrimination it asked for is impossible. A
verification plan item being falsified is a result, not a failure — `ADR-0063` records the same outcome for a
different assumption.

**8. Three guards were falsified with compiling mutants.**

Carrying a rejected cursor forward (2 tests detected), making the predicate unfailable (2), and reporting a
refusal as an advance (2).

## Consequences

- **A documented remedy is reachable for the first time.** `HistoryPruned` and `TokenInvalidated` could not be
  produced by any input before this change, so the sync half of the connector had no path to a resync.
- **A predicate no longer claims knowledge the provider does not publish**, and the research record says
  precisely which of its own items turned out to be unanswerable.
- **The caller owns the inference, where the method name is in scope.** This is the shape `ADR-0048` uses for a
  conditional signal: the component with the context supplies the input rather than a lower layer guessing.
- **`SyncSignal` makes the third case explicit.** A refusal that is neither an advance nor a dead cursor is
  carried with its classification rather than swallowed, so a caller can report *why* it could not advance.

## Limits

- **No request is sent.** The signal is produced by a caller that does not exist yet: no transport for
  `history.list`, no sync loop, and no resync orchestration. The module fixes a decision's shape and nothing has
  exercised it end to end.
- **Nothing performs the full sync.** `HistoryPruned` says a resync is required and no component can carry one
  out, so a caller that reaches this variant has a verdict and no remedy.
- **The `404` reading is still an inference, now named as one.** The doc argues it is safe because the wrong
  reading is self-correcting; it does not argue the response distinguishes the cases, because it cannot. A
  provider that changed its 404 behaviour would not be caught by any test here.
- **Calendar's `400`-is-a-query-error path has no fixture.** The classification is tested and a `400` maps to
  `SyncSignal::Refused` by a caller's choice, but nothing pins which reason code accompanies it.
- **`SyncSignal::Advanced` carries a `String` per call.** The history id is copied into a cursor immediately, so
  the allocation is bounded by the caller's loop rather than by anything here; a caller that hoisted the signal
  out of its loop would allocate once, which is a performance detail no test asserts.
- **The two `advance_*` functions still differ in what they check**, deliberately: Gmail's monotonic marker is
  refused when it moves backwards and Calendar's opaque token has no ordering. A caller that confused the two
  would get a compile error on the `SyncCursorKind`, not on the signal — so the signal does not protect against
  calling the wrong function.
