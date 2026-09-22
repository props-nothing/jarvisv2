# Protocol Architecture

## Principles

- Domain types are not wire types.
- Every external protocol is versioned, authenticated, bounded, and observable.
- Streaming has sequence, cancellation, heartbeat, and reconnect semantics.
- Additive evolution is preferred; incompatible changes require a new major protocol version.
- Generated schemas/clients come from one source where practical.

## Local Daemon Protocol

The CLI and desktop use a local transport by default:

- Unix domain socket on Linux and macOS
- named pipe on Windows
- authenticated loopback HTTP as a **peer transport**, not a fallback. ADR-0011 settled this:
  the HTTP surface is how clients that cannot speak a pipe or socket (a browser, the Tauri web
  view, an OpenAI-compatible voice brain) reach the daemon, while local IPC stays **preferred**
  for the CLI and desktop because it is OS-protected and needs no port.

Local transport still binds requests to a client identity and profile. Filesystem presence or local-user status is not automatically authorization for every operation. Socket/pipe permissions are restrictive and `doctor` verifies them.

The protocol carries a handshake with client version, protocol versions, capabilities, profile, and daemon build/schema information. The daemon supports a documented compatibility window and produces an actionable upgrade error outside it.

### Implemented Protocol v1 (P1-008)

The first local slice is implemented and verified end to end on Windows:

- Framing: a 4-byte big-endian payload length followed by UTF-8 JSON, capped at 64 KiB per frame. A declared length above the cap is rejected before allocation.
- Handshake: the client sends credential, `client_id`, client kind, client version, a protocol `[minimum, maximum]` window, and capability tokens. The daemon replies with an accepted `DaemonHandshake` or a `Rejected` error; a rejection carries a stable `ErrorCode`, never a bare success.
- Negotiation: the highest version in the intersection of both windows is selected; inverted windows, a too-old client, and a too-new client fail closed with a distinct error.
- Authentication: a per-profile 32-byte credential stored beside the configuration (Unix `0600`, Windows current-user-only ACL through the private config directory). Comparison is length-independent and content-constant-time; an accepted-or-rejected handshake is the only outcome.
- Endpoint: Unix domain socket at `runtime/jarvis.sock`; Windows byte-mode named pipe `\\.\pipe\jarvis-<profile>-<root-hash>` with remote clients rejected and first-instance protection on the daemon's initial instance. Windows pipe names are machine-global, so the runtime directory is folded in through a stable hash: otherwise a portable profile named `default` would collide with a natively installed `default`.
- Commands: `status` and `health`. Requests are bounded per connection and the daemon returns a stable `WireError` envelope with a correlation ID.

### Implemented HTTP Transport (`P2-007`)

ADR-0011 makes the HTTP API a first-class, **separately enabled** peer transport. What exists:

- Routes: `POST /api/v1/runs`, `GET /api/v1/runs/{id}`, `POST /api/v1/runs/{id}/cancel`,
  `GET /api/v1/runs/{id}/events`, `GET /api/v1/runs/{id}/stream`,
  `POST /api/v1/tools/{tool}/calls`, plus `/health/live` and `/health/ready`.
- Authentication is the **same** profile credential, verified through the same
  `ClientCredential::matches`, presented only in the `Authorization: Bearer` header. There is no
  second authentication implementation and no third-party auth middleware, so the two transports
  cannot disagree about who is admitted. The health routes are authenticated too: an
  unauthenticated liveness endpoint would tell any local process whether a daemon is running.
- Errors reuse `WireError`, so one error vocabulary spans transports. A test asserts the HTTP
  refusal carries `ErrorCode::Authentication`, which is the same code the local transport
  reports for the same condition.
- The stream is replayable from the durable `run_events` record by sequence cursor
  (`Last-Event-ID`), and a cursor beyond what the daemon holds is an explicit resync
  (`409`) rather than an empty success. A malformed cursor is refused rather than defaulted:
  defaulting it downward replays events the client already holds, and defaulting it upward skips
  events, and both failures look like a working stream.
- Bound to loopback and **off by default** (`daemon.http_enabled`, `JARVIS_HTTP_ENABLED`;
  `daemon.http_port`, `JARVIS_HTTP_PORT`, default `8765`). Reaching a non-loopback interface is
  remote mode, owned by `P10-004` as an explicit TLS-terminated configuration.

### The Tool Call Route (`P3-012` partial)

`POST /api/v1/tools/{tool}/calls` is the first path from a client to a tool adapter. ADR-0023 records
why it is composed in the daemon and what it may read.

- The body is exactly `run_id` and `arguments`, and it is decoded with `deny_unknown_fields`. The
  **workspace comes from the stored run's row** and the scope from the daemon, never from the request.
  A request that names a `workspace_id` is `422` rather than accepted-with-the-field-ignored, because an
  ignored field reads as an accepted one. A test asserts both the positive control (a granted root
  really is read) and that refusal.
- A call is attributed to an existing run, so an unknown `run_id` is `404`: without the stored row there
  is no workspace to decide against.
- **A refusal and a failure are different shapes.** A policy denial is `403` with a `reason_code` — a
  correct answer the client must not retry. An executed call is `200` **regardless of outcome**, with
  the outcome, the provider evidence, the reason, the content, and the truncation flag as separate
  fields, so a `failed` outcome cannot be mistaken for a success. A traversal is the second shape:
  confinement refuses it inside the adapter, producing `state: "failed"` with `output: null`, not a
  transport error (ADR-0020's fifth rule).
- A held decision is `202` with the `call_id` and the `required_strength`. **Nothing lets a human
  decide it yet**, so the call stays `requested` — see the limits in ADR-0023.
- With no `daemon.tool_workspace_roots` there is **no pipeline at all**, and the route is `404` naming
  that key, rather than a tool that fails every call.

Still unbuilt from the surface below: `sessions`, `GET /api/v1/tools`, `approvals`, `memories`,
`connectors`, `runtimes`, and `models`. A run is driven to a terminal state by the executor
(`P2-009`), and a restart settles any run it interrupted (`P2-010`).

### The CLI Over This Transport (`P2-008`)

`jarvis ask <objective...>` is the first client of the HTTP surface, and ADR-0012 records why it uses
HTTP rather than local IPC: runs are defined only on `/api/v1`, so a client uses the transport that
carries the operation it needs. `status` and `health` stay on local IPC, which ADR-0011 records as the
preferred transport for the CLI.

- The endpoint is `jarvis_core::LoopbackHost`, which holds **a port only**, so a client cannot be aimed
  at a host the daemon does not serve. The port comes from the same configuration the daemon binds.
- The credential is sent **only** as `Authorization: Bearer`, never in a query string.
- A run identifier interpolated into a path is validated against a strict character set first, so a
  value containing `/`, `?`, `#`, or a percent-escape is refused rather than changing the request's
  target.
- The CLI contains no orchestration: it sends an objective and no identity, and the daemon resolves the
  workspace and user from its own seeded rows.
- `jarvis ask` needs the HTTP transport enabled, and says so with the exact config keys when it is not,
  rather than reporting a connection failure.
- **The CLI does not set a client-level request timeout.** A `reqwest` total timeout bounds the whole
  response body, so on a client that also serves an open-ended SSE stream it kills every healthy stream
  at the deadline. Non-streaming calls set a per-request timeout; only `read_timeout` bounds a stream.
  This was found by running the client, not by a test.

### Multi-Turn Conversations (`P2-009b`)

A conversation is **runs sharing one session**, and ADR-0014 records why: a run settles once and a
settled run emits no further events, so a conversation cannot be one long-lived run, and keeping the
history in the client would make the server unable to enforce policy over it or audit it.

`jarvis chat` reads a turn per line and starts a run per turn:

- `POST /api/v1/runs` accepts an optional `session_id`. Present means "continue this conversation";
  absent means a new one, which is the ordinary single-turn case.
- The **daemon** replays the session's transcript into the model call, so history is not a client
  artefact and a second client sees the same conversation. The selection is the context assembler's,
  and its result is the manifest the daemon records.
- The **session identifier is a trust boundary, not a capability.** Attaching a run to a session is one
  statement whose predicate includes the workspace, the user, and `status = 'active'`. A session of
  another workspace or user is refused as `404`, deliberately, so the refusal does not confirm that
  somebody else's conversation exists. An archived session is `409`, because that one is actionable:
  the caller starts a new conversation rather than retrying.
- The client never invents a session identifier. `chat` prints the one the daemon issued and sends that
  back, so it cannot end up addressing a session that does not exist.
- The replay window is the **newest** turns, bounded, because a transcript grows without limit and a
  model's context does not. A turn the budget cannot hold is excluded with a recorded reason rather
  than silently dropped.
- A failed **turn** does not end the conversation; a refused **session** does, because every later turn
  would fail the same way.

## HTTP API

Initial versioned surface:

```text
GET  /health/live
GET  /health/ready
GET  /api/v1/status

POST /api/v1/sessions
GET  /api/v1/sessions/{id}
POST /api/v1/runs
GET  /api/v1/runs/{id}
POST /api/v1/runs/{id}/cancel
GET  /api/v1/runs/{id}/events

GET  /api/v1/tools
GET  /api/v1/approvals
POST /api/v1/approvals/{id}/decisions

GET  /api/v1/memories
POST /api/v1/memories
PATCH /api/v1/memories/{id}
DELETE /api/v1/memories/{id}

GET  /api/v1/connectors
GET  /api/v1/runtimes
GET  /api/v1/models
GET  /api/v1/workflows
GET  /api/v1/events
```

OpenAPI is generated and checked for unintended breaking changes. Pagination uses stable cursors. Mutating requests accept an idempotency key where replay is plausible.

## Streaming

Use SSE for one-way run/activity streams and WebSocket only for genuinely bidirectional low-latency sessions such as interactive voice/control.

Every stream event includes:

```text
protocol_version
stream_id
sequence
event_id
event_type
timestamp
run/session/correlation IDs
payload
```

Clients reconnect with the last observed event/sequence. The server replays from durable records within retention or returns an explicit resync requirement. Heartbeats are protocol events, not fake content.

### The Durable Record Behind Replay (P2-007b)

The replay requirement is satisfied by `run_events`, a per-run ordered log added by migration
`0004_run_events.sql`. `sequence` is scoped to one run and starts at 1, and the writer
allocates it **inside** the insert, so a gapless stream is a constraint rather than a
convention. Decision and rejected alternatives: [ADR-0011](../adr/0011-run-events-and-http-transport.md).

Three properties are enforced rather than assumed:

- **A settled run emits nothing more.** An append to a terminal run is refused, so the
transcript cannot continue after settlement.
- **A terminal event is always last.** This spans the whole stream, not the returned page,
because that is a property of the stream rather than of one row; a page that ended before
the terminal event would otherwise hide a later row.
- **A requested position beyond the stored stream is a resync, not an empty reply.** For a
reconnecting client the two are otherwise indistinguishable.

An event kind is a closed set, because a client switches on it. Unknown **additive fields**
are tolerated; an unknown **kind** is refused at the writer.

### HTTP Is A Peer Transport, Not A Fallback

The `/api/v1` surface is a first-class daemon transport alongside local IPC, not a
platform-constraint fallback required only where a pipe or socket is unavailable. It exists
for clients that cannot speak a
named pipe or a Unix socket: a browser, the Tauri web view, and provider callbacks such as
an OpenAI-compatible voice brain. Local IPC stays the preferred transport for the CLI and
desktop because it is OS-protected and needs no port.

Loopback binding by default, the **same** profile-bound credential, and the **same**
`WireError` envelope as protocol v1 — one authentication implementation and one error shape
across transports. Both dispatch into the same application commands. Routing code lives in
`apps/jarvisd` and REST DTOs in `jarvis-protocol`; the domain crates gain no HTTP dependency.
Reaching a non-loopback interface is remote mode (`P10-004`).

## Error Envelope

Errors have a stable code, safe message, retryability, correlation ID, and optional field violations. Internal/provider details are logged in redacted form and not returned by default.

Classes include authentication, authorization, validation, conflict/version, approval-required, unavailable capability, rate-limited, timeout, cancelled, transient upstream, permanent upstream, ambiguous effect, and internal.

## MCP

- Local server mode: stdio.
- Remote mode: Streamable HTTP at `/mcp`.
- Negotiate protocol version and capabilities; do not hard-code the newest draft.
- Use current MCP authorization requirements for remote access.
- Convert MCP definitions/results at the adapter boundary.
- Scope each MCP client to explicit workspace and tools.
- Test with the official MCP Inspector and at least one independent SDK.

Legacy SSE support is adapter/version driven only when a concrete integration still requires it.

## Runtime Worker Protocol

The JARVIS runtime protocol is separate from MCP because it manages an agent execution lifecycle, not just tools/resources.

It supports:

- handshake and capability negotiation
- start/resume/cancel
- ordered runtime event stream
- host-mediated tool requests
- human/input requests
- artifacts and usage
- liveness and graceful settlement
- opaque native-session binding

Transport may be framed stdio for local workers or authenticated HTTP/WebSocket for remote workers. The same conformance suite applies to both.

## OpenAI-Compatible Voice Boundary

For ElevenLabs Custom LLM mode, JARVIS will expose authenticated compatibility endpoints:

- `POST /v1/responses` as the preferred implementation
- `POST /v1/chat/completions` for compatibility

As verified from official ElevenLabs documentation on 2026-09-20, both require SSE. Responses streams require `response.output_text.delta` and `response.completed` events and end with `data: [DONE]`; Chat Completions uses standard data chunks and `[DONE]`. Function/system tools arrive in OpenAI-compatible form.

This surface is a protocol adapter into a JARVIS run. It is not direct access to a model provider. Authentication, voice-session identity, workspace, context, policy, tools, and audit remain JARVIS-owned. Unknown extra fields are preserved only when explicitly safe; `elevenlabs_extra_body` is parsed through a bounded allowlist.

The implementation must re-verify current docs and fixtures before release.

## Webhooks

Provider-specific ingress endpoints:

- read the raw bounded body
- verify signature and timestamp before parsing trusted fields
- map endpoint/configuration to an expected provider and workspace
- enforce content type and size
- deduplicate provider event ID or stable body-derived key
- persist then acknowledge quickly
- process asynchronously
- record delivery attempts and dead letters

Never log raw webhook bodies by default.

## Versioning

Track independently:

- application version
- local daemon protocol version
- REST API major version
- event schema version
- runtime protocol version
- database schema version
- configuration schema version
- connector/tool version
- negotiated MCP specification version

`jarvis status --json` and `jarvis doctor --json` expose these without secrets. Updates preflight compatibility before replacing a running daemon.