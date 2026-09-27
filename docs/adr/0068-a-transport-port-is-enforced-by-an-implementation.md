# ADR-0068: A transport port is only enforced by an implementation that exists

- **Status:** Accepted
- **Date:** 2026-09-27
- **Slice:** `P5-005` (the Google HTTP transport).
- **Relates to:** `ADR-0062` (an unanswered request is classified by whether it reached the provider),
  `ADR-0061` (a credential cannot be rendered or serialized), `ADR-0027` (an MCP endpoint is a validated value
  whose client this project builds), `ADR-0067` (a staleness signal needs a producer).

## Context

`ADR-0062` declared `GoogleTransport` with a list of things an implementation "must not do": follow a redirect,
retry, read a proxy from the environment, or return an error for a non-2xx status. The port shipped with those
requirements **unenforceable**, because the only implementations were test doubles. A test double has no
redirect policy, no proxy configuration, and no notion of a retry — so every requirement was satisfied by
construction and none was tested. The crate's own limits recorded this honestly ("the port's own requirements
are unenforced … they become testable only when a real transport is written").

## Decision

Implement the port against `reqwest` 0.13.5 in `crate::google::http`, with:

- `redirect(Policy::none())` — so a `3xx` is a `TransportResponse` carrying its status, and the caller's
  `classify` sees it;
- `no_proxy()` — explicit, because `reqwest`'s `system-proxy` default is on;
- exactly one request per `send` — no retry branch exists to be disabled;
- `timeout` and `connect_timeout` — the port requires self-bounding;
- a `classify_error` map that checks `is_timeout()` **before** `is_connect()`.

## The ordering, which is the substance

`is_timeout()` first, although `is_connect()` is more specific where both apply. The reason is the failure
direction: `TransportFailure::Timeout` is **ambiguous** — the request may have been written — so choosing it
where the certain variant might also fit can only make a caller less willing to retry. The reverse mistake would
let a non-idempotent effect repeat, which is a second effect. A pure connect failure is not a timeout, so
`Connect` is still reported for DNS and refused-connection failures and the specific case is not lost.

`Send` is the final arm. A failure this map does not recognise is treated as one that **may** have reached the
provider, because an unrecognised failure is precisely the case where certainty is unwarranted.

## What is deliberately absent, and why each absence is a decision

- **No `error.is_redirect()` arm.** With `Policy::none()` no redirect produces an error, so the arm could never
  fire. That is the unreachable-refusal defect `P5-001` and `P5-003` each record; a branch that cannot run reads
  as protection while enforcing nothing.
- **No body-size bound.** `TransportFailure` has no variant meaning "the answer was too large to read", and
  `Body` means the body could not be read rather than that this client declined to. A streaming cap needs
  `bytes_stream` plus a policy that belongs with `P5-009`'s output handling. Recorded as a limit.
- **No token refresh** — the token is presented as given, so a stale one becomes a provider refusal, which is
  the honest reading.

## Consequences

- Four port requirements became **controls with tests**: no redirect followed (asserted by the redirect
  *target* receiving zero connections), no retry, no environment proxy, and a non-2xx as a response.
- The `reqwest` dependency adds **no package**: it resolves to the `=0.13.5` already in the lock through three
  other crates, and `default-features = false` with `rustls` keeps the bundled-TLS policy (`cargo deny` all
  four categories ok, one lock line changed).
- `repository-layout.md` already named `client.rs` the "provider HTTP client"; the **decisions** stay in
  `client.rs`/`request.rs`/`operations.rs`, none of which names a `reqwest` type. The transport is the edge.
- `HttpRequest` gains a `#[cfg(test)]` `rebase_to` seam so a test can aim a **real** product request at a
  loopback server. It rewrites the origin alone, so the path and encoded query under test are the ones the
  product builds.
- **Still not a live path.** No request has been sent to Google; the transport has **no production caller**,
  because no composition root constructs the connector. This closes the *enforceability* gap, not the
  *verification* gap.

## Alternatives considered

- **Leave the port unimplemented until a composition root needs it.** Rejected: the requirements had already
  been written down and read as satisfied, which is worse than not having them. `ADR-0067`'s lesson — a
  parameter with no producer — applies to a *constraint* with no implementation.
- **Mock `reqwest`.** Rejected: the four requirements are properties of the client's *configuration*, and a mock
  would satisfy them by construction exactly as the test doubles did.
- **Use a framework for the test server.** Rejected: a shared library between client and server can share a
  misunderstanding of the specification, which is why `jarvis-mcp-transport` hand-writes its server.
