# ADR-0115: One mechanism's acknowledgement was a decision and its twin's was a bool that was always true

- **Status:** Accepted
- **Date:** 2026-10-01
- **Slice:** `P5-005` (the Google connector — one acknowledgement vocabulary for both push mechanisms).
- **Relates to:** `ADR-0103` (the Calendar ingest whose `acknowledges` this types), `ADR-0104` (the Gmail ingest
  whose `acknowledgement` is the shape being shared), `ADR-0094` (the subscription-global cost that makes the
  *reason* for an acknowledgement matter), `ADR-0107`/`ADR-0114` (the sibling-asymmetry shape — two things that
  should behave alike and did not), and `ADR-0035` (an enum rather than a `bool`).

## Context

The connector has **two** push ingest paths — one per Google mechanism — and each answers the same question:
*what should be sent back to the provider, so the delivery is acknowledged or redelivered?* The answer is the
same vocabulary in both cases, and it lives in one place: [`DeliveryAck`](crate::google::pubsub::DeliveryAck),
with `Accept`, `Retry`, and `AbandonAndAcknowledge`.

**The two paths answered it with two different types.**

```rust
// routing.rs — the Gmail ingest
pub const fn acknowledgement(&self) -> DeliveryAck { /* Accept | AbandonAndAcknowledge */ }

// channel.rs — the Calendar ingest
pub const fn acknowledges(&self) -> bool { true }
```

The Calendar method returned a **`bool` that was always `true`**, so it could not distinguish the two states its
sibling's `Accept`/`AbandonAndAcknowledge` split exists to keep apart:

- **`Accept`** — the delivery passed every control. For `Changed` the work is accepted as well; for `Handshake`
  the delivery is good and starts nothing. Either way the caller did what the message asked.
- **`AbandonAndAcknowledge`** — the delivery can never be processed, so it is acknowledged **and the drop is
  recorded**. The message is gone from the queue, deliberately.

Those are opposite downstream consequences — *work done* versus *work irrecoverably discarded* — and a caller
that logged only `acknowledges() == true` would record a dropped delivery as a success. The method's own doc
even said *"a future variant that should be retried … has a place to say `false`"*, which is the tell: it
reasoned about the retry dimension (which is always `false` here) and never noticed that the **other** dimension
— *which kind of yes* — is the one the sibling's type carries and its `bool` cannot.

**And that asymmetry is the same shape this repository keeps finding.** `ADR-0107` found two teardown steps that
should have matched and did not; `ADR-0114` found two path builders that should have encoded their identifier
and only one did. Here two ingest paths should expose one vocabulary and only one did — and again it is invisible
until the two are read **side by side**, because each is internally consistent.

## Decision

**1. `ChannelIngest::acknowledgement()` returns the same `DeliveryAck`, and `acknowledges()` is removed.**

```rust
pub const fn acknowledgement(&self) -> DeliveryAck {
    match self {
        Self::Handshake | Self::Changed { .. } => DeliveryAck::Accept,
        Self::Unreadable(_) | Self::Unroutable(_) | Self::Rejected(_) => {
            DeliveryAck::AbandonAndAcknowledge
        }
    }
}
```

**Every variant still acknowledges** — the finding is about *which reason*, not *whether* — so the reason each
answer is given is unchanged and the `ADR-0094` argument (a refusal is charged to the whole subscription) still
holds: none of the three refusals is repaired by another attempt, so all three abandon rather than retry.

**2. The method is renamed to match its twin, and the old name is not kept as a shim.**

`acknowledges()` returning `bool` and `acknowledgement()` returning `DeliveryAck` were two names for one
question; keeping both would be the duplication this repository removes everywhere else. A caller that only
wants "does this acknowledge" asks `outcome.acknowledgement().acknowledges()` — the predicate
[`DeliveryAck`](crate::google::pubsub::DeliveryAck) already exposes — so no capability is lost and the
*reason* is available at the same call site.

**3. The property is asserted as a cross-mechanism equivalence, not restated in a comment.**

Two tests drive the **same** outcome kind through **both** mechanisms and assert the acknowledgements are equal:
the accepted case (`ChannelIngest::Changed` ↔ `GmailIngest::Changed`) → both `Accept`; the unroutable case
(`ChannelIngest::Unroutable` ↔ `GmailIngest::Unroutable`) and the refused case (`ChannelIngest::Rejected` ↔
`GmailIngest::Unreadable`) → both `AbandonAndAcknowledge`. A future change that made one path diverge fails
there rather than in a prose sentence.

## Consequences

- **One question, one vocabulary.** A caller that handles a Gmail acknowledgement handles a Calendar one, and
  neither path collapses "done" and "dropped" into a single value.
- **The `ADR-0094` reasoning is now carried by a type rather than a comment.** The doc said every outcome
  acknowledges "because none is repaired by retrying"; the type now says **why** each does, and a reader of a
  call site sees `AbandonAndAcknowledge` rather than having to trust a `true`.
- **Three guards falsified A-B-A**, all compiling: (1) the refusals reporting `Accept` (the collapse the `bool`
  allowed) → **detected** (`left: Accept`, `right: AbandonAndAcknowledge`); (2) an accepted delivery reporting
  `AbandonAndAcknowledge` → **detected** (`left: AbandonAndAcknowledge`, `right: Accept`); (3) reverting
  `acknowledgement` to the "always `Accept`" behaviour and running the **new parity test** → **detected**
  (`left: Accept`, `right: AbandonAndAcknowledge`), which is the mutation the parity test exists to catch.
- **The four existing assertions were tightened rather than relaxed.** Each `assert!(outcome.acknowledges())`
  became an `assert_eq!(outcome.acknowledgement(), …)` naming the **specific** `DeliveryAck` the outcome owes,
  so the tests now pin the reason as well as the acknowledgement — the same "assert the specific value, not
  `is_ok`" rule this repository records for ordered validation.

## Alternatives considered

- **Keep `acknowledges() -> bool` and add a separate `acknowledgement() -> DeliveryAck`.** Rejected: two names
  for one question is the duplication this repository removes everywhere else, and a caller reaching for the
  `bool` would keep getting the half of the answer that cannot distinguish a success from a drop. One method
  returning the richer type loses nothing, because `DeliveryAck::acknowledges()` is the predicate.
- **Give `ChannelIngest` a `bool` and the Gmail ingest a `bool` too, for symmetry.** Rejected: it would move
  the defect the other way — the Gmail path's `DeliveryAck` is right, and a `bool` erases the reason
  `AbandonAndAcknowledge` exists (`ADR-0094`). Symmetry is not the goal; one **correct** vocabulary is.
- **Leave the Calendar `bool` because "every variant acknowledges anyway".** Rejected — and this is the
  inference that hid the defect. "All variants return `true`" is true of the *acknowledgement* and false of the
  *reason*, and the reason is the half a caller acts on (record a drop, versus record work done). A method whose
  return is constant is the shape to check for: **a value that is always the same is either a fact worth
  asserting or a type that is missing a variant.**
- **Return `Option<DeliveryAck>` with `None` for `Accept`.** Rejected: `None` would mean two things ("no
  reason" and "accepted"), which is the `Option`-collapsing defect `DeliveryRoute` and `ChannelRoute` both
  refuse.

## Conditions that would justify revisiting

- **A transient failure becomes an ingest outcome** (a held store, an in-flight token refresh), at which point
  `DeliveryAck::Retry` gains its first user and the comment *"a future variant that should be retried has a
  place to say `false`"* becomes a live branch — for **both** mechanisms, since they now share the vocabulary.
- **A push handler is built** that reads these outcomes, at which point the acknowledgement is sent on a real
  response and the `Accept`/`AbandonAndAcknowledge` distinction reaches a metric rather than only a test.
- **A third push mechanism** (Microsoft Graph, `P5-006`) is added, at which point the two-equivalent-tests
  pattern is the template for asserting its ingest classifies identically to the other two.
