# ADR-0069: Two tested halves do not test the seam between them

- **Status:** Accepted
- **Date:** 2026-09-27
- **Slice:** `P5-005` (the Google read path, end to end over a socket).
- **Relates to:** `ADR-0068` (a transport port is only enforced by an implementation that exists),
  `ADR-0067` (a staleness signal needs a producer), `ADR-0063` (a wire fixture declares whether it is a capture
  or a shape), `P3-006a` ("two values that must agree, with nothing holding both").

## Context

By the end of the transport slice, every part of the Google read path had a green test:

- the request builders and parsers, tested as pure functions;
- `GoogleReadTool`, tested as an adapter against a **`Scripted`** transport;
- `ReqwestTransport`, tested against a **hand-written request** it was handed directly.

So the adapter had never run against the real transport, and the transport had never been handed a request the
adapter built. Both halves were correct and the **join** was unverified. This is the same shape `P3-006a`
recorded for the authorization seam — where each slice was self-consistent and the security defect lived only in
the space between two correct modules — and the same *kind* of gap `ADR-0067` and `ADR-0068` each found from a
different direction (a variant with no producer; a constraint with no implementation). Here it is a **joint with
no test**.

## Decision

Add seam tests in `http_tests.rs` that drive `GoogleReadTool` against a real `ReqwestTransport` over a real
socket, with two recorded test seams so the product's own path is exercised rather than a reconstruction:

- `HttpRequest::rebase_to(origin)` — rewrites the **origin alone**, so the path and the percent-encoded query
  under test are the ones the product builds;
- `GoogleReadTool::run_with_origin(..)` — `#[cfg(test)]`, and the production `apply_origin` is `#[cfg(not(test))]`
  so a shipped build contains no reference to the seam at all.

And, critically, one test that drives **all four declared operations**: `every_operation_reaches_the_socket_…`
asserts each one's target path against its own server.

## Why the cross-operation test is the one that matters

The three single-operation seam tests would pass if *every* operation were routed to the same endpoint: they
assert that the expected data came back, and a mis-routed call that still answered with a parseable body would
be indistinguishable. The cross-operation test compares a **path per operation**, so a mis-route fails with the
path that was actually addressed.

This was verified, not asserted. Mutating the history builder's path to `/users/me/messages` produced:

```
google.gmail_history_list must address /gmail/v1/users/me/history, got
/gmail/v1/users/me/messages?startHistoryId=12345
```

Two earlier attempts at a routing mutant were **rejected by the compiler** (`VACUOUS`, not `FAIL`) — changing a
builder's identity changes its argument types, so the mutant could not be built. That is a property worth
recording: a routing mistake inside one module is usually unrepresentable, which is *why* the seam — the place
the routing is *selected* and the request is *sent* — is where the defect can live.

## Consequences

- The join is exercised: `dispatch` → `request_for` → `HttpRequest` → `ReqwestTransport::send` →
  `TransportResponse` → `interpret` → `ToolCallResult`, over a socket, with the request observed on the wire
  (method, encoded query, `Accept`, bearer header, and the credential's absence from the URL).
- A mis-routed operation now fails with the path it addressed rather than passing on a parseable answer.
- The adapter's origin seam is `#[cfg(test)]`-only on **both** sides, so no shipped build can reach it.
- **Recorded limit, unchanged:** the responses are still written by the test file, so this proves the seam and
  not the record. The opt-in live smoke test remains the only thing that would.

## Alternatives considered

- **Let the seam be covered by the live smoke test.** Rejected: a gated, credential-dependent, cost-labelled
  test runs rarely, and the seam is exactly where a defect is both likely and cheap to catch offline.
- **Have the test build its own `HttpRequest` against the loopback origin.** Rejected: it would test the
  transport against a request the product does not build, which is the gap this ADR closes. The seam rewrites
  the origin and nothing else.
- **One test per operation with a shared assertion.** Rejected for the reason above — per-operation *paths* are
  what make a mis-route observable.
