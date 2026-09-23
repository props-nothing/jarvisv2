# ADR-0038: A network request is never a local caller, and a policy refusal is a wire answer

**Status:** Accepted

**Date:** 2026-09-23

## Context

`P3-009i` built `RequestGate` and then recorded that **nothing binds** — the gate decides correctly when handed
four values, and no request has ever reached it. Closing that needs a layer that derives the four values from a
real HTTP request, and deriving them is where a binding layer goes wrong in a way no policy test can see: a
header read under the wrong name, an absent `Origin` collapsed into an admitted one, a credential accepted
without its scheme. Every one of those leaves the gate's own tests green.

## Decision

**1. The endpoint is a `tower` service that wraps the SDK's service, not a route handler.**

The SDK's Streamable HTTP service **is** a `Service<Request<Body>>`, so wrapping it is one `impl Service` rather
than a second routing layer and a second body type. `axum::Router::fallback_service` accepts any `Service`, so a
daemon mounts this without a route table of its own. And the wrapping is what makes the layer provable: driving a
real request through it is the only evidence that the values were *derived* rather than handed over.

**2. The SDK's service stays out of the crate's public surface, and the boundary test was extended to the shape
that would leak next.**

The field type is a private alias and `into_service` returns `impl Service<Request<B>, …>`, so a caller can mount
the service while the crate's contract names only `http` types. `ResponseBody` is
`BoxBody<Bytes, Infallible>` — the SDK's own response type — and naming it is a fact about `http-body-util`
rather than about MCP, which is why it may be public while the service may not.

The new shape is a **type alias**: a `pub type` is a public declaration in a signature-like position with no
`fn` in it. `boundary_tests.rs` had a probe for `pub fn` only, so a `pub type Leaked =
StreamableHttpService<…>` was added to the real source as a **live probe of the scanner** — and the scanner
reported it by line. The probe was then removed and replaced by a test over the same text, which **failed on its
first run** because a fixture without a `use rmcp::…` line cannot trigger the imported-name form. The scan keys
imported names off the imports *in the same file*, which is correct behaviour and was discovered rather than
assumed.

**3. A request that arrived over a network listener is `CallerOrigin::Remote`, always.**

This is the slice's most consequential decision. `CallerOrigin::Local` means **the operator on their own
machine**, and `P3-009h` established that it is supplied by the request layer rather than derived from anything
a caller sends. What establishes it is the daemon's **transport** — a named pipe, or a socket it opened itself
and never exposed. A loopback *bind* is not that evidence: any local process, and a browser on the same machine,
can reach `127.0.0.1`.

Deriving `Local` from a peer address would be `CallerOrigin`'s own defect one layer down — a property of the
request deciding admission without a credential — and it would do it in the one place that looks most like
evidence. So this layer never claims `Local`, and a daemon that wants a local caller admitted does so through
the **allowlist**, which is a credential check like any other.

Falsified by changing the literal to `CallerOrigin::Local`: three tests fail, including
`a_remote_caller_with_no_credential_is_refused_before_the_handler_runs`, which is the anonymous remote caller
being admitted.

**4. A credential requires its scheme, compared case-insensitively.**

`Authorization: Bearer <digest>`, and the scheme's **presence** is required while its spelling is not. A bare
digest in `Authorization` is not valid HTTP, and accepting it would admit a value no real client sends — the
same reasoning `Fingerprint::parse` uses to refuse a pasted URL. The scheme is compared with
`eq_ignore_ascii_case` because HTTP auth schemes are case-insensitive, so a client sending `bearer` is not
refused for spelling while a client sending `Basic` is refused for content.

Falsified two ways: removing the scheme check admits the bare digest (`200` where `401` is required), and
removing the case-insensitivity would refuse `bearer`. Both directions are pinned, because each is a plausible
"tightening" of the other.

**5. An oversized `Origin` is truncated, never treated as absent.**

An absent `Origin` is **admitted** by the specification's own rule — a non-browser client sends none — so
collapsing a hostile or unparseable value into "absent" would invert the control. Only the **copy** is bounded,
because a hostile request must not make this layer allocate; the truncated value is still passed to the policy
as a present value, which refuses it.

Falsified by adding a `.filter(…)` that turns an oversized header into `None`: the response became `200 OK` with
the full tool list served. That is the inversion, demonstrated rather than argued.

**6. The refusal answers the policy class and never the verdict.**

`403` for an origin and `401`/`429` for a caller, from `RequestRefusal::refusal_status`, with a short JSON-RPC
body whose `message` names **the policy** rather than the verdict. `RequestRefusal::reason` *does* name the
verdict and is for the log: telling a hostile caller whether its origin was unlisted or malformed would help it
enumerate the allowlist. The `-32000` code is JSON-RPC's implementation-defined range, because the revision
reserves specific codes for protocol conditions and reusing one would send a caller looking for a protocol bug.

**7. This layer holds no policy, only extraction.**

There is no `if origin …` beyond reading the header, no admission branch, and no restatement of the order. The
gate decides, and the layer answers the status it returns. A second place the order lives is a second place it
can be wrong — which is what `P3-009f` spent a slice discovering about a documented invariant with no test.

**8. `check_servable` is called here and not repeated.**

An endpoint over a handler that serves nothing is refused at construction, through
`ServingConfig::check_servable`. The check lives in one place because two copies are two things to keep in step,
and a client that connected successfully to a server offering nothing could not tell a misconfiguration from an
empty registry.

## Consequences

- **The limit every `P3-009` slice recorded is closed at the layer**: a real HTTP request now passes both
  policies, with `Origin` first, and a refusal has a status. What remains for the daemon is choosing the port and
  the bind address, which `repository-layout.md` gives to the composition root.
- **A network-reachable JARVIS MCP server still admits nobody remote**, because the token is unvalidated: RFC
  8707 audience binding and RFC 9728 Protected Resource Metadata are the OAuth slice. `CallerOrigin::Remote` plus
  a fingerprint allowlist means an operator may admit a caller whose credential they configured by hand, and
  nothing else. That is recorded rather than implied, and it is why `CallerAdmission::local_only` remains a
  sensible default.
- **`spent_budget` is still `false`.** This layer has no counter, so the rate limit the gate can apply never
  fires. Passing a literal rather than a parameter makes that visible at the call site instead of leaving a
  reader to infer it from a missing argument list.
- **Nothing consumes the `RequestAdmission`.** It is logged on a span and then dropped, so a refusal is
  observable and an admission is observable but **not attributable**. A `run_events` row for an MCP call needs a
  correlation id an MCP request does not carry (`P3-012`), and minting one here would put a fabricated identity
  in the audit trail.
- The falsifications above are the evidence: four mutations, each failing the property it names and — for the
  order swap, run at this layer too — failing `a_request_that_fails_both_policies_is_reported_as_an_origin_refusal`
  with `left: 401, right: 403`.

## Alternatives rejected

- **A hand-written `axum` route handler.** A second body type, a second routing decision, and the SDK's service
  would have to be called with a reconstructed request.
- **Return the SDK's service from a public accessor.** The boundary violation `P3-009f` removed; the daemon does
  not need the type, only something mountable.
- **Take the peer address and derive `Local` from `is_loopback`.** A browser on the same machine is a local
  process, so this would admit a web page as the operator — a self-reported property deciding admission, one
  layer down.
- **Accept a bare digest as well as a scheme.** Admits a value no HTTP client sends, and a string a caller can
  craft more freely than a valid `Authorization` value.
- **Require `Bearer ` with an exact case match.** Refuses a conforming client over spelling, which is a
  false rejection rather than a tightening.
- **Treat an oversized `Origin` as absent.** Absent is admitted, so this inverts the control.
- **Put the refusal verdict on the wire.** Helps a hostile caller enumerate the allowlist.
- **Repeat `check_servable` in the endpoint.** Two copies of one check, which is the pair-of-controls shape this
  phase keeps finding one level up.
- **Give the endpoint its own rate-limit counter.** A policy that mutates per request cannot be compared, logged,
  or reused, which is why `CallerAdmission` takes `spent_budget` as a value.

## Conditions that would justify revisiting

- **A local transport for the MCP endpoint** (a named pipe, or a socket the daemon opens and holds). That is the
  only thing that would establish `CallerOrigin::Local`, and decision 3 is the sentence to change.
- **OAuth resource-server support arriving.** Audience-bound tokens would replace the fingerprint allowlist for
  remote callers, and `spent_budget` would then have a natural counter behind it.
- **A second request body type reaching the same layer** (a streamed body, an SSE request). The service is
  generic over the body, so the change is a bound rather than a rewrite; `ResponseBody` is the part that would be
  re-examined, because it is the SDK's response type passed through unchanged.
