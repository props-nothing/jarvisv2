# ADR-0040: Conformance is measured against the protocol's schema, not against the SDK

**Status:** Accepted

**Date:** 2026-09-24

## Context

`P3-010` asks for conformance and interoperability tests, and the obvious reading of that is "drive the MCP
Inspector and the official conformance suite against this server." The obstacle is recorded from the previous
slice: `jarvisd` builds its endpoint with `CallerAdmission::local_only()`, and `P3-009g` made that
**enforce** — a *remote* caller is refused even on loopback, because a loopback bind is not evidence that the
caller is the operator. Both the Inspector and the official harness are third-party clients, so both are
refused before any MCP method is reached. Making that work needs audience-bound tokens (RFC 8707) and Protected
Resource Metadata (RFC 9728), which remain unbuilt.

So the live client-level test is blocked on an authorization slice. What is *not* blocked is the question
conformance actually cares about: **does this server's output satisfy the protocol?** That question has an
authority that is not a client, and asking it first is what found a real defect.

## Decision

**1. The authority is the revision's machine-readable schema, vendored as a derived slice.**

The official JSON Schema for `2026-07-28` is a single document published by the specification project. This
crate vendors the **`$defs` closure reachable from the result types it emits** — 11 definitions, 41 KB,
extracted by [`extract.ps1`](../../../crates/jarvis-mcp-transport/tests/spec/extract.ps1) — with the source URL,
access date, licence, and extraction rule in
[`tests/spec/README.md`](../../../crates/jarvis-mcp-transport/tests/spec/README.md).

A **derived** slice rather than a hand-picked one, because a hand-picked set is how a slice quietly stops
covering the field that changes: nobody notices a definition that *should* have been added.
`the_slice_is_closed_under_reference` fails if the file ever references a definition it does not carry.

**2. The validator is a general-purpose JSON Schema implementation, not an MCP library.**

`jsonschema` has no MCP knowledge and no relationship to `rmcp`. This is the whole point: the defect this found
**is an SDK default**, so a test asserting "the SDK serialized what we asked it to" would have agreed with the
bug. The check now has three layers, none sharing an assumption with another — the protocol's schema, a
general-purpose validator, and JARVIS's serialization.

This is the same reasoning `P3-007` recorded for negotiation ("a fixture sharing the code's assumptions cannot
find a revision defect") applied one level up: there the fixture shared the *code's* assumptions, here the
validator would have shared the *library's*.

**3. The test validates the server's own document, and the first version did not.**

The initial test assembled its JSON from `tool_list()` plus the two constants. **The falsification attempt is
what exposed that**: removing the builder call changed nothing, because the test never read the builder. The
construction was extracted into `JarvisMcpServer::tools_list_result` — reachable by both the trait method and
the test — so the validated document is the value the transport actually sends.

*A test that restates the code it checks is not checking it.* The corollary is that a falsification attempt
which does **not** fail is a finding about the test, not a reprieve.

**4. `cacheScope` is `"private"` and `ttlMs` is `0`, and both are posture.**

The revision requires both on cacheable results. The values are not tuning:

- **`private`** because the endpoint is admission-gated. The schema's own distinction is whether a response "does
  not contain user-specific data" and may therefore be cached "across authorization contexts" by an
  intermediary. Answering `public` would tell a caching proxy it MAY serve one caller's tool list to another,
  contradicting the control the endpoint exists to apply.
- **`0`** because the served set derives from a policy an operator can change. A cached list would keep offering
  a tool this server had stopped serving — or keep withholding one it had started serving — and the whole point
  of the served surface is that it is the authority on what a caller may reach. `0` is the schema's own wording
  for "immediately stale".

## Consequences

- **A real conformance defect was found and fixed**: `tools/list` was emitting a document the revision's schema
  rejects, because `rmcp` omits the cache hints by default for multi-era compatibility — a reason that does not
  apply to a server that advertises exactly one era. `DiscoverResult` was *not* affected (its constructor sets
  both), and `CallToolResult` requires only `["content", "resultType"]`, so the defect was isolated to the one
  method. Removing either builder call now fails with `"ttlMs" is a required property`.
- **The live client-level test is recorded as blocked, with the reason.** Running the official
  `modelcontextprotocol/conformance` suite or the Inspector against this daemon needs a caller the allowlist can
  admit, which needs RFC 8707/9728 tokens. It is named as the next step rather than implied to have happened,
  and the daemon's `local_only` posture is the constraint rather than an oversight.
- **The official suite's `--requirements <revision>` flag is the form a tier claim needs.** `--suite` and
  `--spec-version` describe the suite as it grows; `requirements/<revision>.yaml` names exactly what that
  revision required, and only scored scenarios affect the exit code. A shared scenario must run **twice** — once
  per era — because `2025-11-25` and earlier use the stateful handshake and `2026-07-28` is stateless.
- **A vendored schema slice is a maintenance obligation.** It is a copy of external material, so a revision
  change means regenerating it, and `the_slice_matches_the_revision_the_server_advertises` fails if the
  advertised revision moves without a matching slice. A server that started serving several eras would need one
  slice per era, which is why the single-revision narrowing is asserted rather than assumed.
- **The slice covers results, not requests.** Requests are well covered by the SDK's own generated types, and
  the defect class this catches is a *defaulted field on an outgoing result*. A future slice for request
  validation is a separate decision, not an oversight.
