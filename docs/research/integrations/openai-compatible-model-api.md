---
integration: openai-compatible-model-api
status: researched
last_verified: 2026-09-21
owners: []
selected_spec_version: Chat Completions v1 (REST `2020-10-01`)
selected_sdk: none (direct HTTP; no official Rust SDK selected)
---

# OpenAI-Compatible Model API

## Scope

The first model transport for `P2-001`–`P2-003`: an OpenAI-compatible
`POST /v1/chat/completions` request, including streaming, usage accounting, and
normalized errors, against OpenAI itself and against self-hosted
OpenAI-compatible servers (Ollama is used as the interoperability reference).
Embeddings, images, audio, realtime, and the Responses API are out of scope.

## Official Sources

| Source | URL/version | Accessed | Purpose |
| --- | --- | --- | --- |
| OpenAI API docs index | https://developers.openai.com/api/docs/llms.txt (redirected from `platform.openai.com/docs/llms.txt`) | 2026-09-21 | official discovery index |
| API overview and shared behavior | https://developers.openai.com/api/reference/overview.md | 2026-09-21 | auth, headers, request IDs, rate-limit headers, backwards compatibility |
| Deprecations | https://developers.openai.com/api/docs/deprecations.md | 2026-09-21 | which APIs carry shutdown dates |
| Error codes | https://developers.openai.com/api/docs/guides/error-codes.md | 2026-09-21 | error types, codes, status mapping, `Retry-After` |
| Streaming guide | https://developers.openai.com/api/docs/guides/streaming-responses.md | 2026-09-21 | SSE semantics and event typing |
| Text generation guide | https://developers.openai.com/api/docs/guides/text.md | 2026-09-21 | API-surface recommendation, message roles |
| Ollama OpenAI compatibility | https://docs.ollama.com/api/openai-compatibility | 2026-09-21 | self-hosted interoperability surface |
| Ollama docs index | https://docs.ollama.com/llms.txt | 2026-09-21 | discovery for the vendor used as interop reference |

`platform.openai.com/docs/llms.txt` now redirects to
`developers.openai.com/api/docs/llms.txt`; the old host returns HTTP 403 to direct
fetches. Recorded because a stale bookmark looks like a missing index.

## Verified Contract

### Which Surface To Implement

OpenAI recommends the Responses API for new text generation ("If you're building
any text generation app, we recommend using the Responses API over the older Chat
Completions API"), and the Assistants API was shut down on 2026-08-26.

Chat Completions is nevertheless the selected transport, because:

- **No shutdown date exists for it.** The deprecation page lists `v1/prompts`
  (2026-11-30), Evals (2026-11-30), and Agent Builder (2026-11-30), plus model
  retirements — but no Chat Completions endpoint shutdown. It is *legacy*
  ("models and endpoints that no longer receive updates"), not deprecated.
- **It is what "OpenAI-compatible" actually means.** Ollama supports
  `/v1/chat/completions` fully, while its `/v1/responses` support is explicitly
  **non-stateful only** (added in Ollama v0.13.3; no `previous_response_id`, no
  `conversation`, no `truncation`), so a Responses-based adapter would be OpenAI-only
  in practice and would break the local-model path JARVIS needs.
- **JARVIS owns conversation state.** `FR-RUN-001` persists sessions and messages in
  SQLite, so OpenAI-hosted state (`previous_response_id`, conversations) is a
  capability to ignore, not to depend on.

### Request

`POST /v1/chat/completions`, `Authorization: Bearer <key>`, `Content-Type: application/json`.

Fields JARVIS will send, and their confirmed availability:

| Field | OpenAI | Ollama | Notes |
| --- | --- | --- | --- |
| `model` | yes | yes | required |
| `messages[]` | yes | yes | `role` in `system`/`developer`/`user`/`assistant`/`tool` |
| `content` (string or parts) | yes | yes (text + base64 image; image **URL** unsupported) | |
| `stream` | yes | yes | SSE |
| `stream_options.include_usage` | yes | yes | usage arrives as a final chunk |
| `temperature`, `top_p`, `max_tokens` | yes | yes | |
| `stop`, `seed`, `frequency_penalty`, `presence_penalty` | yes | yes | |
| `response_format` | yes | yes (JSON mode) | |
| `tools` | yes | yes | tool **choice** is not interoperable, see below |

Deliberately avoided: `tool_choice`, `logit_bias`, `user`, `n`, `logprobs` — Ollama
marks all of these unsupported, so depending on them would silently narrow the
self-hosted path.

Message roles follow the model spec's chain of command: `developer` (application
instructions) outranks `user`. Ollama's older surface uses `system` for the same
purpose, so the adapter must map a JARVIS "instructions" concept onto the role the
selected server accepts rather than assuming one spelling.

### Streaming

`stream: true` returns server-sent events. Ollama confirms `stream_options.include_usage`
is supported, so usage arrives in a final chunk rather than being unavailable on the
streaming path — this is what makes usage accounting work without a second request.
Per the API overview, **adding new event types is a backwards-compatible change**, so
the decoder must ignore unknown event shapes rather than fail.

### Usage And Cost

A usage object carries prompt/completion/total token counts. Because
`include_usage` is supported by both targets, `FR-RUN-001` usage records can be
populated from the same call that produced the text.

### Rate Limits And Headers

Response headers include `openai-processing-ms`, `openai-version` (currently
`2020-10-01`), `x-request-id`, and `x-ratelimit-*` counters. Request headers must
stay under 64 KiB total. The client may supply `X-Client-Request-Id`: ASCII, at most
512 characters, unique per request, and only for supported endpoints
(chat/completions is listed).

`X-Client-Request-Id` is directly useful to JARVIS: it lets a correlation ID survive
a network failure where no `x-request-id` response header arrives, which is exactly
the `unknown` effect case the security model distinguishes from `failed`.

### Errors

Errors carry `error.type`, `error.code`, and `error.param`. Confirmed classes:
`invalid_request_error` (400), authentication errors (401), unsupported
region (403), `rate_limit_error` (429, including the `slow_down` code for
ramp-rate), quota/spend errors under `insufficient_quota` with specific `code`
values, and `service_unavailable_error` / `server_is_overloaded` (503).

`Retry-After` is present on many 429s and 503s and is authoritative when it appears.
Retrying billing, spend, or quota errors does not restore access — they must map to a
non-retryable class, or a retry loop will spin against a wall.

## JARVIS Mapping

- Provider is a transport and credential holder; the model is the capability. Both
  identifiers stay separate from the agent runtime (`FR-MODEL-001`).
- Provider error classes map onto the existing `jarvis_core::ErrorCode`:
  `invalid_request_error` → `Validation`; 401 → `Authentication`; 403 →
  `Authorization`; 429 and `slow_down` → `RateLimited`; 5xx/overloaded →
  `TransientUpstream`; connection and timeout → `Timeout`/`TransientUpstream`.
- **Billing, spend, and quota errors map to `PermanentUpstream`, not
  `UnavailableCapability`.** This is a correction found while checking the mapping
  against `ErrorCode::is_retryable()`, which returns `true` for
  `UnavailableCapability`. OpenAI documents that "retrying billing, spend, or quota
  errors won't restore API access" and that the limits must be updated first, which
  is exactly `PermanentUpstream`'s definition ("will not succeed without a change").
  Routing quota through `UnavailableCapability` would have made a retry loop spin
  against a wall on the default policy, because the defect is in the *choice of
  code*, not in the retry logic.
- Provider request IDs are recorded as external references only, never as JARVIS
  identifiers.
- The API key is a `SecretRef` resolved at the adapter boundary, never placed in a
  URL or a log line.

## Decisions

- Implement `POST /v1/chat/completions` as the interoperable subset, not the
  Responses API, because the Responses path is not portable across the self-hosted
  servers JARVIS must support.
- Avoid `tool_choice`, `n`, `logit_bias`, and `logprobs` in the shared contract so
  the same request works against OpenAI and Ollama.
- Send `stream_options.include_usage` so streaming runs still yield usage.
- Send `X-Client-Request-Id` carrying the JARVIS correlation ID so an
  accepted-but-unanswered request can still be traced.
- Treat an unknown 2xx stream event shape as ignorable, because new event types are
  documented as backwards compatible.

## Rejected Alternatives

- **Responses API as the first adapter**: recommended by OpenAI for new work, but
  Ollama supports only its non-stateful subset, so it would exclude local models.
- **Assistants API**: shut down 2026-08-26.
- **An official OpenAI Rust SDK**: none is published for Rust; direct HTTP keeps the
  dependency surface small and avoids a vendor type crossing the domain boundary.
- **Depending on `previous_response_id` / conversation objects**: JARVIS owns
  session state in SQLite; delegating it to a provider would violate canonical
  ownership.
- **Retrying 429 without reading `Retry-After`**: documented as wrong; the header is
  authoritative when present.
- **Treating quota errors as retryable**: documented as not restoring access.

## Verification Plan

- Offline contract tests decode sanitized fixtures for: a non-streaming response, a
  multi-chunk stream ending in usage, an unknown event type, a truncated stream, and
  each documented error class.
- Falsification: send a request whose messages contain a canary secret and prove no
  log, error, or diagnostic line contains it.
- Falsification: force a 429 with `Retry-After` and prove the retry waits at least
  that long rather than using the default backoff.
- Falsification: force a billing/quota error and prove no retry is attempted.
- Falsification: interrupt a stream mid-response and prove the run does not claim a
  complete answer.
- Opt-in live smoke against a local Ollama server, and separately against OpenAI,
  both credential-gated and never part of the default suite.

The cheapest test that would disprove the central assumption is replaying a recorded
Ollama streaming fixture: if the adapter cannot decode a real local-server stream, the
"OpenAI-compatible" premise is wrong before any live call is made.

## Unresolved Questions

- No HTTP client has been selected. `reqwest` is the likely choice and brings a TLS
  stack; the license and duplicate-crate policy must be checked before adopting it.
- The `system` versus `developer` role spelling differs across compatible servers; the
  adapter needs a probe or a documented fallback, and that is not yet designed.
- Ollama ignores the API key value but requires the client to send one, so a local
  server needs a placeholder credential rather than a real secret. How JARVIS models
  "no credential required" versus "credential required" is not yet decided.
- Streaming cancellation semantics (whether aborting the HTTP request stops provider
  billing) are not documented in the sources consulted.
