# ADR-0092: A response field with no reader, and a worked example that uses two numbers

- **Status:** Accepted
- **Date:** 2026-09-30
- **Slice:** `P5-005` (the Google connector contract — Gmail push and the sync it triggers).
- **Relates to:** `ADR-0066` (a cursor decision takes a signal, because only the caller knows the method),
  `ADR-0087` (the watch lease, whose `expiration` this response's other half sits beside), `ADR-0090` (a
  refusal keeps the cursor), and `ADR-0091` (a value redacted in one place and printed in another — the same
  "recorded in one place, not acted on in another" shape, in a different direction).

## Context

`google::watch` reads a `users.watch` response and keeps **one** of its two documented fields.

The `users.watch` reference gives the response as:

```json
{ "historyId": "1234567890", "expiration": "1431990098200" }
```

`parse_watch_expiration` reads `expiration` — including its two traps (a string, carrying milliseconds) — and
`WatchError` has a variant for each way that field can be wrong. The response's **other** field, `historyId`,
is read past. There is no reader for it, no type that holds it, and no error variant for its absence.

What the field is for is stated plainly in the same guide that documents the response:

> "The response contains the current mailbox `historyId` for the user. **Your client receives notifications for
> all changes after that `historyId`.** If you need to process changes before this `historyId`, refer to
> Synchronize clients with Gmail."

and, two paragraphs later, the guide's own worked example:

> "For example, use the `history.list` method to identify changes that occurred between your initial `watch`
> request and the receipt of the notification message shared in the previous example. **Pass `1234567890` as
> the `startHistoryId` to `history.list`. Afterward, you can persist `9876543210` as the last known
> `historyId` for future use cases.**"

**The worked example uses two different numbers, and they are the two ends of one operation.**

- `1234567890` is the watch **response's** `historyId` — the anchor, the `startHistoryId` a first sync passes.
- `9876543210` is the position the **resulting sync** ends at — what the mailbox has moved to, and what the next
  sync starts from.

So the anchor a fresh watch produces is exactly the value this crate does not read, and the guide's own example
is a fixture that can tell the two apart. That is rare: usually a documentation example is one value of one
type, and any reading of it that type-checks passes. Here a reader that returned the *response's* id as "the
new position" produces a plausible `historyId`, satisfies every equality assertion written against a
single-number example, and is **wrong in a way that never terminates**: an anchor does not move when the mailbox
does, so a sync that stored it as the position would re-read the same window on every run.

**Why the omission is invisible.** The reader is named for the field it reads — `parse_watch_expiration` — and
that name is accurate. Nothing in the module claims to read the whole response. So a caller that needed an
anchor had no function to call and no error to handle: the response simply had one fewer field than the
reference documented, and the gap was in the shape of the API rather than in any single line.

## Decision

1. **`parse_watch_anchor` reads the response's `historyId` as text**, because the value's only operations are
   "equal", "greater" and "put in a query string": `advance_gmail_history` parses it to decide *direction*, and
   `SyncCursor::new` applies its own bound to a token. A numeric type here would be invented for a value nothing
   does arithmetic on.

2. **`WatchResponse` holds both fields, and `parse_watch_response` is the reader to use.** The two public field
   readers stay, because each has traps worth testing in isolation; the combined reader is what makes it
   impossible to consume the response and hold **one** field, which is the defect this ADR records. The struct
   is the fix: a caller holding a `WatchResponse` has read both, because the only way to build one is the
   function that reads both.

3. **Two new error variants, `MissingHistoryId` and `HistoryIdNotAString`, separate from the expiration's.**
   The remedies differ — an anchor that is absent makes the first sync unanchorable, an expiration that is absent
   makes the lease unreadable — so a caller debugging one is not pointed at the other. `parse_watch_response`
   reads `expiration` **first**, deliberately: a caller that cannot tell when the lease ends cannot use the
   anchor either, so a response failing both reports the fact that stops the watch working at all.

4. **The anchor is named `anchor`, not `history_id`.** The type's own field name is where the confusion would
   live, and `historyId` is ambiguous in this API — it names the response's anchor *and* the position a sync
   ends at. `anchor` says which one, and `expires_at` sits beside it so the two halves of the response are one
   value rather than two lookups that could drift apart.

5. **A missing `historyId` is refused rather than defaulted.** A caller that treated "no anchor" as "sync from
   the beginning" would take the most expensive path exactly when the provider had failed to supply the cheap
   one, which is the fail-open direction. Refusing is the reading whose error surfaces at the call.

## Consequences

- **The guide's two-number example is a test**, and it is the only kind of test that can catch the confusion:
  it asserts the anchor is `1234567890` **and** `assert_ne!` that it is not `9876543210`, so a change to either
  the reader or the reading fails loudly and names which document decides.
- **Two guards were falsified A-B-A**, both compiling: reading the anchor from the `expiration` key (caught by
  three tests) and returning the lease while dropping the anchor (caught by one). The second is the *original
  defect* re-introduced, which is what makes it the meaningful pair — the guard fails on the mistake it was
  written for rather than on a hypothetical.
- **A generalisation worth keeping:** *a reader named for one field is not a reader for its response.* An
  accurate name makes the omission invisible, because nothing is mislabelled — there is simply no function for
  the other field. When a provider's response carries two facts and a module reads one, the question is not
  "is this reader right" but **"which of the response's documented fields has no reader"**.
- **And a second:** *a documentation example with two values is a stronger fixture than one with one.* A
  single-value example cannot falsify a conflation of two same-typed fields, because every reading of it
  type-checks. The guide's `1234567890`/`9876543210` pair is what makes this defect falsifiable at all, and it
  was worth re-reading the guide for the *example* rather than only the field list.
- **A limit remains:** no `users.watch` request is built here, so the anchor's consumer is still a future sync
  loop. This ADR makes the anchor **available and correct**; it does not make anything read it in production.
  The anchor is also unvalidated as a cursor — `SyncCursor::new` applies its bound when the value is stored, and
  nothing here calls it, so a provider sending an unusable `historyId` is caught at the cursor rather than here.

## Alternatives considered

- **Add a `history_id` field to `WatchResponse` and leave `parse_watch_expiration` as the only reader.**
  Rejected: the response would still have a documented field with no reader, which is the defect. The combined
  reader is the point.
- **Return a `(String, UtcTimestamp)` tuple instead of a struct.** Rejected: the two values are both `historyId`
  -adjacent, and the tuple's positions do not say which is which — a struct with named fields is what stops the
  reader and its caller from swapping them, which is the error class at issue.
- **Return the anchor as a `u64`.** Rejected: `historyId` is documented as a string, is compared and passed
  through rather than computed with, and `advance_gmail_history` already parses it only to decide direction. A
  numeric conversion would introduce a second place a parse can fail for no gain, and it would lose a value the
  provider might not have made decimal.
- **Treat a missing `historyId` as "sync from the start".** Rejected: it takes the most expensive path when the
  provider has just failed to supply the cheap one, and it reports success. The refusal surfaces at the call.
- **Fold the anchor into `parse_watch_expiration` and rename it.** Rejected: the name would become a lie about
  what it returns, and the function's own two documented traps are worth keeping in a reader that reads only
  the field those traps are in.
- **Reject a `historyId` that is not a plausible cursor shape (empty, oversized, control characters) here.**
  Rejected as the wrong layer: `SyncCursor::new` already refuses exactly those, with a message naming a cursor,
  and duplicating the rule would give two definitions of an unusable token that could drift. The reader reports
  the field's *type and presence*; whether the value is a usable position is the cursor's decision.

## Conditions that would justify revisiting

- A `users.watch` request is built and a sync loop consumes the anchor, at which point this value's producer and
  consumer are both real and the "no caller" limit is removed.
- Google changes the response to carry an opaque anchor rather than a `historyId`, at which point the field is
  no longer comparable to a cursor position and the naming here should follow.
- A second provider's watch response is added (`P5-006` — Microsoft Graph subscriptions), at which point the
  question of whether the anchor is a `historyId` at all, or a per-provider concept, becomes live. Graph's
  subscription response carries no equivalent position today, which is a reason to keep this Google-specific.
