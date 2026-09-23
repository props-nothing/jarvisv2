# ADR-0034: A delegated check that cannot express the rule is not a control

**Status:** Accepted

**Date:** 2026-09-23

## Context

`P3-009a` established that `rmcp`'s `origin_is_allowed` cannot express the control JARVIS needs, because
`a_port.is_none() || a_port == o_port` makes a portless allowlist entry a wildcard over **every** port while
treating an explicit default port as literal. At the time that was a *design* argument for owning the
comparison.

`P3-009b` turned it into a **functional** one. The SDK's `validate_origin_header` compares against
`allowed_origins` directly and has no extension point — `with_allowed_origins` is the whole surface. There is
no way to hand it JARVIS's comparison. So origin validation **cannot be delegated at all**.

The slice also closed a gap its own predecessor recorded: `P3-009e`'s tests called `JarvisMcpServer::invoke`
directly, so the trait methods `list_tools` and `call_tool` — the ones the SDK actually routes to — had
**never been executed**. A signature or response mismatch would have compiled and been wrong.

## Decision

**1. The SDK's origin check is disabled, and JARVIS's policy is the control.**

`ServingConfig::sdk` calls `disable_allowed_origins`, and `ServingConfig::origin_check` is the decision.
Enforcement at the request layer is `P3-009c`'s, because that layer owns the request.

**Leaving both enabled would be worse than leaving the SDK's off.** Two checks with different rules would
disagree about a portless entry, and the permissive one would be part of the answer — while a reader seeing
`allowed_origins` populated would believe *that* was the control. A disabled field with the reason recorded is
honest; an enabled field whose rule is wrong is a false assurance.

**2. Every permissive SDK default is a stated value, and the tests assert the difference.**

| Field | Set to | Left alone would mean |
| --- | --- | --- |
| `legacy_session_mode` | `false` | an `Mcp-Session-Id` the revision removed |
| `stateless_protocol_metadata_required` | `true` | an absent `MCP-Protocol-Version` treated as `2025-03-26` |
| `max_request_body_bytes` | the configured bound | the SDK's default, which nothing here stated |
| `allowed_hosts` | loopback only | already fail-closed, but stated so a change is visible |
| `allowed_origins` | disabled, check is elsewhere | validation off, or a port wildcard |
| session manager | `NeverSessionManager` | an in-memory store for a protocol with no sessions |

`the_hardened_configuration_differs_from_the_sdk_default_in_every_permissive_field` reads the SDK's `Default`
alongside the hardened value, so a dependency upgrade that changes a default fails a test rather than silently
changing the policy.

**3. A configuration that allows a public origin is refused outright.**

Serving off-host needs audience-bound tokens (RFC 9728 Protected Resource Metadata and RFC 8707 resource
indicators), which are not built and are their own slice. So `ServingConfig::new` returns
`RemoteBindRequired` for a policy whose origins are not on this host. A remote bind is refused rather than
warned about, because **a remotely reachable MCP server without audience-bound tokens is an unauthenticated
control plane** — and a warning is a statement nobody acts on.

The check is `ServerExposure::requires_remote_bind`, which tests the **host** rather than the presence of an
entry, so `http://localhost:3000` is served and `https://jarvis.example.com` is refused.

**4. An endpoint that would advertise nothing is refused.**

`checked_service` returns `NoToolsAdvertised` for an empty served set. A client connecting successfully to a
server offering nothing cannot tell a misconfiguration from an empty registry, which is the same reasoning
that makes an empty tool pipeline `None` rather than empty.

**5. The handler is driven through the real SDK service in a test.**

`tests/serving.rs` builds an `http::Request`, hands it to `StreamableHttpService`, and reads the
`http::Response` — the path a remote client's POST takes. This is where three facts were **discovered rather
than assumed**:

- **The `Accept` header must list both `application/json` and `text/event-stream`.** A request sending only
  `application/json` is answered `406 Not Acceptable`. `P3-009a` recorded this as a client obligation; the
  fixture found that the SDK enforces it on the server side too.
- **A refused tool call is `200` with `result.isError: true`**, not a `4xx`. That is the decision in
  `P3-009e` seen from outside: the refusal is a **result about the call**, which is why a caller can read it.
- **A `GET` is `405`** and an oversized body is `413`, both from the SDK, now pinned.

**6. `Clone` on the handler is stated as a compile-time assertion.**

The service factory constructs a handler **per request**, so `JarvisMcpServer` must be `Clone`. This is
expressed as a `const _: () = { … }` item rather than a runtime test, so removing the derive is a compile
error naming the requirement instead of a trait-bound failure inside a dependency. The runtime test that
remained asserts the clone keeps the served set, which is the property a factory needs.

## Consequences

- **A check that cannot express the rule is not a control, and the honest response is to own it rather than
  wrap it.** The reusable form: when a dependency's validation has no extension point and its rule is wrong,
  disabling it **with a recorded reason** beats enabling it, because enabling it creates a second answer that
  a reader will take for the control.
- **The `P3-009e` gap is closed.** `list_tools`, `call_tool`, and `server/discover` are now executed through
  the SDK, so a response-shape mismatch has somewhere to fail. `server/discover` also confirms the modern
  revision is advertised and `2025-11-25` is not.
- **A falsification corrected a test's own claim.** `a_request_without_a_protocol_version_header_is_refused`
  was written as evidence for `stateless_protocol_metadata_required(true)`. Removing that line did **not**
  fail it, because the refusal comes from the SDK's body-versus-header agreement rule — a body `_meta` version
  requires the matching header whatever the flag says. So the test was pinning a different property, and
  `a_request_with_no_protocol_signals_at_all_is_refused` was added to isolate the flag. This is the second
  time in this phase a test's comment claimed a mechanism the falsification did not support.
- Still **not built**, recorded rather than implied: **nothing is bound.** `ServingConfig` produces the SDK's
  service, and binding it to a socket is the daemon's, because `repository-layout.md` gives network listeners
  to the composition root — so this is reachable from a test and not yet from a client. **Origin enforcement
  over a real request** is `P3-009c`'s, since the decision is here and the layer that owns the request is
  there. No daemon wiring, no per-client allowlist, no rate limiting.

## Alternatives rejected

- **Enable the SDK's origin check alongside JARVIS's policy.** Two answers with different rules, one of them
  admitting any port, and the permissive one becomes part of the result.
- **Wrap the SDK's check by populating `allowed_origins` from the policy.** Impossible for the intended rule,
  and it would silently widen it — the portless-entry wildcard means the narrowest entry is the broadest.
- **Warn about a remote-bind policy instead of refusing.** A warning nobody acts on, for a configuration that
  would expose an unauthenticated control plane.
- **Bind the endpoint here.** Network listeners belong to the composition root, and a library crate holding a
  socket would put the daemon's topology in an adapter.
- **Use the SDK's default session manager.** It stores state for a protocol that removed sessions.
- **Leave `max_request_body_bytes` at the SDK's default.** An unstated bound is a value that changes without a
  line in this repository changing — the rule this phase has now applied four times.
- **Keep the handler's trait methods untested because `invoke` is tested.** The trait methods are what the SDK
  routes to; testing the method the SDK never calls proves the rule but not the wiring.
