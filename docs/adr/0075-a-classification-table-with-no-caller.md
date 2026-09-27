# ADR-0075: A classification table with no caller decides nothing

- **Status:** Accepted
- **Date:** 2026-09-27
- **Slice:** `P5-005` (the Google refusal path).
- **Relates to:** `ADR-0058` (a provider's decisions belong in the crate with no socket), `ADR-0062` (an
  unanswered request is classified by whether it reached the provider), `ADR-0073` and `ADR-0074` (the two
  other "declared but not produced" forms found in this module), `P5-001`.

## Context

`google::client::classify` is the table `ADR-0058` was written around. Its own doc states the case for its
existence:

> **A 403's meaning comes from its `reason`, never from the status.** Four documented reasons share the status
> and have three different remedies … A classifier switching on the status alone would retry an
> administrator's decision.

It had **no production caller**. `operations::refusal` read the machine-readable reason code out of the body and
formatted it, and:

- never called `classify`, so `RetryClass` and `RetryGuidance` — the retry decision the table computes — reached
  nothing;
- discarded `TransportResponse::retry_after_seconds`, which the transport carries, documents as "not
  interpreted … whether a stated delay is *honoured* is `client::classify`'s decision", and passes through
  unread. A `429` with `Retry-After: 37` produced a reason that named neither the class nor the delay.

`RetryDecision::provider_request_id` is also unpopulated in production, and that part is a genuine gap rather
than an oversight: `TransportResponse` carries the status, the delay and the body but **no headers at all**, so
there is nowhere for an identifier to come from.

> **Correction (ADR-0076).** The first version of this paragraph asserted that *"Google returns the identifier
> in a response **header**"*. That was an **assumption stated as a finding**. The source this ADR was written
> from — Gmail's `handle-errors` guide, read 2026-09-15 — says only that the API "returns two levels of error
> information: HTTP error codes and messages in the header; A JSON object in the response body", and names
> **no request-id header**. So the honest statement of the gap is the one above: there is no place for it to
> come from, *and whether Google supplies one at all is unverified*. The identifier stays unpopulated as a
> recorded limit rather than being filled with a value from the body, and the header name is now an explicit
> open question rather than a premise.

## Decision

`refusal` calls `classify`, and the class and the delay are appended to the bounded reason:

```
the provider refused the call: rateLimitExceeded (throttled); retry after 37s
the provider refused the call: domainPolicy (permanent)
the provider answered 503 (provider_fault); retry after 1s
```

Three properties of that shape are deliberate:

1. **The delay is appended only when `guidance.delay_seconds()` is `Some`.** The permanent, authentication and
   reconcile arms carry no seconds, so a refusal that must not be retried cannot show a delay even when the wire
   carried one — a caller that saw "retry after 60s" beside `domainPolicy` would back off and retry an
   administrator's decision. A test sends a `403 domainPolicy` **with** `Retry-After: 60` and asserts the
   absence.
2. **An unparseable body is still classified**, by its status, because the status is a fact when the body is
   not. The distinction that survives is "we know little" versus "we know nothing", applied to the retry
   decision rather than to the fact of the refusal.
3. **`Unknown` says so and carries no delay.** An unrecognised status is the fail-closed direction —
   `RetryClass::Unknown` refuses a retry whatever the idempotency — so a `418` reads `unknown` and `Reconcile`
   offers no seconds. A reader cannot mistake it for something worth retrying.

The class rides in the reason because **`jarvis-tools`' `AdapterError` has no field for a connector's own retry
vocabulary**, and `ToolOutcomeRecord`'s reason is the one bounded channel that reaches a caller. That is a real
constraint rather than a preference, and it is why the reason is a rendering of the decision rather than the
decision itself.

## Consequences

- The table decides something, so its 403-reason cases are observable from a stored failed call rather than only
  from a unit test.
- Two guards were falsified A-B-A with compiling mutants: dropping the stated delay (detected, and the failure
  printed the fallback floor `retry after 1s` — so the mutation was genuinely a loss), and removing the class
  from the reason (detected, printing `rateLimitExceeded ()`).
- **A provider request identifier still cannot be carried**, because the transport keeps no headers. That is now
  the sharpest remaining gap in this path, and it is a `TransportResponse` change rather than a
  `google::operations` one.

## Alternatives considered

- **Return the `RetryDecision` from the adapter instead of a string.** Rejected: `ToolExecutor`'s contract has no
  place for it, and changing that contract to carry one connector's vocabulary is the boundary violation
  `ADR-0047` and the repository-layout graph forbid.
- **Leave the table uncalled and delete `Retry-After` from `TransportResponse`.** Rejected: the delay is the
  thing a `Throttled` answer needs, and deleting it would remove the transport's only record of the provider's
  instruction.
- **Populate `provider_request_id` from a response header by adding headers to `TransportResponse` now.**
  Deferred rather than rejected: it is a type change with its own falsifying test, the current slice's claim is
  about the classification rather than the header, and **the header name is itself unverified** (see the
  correction above). Building a reader before settling the name would be writing code against a remembered
  convention rather than a read one.
