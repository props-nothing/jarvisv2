# ADR-0081: Calendar's 410 is unambiguous only once the reason is read

- **Status:** Accepted
- **Date:** 2026-09-30
- **Slice:** `P5-005` (the Google connector contract).
- **Relates to:** `ADR-0058` (a provider's decisions belong in the crate with no socket), `ADR-0066` (a cursor
  decision takes a signal, not a status), `ADR-0067` (a staleness signal needs a producer), the research
  record's "410-is-staleness test" plan item, `P5-005`.

## Context

The research record contrasted Gmail's unclassifiable staleness signal with Calendar's:

> `P5-001`'s `SyncCursorKind::can_be_detected_as_stale()` … rather than as absence. **Calendar's 410 has no
> such ambiguity.**

and the code encoded that reading as a status test:

```rust
pub const fn calendar_status_requires_resync(status: u16) -> bool {
    status == 410
}
```

Reading the Calendar **errors** page — a page the record's source table did not list for this fact — showed
that the status alone does not choose a remedy. It publishes **three** bodies for `410 Gone`:

| `reason` | Message | Suggested action |
| --- | --- | --- |
| `fullSyncRequired` | "Sync token is no longer valid, a full sync is required." | wipe the store and re-sync |
| `updatedMinTooLongAgo` | "The requested minimum modification time lies too far in the past." | wipe the store and re-sync |
| `deleted` | "Resource has been deleted" | **"no further action is necessary"** |

So the claim was true of the **status** and false of the **decision**. "Calendar's 410 has no such ambiguity"
was written while contrasting it with a Gmail `404` whose *cause* cannot be determined; the Calendar `410`'s
*cause* is just as load-bearing, and one of the three causes means the opposite of a resync.

The wrong reading is expensive rather than merely untidy. `advance_calendar_sync` maps
`SyncSignal::CursorUnusable` to `SyncAdvance::TokenInvalidated` with **no cursor**, and the sync guide's remedy
for that is "a full wipe of the client's store" — so a connector that resynced on every `410` would discard a
whole sync store when a user **deleted one event**.

This is the same shape as the Gmail `403` the classifier's own doc is built around — one status, several
reasons, different remedies — applied to the call where the wrong branch is most destructive.

## Decision

The reason becomes a type, the producer reads it where it can, and the status-only predicate is kept but
narrowed in meaning.

```rust
pub enum CalendarGoneReason { FullSyncRequired, ResourceAlreadyDeleted, Unrecognised }

pub fn calendar_signal(
    status: u16,
    next_sync_token: Option<&str>,
    gone_reason: Option<&str>,
    refusal: RetryDecision,
) -> SyncSignal
```

Four properties are deliberate:

1. **`ResourceAlreadyDeleted` does not resync.** It is carried as `SyncSignal::Refused` instead — a refusal *for
   the call*, because a delete of an already-deleted event did not do what was asked, even though nothing needs
   repairing. Refusing rather than reporting success is the same rule `P3-005` records for an outcome the
   adapter could not establish.

2. **An unrecognised `410` still resyncs, and the direction is argued rather than inherited.** This is the
   **opposite** of the crate's fail-closed rule for retry classes, and the difference is what is being
   protected: an `Unknown` retry class refuses because a retry could send a **second effect**, while here the
   thing at risk is a **store's liveness**. Resyncing needlessly costs a slower next sync; *not* resyncing a
   genuinely dead token costs a store that never syncs again and never says so. Stated on the variant so a
   reader meets the reasoning rather than inferring a contradiction.

3. **The status-only predicate survives, with its competence narrowed.** It cannot be deleted — a caller holding
   an unparseable body still needs to recover a dead token — and it cannot be made strict, or the unreadable
   case regresses. So it stays permissive and its doc now says explicitly that a caller who **can** read the
   reason must use `CalendarGoneReason::requires_resync` instead. Two functions with one job is normally a
   defect; here the difference is which information the caller has, and both cases are real.

4. **The two sync-token causes share one variant** (`fullSyncRequired`, `updatedMinTooLongAgo`) because they
   share a remedy, which is the grouping `GmailErrorReason::needs_a_person` already applies to its own pair.
   The distinction survives in the response for a diagnostic; it does not change the decision.

## Consequences

- A deleted event no longer costs a sync store, and the fixture pair
  (`calendar_error_410_full_sync_required.json` / `calendar_error_410_resource_deleted.json`) is what makes the
  claim falsifiable: the two carry the **same status** and opposite remedies, so a connector reading the status
  gives them the same answer and the test says so.
- The record's "Calendar's 410 has no such ambiguity" is **corrected in place** to say what is true — the status
  is unambiguous *as a status*, and the decision needs the reason.
- The record's "410-is-staleness test… **PARTIALLY WRITTEN**" plan item is completed: three Calendar fixtures
  now exist (the two 410s and a 400), and the 400 case is asserted to be a caller's mistake that neither retries
  nor resyncs.
- **Two mutants were falsified A-B-A**, both compiling:
  - making `calendar_signal` resync on the status alone (detected by the fixture-pair test);
  - folding `deleted` into `FullSyncRequired` in the parser (detected by **two** tests, including the unreadable
    case whose control is the `deleted` reason).
- **A limit remains:** `calendar_signal` is a producer with no production caller yet — there is no
  `events.list` request to obtain a status and a body from. Its callers appear with the transport binding, the
  same pipeline-side gap `ADR-0075` records for `provider_request_id`. The tests are what currently hold the
  distinction in place.

## Alternatives considered

- **Keep the status-only test and document the `deleted` case as a known cost.** Rejected: the documented action
  for that case is "no further action is necessary", so the code would be doing something the provider
  explicitly says not to, and a whole store is the price.
- **Resync on nothing and refuse every `410` until the reason is understood.** Rejected: a dead token would
  never recover, which is the failure `advance_calendar_sync`'s `TokenInvalidated` variant exists to prevent.
- **Make an unrecognised `410` refuse, for consistency with `RetryClass::Unknown`.** Rejected: consistency with
  a rule about second effects would be bought at the cost of a store that never syncs. The inconsistency is the
  point, and it is recorded on the variant.
- **Make `calendar_status_requires_resync` return `false` so callers must read the reason.** Rejected: a caller
  with an unparseable body would lose the only recovery it has, and the function's name would then be false.
- **Delete the status-only predicate.** Rejected for the same reason, and because `advance_calendar_sync`'s own
  doc explains the two answer different questions (a failure versus a success).

## Conditions that would justify revisiting

- The Calendar errors page adds a fourth `410` cause, which the unreadable-reason direction already handles but
  which should be named if it has its own remedy.
- A measured run shows `deleted` arriving on `events.list` for a sync token rather than on a delete, which would
  mean the reason is not scoped as the page implies.
- A batch sender or a sync driver is built, which would give `calendar_signal` a production caller and make the
  distinction a live decision rather than a tested one.
- Gmail's `404` gains a `reason`, which would let the two providers' ambiguity discussions be unified.
