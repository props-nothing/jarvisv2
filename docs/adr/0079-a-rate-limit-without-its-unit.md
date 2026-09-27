# ADR-0079: A rate limit without its unit is a figure read as the wrong thing

- **Status:** Accepted
- **Date:** 2026-09-27
- **Slice:** `P5-005` (the Google connector contract).
- **Relates to:** `ADR-0054` (a connector is described before it is trusted), `ADR-0074` (a doc naming another
  module is a claim), `ADR-0077` (a bound that is documented but not applied), the research record's "quota-cost
  test" plan item, `P5-005`.

## Context

The research record calls the per-method quota figure *"the single most important number for sizing a first
sync"*, and records the published table: `messages.get` 20 units, `messages.list` 5, `history.list` 2,
`getProfile` 1. It also records the project ceilings: 1,200,000 units per minute per project and 6,000 units per
minute per user.

The connector declared those ceilings as a `RateLimit`:

```rust
pub struct RateLimit {
    /// The requests allowed per window, sustained.
    pub per_window: u32,
    /// The most requests allowed in one burst.
    pub burst: u32,
    ...
}

#[must_use]
pub const fn sustained_per_second(&self) -> u32 {
    self.per_window / self.window_seconds
}
```

Every field and the accessor say **requests**. Google's figures are **quota units**, and its own page defines
them as "an abstract unit of measurement representing Gmail resource usage" with per-method costs from 1 to 100.

So `sustained_per_second()` returned **20,000** for the Gmail read path. In requests, the real figure for
`messages.get` is 1,200,000 ÷ 20 ÷ 60 = **1,000**. A scheduler planning from the declared value would have been
**20× over quota**, silently — the connector behaves correctly right up to the point the provider starts
refusing calls. And the divergence is per operation, not per connector: the same ceiling is 60,000
`messages.get` calls per minute and 600,000 `history.list` calls per minute, a 10× spread.

A second symptom of the same missing idea was sitting beside it. The connector had **two** rate-limit functions:

```rust
fn gmail_read_rate_limit() -> RateLimit { /* 1_200_000, 60, 6_000 */ }
fn gmail_history_rate_limit() -> RateLimit { /* 1_200_000, 60, 6_000 */ }
```

They were **byte-identical**, and the second one's doc justified its existence:

> A separate entry rather than a reuse of [`gmail_read_rate_limit`], because the two calls have different
> documented costs and one limit for both would either over-state an incremental sync or under-state a message
> read.

The costs *do* differ (20 against 2) — the stated reason was true — but a per-call cost is **not a property of
a rate limit**. `RateLimit` has no field for it and never did, so two identical limits were carrying a
distinction that belonged to the operation. A duplicate wearing the name of a distinction, where the
distinction was real and in the wrong place.

## Decision

The unit becomes a field, the per-call cost becomes a type, and the duplicate disappears.

```rust
pub enum RateLimitUnit { Requests, CostUnits }
pub enum QuotaCost { Documented(u32), Unstated }

impl RateLimit {
    pub const fn sustained_requests_per_second(&self) -> Option<u32> {
        match self.unit {
            RateLimitUnit::Requests => Some(self.per_window / self.window_seconds),
            RateLimitUnit::CostUnits => None,
        }
    }
}

impl QuotaCost {
    pub const fn calls_per_window(self, limit: &RateLimit) -> Option<u32>;
}
```

Five properties are deliberate:

1. **`sustained_requests_per_second` answers `None` for a cost-unit limit.** A cost-unit limit has *no* request
   rate until an operation's cost is known, so the value a scheduler would most easily mistake is not offered
   at all. `sustained_per_second` remains, and its doc now says the result is **in `Self::unit`** rather than
   requests.

2. **`QuotaCost::Unstated` is not `Documented(1)`.** The obvious default would compute the whole cost-unit
   allowance as a request rate and over-plan by the operation's real cost — the same failure, one step earlier.
   It is also the `Default`, so an author who did not consult a cost table has established nothing rather than
   receiving a silently cheap figure.

3. **The cost lives on the operation, not on the limit.** That is what makes the two duplicate functions
   collapse into one `gmail_rate_limit()`: there is one provider ceiling, shared by every Gmail read, and a
   per-method cost is a different fact about a different thing. The ADR-0077 lesson applied to a struct —
   a field carrying two facts is a field that will be read as one of them.

4. **`calendar_events_read` declares `Unstated`, and that is shipped deliberately.** Google's Gmail quota page
   publishes no Calendar cost ("not published on this page" in the research record), so the honest declaration
   is that the cost is unknown. Defaulting it to 1 would invent a figure the provider never stated.

5. **A documented cost of zero derives nothing.** No call costs nothing, so a zero is a defect in the table
   rather than a free operation, and dividing by it would panic. Reported as `None` alongside the other two
   reasons a rate is underivable, because all three mean "this operation has no computable request rate".

## Consequences

- The declaration no longer over-states the request allowance by the per-call cost factor. A scheduler that
  converts has the arithmetic it needs, and one that does not is told `None` rather than given 20,000.
- The per-call cost — the record's "single most important number for sizing a first sync" — is now a value with
  a reader and a test, where before it existed only inside prose `description` strings. The record's own
  **"quota-cost test"** plan item, open until this slice, is written: `5 + 20N` is now checkable from declared
  values rather than from a description a human has to parse.
- The two byte-identical rate-limit functions became one, and a real distinction moved to where it belongs.
- **Two mutants were falsified A-B-A**, both compiling:
  - declaring the Gmail limit in `Requests` (detected by **three** tests — the unit assertion, the
    `sustained_requests_per_second` assertion, and the per-call conversion);
  - making `Unstated` behave as a cost of one (detected by
    `an_unstated_cost_derives_no_request_rate_rather_than_assuming_one`).
- **Two further defects found by reading the code against its own docs, and fixed here:**
  - `SCOPE_OPENID`'s doc said `users.getProfile` "is the operation declared below" — **no profile operation
    exists**. The scope is requested and the operation is not declared, so the doc now says exactly that, and
    names the profile read as a later addition rather than presenting it as present.
  - `RateLimitError::RetryAfterTooLong`'s sibling pattern: the `RateLimit::new` error message said "a rate
    limit must allow 1 to 1000000 **requests** per window" while the figure may be cost units. Corrected to
    "per window", because the unit is now a field rather than an assumption.
- **A limit remains:** `RateLimit` still has no **daily** window, so Google's 80,000,000-unit daily threshold —
  which cannot be raised — is recorded in the research record rather than in a declaration. Adding a window
  kind is a separate decision with its own falsifying test.

## Alternatives considered

- **A comment on `per_window` saying "units for Google".** Rejected: that is the arrangement this ADR removes.
  The figure and its unit would live in different places, and a scheduler reading the field would not see the
  comment.
- **A boolean `counts_units`.** Rejected: a reader has to remember which polarity means what, and the name
  reads as an implementation detail rather than as the fact that makes the figure unusable as a request rate.
- **Divide the cost inside `sustained_per_second`.** Rejected: the rate limit does not know which operation is
  calling it, and giving it an operation parameter would put a per-method table inside a provider ceiling.
- **Keep the two identical functions "for clarity".** Rejected: they were identical, and their doc named a
  distinction the type could not carry. If the costs ever needed separate limits, that would be because the
  *ceilings* differ, and the change would be visible as one.
- **Default `quota_cost` to `Documented(1)`.** Rejected: it is the over-planning failure in its purest form,
  and it would make "we did not check" and "we checked and it is cheap" the same declaration.

## Conditions that would justify revisiting

- A provider publishes a limit whose unit is neither requests nor a cost unit it also publishes per method.
- A measured run shows a cost-unit conversion still over-planning, which would mean the per-user ceiling rather
  than the project ceiling is the binding constraint for a given workload.
- A second connector family (Microsoft, GitHub) publishes per-call costs, which would make `QuotaCost` a shared
  manifest field rather than a `ratelimit` one.
- A daily window is added to `RateLimit`, which would let the 80,000,000-unit threshold become a declaration.
