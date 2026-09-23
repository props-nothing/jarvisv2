---
integration: model-context-protocol
status: researched
last_verified: 2026-09-22
owners: []
selected_spec_version: "2026-07-28"
selected_sdk: "rmcp 3.4.0 (Apache-2.0, Tier 1)"
---

# Model Context Protocol

## Scope

Selecting the protocol revision and SDK version JARVIS will implement, and recording the contract
facts that decide the adapter boundary. In scope: version negotiation, both standard transports
(stdio, Streamable HTTP), authorization shape, tool-schema translation, and the limits a tool call
must respect.

Out of scope: implementing the adapter (`P3-008`), the server-facing exposure and per-client
allowlists (`P3-009`), and the Inspector conformance suite (`P3-010`). Optional extensions (tasks,
apps, skills) are **not** selected here.

## Official Sources

| Source | URL/version | Accessed | Purpose |
| --- | --- | --- | --- |
| `llms.txt` index | https://modelcontextprotocol.io/llms.txt | 2026-09-22 | discovery; enumerates every spec revision + SEPs |
| Current specification | https://modelcontextprotocol.io/specification/2026-07-28/index.md | 2026-09-22 | normative contract |
| Key changes | https://modelcontextprotocol.io/specification/2026-07-28/changelog.md | 2026-09-22 | what changed vs 2025-11-25 |
| Versioning & compatibility | https://modelcontextprotocol.io/specification/2026-07-28/basic/versioning.md | 2026-09-22 | negotiation, modern/legacy/dual-era, compat matrix |
| Streamable HTTP | https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/streamable-http.md | 2026-09-22 | headers, validation, sessions, resumability, cancellation |
| SDK list + tiers | https://modelcontextprotocol.io/docs/2026-07-28/sdk.md | 2026-09-22 | official SDK selection |
| SDK tiering rules | https://modelcontextprotocol.io/community/sdk-tiers.md | 2026-09-22 | what Tier 1 commits a vendor to |
| Rust SDK crate metadata | https://crates.io/api/v1/crates/rmcp | 2026-09-22 | exact version, license, MSRV, feature flags |
| Rust SDK API docs | https://docs.rs/rmcp/latest/rmcp/ | 2026-09-22 | transports, features, lifecycle modes |
| Rust SDK source | https://github.com/modelcontextprotocol/rust-sdk (`main`) | 2026-09-22 | negotiation behaviour, version constants |
| Deprecated features | https://modelcontextprotocol.io/specification/2026-07-28/deprecated.md | 2026-09-22 | features new implementations must not adopt |
| Separate docs `llms.txt` | `not found` — the single `llms.txt` above covers both docs and specification | 2026-09-22 | — |

## Verified Contract

### The 2026-07-28 revision is a stateless rewrite — this is the decisive fact

Revision `2026-07-28` **removes the protocol-level session and the `initialize` handshake entirely**
(SEP-2575, SEP-2567). Every request is self-describing:

- The protocol version, client capabilities, and client identity travel **per request** in the
  `_meta` object, under `io.modelcontextprotocol/protocolVersion`,
  `io.modelcontextprotocol/clientCapabilities`, and `io.modelcontextprotocol/clientInfo`. Servers
  identify themselves in each result's `_meta` as `io.modelcontextprotocol/serverInfo`.
- **`server/discover` is mandatory for servers.** It advertises supported protocol versions,
  capabilities, and identity. Clients MAY call it up front, or may simply invoke any RPC and handle
  `UnsupportedProtocolVersionError`.
- Version mismatch returns an `UnsupportedProtocolVersionError` listing the versions the server *does*
  support, and the client retries with a mutually supported one. There is no handshake to negotiate in.
- The specification names three eras: **Modern** (`2026-07-28`+), **Legacy** (`2025-11-25` and
  earlier, handshake-based), and **Dual-era** (supports both). The compatibility matrix states plainly
  that a modern client against a legacy server **fails**, and that a legacy client against a modern
  server **fails** with no fall-forward mechanism.

Other core changes that affect an adapter:

| Change | Was | Now |
| --- | --- | --- |
| Session | `Mcp-Session-Id` header | **removed**; cross-call state uses server-minted handles passed as ordinary tool arguments |
| `initialize` | handshake | **removed** |
| List stability | per-connection | `tools/list`, `resources/list`, `prompts/list` no longer vary per connection |
| Server-initiated requests | JSON-RPC requests on the SSE stream | **MRTR**: server returns `InputRequiredResult` (`resultType: "input_required"`) carrying `inputRequests`; the client retries the original request with `inputResponses` |
| Result discriminator | absent | `resultType` is **required**: `"complete"` or `"input_required"`. Clients **MUST** treat an absent field from an older server as `"complete"` |
| Change notifications | `HTTP GET` stream + `resources/subscribe` | `subscriptions/listen` (one long-lived POST-response SSE stream, opted in per notification type, tagged `io.modelcontextprotocol/subscriptionId`) |
| `resources/subscribe` / `resources/unsubscribe` | present | **removed** |
| `ping`, `logging/setLevel`, `notifications/roots/list_changed` | present | **removed**; log level is per-request via `io.modelcontextprotocol/logLevel`, and a server **MUST NOT** emit `notifications/message` for a request that omitted it |
| SSE resumability | `Last-Event-ID` + event ids | **removed**. A broken response stream loses the in-flight request; the client **MUST** re-issue it as a **new** request with a **new** id |
| Tasks | experimental core | moved to an extension, `io.modelcontextprotocol/tasks` (polling `tasks/get`, `tasks/update`; `tasks/list` and blocking `tasks/result` removed) |
| Resource-not-found error | `-32002` | `-32602` (Invalid Params), to align with JSON-RPC |
| Error-code allocation | ad hoc | `-32000`..`-32019` implementation-defined (existing SDK use grandfathered), `-32020`..`-32099` reserved for the specification |

### Version negotiation, precisely

- The spec text is explicit that **an `initialize` request selects legacy semantics whatever version
  it names**, because the handshake does not exist in `2026-07-28`. The SDK's source repeats this in
  `is_legacy_request`, with a comment saying the version in `initialize.params` "never routes it to the
  stateless path".
- `is_legacy_version(v)` is a **string comparison**: anything `< "2026-07-28"` is legacy. So the
  boundary is lexical, not a list membership test.
- A server answering `initialize` **must not agree to a version that dropped the handshake**. If a
  server supports only modern versions, it returns `UnsupportedProtocolVersionError` for `initialize`
  rather than echoing a modern version back.

### Streamable HTTP: request contract

- **One endpoint, POST only.** Each JSON-RPC message is its own POST. GET and DELETE to the endpoint
  are `405 Method Not Allowed` in this revision.
- **Required headers on every POST**: `MCP-Protocol-Version` (must equal the body's
  `_meta.protocolVersion`), `Mcp-Method`, and `Mcp-Name` for `tools/call`, `resources/read`,
  `prompts/get`.
- **Header values must match the body**, and a mismatch is `400 Bad Request` + JSON-RPC error
  `-32020 HeaderMismatch`. The specification's stated reason is a *security* one: a load balancer
  routing on the header while the server executes the body value would let the two disagree. Servers
  that process the body **MUST** validate this.
- **`Origin` validation is a MUST** — if present and invalid, reply `403`. Servers **SHOULD** bind
  `127.0.0.1` only when local. The stated threat is DNS rebinding against a local server from a
  remote web page.
- A JSON-RPC **notification** POST is `202 Accepted` with no body.
- A JSON-RPC **request** POST returns either `application/json` or `text/event-stream`. Clients must
  accept both. Notifications on that stream must relate to the originating request; the final response
  SHOULD terminate the stream.
- Servers SHOULD send `X-Accel-Buffering: no` when opening an SSE stream, and are encouraged to emit
  SSE comment keep-alives (`:` lines) on long-lived streams. Clients must ignore comment lines.
- **Closing the SSE response stream IS the cancellation signal.** On HTTP, `notifications/cancelled` is
  not used at all — it remains a stdio-only mechanism.

### `x-mcp-header`: mirroring tool parameters into headers

Servers MAY annotate a tool parameter with `x-mcp-header` to mirror it into an `Mcp-Param-{Name}`
header. Clients **MUST** support it (rejecting the tool if the annotation is invalid), and this is the
part of the revision with the most sharp edges:

- The annotated value MUST be a **primitive**: `integer`, `string`, or `boolean`. `number` is
  **explicitly not permitted**, and integer values must sit inside the JavaScript safe range
  (±2^53−1).
- The property must be **statically reachable from the schema root through `properties` keys only** —
  the chain MUST NOT pass through `items` or any array keyword, `oneOf`/`anyOf`/`allOf`/`not`,
  `if`/`then`/`else`, or `$ref`. Nested object properties are fine as long as every step is a
  `properties` key. An annotation anywhere else makes **the tool definition invalid**.
- Names must be unique case-insensitively and must satisfy HTTP token syntax.
- A client that finds an invalid annotation **MUST exclude that tool from `tools/list`** — not fail the
  list. It SHOULD log the tool name and reason.
- Values that cannot be a plain ASCII header value (non-ASCII, control characters, leading/trailing
  whitespace) MUST use the Base64 sentinel `=?base64?{value}?=`, and a value that merely *looks* like
  the sentinel must also be encoded. The same rule covers `Mcp-Name`.

### Authorization

- Remote MCP authorization remains OAuth-oriented, and the spec now requires clients to key persisted
  credentials **by issuer**, never reusing them with a different authorization server, and to
  re-register when the authorization server changes.
- Authorization servers SHOULD include `iss` (RFC 9207) and clients **MUST** validate it against the
  recorded issuer before redeeming a code.
- Clients must send an appropriate `application_type` during Dynamic Client Registration.
- **OAuth 2.0 Dynamic Client Registration (RFC 7591) is deprecated** as a registration mechanism in
  favour of Client ID Metadata Documents. It still works for compatibility, so an adapter must not
  assume either mechanism.
- Stdio carries no transport authorization at all. Its only control is launch isolation, which is why
  the environment must be curated rather than inherited.

### Data And Compliance

Nothing in the protocol requires telemetry. The relevant data concerns are JARVIS-side: whatever a tool
returns enters the context pipeline and must carry a trust label and sensitivity ceiling, and the
`_meta` trace-context keys (`traceparent`, `tracestate`, `baggage`) are optional OpenTelemetry
conventions a server may propagate.

### Versions And Deprecations

**Deprecated — a new implementation must NOT adopt these:**

- **Roots, Sampling, and Logging** (SEP-2577). The spec's own suggested migrations are: pass
  directories via tool parameters, resource URIs, or server configuration instead of Roots; call LLM
  provider APIs directly instead of Sampling; log to `stderr` (stdio) or use OpenTelemetry instead of
  Logging. The minimum deprecation window is **twelve months**, governed by a new feature-lifecycle
  policy.
- The **HTTP+SSE transport** from `2024-11-05` (deprecated since `2025-03-26`) — migrate to Streamable
  HTTP.
- `includeContext` values `"thisServer"` / `"allServers"`; omit the field or use `"none"`.
- **OAuth 2.0 DCR** (RFC 7591), per above.

**SDK selection.** The Rust SDK `rmcp` is now **Tier 1** — 100% conformance-test pass rate, new
features delivered before a spec release, issue triage within two business days, a stable release, and
published documentation/roadmap. That is a materially strong commitment: a Tier 1 SDK is relegated to
Tier 2 if *any* conformance test fails continuously for four weeks.

Selected: **`rmcp` 3.4.0**, published `2026-09-15T15:44:08Z`.

- License **Apache-2.0**. `Apache-2.0` is already in `deny.toml`'s allow list, so no licence entry
  changes. (The crate's declared licence has varied across its history — the same crates.io response
  shows `MIT/Apache-2.0` at 0.6.0 and `MIT` at 0.8.5 through 0.12.0 — so the *current* value is what
  was verified here, and a version bump must re-check it rather than assume a stable identifier.)
- `rust_version = 1.88`, `edition = "2024"`. This workspace is on `1.98.1` and edition 2024, so the
  MSRV is satisfied with margin.
- No `bin_names`, so enabling it adds a library only — no binary that a release has to account for.
- Feature flags relevant to this work: `client`, `server`, `macros`, `schemars`, `auth`,
  `elicitation`, `transport-io`, `transport-child-process`, `transport-streamable-http-client`,
  `transport-streamable-http-client-reqwest`, `transport-streamable-http-server`. The crate declares
  `default = ["base64", "macros", "server"]`.
- **Transports are a pluggable `Transport` trait**, with two built-in pairs: stdio
  (`TokioChildProcess` client / `transport-io` server) and Streamable HTTP
  (`StreamableHttpClientTransport` / `StreamableHttpService`).
- **Client lifecycle is explicit**: `serve()` is legacy `initialize`; `ClientLifecycleMode::Discover`
  is the modern stateless startup; `ClientLifecycleMode::Auto` probes `server/discover` and falls back
  to legacy, treating only a **correlated, non-modern JSON-RPC error** as evidence of a legacy peer —
  a transport error becomes `Err` rather than a silent fallback. The auto-discovery timeout is
  **10 seconds**.
- TLS backends are opt-in and separate (`reqwest` = rustls; `reqwest-native-tls`;
  `reqwest-tls-no-provider`). Our existing policy is rustls with no system OpenSSL, so the `reqwest`
  variant is the one that matches.

## JARVIS Mapping

| MCP concept | JARVIS side |
| --- | --- |
| `tools/list` entry | **not** a `ToolDefinition`. It is adapter input. A canonical `ToolDefinition` is constructed with a JARVIS namespace (`mcp.<server>.<tool>`), an effect set, a risk level, and scopes that the server never supplies. |
| `tools/call` | One `ToolExecutionRequest` through the existing pipeline: schema validation → policy → receipt → admit → execute → recorded outcome. |
| `inputSchema` (2020-12) | `ToolSchema`, which is already 2020-12-only and offline. The revision's loosening to *any* 2020-12 keyword, plus its new `$ref`-resolution rules, means an incoming schema may reference documents; `jarvis-tools` refuses a remote `$ref`, so a server schema that needs one must be refused or inlined, not fetched. |
| `outputSchema` / `structuredContent` | Bounded output only. The revision allows any JSON value in `structuredContent`, which is a size hazard: it is truncated and bounded exactly like text. |
| `x-mcp-header` | Adapter concern, inside the transport. It must never leak into `ToolDefinition`: the annotation is a wire hint, not a JARVIS capability. |
| Server-provided tool name | **Untrusted input.** It becomes part of a canonical identifier, so it must pass the same `<namespace>.<name>` charset rules as any other tool id and must not be able to smuggle a dot that re-parses as a different tool. |
| `isError` / tool error | Maps onto the existing outcome vocabulary. A tool-level error is a `Failed` outcome; whether anything *reached* the server decides `Failed` vs `Unknown`, which is the same distinction `AdapterError::RefusedBeforeReaching` vs `AmbiguousAfterReaching` already draws. |
| `notifications/progress` | Progress evidence, bounded, and never a substitute for an outcome. |
| Cancellation (closing the SSE stream) | The run's cancellation must be able to close the stream. This is the hook `P3-011` needs. |
| `subscriptions/listen` | A long-lived server→client stream; it is **not** the run event log and must not be confused with `run_events`. |
| MRTR `inputRequests` | An approval-shaped interaction. It must route through the JARVIS approval path, never be answered by the adapter itself. |
| Server identity / negotiated version | Adapter state, auditable, and never a source of authority. |

`policy_version`, approvals, idempotency, and the audit record stay JARVIS-owned. MCP authorization is
**not** JARVIS authorization, and a server being reachable is not a reason for a call to be permitted.

## Decisions

1. **Target protocol `2026-07-28`, implemented as a modern client and a dual-era server.**
   Rationale: the client only ever talks to servers we choose, so it can be modern-only and simpler.
   The server is exposed to clients we do not control, so refusing legacy clients outright would be an
   unforced compatibility loss — and the compatibility matrix confirms legacy clients have no
   fall-forward path, so a dual-era server is the only shape that serves both.
2. **Select `rmcp` 3.4.0**, pinned, with the SDK hidden behind `jarvis-tools` adapters. Tier 1 plus
   conformance testing is a concrete commitment rather than an aspiration, and the alternative is
   hand-rolling JSON-RPC framing and negotiation for a protocol that is still changing.
3. **Do not adopt Roots, Sampling, or Logging.** They are deprecated with a twelve-month removal
   window, and each has a spec-suggested migration that is strictly better for us: Roots become tool
   parameters or configuration (which is how workspace grants *should* be expressed anyway),
   Sampling is replaced by calling the provider directly (which is the architecture we already have),
   and Logging by `stderr`/OpenTelemetry (which is also already ours).
4. **Modern client startup uses the `Discover` lifecycle**, not `serve()`. `serve()` is the legacy
   `initialize` handshake and would land a modern server in a legacy session.
5. **`Auto` is the fallback for a server whose era is unknown.** It probes `server/discover`, treats a
   correlated non-modern JSON-RPC error as "legacy" and falls back, and — importantly — treats a
   transport error or an uncorrelated response as a hard error rather than silently retrying legacy.
6. **Treat the SDK's `ProtocolVersion::LATEST` as untrustworthy for our purposes.** It is
   `V_2025_11_25`, not `V_2026_07_28`. Every server must narrow/declare its supported versions
   explicitly and every client must name `V_2026_07_28` explicitly; relying on `LATEST` would
   silently negotiate the legacy handshake.
7. **Validate `Origin` and bind loopback for any locally hosted MCP server**, and validate
   header↔body agreement. Both are MUSTs in the spec and both are cheap; the failure mode each
   prevents is a security one.
8. **Never fetch anything a server schema references.** `jarvis-tools` already refuses a remote `$ref`;
   this revision makes schemas more expressive, so the refusal becomes *more* load-bearing, not less.
9. **Ignore `Mcp-Session-Id`, `Last-Event-ID`, and GET on the endpoint** when acting as a modern
   server, and **re-issue a lost request under a new id** instead of trying to resume. There is no
   resume mechanism to implement.
10. **Treat an absent `resultType` as `"complete"`**, as the spec requires, so a legacy peer is not
    misread as malformed.

## Rejected Alternatives

- **Implement `2026-07-28` as hand-written JSON-RPC over HTTP** rather than via the SDK. Rejected: the
  header/body validation rules, the Base64 sentinel, the `x-mcp-header` reachability analysis, and the
  version gating are all fiddly protocol detail with conformance tests available; reimplementing them is
  the highest-risk, lowest-value code in the integration.
- **Serve modern-only.** Rejected on the compatibility matrix: a legacy client has no fall-forward
  mechanism, so this silently breaks every older client for no gain.
- **Client as dual-era.** Rejected: an unnecessary second code path. We choose the servers our daemon
  talks to, so `Discover` with `Auto` as a safety net covers every server we would actually configure.
- **`ProtocolVersion::LATEST` as the target.** Rejected: it is `2025-11-25`, so this would implement the
  deprecated handshake while believing it implemented the current revision.
- **Adopt the Tasks extension now.** Rejected: it moved out of core precisely because it is unsettled
  (`tasks/list` and blocking `tasks/result` were removed in the same revision). Long-running work is a
  JARVIS run, not an MCP task.
- **Adopt Sampling** so a server can use our model. Rejected: deprecated, and it would let a remote
  server spend our model budget through a path outside the run/audit model.
- **Use Roots to tell a server where our files are.** Rejected: deprecated, and it inverts the
  workspace grant — the server would be told a directory rather than confined to one. The ADR-0020
  handle is the boundary.
- **Treat an MCP tool listing as a `ToolDefinition`.** Rejected: it would let a third party choose its
  own risk level and effects.
- **`rmcp` 3.3.0 or 3.2.0** to avoid the newest release. Rejected: no reason to prefer an older
  release of the same minor line; the version and licence are the same shape and the newest has the
  most conformance fixes.

## Verification Plan

Ordered cheapest-first. A test only counts if it can fail.

1. **Offline schema/fixture tests (the cheapest discriminator).** Capture real `server/discover`,
   `tools/list`, and `tools/call` frames from a reference server, sanitise them, and assert the
   translation into canonical `ToolDefinition` values. **This is the test that would disprove the
   central assumption** — that a server-supplied tool can be mapped into a scoped canonical definition
   without the server influencing its risk or effects.
2. **Negotiation tests.** A modern peer, a legacy peer, and a dual-era peer; assert the modern client
   never lands in a legacy session, that an unknown version produces the retry-then-succeed path, and
   that `initialize` never agrees to a version that dropped the handshake.
3. **`x-mcp-header` rejection tests.** A schema with the annotation inside `oneOf`, under `items`,
   on a `number`, and with a duplicate name case-insensitively must each cause **that tool** to be
   excluded from `tools/list` while the rest of the list survives.
4. **Header/body mismatch tests.** Omit `Mcp-Method`; send a `Mcp-Name` that disagrees with the body;
   send a Base64-sentinel value and a plain value matching the sentinel pattern. Assert `400` + `-32020`.
5. **`Origin` tests.** Valid, absent, and hostile `Origin` values; assert `403` for the hostile one and
   that a local server bound to loopback is unreachable off-host.
6. **Cancellation tests.** Close the SSE stream mid-call and assert the work stops and the run records
   a truthful terminal state (not a success).
7. **Lost-stream tests.** Break a response stream and assert the request is **re-issued with a new
   id**, and that a re-issue does not produce two effects (the idempotency ledger is what must make
   this safe).
8. **stdio isolation tests.** Assert the child does not inherit the daemon's environment, that stderr
   is bounded, that a crash is detected, and that a timeout is enforced.
9. **Oversized-output tests.** A `structuredContent` payload far over the bound must truncate with the
   flag set, not be stored whole.
10. **Opt-in live smoke test.** One handshake + `tools/list` + one read-only `tools/call` against a
    sandboxed reference server, gated behind an explicit environment variable so CI does not need
    network access.

## Unresolved Questions

1. **Does the dual-era server requirement conflict with the daemon's current single-transport shape?**
   Impact: `P3-009` may need a second listen path. Blocks: the server-exposure design, not the client.
2. **Which optional extension is needed first, if any?** Tasks is the likeliest but was just revised.
   Impact: none now. Blocks: nothing — deliberately deferred.
3. **Does any target server still require the deprecated HTTP+SSE transport?** Impact: would need a
   legacy transport path. Blocks: adapter scope, and can only be answered by naming actual servers.
4. **Is `negotiate_initialize` on the server side reachable for a handler that overrides
   `initialize`?** The SDK exposes it and tests it, but the interaction between an overridden handler
   and a narrowed `supported_protocol_versions` is subtle enough that `P3-009` must test it rather than
   infer it.
5. **RESOLVED (2026-09-22, `P3-008e`): the feature set is measured and admits.** `cargo deny check`
   reports advisories, bans, licenses, and sources **all ok** with `rmcp = 3.4.0` in the graph, using
   `default-features = false` plus `client`, `transport-child-process`, and
   `transport-streamable-http-client-reqwest`. The graph gained no duplicate of anything the
   workspace pins: `tokio` 1.53.1, `reqwest` 0.13.5, `serde` 1.0.229, `serde_json` 1.0.151, and
   `tracing` 0.1.44 all resolve to the versions already locked, and `process-wrap` 10.0.0
   (`Apache-2.0 OR MIT`) is genuinely new. `request-state` (which would pull `hmac` and `sha2` 0.11
   against our pinned `sha2` 0.10.9) is not enabled, which is why no duplicate arises — that is a
   consequence to re-check on any bump, not a property to assume.
6. **Whether `rmcp`'s `auth` feature is usable for our authorization model.** JARVIS authorization is
   not MCP authorization, so the question is only whether the SDK's OAuth machinery can be confined to
   the adapter. Unresolved pending `P3-008`.
7. **A server may declare a JSON Schema dialect JARVIS does not implement, and the specification's own
   examples invite it.** The `Tools` page states `inputSchema` "Defaults to 2020-12 if no `$schema` field
   is present" and shows a **`draft-07` example** as a supported form. `jarvis-tools` implements 2020-12
   only and refuses another declared dialect rather than reinterpreting it, so such a tool is excluded.
   Impact: a server following the spec's own draft-07 example is unreachable through MCP. Blocks:
   nothing yet — no target server has been named — but it needs either a documented operator-facing
   refusal or a draft-07 path, and `P3-008`/`P3-009` should decide which.

## Contract Facts Established During Implementation

These were resolved while building `P3-008a`/`P3-008b` and are recorded here because they are facts
about the protocol, not decisions of ours.

- **A tool name and a JARVIS identifier do not have the same alphabet.** MCP names `SHOULD` be 1–128
  characters of `A-Za-z0-9_.-` and are **case-sensitive**; JARVIS names are lowercase-only, cap a
  segment at 48 characters, and forbid a dot in the name half. The two sets are not reconcilable by
  rewriting, which is why `P3-008a` refuses or digests instead (ADR-0024).
- **`tools/list` may legally contain duplicates and is required to be deterministic.** The
  specification requires the set not to vary per-connection and `SHOULD` return a deterministic order,
  but it does not forbid the same tool name twice. A repeat of one name from one server is therefore
  treated as a refresh rather than a collision.
- **`annotations` is explicitly untrusted.** The `Tools` page carries a warning that clients "**MUST**
  consider tool annotations to be untrusted unless they come from trusted servers", and the annotations
  that exist (`readOnlyHint`, `destructiveHint`, `idempotentHint`) are exactly the facts a policy engine
  needs. ADR-0025 is the consequence: the posture comes from an operator.
- **`outputSchema` is optional and `structuredContent` may be any JSON value.** A tool with no declared
  output schema therefore has no schema to validate against, which is the truthful encoding of "the
  server made no claim" — the protection for such a result is the output-size bound, not validation.
- **The specification requires per-tool exclusion, not list failure.** An invalid tool definition must
  be excluded from `tools/list` with a warning naming it, so that "a single malformed tool definition
  does not prevent other valid tools from being used". Both `P3-008a` and `P3-008b` follow this.

## Verification Log

| Date | Versions checked | Relevant change or no-change evidence | Researcher |
| --- | --- | --- | --- |
| 2026-09-20 | spec/docs 2026-07-28, Rust SDK `main` | Initial architecture verification; exact release selection deferred. **Superseded below.** | GitHub Copilot |
| 2026-09-22 | spec 2026-07-28 (`llms.txt`, changelog, versioning, streamable-http); `rmcp` 3.4.0 metadata; SDK `main` source | **Substantive change since the previous entry.** `2026-07-28` is a stateless rewrite: `initialize` removed, per-request `_meta`, mandatory `server/discover`, sessions and SSE resumability removed, `subscriptions/listen` replaces the GET stream and `resources/subscribe`, MRTR replaces server-initiated requests, required `resultType`, error codes renumbered (`UnsupportedProtocolVersion` → `-32022`, resource-not-found → `-32602`), Roots/Sampling/Logging deprecated, Tasks moved to an extension. Rust SDK is now **Tier 1**; selected **`rmcp` 3.4.0** (2026-09-15, Apache-2.0, MSRV 1.88, edition 2024). Found that `ProtocolVersion::LATEST = V_2025_11_25` — the SDK default is *not* the current revision. | GitHub Copilot |
| 2026-09-22 | `rmcp` 3.4.0 vendored source (`service/client.rs`, `model.rs`, `transport/async_rw.rs`), crates.io API, `cargo tree`/`cargo deny` with the SDK admitted | **Re-verified during `P3-008e`, from source rather than metadata.** Confirmed `LATEST = V_2025_11_25` at `model.rs` (`pub const LATEST: Self = Self::V_2025_11_25;`), which **contradicts the SDK's own README** — the README calls `LATEST` "newest stable version this SDK defaults to" while the same file states the SDK implements `2026-07-28`. The source is the contract; the README is not. **New finding:** `DEFAULT_AUTO_DISCOVER_TIMEOUT` (10s) is applied **only** in the `ClientLifecycleMode::Auto` arm of `serve_client_with_lifecycle` — `Discover` has **no deadline**, so a client using `Discover` against a silent peer waits indefinitely. This is a real gap for a daemon starting against a dead or legacy server, and it is why `jarvis-mcp-transport` wraps the negotiation in its own timeout. Also verified: stdio framing is newline-delimited JSON-RPC (`read_until(b'\n', ..)` / `put_u8(b'\n')` in `transport/async_rw.rs`), `(R, W)` pairs are valid transports, and `ServerPeerInfo.server_info` is `Option<Implementation>` because "discovery responses are not required to provide it" — so a conforming modern server may report **no identity**. Deny gate measured: all four categories ok. | GitHub Copilot |
| 2026-09-23 | `rmcp` 3.4.0 vendored source (`model.rs` `ts_union!`, `model/mrtr.rs`, `model/task.rs`, `service/client.rs`), read while implementing `tools/call` | **Three facts that change error handling, all read from source.** (1) The protocol's result union is `#[serde(untagged)]` — `ts_union!(@declare_end ..)` emits it — so a result whose `resultType` disagrees with its fields does **not** fail as "a malformed `input_required`"; it becomes the SDK's generic `ServiceError::UnexpectedResponse`, which names nothing. Hence `CallError::Undecodable`, which is distinct from `Unavailable` because the peer *did* answer. (2) `CallToolResponse` is `#[non_exhaustive]` with `Complete`/`InputRequired`/`Task`, and `call_tool_once` returns it directly (no MRTR driving), so the modes JARVIS cannot serve are refusable by name; the high-level `call_tool` drives MRTR rounds through a local `ClientHandler`, which **JARVIS does not register** — using it would produce a failure about a missing handler rather than a named refusal. (3) `CreateTaskResult` **flattens** the seed `Task` (`#[serde(flatten)] pub task: Task`) rather than nesting it under a `task` key, and `InputRequiredResult` requires `resultType: "input_required"` **and at least one of** `inputRequests`/`requestState` (custom deserializers, both present to stop greedy matching in the untagged union). A fixture written with the nested shape decoded as nothing and surfaced as `Undecodable` — which is how (1) was found. | GitHub Copilot |
| 2026-09-23 | `rmcp` 3.4.0 vendored source (`transport/common/reqwest/streamable_http_client.rs`, `transport/streamable_http_client.rs`, `transport/common/mcp_headers.rs`), read while implementing `connect_http` | **Two facts that changed the client design, both read from source.** (1) **The SDK's `default_http_client` does not call `no_proxy`.** Its builder is `reqwest::Client::builder().pool_max_idle_per_host(0).redirect(Policy::none()).build()`, so proxy support is off **only because the SDK's manifest declares `reqwest` with `default-features = false`** (verified in its `Cargo.toml`: `version = "0.13.2", features = ["json","stream"], default-features = false`). That is a fact about a *dependency's* manifest, not a property of anything in this workspace, so a feature unification elsewhere could re-enable an unchosen intermediary with no local change. This crate therefore builds the client itself and passes it via `StreamableHttpClientTransport::with_client` (`reqwest::Client` implements the SDK's `StreamableHttpClient` trait, impl at `streamable_http_client.rs:49`). (2) The transport discriminates responses by **`Content-Type` prefix**: `text/event-stream` → SSE stream, `application/json` → JSON body, anything else → `UnexpectedContentType`; a non-2xx response whose body parses as a JSON-RPC error is surfaced as `McpError` rather than lost. Requests carry `Accept: text/event-stream, application/json` plus (on modern protocol versions) `Mcp-Method`/`Mcp-Name`, both confirmed on the wire by a hand-written HTTP server in `tests/http.rs`. | GitHub Copilot |
| 2026-09-23 | `rmcp` 3.4.0 behaviour observed through `tools/call`, while implementing the `ToolExecutor` adapter | **What the transport does *not* classify, and therefore what an adapter must.** `McpCallResult` carries `is_error` straight from the server's `isError` and does no outcome reasoning — deliberately, because the transport cannot know whether an effect happened. Three cases were confirmed against a live scripted peer: (a) a tool answering with `isError: true` returns **`Ok`** from `call_tool`, not an error, so an adapter that only handled `Err` would record a tool's own refusal as a transport success; (b) a JSON-RPC error arrives as `CallError::PeerError` and an undecodable result as `CallError::Undecodable` — the latter because the result union is untagged, so a shape mismatch matches no variant; (c) **a peer that closes the connection after receiving a POST produces `CallError::Unavailable`, the same variant a connection that never opened produces**, so the classification of "sent, no answer" versus "nothing sent" is information only the adapter's own sequencing has. That is why the adapter treats `Unavailable` as ambiguous rather than as a refusal. | GitHub Copilot |
