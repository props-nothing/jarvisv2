# ADR-0080: A limit and a recommendation are two facts, and a figure with no source is neither

- **Status:** Accepted
- **Date:** 2026-09-30
- **Slice:** `P5-005` (the Google connector contract).
- **Relates to:** `ADR-0058` (a provider's decisions belong in the crate with no socket — which introduced the
  wrong constant), `ADR-0079` (a rate limit without its unit), the research record's "full-sync budget test"
  plan item, `P5-005`.

## Context

`google::client` declared one batch constant:

```rust
/// The largest batch Gmail accepts.
///
/// Two documented facts, in tension: batching is what makes a full sync affordable, and "larger batch sizes
/// can trigger rate limiting". So this is the ceiling the provider states and a caller must *also* pace.
pub const GMAIL_BATCH_LIMIT: u32 = 50;
```

Reading the **batch reference** — the page that actually documents this — showed three problems at once.

**1. The figure was wrong for what it was named.** The page is explicit in both directions:

> You're limited to **100** calls in a single batch request. If you must make more calls than that, use
> multiple batch requests.

> Larger batch sizes are likely to trigger rate limiting. We recommend sending batches of no more than **50**
> requests.

So **100** is the hard limit and **50** is a *recommendation*. The constant was named "the largest batch Gmail
**accepts**" and set to the recommendation, which is false: a batch of 60 is accepted and merely invites
throttling.

**2. One constant collapsed a refusal into a slowdown.** These are different failures with different remedies.
Exceeding 100 **fails the request**; exceeding 50 **degrades throughput**. A caller cannot tell which it is
holding, so it cannot know whether a size between the two figures is a bug or a trade-off. This is the same
"two values standing for more than two situations" shape `ADR-0076` and `ADR-0079` each found, in the direction
that loses information a caller needs.

**3. The figure had no source, and was attributed to a page that does not state it.** The research record's
"Other limits" list read:

> **Batch requests: no more than 50**, and "larger batch sizes can trigger rate limiting"…

and the Verification Log attributes it to the **quota page**:

> …the batch ceiling of 50; the daily threshold cannot be raised…

The quota page states **no batch ceiling at all**. The batch reference —
`https://developers.google.com/workspace/gmail/api/guides/batch` — is **absent from the record's source table
entirely**. So the number was real, half-right in value and half-right in meaning, and attached to the wrong
document. A reader checking the record against the quota page would not find it; a reader trusting it would
believe 50 is a ceiling.

This is the failure `external-research.md` warns about arriving in its quietest form: not an invented number,
but a **real number from an unrecorded source, filed under a recorded one**.

## Decision

The two facts become two constants, the distinction becomes a type with a producer, and the record gains its
missing source.

```rust
pub const GMAIL_BATCH_HARD_LIMIT: u32 = 100;   // "You're limited to 100 calls"
pub const GMAIL_BATCH_RECOMMENDED: u32 = 50;   // "We recommend sending batches of no more than 50"

pub struct BatchPlan { batch_size: u32, requests: u32, final_batch_size: u32 }
pub fn batch_plan(calls: u32, batch_size: u32) -> Result<BatchPlan, BatchPlanError>
```

Four properties are deliberate:

1. **The names carry the distinction.** `HARD_LIMIT` is refused above; `RECOMMENDED` is reported above. A
   caller reading either name knows which failure it faces without opening a doc comment, which is exactly
   what "the largest batch Gmail accepts" prevented.

2. **A size above the recommendation is *allowed* and reported, not refused.** Refusing it would make the
   connector stricter than Google and would hide the 50–100 range the API genuinely accepts.
   `BatchPlan::is_within_recommendation` is the predicate, and its `false` means "this invites throttling"
   rather than "this is refused". Asserted in both directions, so it is not a predicate that is false for
   everything — the defect `ADR-0078` found in its own first draft.

3. **The plan counts the partial final batch.** 101 calls at 50 per batch is **three** requests, not two, and
   a floor division silently drops the remainder's request — skipping part of a sync while reporting success.
   `final_batch_size` is carried so the partial batch is a value rather than a recomputation, and `0` means an
   exact division rather than "a repeated full batch".

4. **The two constants have a producer, not only a test.** `batch_plan` is the reader, and it satisfies the
   record's own open item — *"a full-sync budget test that asserts batching… must batch (≤50 per batch) *and*
   must respect that batches trigger rate limiting"*. The test now asserts the shape of a 1,001-call first sync
   (21 requests of 50, last holding one) rather than two isolated constants.

## Consequences

- The declaration no longer tells a caller that a permitted batch size is refused, and no longer loses the fact
  that a permitted size may be throttled.
- The research record gains the **batch reference** as a source row and corrects the "Other limits" entry and
  the Verification Log attribution, so the figure is filed under the page that states it.
- **Two mutants were falsified A-B-A**, both compiling:
  - setting `GMAIL_BATCH_HARD_LIMIT` back to `50` (detected by **three** tests — the pinned value, the
    ordering assertion, and the accept/reject boundary);
  - flooring the request count instead of rounding up (detected by
    `the_batch_plan_counts_the_partial_final_batch_rather_than_dropping_it`).
- **A limit remains:** `batch_plan` is arithmetic and a type, not a scheduler — nothing yet *paces* batches
  against a rate limit, so `is_within_recommendation` has a test consumer rather than a production one. That is
  the same pipeline-side gap `ADR-0075` records for `provider_request_id`; a batch sender arrives with the
  transport binding.
- Also recorded: the batch reference states a fact the record did not carry, and it is worth naming because it
  affects sequencing — *"A set of n requests batched together counts toward your usage limit as **n** requests,
  not as one request."* Batching saves HTTP connections and **no quota**.

## Alternatives considered

- **Keep one constant at 100 (the hard limit).** Rejected: it loses the recommendation entirely, and the record's
  own point is that batching is *itself* a rate-limit trigger, so a caller that only knew the ceiling would
  batch at 100 and get throttled.
- **Keep one constant at 50, documented as "recommended".** Rejected: it under-states the accepted limit, so a
  caller would never use the permitted range and a genuine future need to reduce requests would look impossible.
- **Two constants and no type.** Rejected: they would be read only by the tests that pin them — the
  "declaration nothing produces" shape this phase has found five times — and the partial-batch arithmetic is
  exactly the part a caller gets wrong.
- **Refuse a size above the recommendation.** Rejected: stricter than the provider, and it would make the
  distinction invisible by removing the range it describes.
- **Correct the value and leave the record's source list.** Rejected: the figure's provenance is the defect. A
  corrected number filed under the wrong page is still a record that cannot be re-verified.

## Conditions that would justify revisiting

- The batch reference changes either figure, which the pinned test and the record's Verification Log are
  designed to surface.
- A measured full sync shows throttling at a batch size within the recommendation, which would mean the
  recommendation is the binding constraint and deserves its own rate-limit treatment.
- A batch sender is built, which would give `batch_plan` a production caller and make `is_within_recommendation`
  a decision rather than a report.
- Google publishes a batch cost model that charges a batch as one call, which the current page explicitly denies.
