# ADR-0118: A diagnostic that contradicts the predicate the platform gates on

- **Status:** Accepted
- **Date:** 2026-10-01
- **Slice:** `P5-005` (the Google connector — the health state's staleness reaching the diagnostic that must
  report it).
- **Relates to:** `ADR-0116` (the slice immediately before this one, in the same module and about the same
  report), `ADR-0092` (a value with a producer and no reader — the field whose reader this adds), `ADR-0021`
  (two values that must agree, with nothing making them: here the report's severity and `permits_calls_at`),
  and `ADR-0044` (a stored value that was not the one a report needed).

## Context

`security.md`'s rule for the platform is *"missing or stale evidence fails closed"*, and
`crate::health::ConnectorHealth` enforces it:

```rust
pub fn is_fresh_at(&self, now: UtcTimestamp, freshness_seconds: u64) -> bool
pub fn permits_calls(&self) -> bool                              // the state, as it was
pub fn permits_calls_at(&self, now, freshness_seconds) -> bool    // the state, now
```

The module doc is explicit that the third is *"the method a caller should use"*, and that *"calling
`permits_calls` on an unfresh state is the defect this exists to prevent: a `Connected` observation from
yesterday permits nothing today, and only the pair of methods makes that sayable."*

The diagnostics report did not use it:

```rust
let health_severity = if health.permits_calls() {   // no instant, no bound
    DiagnosticSeverity::Info
} else {
    DiagnosticSeverity::Error
};
```

Four facts made that a defect rather than a simplification:

1. **A report that says `connected` about a state that permits no call is wrong when it is read.** `Info` is
   the severity that means *nothing to do here*. An operator reading it does not probe again — the report was
   the evidence, and it asserted the opposite of what the platform gates on.
2. **The report contradicted a predicate in the same crate.** `permits_calls_at` is the answer the rest of the
   platform uses; a diagnostic that disagrees with it is worse than a missing diagnostic, because it is
   *read instead of* the truth.
3. **Nothing told the reader either of the two facts the rule turns on.** The report carried
   `HealthObservedAt` — the instant — and carried **neither** the bound the platform applies **nor** now. An
   operator could not see the contradiction, because the two values needed to see it were not in the report.
   That is `ADR-0092`'s shape: `HealthObservedAt` had a producer and no reader that could act on it.
4. **`DiagnosticField` is documented as a closed set chosen so `is_loggable()` is true for all of it and
   `may_reach_a_model()` is false for exactly one field** — a set of facts a diagnostic may report. "This
   state is too old to act on" is such a fact and had no variant, so the set was missing the one field the
   platform's central rule needed.

## Decision

**1. `DiagnosticField::HealthStale` is added, and `diagnostics_for` reports it when the state is stale.**

The field is emitted **only when it is true**, and the absence means fresh — the rule `MissingScopes` already
uses, because a negative finding that is always present is one a reader stops seeing.

**2. `diagnostics_for` takes `now` and `freshness_seconds` and derives the severity from
`permits_calls_at`.**

```rust
pub fn diagnostics_for(
    health: &ConnectorHealth,
    auth_state: AuthState,
    missing_scopes: &[String],
    now: UtcTimestamp,          // new
    freshness_seconds: u64,     // new
) -> Vec<DiagnosticFinding>
```

The instant is a **parameter rather than a clock read**, for the reason the health module's own
`is_fresh_at` records: a value that asked the system clock about its own age could not be checked against a
supplied instant, and `jarvis_core::Clock` exists so time is injectable. The component with the evidence (the
daemon's clock) supplies it; this function is a pure function of its arguments, which is what keeps every
branch testable — the property the function's own doc already claimed.

**3. Staleness and unusability are reported as the two independent facts they are.**

A stale state is an `Error` health state **and** a `Warning` staleness finding; a fresh `NeedsReauth` is an
`Error` health state and **not** stale. Neither finding suppresses the other, and the two predicates are not
interchangeable — see the falsification record below, where conflating them survived the whole suite.

## Consequences

- **The report and the predicate now agree.** A state the platform refuses to act on is reported as unusable,
  so an operator cannot read a diagnostic that contradicts the gate.
- **The staleness is named rather than left to be derived.** The bound and the instant are both inputs now, so
  "why is this stale" is answerable from the report.
- **The field set grew to 21**, and the completeness test's count was updated in the same change, so the new
  field is covered by the loggability, model-exposure, renderability and distinctness assertions that pin the
  module's security property.
- **Three guards falsified A-B-A, all compiling.** (1) the freshness inputs ignored entirely (severity from
  `permits_calls()`, `stale = false`) → **detected by both tests**; (2) staleness conflated with unusability
  (`stale = !permits_calls_at(..)`) → **detected by the second test only, after a missing detector was added**;
  (3) the supplied bound ignored (`u64::MAX`) → **detected by both tests**.
- **A mutant survived the first version of this change, and that is recorded rather than tidied away.** With
  `stale = !permits_calls_at(..)` the whole suite passed, because every state the staleness test used was
  `Connected` — where `is_fresh_at` and `permits_calls_at` agree. The missing detector was a state that
  **refuses and is fresh**, which is exactly the pair that separates the dimensions, and
  `staleness_and_unusability_are_two_dimensions_and_a_mutant_is_why_this_exists` is that detector. The lesson
  is the one mutation testing keeps reteaching: a predicate with two conjuncts is exercised by a value where
  they differ, and a fixture where they agree cannot see the difference.

## Alternatives considered

- **Report only the instant and leave the reader to compare it against a bound.** Rejected: the bound is not in
  the report, and *now* is not either — so the comparison the rule needs is impossible from the report alone.
  This is the alternative the defect was: the instant was there, and nothing about it.
- **Read the clock inside `diagnostics_for`.** Rejected: `jarvis_core::Clock` exists so time is injectable, and
  a report that consulted a clock internally could not be tested at both sides of the bound — the assertions
  that would fail are exactly the ones this change exists to add.
- **Emit `HealthStale` unconditionally with a boolean value.** Rejected: `DiagnosticField` is a set of facts a
  diagnostic reports, not of predicates with values, and a field that is present on every report is one a
  reader stops seeing. The `MissingScopes` precedent is the rule.
- **Make the stale state's `HealthState` finding `Warning` rather than `Error`.** Rejected: the state cannot be
  acted on, which is what `Error` means here, and `Warning` is documented as *"something needs attention but
  the connector still works"* — which is false of a state nothing may call through.
- **Fold staleness into `HealthState` and report no second finding.** Rejected: they are independent facts
  (a stale healthy state and a fresh refusal are both real), and one field cannot carry both without a caller
  having to decode a single severity back into two situations.
- **Take a `freshness_seconds` only, defaulting `now` from the state's own instant.** Rejected: that would make
  every state fresh by construction, which is the bug restated as a signature.

## Conditions that would justify revisiting

- **A caller renders the report** (`doctor`, a UI), at which point the two severities and the staleness field
  are presented together and the presentation may want its own ordering rule.
- **A second source of staleness appears** (a cursor's age, a lease's expiry), at which point the pattern
  "value plus bound plus now" is repeated and may deserve one type rather than three parameters.
- **`DEFAULT_FRESHNESS_SECONDS` stops being the bound most callers pass**, at which point the daemon's policy
  owner is supplying something else and the parameter's default should be reconsidered rather than assumed.
