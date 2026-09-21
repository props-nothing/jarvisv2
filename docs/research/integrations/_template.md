---
integration: replace-me
status: planned
last_verified: YYYY-MM-DD
owners: []
selected_spec_version: null
selected_sdk: null
---

# Integration Name

## Scope

State the exact operations being researched and what is explicitly out of scope.

## Official Sources

| Source | URL/version | Accessed | Purpose |
| --- | --- | --- | --- |
| `llms.txt` or docs index | | | discovery |
| API/specification | | | normative contract |
| OpenAPI/AsyncAPI/schema | | | generated shapes |
| official SDK/source | | | implementation details |
| changelog/release notes | | | compatibility/deprecations |
| security/privacy/limits | | | risk and operations |

If an expected source does not exist, write `not found` and how official docs were discovered instead.

## Verified Contract

### Operations And Transport

Endpoints, methods, framing, streaming, events, ordering, termination, pagination, and required fields.

### Authentication And Authorization

Auth flow, scopes, token placement, refresh/revocation, callback/webhook verification, and least-privilege plan.

### Limits And Failure Semantics

Rate/concurrency/size/time limits, idempotency, retries, ambiguous outcomes, provider request IDs, and error classes.

### Data And Compliance

What leaves JARVIS, retention, deletion, residency, telemetry, compliance restrictions, and pricing/cost concerns.

### Versions And Deprecations

Selected versions, preview/stable status, compatibility window, breaking changes, and migration guidance.

## JARVIS Mapping

Provider concepts mapped to JARVIS domain types, tool effects/risk/scopes, secret references, events, audit evidence, and degradation behavior.

## Decisions

List adopted choices with reasons. Distinguish them from external facts.

## Rejected Alternatives

List meaningful alternatives and why they were rejected.

## Verification Plan

- offline schema/fixture tests
- independent conformance tests
- auth/refresh/revoke tests
- signature/replay tests
- pagination/rate-limit/error tests
- idempotency/unknown-outcome tests
- opt-in live smoke test

Name the cheapest test that would disprove the central assumption.

## Unresolved Questions

Each question includes impact and the capability it blocks. Never silently convert an unknown into an assumption.

## Verification Log

| Date | Versions checked | Relevant change or no-change evidence | Researcher |
| --- | --- | --- | --- |
| YYYY-MM-DD | | | |