# ADR-0076: `Retry-After` is three situations, not two

- **Status:** Accepted
- **Date:** 2026-09-27
- **Slice:** `P5-005` (the Google refusal path).
- **Relates to:** `ADR-0075` (the classification table, whose `429` arm this corrects), `ADR-0058` (a provider's
  decisions belong in the crate with no socket), `ADR-0062` (an unanswered request is classified by whether it
  reached the provider), `ADR-0068` (the transport that carries `Retry-After`), `P5-005`.

## Context

`RFC 9110` §10.2.3 defines the field's value as **an HTTP-date or a number of seconds**, and states the two
alternatives in the same sentence:

```
Retry-After = HTTP-date / delay-seconds
delay-seconds = 1*DIGIT
```

`ADR-0075` wired `client::classify` into the refusal path and made a `429`'s stated delay visible in the bounded
reason. It represented that delay as `Option<u32>`:

```rust
pub retry_after_seconds: Option<u32>,
```

`Option` has **two** values and the field has **three** situations:

1. the header was **absent** — the provider stated nothing;
2. `Retry-After: 120` — a stated `delay-seconds`;
3. `Retry-After: Fri, 31 Dec 1999 23:59:59 GMT` — a stated `HTTP-date`.

The transport parsed digits only (`value.trim().parse::<u64>().ok()`), so situation 3 became `None` — the same
value as situation 1. The classifier then could not tell them apart, and both fell to the documented floor:

```rust
429 => (
    RetryClass::Throttled,
    match retry_after_seconds {
        Some(seconds) => RetryGuidance::RetryAfterSeconds(seconds),
        None => RetryGuidance::BackoffSeconds(GOOGLE_RETRY_FLOOR_SECONDS),
    },
),
```

**A `Retry-After` stated as an `HTTP-date` was therefore read as no `Retry-After` at all.** That is not merely a
loss of information; it is the direction that retries **too soon**, because the field's entire purpose is to say
*do not ask again before this time*. RFC 9110 §5.6.7 makes the date form one a recipient **MUST accept**, so the
form is not exotic — it is one of the two the field is defined to carry.

The existing test had already recorded the shape of the gap, in the words a previous slice chose for it:

```rust
// An unreadable value is **absent**, not zero.
assert_eq!(response.retry_after_seconds, None);
```

That assertion was correct as a statement about the **old type** — a `None` is not a zero — and it is exactly the
conflation this ADR removes: *absent* and *unreadable* were the same value, and the test could not tell them
apart either.

This is the same class of defect the P5-005 slices have found four times already, in its **value** form rather
than its variant, constraint, outcome, or policy-table form: **a declaration whose two values stand for more than
two situations.**

## Decision

The field is represented by a type with three states, and the parse is a pure function beside it.

```rust
pub enum RetryAfter {
    Seconds(u32),
    NotSeconds,
}

pub fn parse_retry_after(value: Option<&str>) -> Option<RetryAfter>
```

`None` means **the header was absent**. `NotSeconds` means **a value was present and is not a number of seconds
this client can use**. `TransportResponse::retry_after` is `Option<RetryAfter>`.

Five properties of that shape are deliberate:

1. **A stated-but-unreadable delay is never reported as an absent one.** The classifier keeps the three cases
   apart, and the middle one says so:

   ```rust
   match retry_after {
       Some(RetryAfter::Seconds(seconds)) => RetryGuidance::RetryAfterSeconds(seconds),
       Some(RetryAfter::NotSeconds) => {
           RetryGuidance::BackoffAfterUnreadableDelay(GOOGLE_RETRY_FLOOR_SECONDS)
       }
       None => RetryGuidance::BackoffSeconds(GOOGLE_RETRY_FLOOR_SECONDS),
   }
   ```

2. **The variant is named for what is readable, not for the conclusion.** The grammar's other alternative is an
   `HTTP-date`, so `HttpDate` was the obvious name — and it would be a **lie** for an all-digit value too large
   for a `u32`. `delay-seconds = 1*DIGIT` has **no upper bound**, so `Retry-After: 99999999999999999999` is
   `delay-seconds` by grammar and simply does not fit; a plain `parse::<u32>().ok()` would return `None` for it,
   which reads as "retry now". Both land in `NotSeconds` because the only distinction a retry decision needs is
   *"can this client express the stated delay in seconds"*, and naming the variant after the diagnosis would
   have hidden the case where the diagnosis is wrong.

3. **A value that is neither form is grouped with the date form, not dropped.** The type does not claim to be a
   date parser. An unrecognised value is always read as a **stated** delay this client cannot use, never as an
   absent one, so a malformed value fails toward caution rather than toward an immediate retry.

4. **The wait is the floor for both non-numeric cases, so the words are what distinguish them.** A guidance
   variant (`BackoffAfterUnreadableDelay`) rather than a different number, because honouring the date form needs
   a clock and a transport that read its own clock would be making the retry decision it does not own.
   `Refusing` to retry instead was rejected: throttling is the single most retryable class, and a `429` whose
   delay is unreadable is still transient. A silent fallback was rejected because it would present a JARVIS floor
   as the provider's own instruction.

5. **An empty value is absent.** Both grammars require content (`1*DIGIT`, `HTTP-date`), so a present-but-empty
   value carries nothing to interpret. This is the one case where "present" and "absent" legitimately coincide.

### The correction to `ADR-0075`'s other recorded limit

`ADR-0075` also asserted, of `RetryDecision::provider_request_id`:

> Google returns the identifier in a response **header** …

That was an **assumption stated as a finding**. The source `ADR-0075` was written from — Gmail's
`handle-errors` guide, read 2026-09-15 — says only that the API "returns two levels of error information: HTTP
error codes and messages in the header; A JSON object in the response body", and names **no request-id header**.
So the claim is corrected in place, in `ADR-0075` and in `TODO.md`, to say what is actually true: there is no
place for an identifier to come from, **and whether Google supplies one is unverified**. The identifier stays
unpopulated as a recorded limit, and the header name is now an open question rather than a premise. This ADR
makes no attempt to build that reader; settling an unverified name first is the point.

## Consequences

- A `429` whose delay arrives in the date form is no longer indistinguishable from one that carried no delay.
  The retry decision's **timing** is unchanged — both wait the floor — but the decision's **stated reason** now
  matches the wire, so an operator reading a stored outcome is not told the provider stated nothing when it
  stated a time.
- The reason renders the distinction: `… (throttled); retry after 1s (the provider stated a delay this client
  could not read)`.
- `RetryGuidance` gained a variant, so the complement property in `ratelimit_tests` (a guidance states a delay
  exactly when it permits a retry) was extended to include it — a property asserted over an incomplete list of
  variants is a property about the list, not about the type.
- The transport's two `reqwest` sites now share one parser, so the grammar and the type that represents it live
  together and the interpretation cannot drift between the read and the form path.
- **Two mutants were falsified A-B-A**, both compiling:
  - collapsing the `NotSeconds` arm into the absent arm in `classify` (detected; the failure printed
    `left: BackoffSeconds(1)` against `right: BackoffAfterUnreadableDelay(1)`);
  - making `parse_retry_after` return `None` for the non-numeric form (detected in two tests, including the
    transport test whose expectation this ADR changes).
- **A limit remains:** the date form is recognised but not converted to a delay. Doing so needs a clock and a
  policy for a skewed or hostile one (RFC 9110 §8.8.1 is explicit that a validator is not a trust mechanism), so
  it is a separate decision with its own falsifying test rather than something this slice implies it has solved.

## Alternatives considered

- **Keep `Option<u32>` and document the gap.** Rejected: the gap is the **unsafe direction** — a stated wait read
  as a missing one — and a doc comment does not make the two values distinguishable at a call site.
- **Represent the date form as `HttpDate`.** Rejected: it names the diagnosis, and the diagnosis is wrong for an
  oversize `delay-seconds`, which is the same situation and the same hazard.
- **Parse the date to a delay inline in the transport.** Rejected: it needs a clock, and a transport that read
  one would be making a retry decision the port's own doc says it must not (`TransportResponse`'s `retry_after`
  "is not interpreted").
- **Refuse to retry a `429` whose delay is unreadable.** Rejected: it converts the most retryable class into a
  refusal on the strength of a formatting choice, and the cost of retrying at the floor is one call, while the
  cost of not retrying is a dropped run.
- **Return the `RetryAfter` from the transport as a raw `String`.** Rejected: it moves the grammar into every
  caller and makes "present but unreadable" a string comparison rather than a type.

## Conditions that would justify revisiting

- Google's guides change to state a request-id header, or a live capture shows one — then
  `provider_request_id` gains a producer and `TransportResponse` gains a place for headers.
- A measured case shows the date form occurring in practice with a delay materially above the floor, which would
  justify a clock-injected conversion with its own bound.
- A provider is added whose `Retry-After` uses only the date form, which would make the conversion a
  cross-provider requirement rather than a Google refinement.
