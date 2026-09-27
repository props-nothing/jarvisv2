# ADR-0074: A doc naming another module is a claim about code, and the code never went there

- **Status:** Accepted
- **Date:** 2026-09-27
- **Slice:** `P5-005` (the refresh path).
- **Relates to:** `ADR-0073` (a retry-safety variant needs a producer), `ADR-0069` (two tested halves do not
  test the seam), `P5-004` (the research that found a defect in shipped code by reading a doc comment as a
  claim).

## Context

This round set out to give `RefreshExchange` a producer, because `refresh` and `classify_refresh` had no caller
outside their tests — the defect `ADR-0073` records for `TokenRequestOutcome`, one layer over in the same module.

`TokenEndpointAnswer::classify_refresh` carried this doc:

> Exists so the rotation rule is expressed once. **The classification itself is
> [`crate::token::RefreshExchange::classify`]'s** — this only supplies the two inputs a transport cannot supply
> from a socket.

The body did not do that. It called a private `refresh_outcome(&self, vendor_says_revoked)`, a local `match`
that restated **both** rules the shared classifier owns:

- the rotation test (`granted.refresh_token.is_some()` ⇒ `Rotated`), with its own copy of the "arrival, not
  storage" comment;
- the ordering (`failure.transient` first), with its own copy of the "a 503 can carry `invalid_grant`" comment.

So the crate held two implementations of one rule, and a doc naming the other as the authority. `P5-004`
recorded this exact class: **a doc comment saying "the rule lives in X" is a claim about code, and when a doc
names a place, that place must be read.** Here X was the wrong module because the code never went there.

## How it was found — and this is the reusable part

The mutation was applied to the **shared** classifier as a falsification probe for the new tests:

```
if failure.transient {   →   if false { }
```

**It survived every test in the crate**, including the one written to assert that ordering — and an earlier
attempt at the same mutation reported a misleading `RESTORED=True` because the backup had been captured while a
mutant was applied (two mutants existed at once; the file verified byte-identical to a corrupted baseline).

A surviving mutation is normally a weak-test signal. Here it was a **routing** signal: the path under test never
called the mutated function. The way to tell the two apart is to check whether the mutated code is *reachable*
from the assertion — `grep` for its callers, not for its definition.

## Decision

`classify_refresh` becomes a one-line delegation to `RefreshExchange::classify`, passing the disjoint
`response()`/`failure()` pair. The private `refresh_outcome` is deleted, along with what it restated:

- the rotation test, and the `rotated_reference` computation — `RefreshExchange::classify` already derives it
  from `response.has_refresh_token`, so a caller cannot disagree with it about whether a rotation happened;
- the ordering and the `vendor_says_revoked` precedence, which now exist once.

`RefreshOutcome`'s import becomes `#[cfg(test)]`-only, which is the compiler confirming that the production path
no longer reaches it directly.

The refresh producer `refresh_with` is added alongside, so `RefreshExchange` finally has a caller outside a test.

## Consequences

- One implementation of the rotation rule and one of the transient ordering, both in the shared module that the
  docs name.
- The mutation that survived before is now **detected**, with `left: Expired, right: Transient` — so the
  ordering test is genuinely sensitive, and was only blind while the code took a different path.
- The test-only import is a compile-time witness: if a future edit reintroduces a local classification, the
  import becomes unused and the build fails.
- **The lesson generalises past this file.** A doc that says a rule lives elsewhere is a claim about reachability,
  and reachability is checkable — by the compiler when the delegation is real (as the unused import shows), and
  otherwise by asking which function a call site actually calls.

## Alternatives considered

- **Keep the local `refresh_outcome` and delete the doc claim.** Rejected: it keeps two copies of a rule whose
  whole content is an ordering, and the copy that exists is the one a reader will not find.
- **Have `classify_refresh` call `classify` but keep computing `rotated_reference` locally.** Rejected: that
  field is `classify`'s output and computing it here is the second answer this change removes.
- **Leave the test-only import in place unconditionally.** Rejected by the compiler: it is genuinely unused in a
  production build, and the `cfg(test)` split makes the reachability claim explicit rather than incidental.
