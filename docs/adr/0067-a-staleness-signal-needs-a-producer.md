# ADR-0067: A staleness signal needs a producer, not only a parameter

- **Status:** Accepted
- **Date:** 2026-09-27
- **Slice:** `P5-005` (the Gmail incremental-sync read, `users.history.list`).
- **Relates to:** `ADR-0066` (a cursor decision takes a signal, not a status), `ADR-0058` (a provider decision
  belongs in the crate that does not own a socket), `ADR-0063` (a wire fixture declares whether it is a capture
  or a shape).

## Context

`ADR-0066` fixed a real defect: `SyncAdvance::HistoryPruned` — "the finding this module was written around" —
could not be produced by any input, because `advance_gmail_history` took only the new position and never a
status. The fix made the signal a **parameter**:

```rust
pub enum SyncSignal { Advanced { history_id: Option<String> }, CursorUnusable, Refused(RetryDecision) }

pub fn advance_gmail_history(previous: &SyncCursor, signal: &SyncSignal, ...) -> Result<CursorOutcome, CursorError>
```

The reasoning was right: the inference from a `404` to "history pruned" depends on **which method was called**,
and only the caller knows that. But the slice shipped the parameter and **no producer**. There was no
`history.list` request in this crate, so nothing could obtain a status and build a signal from it. The only
construction sites were test fixtures.

## Decision

Add the operation that produces the signal, in the same crate and beside the predicate that answers half its
question:

```rust
pub fn gmail_history_signal(status: u16, history_id: Option<&str>, refusal: RetryDecision) -> SyncSignal

pub fn gmail_history_list(start_history_id: &str, max_results: Option<u32>, page_token: Option<&str>)
    -> Result<HttpRequest, RequestError>

pub fn parse_history_page(status: u16, body: &str) -> Result<HistoryPage, RequestError>
```

plus the `gmail_history_list` manifest operation, its tool definition and schemas, and two fixtures.

## Why a parameter without a producer is still a defect

`ADR-0066`'s own lesson was "a variant nothing can construct is dead code wearing a contract". The fix removed
the *variant* form of it by widening the input — and reproduced the *parameter* form one level up, because
widening a function's signature does not supply the value. The check that would have caught it is the same one
`P5-001` and `P5-003` record:

> For every variant, ask which **production** path constructs it. Grep the constructing sites, not the
> definition. If the only construction is in a test, the production path is missing.

Applying that to `SyncSignal` shows `Advanced` and `Refused` had no producer either — the whole enum was
fixture-constructed. A remedy that is reachable only from a test is a remedy a deployment cannot reach.

## The three outcomes, and the asymmetry that carries the weight

| Status | Signal | Consequence |
| --- | --- | --- |
| `404` | `CursorUnusable` | full resync; the previous cursor is **not** carried forward |
| `200` | `Advanced { history_id }` | advance, or keep the previous cursor when the mailbox was unchanged |
| anything else | `Refused(decision)` | carry the classification; **never** a resync |

The predicate is checked **first**, so the retryable family (`429`, `5xx`) can never fall into the dead-cursor
arm. That asymmetry is the load-bearing part: a resync on a transient failure discards a working store — the
opposite mistake from the one the 404 heuristic tolerates, and a far more expensive one.

## Corrections this slice made to its own first draft, from live documentation

The method reference (`users.history.list`, page footer **2026-04-15**) was fetched before the code was
written, and it corrected two assumptions:

1. **`history_id` is not required in the response.** The draft's output schema required it. The reference says
   the id "can be stored … for a future request" when no `nextPageToken` is returned — a statement about *when
   it is usable*, not about *when it is present*. Requiring it would have made the connector's declaration
   stricter than the provider's.
2. **`historyId` and `nextPageToken` are not a mutually-exclusive pair.** Unlike Calendar's `nextPageToken` /
   `nextSyncToken`, the history response carries `historyId` on **every** success while `nextPageToken` appears
   only mid-walk. A reader that copied the Calendar pattern would take its durable cursor from the field that
   expires when the walk ends. The two fixtures are the pair that pins this.

Both live in **field descriptions**, not the sample JSON — the `ADR-0063` lesson, hit again.

## Consequences

- `SyncAdvance::HistoryPruned` is reachable from a real status through a real request. The resync path is no
  longer fixture-only.
- The declared surface grows by one operation, one tool definition, and one rate-limit entry (`history.list`
  costs **2** quota units against a message read's 20, so reusing the read limit would over-state a sync).
- **Still not a live path.** There is no transport implementation, so no request is sent and no response is
  parsed from Google; the fixtures are hand-built from the record. The connection the *decision* needs is now
  complete in this crate; the connection to *Google* is the transport slice.
- The operation is registered nowhere, so no model can reach it — the same limit every slice of `P5-005`
  records.

## Alternatives considered

- **Leave the signal caller-supplied and let the transport slice supply it.** Rejected: it defers the defect
  rather than fixing it, and the "which method was called" argument `ADR-0066` makes is about where the
  *inference* lives, not about where the *request* does. Both belong here, because both are testable without a
  socket.
- **Make `parse_history_page` return the signal directly.** Rejected: a parser deciding "pruned" would be
  inferring which method was called, which is exactly the thing `ADR-0066` moved out. The parser resolves the
  body; the caller resolves the status.
- **Offer `historyTypes[]`.** Rejected: the parameter filters the change kinds returned, so a sync using it
  would silently drop the kinds it excluded. `messages` is populated on every change and is read instead.
