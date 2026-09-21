---
integration: rust-http-server-and-sse
status: researched
last_verified: 2026-09-21
owners: []
selected_spec_version: axum 0.8.9; hyper 1.11.1 and tower 0.5.3 (already resolved transitively); SSE per WHATWG server-sent-events
selected_sdk: axum 0.8.9 (MIT), http1 + json + tokio + tracing features only
---

# Rust HTTP Server And SSE Response

## Scope

The Rust crates selected to implement the `P2-007` HTTP gateway: routing, request
extraction, bounded request bodies, JSON response encoding, and the Server-Sent Events
response used by `GET /api/v1/runs/{id}/events`.

This is the **server-side** counterpart of
[rust-http-client-and-sse.md](rust-http-client-and-sse.md), which selects `reqwest` as a
client. The two are independent: `reqwest` cannot serve, and `axum` will not replace it.

Out of scope here: the REST path/DTO shapes (`docs/api/contracts.md` owns them), the run
event vocabulary (`jarvis_core` owns it), and the gateway's trust-boundary placement
([ADR-0011](../../adr/0011-run-events-and-http-transport.md) owns it).

## Official Sources

| Source | URL/version | Accessed | Purpose |
| --- | --- | --- | --- |
| `axum` crate metadata | `cargo info axum` -> 0.8.9 | 2026-09-21 | exact version, license, MSRV, feature list |
| `axum` changelog | https://github.com/tokio-rs/axum/blob/main/axum/CHANGELOG.md | 2026-09-21 | release status, breaking changes, SSE notes |
| `axum` README | https://github.com/tokio-rs/axum/blob/main/axum/README.md | 2026-09-21 | safety policy, MSRV, tower relationship, release branch |
| `axum::serve` API | https://docs.rs/axum/0.8.9/axum/fn.serve.html | 2026-09-21 | listener/serve signature, feature gate, return value |
| `axum::response::Sse` API | https://docs.rs/axum/0.8.9/axum/response/struct.Sse.html | 2026-09-21 | `Sse::new`, `keep_alive`, stream bounds |
| `DefaultBodyLimit` API | https://docs.rs/axum/0.8.9/axum/extract/struct.DefaultBodyLimit.html | 2026-09-21 | default 2 MB limit, `max`, difference from tower-http |
| `http-body-util` metadata | `cargo info http-body-util` -> 0.1.5 | 2026-09-21 | version, license, MSRV |
| `tower-http` metadata | `cargo info tower-http` -> 0.6.11 | 2026-09-21 | version, license, MSRV |
| `matchit` metadata | `cargo info matchit` -> 0.9.2 latest | 2026-09-21 | router version axum 0.8 is pinned against |
| `serde_path_to_error` metadata | `cargo info serde_path_to_error` -> 0.1.20 | 2026-09-21 | version, license, MSRV |
| Existing resolved graph | `cargo tree --invert hyper --workspace` / `tower-http` | 2026-09-21 | which HTTP crates this workspace already pins |
| `cargo-deny` policy | repository `deny.toml` | 2026-09-21 | allowed license set and ban rules |

`axum` publishes no `llms.txt`; it is a library, so the crate metadata, changelog, README,
and rustdoc API pages are the authoritative sources, matching the approach recorded for
Phase 1 core dependencies.

## Verified Contract

### Versions And Licenses

| Crate | Version | License | Meets `deny.toml` | Notes |
| --- | --- | --- | --- | --- |
| `axum` | 0.8.9 | MIT | yes | MSRV 1.80; workspace toolchain is 1.98.1 |
| `axum-core` | 0.5.6 (pulled) | MIT | yes | `FromRequest`/`IntoResponse` live here |
| `matchit` | 0.8.x (pulled) | MIT | yes | axum 0.8 pins 0.8; latest standalone is 0.9.2 |
| `serde_path_to_error` | 0.1.20 (pulled) | MIT OR Apache-2.0 | yes | only used by `Json`/`Form` rejections |
| `hyper` | 1.11.1 | MIT | yes | **already resolved** via `reqwest` |
| `http` | 1.5.0 | MIT OR Apache-2.0 | yes | **already resolved** |
| `http-body` / `http-body-util` | 1.1.0 / 0.1.5 | MIT | yes | already resolved / small new |
| `tower` / `tower-http` | 0.5.3 / 0.6.11 | MIT | yes | **already resolved** via `reqwest` |

Verified with `cargo tree --invert hyper --workspace` and `--invert tower-http`: both
trees terminate at `reqwest 0.13.5` under `jarvis-models`, which means the HTTP core
this gateway builds on is **already in the lock file** at versions axum 0.8.9 accepts.
Adding axum therefore introduces roughly three new packages (`axum`, `axum-core`,
`matchit`, plus `serde_path_to_error`) rather than a parallel HTTP stack.

### Release Status Is The First Adopter Hazard

The `main` branch README states plainly that it "contains breaking changes" on the way to
axum 0.9 and that the released code is on the `0.8.x` branch. Two consequences:

- **Pin `0.8.9` exactly**, consistent with how this workspace already pins `reqwest`,
  `sqlx`, and `libsqlite3-sys`. Do not use a floating `"0.8"` requirement, and do not
  follow `main` documentation when the released crate differs.
- The `Unreleased` changelog section already reworks `Router` fallback merging, changes
  `#[from_request(via(...))]` rejection types, makes `axum::serve` apply hyper's default
  `header_read_timeout`, and changes the `serve` future's output type. Any of those would
  break this gateway on upgrade, so an upgrade is a deliberate task with this record
  re-verified, not a `cargo update`.

### Routing And Extraction

- Routing is macro-free: `Router::new().route(path, get(handler))`, with typed `State`
  injection replacing the older `Extension` pattern (`State` is compile-time checked,
  `Extension` fails at runtime).
- Path parameter syntax changed in 0.8 to `/{single}` and `/{*many}`; the older
  `/:single` / `/*many` forms **panic** rather than silently changing behaviour. This
  matters because most examples in circulation show the pre-0.8 syntax.
- Extractors are split: `FromRequestParts` for anything that does not consume the body,
  `FromRequest` for exactly one body-consuming extractor per handler. Two body consumers
  is a compile error, not a runtime one.
- `Json` rejects trailing characters after the JSON document, so a request body of
  `{"a":1}garbage` is refused rather than partially parsed.

### Request Body Bounds

`DefaultBodyLimit` defaults to **2 MB**, applied to `Bytes` and every extractor built on
it (`String`, `Json`, `Form`). This is a security default, not a convenience: before
0.5.16 `Bytes::from_request` consumed the whole body with no length check.

Two properties decide how this workspace must use it:

- `DefaultBodyLimit` is **local**: it applies only to extractors that call it. An extractor
  reading the body directly through `Body::poll_frame` is *not* bounded by it.
- `tower_http::limit::RequestBodyLimit` is **global** and applies regardless of which
  extractor reads the body.

Because `docs/architecture/security.md` requires bounded payload sizes as a control rather
than a convention, this gateway will set `DefaultBodyLimit::max(..)` to the documented
frame ceiling **and** keep the application-level objective length check. The default 2 MB is
larger than any run objective the schema accepts (4096 characters), so leaving it implicit
would allow a request the domain must then reject after buffering it.

### SSE Response

`Sse::new(stream)` where `stream: TryStream<Ok = Event, Error: Into<BoxError>> + Send +
'static`. `Sse::keep_alive` is available **only with the `tokio` feature** and defaults to
**no keep-alive messages** — so a gateway must configure it explicitly.

Notes that matter for the run stream:

- The `sse` module and `Sse` type stopped depending on the `tokio` feature in 0.8.5, but
  `keep_alive` still does. Selecting `tokio` for `serve` therefore also supplies it.
- Arbitrary binary data can be written into an event (0.8.5).
- `Event::json_data` skips SSE-incompatible characters in `serde_json::RawValue`, and
  `sse::Event` panics if a setter is called twice (0.5.0), so an event is built once.
- 0.7.0 added a space between field and value for compatibility; field values containing
  carriage returns are disallowed, which matches the spec.

`docs/architecture/protocols.md` requires SSE event IDs, sequence numbers, protocol
version, and heartbeat-as-protocol-event rather than fake content. `Sse::keep_alive`
emits its own comment-style keep-alive, which is **not** a protocol event; so the gateway
must send its own heartbeat events and treat `keep_alive` as transport-level only.

### Serving And Shutdown

`axum::serve(listener, make_service)` is available with `tokio` plus `http1` or `http2`. Its
documented return value is important and easy to misread:

> Although this future resolves to `io::Result<()>`, it will never actually complete or
> return an error. Errors on the TCP socket will be handled by sleeping for a short while
> (currently, one second).

So `axum::serve` is not a supervision point. A listener error is swallowed and retried by
the library, which is the opposite of this daemon's behaviour elsewhere (a failed local
listener returns and ends the accept loop). The gateway must therefore pair `serve` with
`.with_graceful_shutdown(..)` and treat "the daemon is shutting down" as the only planned
exit, and must not rely on `serve` returning to detect a bind or accept failure.

`axum::serve` is documented as intentionally simple with no configuration ("use hyper or
hyper-util if you need configuration"). For a loopback-only local API that is sufficient;
it is recorded as a deliberate limit rather than an oversight.

### Auth And Trust Boundary

`axum` provides **no authentication**. It supplies extraction and routing only. Every
control in `docs/architecture/security.md` — client identity, workspace binding, scope,
rate limits — must be implemented by this workspace. No tower auth middleware is adopted
for this slice; a first-party extractor that validates the existing profile-bound
credential against the daemon's `CredentialStore` keeps one authentication implementation
rather than two.

## JARVIS Mapping

- Axum is a **gateway adapter**, not a domain concern. `docs/architecture/overview.md`
  lists Axum explicitly among what the domain core must not import, and
  `repository-layout.md` says `jarvis-core` takes "No Axum, SQLx, Tauri, MCP SDK, or
  vendor SDK". The dependency therefore belongs to `apps/jarvisd` (the composition root
  whose "handlers translate protocols into application commands and queries") plus
  `jarvis-protocol` for the versioned DTOs. `jarvis-application`, `jarvis-core`, and
  `jarvis-storage` stay framework-free.
- `axum::Error` / extractor rejection types must not cross into the domain. Rejections are
  mapped to the existing `WireError` envelope (stable `ErrorCode`, bounded `SafeMessage`,
  retryability, correlation ID) exactly as protocol v1 does, so a caller sees one error
  shape across transports.
- The HTTP surface is a **second transport over the same commands** as protocol v1, not a
  second control plane (ADR-0002). It reuses the same credential, the same run repository,
  and the same transition table.
- `DefaultBodyLimit` plus the existing 4096-character objective bound map the
  `docs/architecture/security.md` "payload sizes" and "output bounds" controls to this
  transport.
- SSE sequence/ID fields come from the durable run-event record (`P2-007b`), not from an
  in-memory counter, so reconnect-and-replay is answerable after a daemon restart.
- Provider SDK types do not appear: axum is transport only, and no model provider type is
  involved at this layer.

## Decisions

1. Select **axum 0.8.9**, pinned exactly, as the HTTP routing and SSE library.
2. Depend on it from `apps/jarvisd` only; `jarvis-protocol` owns DTOs and gains no server
   dependency beyond types it already uses.
3. Enable an explicit feature set — `http1`, `json`, `tokio`, `tracing` — with
   `default-features = false`, because the defaults also enable `form`, `query`,
   `matched-path`, `original-uri`, and `tower-log`, none of which this API needs. This is
   the same discipline applied to `reqwest` and `tracing-subscriber` in this workspace,
   where a default feature caused an invisible runtime dependency.
4. Set an explicit `DefaultBodyLimit` rather than accepting the 2 MB default.
5. Configure SSE `keep_alive` explicitly, and treat it as transport-level only; send
   heartbeat protocol events separately.
6. Implement authentication in this workspace as a first-party extractor over the existing
   credential store. No third-party auth middleware.
7. Use HTTP/1.1 only. HTTP/2 is not required for a loopback JSON/SSE API, and enabling it
   adds `h2` to this crate's direct feature surface for no measured benefit. (Both `h2`
   and `hyper`'s HTTP/2 support remain in the tree transitively through `reqwest`.)
8. Bind loopback only, per ADR-0002 ("Network endpoints bind to loopback by default").

## Rejected Alternatives

- **`hyper` directly:** would mean reimplementing routing, extraction, and SSE framing for
  no benefit. Already rejected for the client side in
  `rust-http-client-and-sse.md` for the analogous reason; the same reasoning applies here
  with a stronger bias, since routing and SSE are more work than connection pooling.
- **`axum` at `"0.8"` floating, or tracking `main`:** `main` is mid-0.9 and documented as
  breaking. Rejected; pin the released version, consistent with the rest of the manifest.
- **Default features:** rejected because five of the nine defaults are unused and one of
  them (`tower-log`) adds logging behaviour this workspace configures itself.
- **`actix-web` / `warp` / `poem`:** rejected without evaluation at this depth — axum is
  built on `hyper`/`tower`/`tower-http`, which this workspace **already resolves** through
  `reqwest`. Choosing any other framework would add a parallel HTTP stack for a loopback
  API, and the reuse of an existing resolved graph is the deciding factor.
- **`tower_http::limit::RequestBodyLimit` as well as `DefaultBodyLimit`:** not adopted for
  this slice. It is global, which is stronger, but every body-reading route here uses
  `Json`, so `DefaultBodyLimit` covers the actual surface; adding both would mean two
  limits to keep consistent. Revisit if a route ever reads `Body::poll_frame` directly.
- **Third-party auth/tower middleware:** rejected to keep exactly one credential
  verification path shared with protocol v1.
- **WebSocket for the event stream:** rejected. `protocols.md` reserves WebSocket for
  genuinely bidirectional low-latency sessions and requires SSE for one-way run/activity
  streams.

## Verification Plan

Offline, no network, no credentials:

- A route returns the expected status and JSON for a valid request.
- A malformed identifier and an unknown run produce distinct stable `ErrorCode`s mapped
  into the `WireError` envelope, never a panic or a bare 500.
- A request body above the configured limit is refused **before** the domain sees it, and
  a body at exactly the limit is accepted (the boundary asserted from both sides).
- An unauthenticated, a wrong-credential, and a missing-credential request each fail
  closed, and the failure is not distinguishable in a way that leaks whether a run exists.
- The SSE stream emits an ordered sequence with stable event IDs, tolerates a client
  disconnect without panicking the server, and replays from a supplied last-event ID.
- A sequence gap is reported as an explicit resync requirement rather than skipped.
- Cancellation requested over HTTP settles the run exactly once, observable in the
  transition version.

Cheapest discriminating test: **a refused request body at the configured limit**, driven
through the real router. If a size control can be bypassed by an extractor that reads the
body differently, the security posture is decorative.

## Unresolved Questions

- Whether `Sse::keep_alive` and a protocol heartbeat can coexist without double-sending to
  a client that treats both as liveness. Blocked on implementing the stream, then
  measured.
- Whether axum 0.9's changed `serve` future output and `Router` fallback merging require a
  migration. Blocks any upgrade; not this slice.
- Whether the loopback HTTP fallback should be reachable when native IPC is available.
  `protocols.md` calls the authenticated loopback HTTP fallback "required only if a
  platform/library constraint makes native IPC unavailable", so enabling it unconditionally
  would be a scope decision, not an implementation detail. Blocks enabling it by default.
- Whether `axum::serve`'s socket-error-with-retry behaviour is acceptable for a daemon that
  must report listener death. It contradicts the local listener's fail-fast behaviour;
  needs a decision on whether to pre-bind and validate the listener before serving.

## Verification Log

| Date | Versions checked | Relevant change or no-change evidence | Researcher |
| --- | --- | --- | --- |
| 2026-09-21 | `axum` 0.8.9, `http-body-util` 0.1.5, `tower-http` 0.6.11, `matchit` 0.9.2 (latest; axum 0.8 pins 0.8.x), `serde_path_to_error` 0.1.20; `cargo tree --invert hyper` and `--invert tower-http` | Initial record. axum 0.8.9 is MIT, MSRV 1.80, `#![forbid(unsafe_code)]`. `hyper` 1.11.1, `http` 1.5.0, `tower` 0.5.3, and `tower-http` 0.6.11 are **already resolved** through `reqwest 0.13.5`, so axum reuses the existing HTTP core. `main` is mid-0.9 and documented as breaking, so 0.8.9 must be pinned. `DefaultBodyLimit` defaults to 2 MB and is local, not global. `Sse::keep_alive` requires the `tokio` feature and defaults to off. `axum::serve` never returns an error and retries socket errors itself. No code written | GitHub Copilot |
