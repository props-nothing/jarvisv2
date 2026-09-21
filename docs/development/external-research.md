# External Integration Research Workflow

External APIs, SDKs, agent runtimes, protocols, models, voice systems, operating-system facilities, and hosted services change faster than this repository. Their implementation starts with current official evidence, not model memory.

## When This Is Mandatory

Follow this workflow before adding or changing:

- provider API calls, models, auth, webhooks, quotas, pricing, or streaming
- Gmail, Microsoft Graph, GitHub, Home Assistant, ElevenLabs, or another connector
- MCP, ACP, OpenAI-compatible, SIP, OAuth/OIDC, or another external protocol
- OpenClaw, OpenAI Agents, LangGraph, Temporal, Tauri, or another runtime/framework
- a third-party Rust/Python/TypeScript SDK or a meaningful version upgrade
- platform service, keychain, notification, audio, accessibility, or installer behavior

An existing research record is navigation help, not permission to skip a live check.

## Source Order

Use the first available authoritative source for each claim:

1. Official versioned specification or raw OpenAPI/AsyncAPI/JSON Schema.
2. Vendor/repository `llms.txt` or `llms-full.txt` index leading to official docs.
3. Official documentation page for the selected version.
4. Official SDK source, generated types, examples, and changelog at the selected tag.
5. Official release notes, migration guide, status/limits/pricing page.
6. Maintainer issue/discussion only for documented ambiguities or confirmed defects.

Community posts, copied examples, search snippets, and AI answers are leads only. Never use a third-party blog to override an official contract silently.

## Discover `llms.txt`

Try the official docs host and the narrowest relevant section. Common locations include:

```text
https://docs.vendor.example/llms.txt
https://vendor.example/docs/llms.txt
https://docs.vendor.example/<section>/llms.txt
https://official-project.example/llms.txt
```

Also search the official repository for `llms.txt`, `llms-full.txt`, an OpenAPI/AsyncAPI file, and generated API references. Some sites support appending `.md` to a page URL.

Prefer a section index and individual Markdown pages over loading a multi-megabyte `llms-full.txt`. Record a missing index as `not found`, then continue with official docs; do not invent one.

## Required Procedure

### 1. Define The Question

Write the exact operations and uncertainties. Example: "Create an ElevenLabs outbound Twilio call, correlate provider IDs, authenticate post-call callbacks, and determine whether retries can duplicate a call."

Do not research an entire vendor when one endpoint is in scope.

### 2. Identify Versioned Authority

Record:

- docs index and exact pages
- specification date/version
- SDK package/repository and selected version/tag
- OpenAPI/AsyncAPI/schema URL and revision where available
- changelog/release notes
- access date in UTC

Fetch the live sources during the task even when a prior record exists. If access is unavailable, do not implement guessed wire behavior. Mark the task blocked with the failed source and a safe next step.

### 3. Extract The Contract

Record only relevant facts:

- endpoints, methods, transports, stream framing, events, and termination
- request/response types and required/optional fields
- auth flow, scopes, secret placement, token refresh/revocation
- webhook signature, timestamp, replay, retry, and ordering behavior
- pagination, concurrency, idempotency, rate limits, quotas, and timeouts
- error classes and provider request IDs
- data retention, residency, privacy, compliance, and pricing constraints
- deprecations, preview status, version compatibility, and known ambiguity

Separate facts from JARVIS decisions and assumptions.

### 4. Inspect Official SDK Source

Use the SDK to clarify serialization, headers, event types, and error handling, not to replace reading the protocol. Record whether the adapter will:

- use the SDK
- use generated code from an official spec
- call HTTP directly

Document why, feature flags, transitive/runtime cost, license, and removal plan. Pin or constrain the selected version.

### 5. Design The Boundary

Map provider concepts into JARVIS contracts. Keep provider types inside the adapter. Define:

- normalized operations
- capability/effect/risk/scope metadata
- secret references and egress
- idempotency and ambiguous-outcome behavior
- rate-limit/retry policy
- diagnostics and redaction
- fallback/degradation behavior
- data classification and retention

### 6. Plan Verification

Before implementation, name tests that can disprove assumptions:

- schema/OpenAPI fixture tests
- recorded sanitized wire fixtures
- independent-client/server conformance
- webhook signature mutation and replay
- pagination and rate limits
- invalid/expired auth and reauth
- provider timeout after possible acceptance
- live smoke test with cost and credential gates

Synthetic fixtures are insufficient when they merely mirror code assumptions.

### 7. Write The Record

Copy [the template](../research/integrations/_template.md) to `docs/research/integrations/<integration>.md`. Fill every required section, including unresolved questions. Link it from the [integration index](../research/integrations/README.md).

Implementation may begin only after the record identifies the controlling contract and a discriminating test.

## Freshness

At the start of every external-integration task:

1. Fetch the official index/page again.
2. Check release notes and deprecations since `last_verified`.
3. Compare the selected SDK/spec version.
4. Update `last_verified` and note either changes or "no relevant change observed."

Preview, model catalog, voice/realtime, pricing, and hosted agent APIs receive no grace period; always treat them as volatile. A pinned protocol still requires a current deprecation/security check.

## Security Rules

- Never put live credentials, full tokens, signed URLs, phone numbers, user data, transcripts, cookies, or webhook secrets in research records or fixtures.
- Use vendor-documented placeholder conventions.
- Do not execute installer scripts or examples from research merely to inspect them.
- Review licenses before copying any code or schema beyond what applicable terms permit.
- Record claims that cannot be verified as unresolved; do not smooth over contradictions.

## Completion Gate

- [ ] Live official index/docs checked during this task.
- [ ] Exact versions and access date recorded.
- [ ] Auth, scopes, secrets, and webhook verification recorded.
- [ ] Limits, idempotency, retries, ordering, privacy, and deprecations recorded.
- [ ] Provider concepts mapped to JARVIS boundaries.
- [ ] Discriminating contract/live tests named.
- [ ] Unresolved questions block only the affected capability.
- [ ] No sensitive data or unlicensed copied implementation is present.