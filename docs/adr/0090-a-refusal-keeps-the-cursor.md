# ADR-0090: A refusal keeps the cursor, because the position was never rejected

- **Status:** Accepted
- **Date:** 2026-09-30
- **Slice:** `P5-005` (Gmail and Calendar incremental sync).
- **Relates to:** `ADR-0066` (a cursor decision takes a signal, because only the caller knows the method),
  `ADR-0067` (a staleness signal needs a producer), `ADR-0082` (a `429` is `Throttled`, and `5xx` is
  `ProviderFault`), `ADR-0089` (at-least-once delivery makes throttling routine).

## Context

Both cursor-advance functions returned **no cursor** for a refusal:

```rust
SyncSignal::Refused(decision) => {
    return Ok(CursorOutcome {
        advance: SyncAdvance::Refused(decision.clone()),
        cursor: None,          // ← the defect
    });
}
```

That shape was copied from the `CursorUnusable` arm above it, where it is **right**, and the copying is where
the reasoning stopped being true. `CursorUnusable` means the provider **rejected the position** — Gmail pruned
the history, Calendar invalidated the token — so resuming from it is exactly the failure `ADR-0066` was written
around, and discarding it is the fix.

`Refused` means something else entirely: the *request* failed. It is produced for `429`, `5xx`, `403`,
`400`-class statuses — every classification in `client::classify` that is not an advance. **None of them says
anything about the sync position.** A `429` says "ask again later"; a `503` says "the provider is unwell"; a
`400` says "your query was wrong". The stored cursor remains as valid after each of those as it was before.

So the previous cursor was thrown away **without the provider having rejected it**, and the caller's next sync
restarted from scratch:

- **Gmail**: a `429` on an incremental sync → no cursor → a full resync of the mailbox.
- **Calendar**: a `429` on an incremental sync → no token → a **full wipe of the store**.

That is precisely the mistake `ADR-0067` warns against for the `404` heuristic — "a resync on a transient
failure discards a working store, which is the opposite mistake and a much more expensive one" — committed in
the very arm that handles the transient failures.

**A test pinned it.** `a_refused_advance_carries_the_decision_and_no_cursor` asserted `cursor.is_none()`, with a
comment justifying it as *"so a caller cannot store a new position on the strength of a failure"*. The reasoning
is sound and the subject is wrong: the previous cursor is not a **new** position. It is the caller's existing
one, unchanged, and the check that matters — do not store something the failure did not establish — is satisfied
by returning it unchanged.

**The project's own record contradicted the code.** `ADR-0067`'s outcome table reads:

| Status | Signal | Consequence |
| --- | --- | --- |
| anything else | `Refused(decision)` | carry the classification; **never** a resync |

"Never a resync" is a statement about the *signal*, not about the cursor — and the code turned it into a resync
by dropping the position.

## Decision

**A refusal returns the previous cursor, unchanged.**

```rust
SyncSignal::Refused(decision) => {
    return Ok(CursorOutcome {
        advance: SyncAdvance::Refused(decision.clone()),
        cursor: Some(previous.clone()),
    });
}
```

in **both** `advance_gmail_history` and `advance_calendar_sync`.

This is the same answer the "mailbox unchanged" arm already gives (`Advanced { history_id: None }` →
`Some(previous.clone())`), and for the same reason: **nothing was learned about the position**, so the cursor
that was valid before the call is still valid after it. Two arms now share one answer because they share one
justification, and that is the honest shape rather than a coincidence.

The distinction between the two "no new position" arms is now explicit and worth naming:

| Arm | What it means | Cursor |
| --- | --- | --- |
| `CursorUnusable` | the provider **rejected the position** | **none** — resuming would be wrong |
| `Refused` | the **request** failed; the position is untouched | **the previous one** |
| `Advanced { None }` | the request succeeded and the mailbox did not change | **the previous one** |

## Consequences

- **A throttled incremental sync no longer costs the whole sync.** For Gmail that is one wasted full walk; for
  Calendar it was a full wipe of the store, which is the more serious of the two.
- **The mistake is much more reachable than it was when it was written.** `ADR-0089` established that a
  push-driven sync delivers at-least-once and that a throttled response is *routine*; the push page documents
  push backoff triggered by negative acknowledgements. So the arm that discards the cursor is on the common
  path of the feature this connector is being built for, not an edge case.
- **Two mutants were falsified A-B-A**, both compiling:
  - returning `None` for a refusal again (detected by the rewritten test and the three-signal test);
  - carrying the cursor forward for `CursorUnusable` — the opposite error — detected by **three** tests,
    including the pre-existing `a_gmail_cursor_that_cannot_be_used_requires_a_resync_and_carries_no_cursor`,
    which is the control that proves the fix did not turn into "always keep it".
- **A limit remains:** nothing consumes `CursorOutcome` outside this crate, so the benefit is currently proved
  by tests rather than observed in a deployment — the same pipeline-side gap recorded for `RetryDecision`,
  `calendar_signal` and `watch`. And `Refused` still carries no **guidance about when to retry**: the decision's
  `RetryGuidance` is inside it, but nothing here reads the delay, so the caller must schedule the retry itself.

## Alternatives considered

- **Keep returning `None` and let the caller re-derive the cursor.** Rejected: the caller cannot. It does not
  hold the previous cursor by construction — that is why it is passed in — and an arm that expects the caller to
  reconstruct a value it was given is a contract nobody can satisfy.
- **Add a fourth variant, `RefusedKeepingCursor`.** Rejected: `Refused` already means the request failed, and a
  second variant for the same meaning would invite a caller to treat one as retryable and the other not. The
  cursor field carries the distinction already, and `CursorOutcome`'s doc says exactly why it has two fields.
- **Return the cursor only when the decision permits a retry.** Rejected: it would tie a *cursor* decision to a
  *retry* decision, and a `403 domainPolicy` is permanent while still saying nothing about the position. The
  cursor survives because the position is untouched, whatever the retry answer is.
- **Leave the code and change `ADR-0067`'s table to match.** Rejected: the table is right and the code was
  wrong. "Never a resync" describes what a caller should do with the *outcome*; dropping the cursor is what
  caused one, so the doc and the behaviour disagreed and the behaviour was the defect.
- **Keep the old test and add a new one asserting the opposite.** Rejected: two tests disagreeing is worse than
  one wrong one, and the old test's comment contained a claim — "the previous cursor is a new position" — that
  is simply false. The test was **rewritten**, with its old comment recorded in the new one so a reader sees why
  it changed.

## Conditions that would justify revisiting

- A provider documented to invalidate a cursor on a throttled or failed request, which would make the position
  genuinely suspect after a `Refused` and put that provider's case in the `CursorUnusable` arm.
- A caller observes that a resumed sync after a refusal re-walks history it had already processed, which would
  mean the position is not untouched after all and the reason for this ADR is wrong.
- `SyncSignal::Refused` grows a payload describing *which part of the sync* failed, at which point returning the
  whole previous cursor may be too coarse.
