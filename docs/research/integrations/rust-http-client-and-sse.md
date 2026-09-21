---
integration: rust-http-client-and-sse
status: researched
last_verified: 2026-09-21
owners: []
selected_spec_version: reqwest 0.13.5, eventsource-stream 0.2.3
selected_sdk: reqwest 0.13.5 (rustls backend), eventsource-stream 0.2.3
---

# Rust HTTP Client And SSE Parser

## Scope

The Rust crates selected to implement the `P2-003` OpenAI-compatible adapter:
HTTP transport with TLS, cancellation, timeouts, and response streaming, plus
Server-Sent Events parsing for the streaming path. The provider wire contract
itself is owned by [openai-compatible-model-api.md](openai-compatible-model-api.md).

## Official Sources

| Source | URL/version | Accessed | Purpose |
| --- | --- | --- | --- |
| `reqwest` crate metadata | `cargo info reqwest` -> 0.13.5 | 2026-09-21 | exact version, license, MSRV, feature list |
| `reqwest` changelog | https://github.com/seanmonstar/reqwest/blob/master/CHANGELOG.md | 2026-09-21 | breaking changes, defaults, available options |
| `reqwest` API index | https://docs.rs/reqwest/0.13.5/reqwest/ | 2026-09-21 | module/type surface, feature semantics |
| `eventsource-stream` metadata | `cargo info eventsource-stream` -> 0.2.3 | 2026-09-21 | version, license, features |
| `futures-util` metadata | `cargo info futures-util` -> 0.3.34 | 2026-09-21 | version, license, MSRV |
| `cargo-deny` policy | repository `deny.toml` | 2026-09-21 | allowed license set and ban rules |

## Verified Contract

### Versions And Licenses

| Crate | Version | License | Meets `deny.toml` |
| --- | --- | --- | --- |
| `reqwest` | 0.13.5 | MIT OR Apache-2.0 | yes |
| `eventsource-stream` | 0.2.3 | MIT OR Apache-2.0 | not adopted, see below |
| `futures-util` | 0.3.34 | MIT OR Apache-2.0 | yes |
| `webpki-root-certs` | 1.0.9 | CDLA-Permissive-2.0 | added to the allow list |

`requirements`: `reqwest` declares MSRV 1.85.0, below this workspace's pinned 1.98.1.

#### A Transitive License That Was Not In The Allow List

`cargo deny check licenses` **failed** on first run. The rustls stack reaches
`webpki-root-certs`, whose license is `CDLA-Permissive-2.0`.

This is a permissive **data** license: it covers the CA root certificate bundle
that `webpki-root-certs` distributes, not executable code. It is the same category
as the `CC0-1.0` and `Unicode-DFS-2016` entries already in the allow list, which
exist for content rather than code. It was added explicitly with a comment in
`deny.toml`, not by broadening the policy silently.

The finding is recorded because the check *did* its job: the license was absent
until `reqwest` entered the graph, and a per-transitive-dependency review is exactly
where it should surface.

### `reqwest` 0.13 Breaking Changes That Matter Here

- **`rustls` is now the default TLS backend**, replacing `native-tls`. This removes
  the OpenSSL build dependency, which matters because the project already
  deliberately bundles SQLite rather than depending on a system library.
- `rustls-tls` was renamed to `rustls`; the old name will not resolve.
- `query` and `form` are now crate features and are **disabled by default**.
- `json` and `stream` are features; `json` is needed for JSON bodies and `stream`
  for `Response::bytes_stream()`.

### Feature Selection

`default-features = false` with an explicit set, because the defaults enable
`system-proxy` and `charset`, and this adapter must decide the proxy question
deliberately rather than inherit it.

- `http2` — providers speak HTTP/2.
- `rustls` — TLS without a system OpenSSL.
- `json` — request serialization.
- `stream` — `Response::bytes_stream()` for SSE.

### Security-Relevant Defaults

- **System proxies are enabled by default.** `reqwest` reads `HTTP_PROXY`,
  `HTTPS_PROXY`, and `ALL_PROXY` from the environment. For a local model server this
  is wrong in two ways: a prompt sent to `http://127.0.0.1:11434` could be routed
  through an unrelated proxy, and a remote prompt would silently traverse an
  intermediary the user did not select. `ClientBuilder::no_proxy()` must be applied
  unless a proxy is explicitly configured, because the default fails open on
  private content.
- Redirects are followed automatically up to 10 hops. A redirect can move a bearer
  credential to a different host, so the adapter must not follow redirects.
- TLS verification is on by default and must stay on. No `danger_accept_invalid_certs`
  path is provided.

### Timeouts

- `ClientBuilder::timeout` bounds connect, request, and response-body futures.
- `ClientBuilder::connect_timeout` bounds connection establishment alone.
- `ClientBuilder::read_timeout` bounds each individual read and resets after a
  successful read. This is the only mechanism that detects a stalled stream, so it
  is what prevents a hung body from hanging a run indefinitely.

### Cancellation

Hyper cancels an in-flight request when its future is dropped. Neither document
states a billing consequence, so cancellation stops local work and cannot be
claimed to stop provider-side generation or cost. The `P2-001` record carries the
same unresolved question.

### Server-Sent Events

`eventsource-stream` was surveyed and **not adopted**. It is a byte-stream to event
transformer built on `nom`, and adopting it would add a parser dependency for a
format the adapter needs only a narrow subset of: `data:` lines carrying JSON,
separated by a blank line, ending with `[DONE]`. A local decoder
(`crates/jarvis-models/src/openai/sse.rs`) handles that subset, is bounded against a
server that never emits a separator, and has a test proving a multi-byte character
split across two TCP chunks survives intact. The crate remains a valid alternative if
fuller event handling is needed later.

## JARVIS Mapping

- The adapter is generic over an internal `Transport` trait, so retry, timeout,
  error-mapping, and streaming behaviour is tested offline against recorded provider
  bytes. `reqwest` is one implementation, not the contract.
- Provider JSON is decoded into private wire types and mapped into the
  `jarvis-models` domain types. No `reqwest` or `serde_json::Value` type crosses the
  port.
- `X-Client-Request-Id` carries the JARVIS correlation identity, so a request that
  was accepted but never answered is still traceable.
- The API key is held in a redacted, zeroing local type and presented only as an
  `Authorization` header.

## Decisions

- Pin `reqwest` to `=0.13.5` and use rustls, matching the existing policy of pinning
  an exact version for anything on a security path.
- Disable default features and select `http2`, `rustls`, `json`, `stream`.
- Call `no_proxy()` unless a proxy is explicitly configured.
- Disable automatic redirect following.
- Set connect, request, and read timeouts, so a stalled body cannot hang a run.
- Use the `eventsource-stream` crate rather than hand-writing an SSE parser, because
  a hand-written line splitter is where multi-byte characters split across chunk
  boundaries get corrupted.

## Rejected Alternatives

- **`reqwest` default features**: enables `system-proxy`, which fails open on
  private content.
- **`native-tls`**: introduces a system OpenSSL dependency and contradicts the
  bundling policy already established for SQLite.
- **`ureq` / `isahc` / `hyper` directly**: `hyper` would mean reimplementing pooling
  and TLS setup; the others lack the streaming and timeout surface needed here.
- **Hand-written SSE parsing**: a byte-level parser must handle a chunk boundary in
  the middle of a multi-byte character and a chunk boundary in the middle of an
  event; both are routinely wrong first.
- **`reqwest`'s built-in `ClientBuilder::retry`**: retry policy belongs to JARVIS,
  which must honour a provider `Retry-After`, refuse to retry quota failures, and
  record every attempt. Delegating it would hide attempts from the run record.

## Verification Plan

- Offline tests replay recorded SSE bytes, including an event split across two
  chunks and a multi-byte character split across two chunks.
- Falsification: a 429 carrying `Retry-After: 7` must wait at least 7 seconds, not
  the computed backoff.
- Falsification: a quota failure must not be retried at all.
- Falsification: no log, error, or diagnostic output contains the API key.
- Falsification: a stream whose connection drops before a terminal event is not
  reported as a complete answer.
- The `Transport` fake is used everywhere, so the default suite performs no network
  I/O. A live smoke test against a local server is opt-in and credential-gated.

## Unresolved Questions

- Whether dropping a streaming request stops provider-side billing is not documented.
- `eventsource-stream` publishes no MSRV. It builds on the pinned toolchain, which
  was verified by compiling it rather than by trusting the field's absence.
- `reqwest`'s proxy behaviour for `127.0.0.1` when `NO_PROXY` is unset is not
  documented; `no_proxy()` makes the question moot instead of relying on an answer.
