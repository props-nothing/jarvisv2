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
- authenticated loopback HTTP fallback only where platform/library constraints require it

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
- The authenticated loopback HTTP fallback remains unbuilt; it is required only if a platform/library constraint makes native IPC unavailable.

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