# ADR-0033: Filtering a list is not authorization

**Status:** Accepted

**Date:** 2026-09-23

## Context

`P3-009a` decided which browser origins may reach JARVIS and `P3-009d` decided which of our tools may be
advertised. Both are **values**, and a value decides nothing until something consults it. `P3-009e` is the
handler that consults them — the first place JARVIS is the one being called.

Three facts decided its shape, and the first is the one the ADR is named for.

**1. Nothing in the protocol couples `tools/list` to `tools/call`.** An MCP client may call any tool name
whether or not the server advertised it. A server that only filtered `tools/list` would have a
**catalogue, not a control** — the filtering would describe what a cooperative client sees, while the call
path would accept whatever arrived.

**2. The SDK's server-side defaults advertise the wrong era, and the research recorded this before the
code existed.** From `rmcp-3.4.0`'s source: `ProtocolVersion::default()` is `LATEST`, and `LATEST` is
`V_2025_11_25` — the legacy handshake era. `ServerConfig::default()` (a type alias for `InitializeResult`)
therefore advertises a revision this build does not implement, and
`ServerHandler::supported_protocol_versions` defaults to `ProtocolVersion::KNOWN_VERSIONS`, i.e. **every**
version the SDK knows, including the legacy ones. This is the same `LATEST`-is-legacy fact `P3-008e`
recorded for the client, arriving with a second consequence: on the client it meant naming a version
explicitly, and on the server it means the *advertised* value is wrong.

**3. The SDK's own documentation says a protocol error is rendered opaquely.** `Err(McpError)` reaches a
caller as something like "Tool result missing due to internal error", so a refused tool call that travelled
that way would be a refusal the caller cannot read.

## Decision

**1. The handler holds the served set, and `tools/call` re-derives the decision from it.**

`JarvisMcpServer` is built from `Vec<ServedTool>` — the type only `jarvis_mcp::served_tools` produces, so
**the filtering has already happened** and the handler cannot advertise something the exposure rules
excluded. `serves` is the single predicate both `tools/list` and `tools/call` consult, so the two cannot
disagree; a prefix check at one and an exact match at the other is how a server comes to advertise one set
and accept another.

**2. An unserved name is refused before the runner is consulted, and a test proves *that*.**

The refusal happens in `invoke`, before the runner appears in the control flow. "The call was refused" and
"the call was refused before anything ran" are different claims, and only the second is the security
property — refusing after running is not a control, it is a report. So the test's runner **panics if it is
reached**, and the falsification fails with
`the runner must not be reached for mcp.github.search`.

**3. An unserved name and a nonexistent one get the same answer.**

Both are "not available on this server", so a caller cannot use the difference to enumerate what exists.
A tool this project owns but does not serve is indistinguishable from one that does not exist, which is the
honest answer.

**4. Both handler defaults are overridden by naming one revision.**

`protocol_version` is set to `V_2026_07_28` and `supported_protocol_versions` returns exactly that.
The falsification of the first prints `left: ProtocolVersion("2025-11-25")`, which is the SDK default
stated as evidence. Narrowing the supported list matters more than the advertisement: a dual-era server
would accept an `initialize` handshake whose semantics this build does not drive, and the client could not
tell.

**5. A refused or ambiguous call travels as `CallToolResult::error`, and an unknown outcome says so.**

The outcome table is the honesty boundary one level up from `McpToolAdapter`'s:

| Outcome | Reported as | Why |
| --- | --- | --- |
| `Confirmed` | success with the bounded output | evidence the effect completed |
| `Failed` | error with the reason | evidence no effect occurred |
| `Unknown` | error saying the outcome is **unknown** | reporting success claims an effect that may not have happened; reporting a plain failure invites a repeat of one that may have — the same distinction `AdapterError::AmbiguousAfterReaching` draws |
| any other | error naming the outcome | `Requested`/`Authorized`/`Submitted`/`Cancelled` are not results an adapter reports here, so mapping one onto success or failure would be a claim nothing made |

The protocol offers a caller only success-or-error, so an unprovable outcome must travel **in the text**
rather than in a third state that does not exist. A truncation is likewise stated in the content, because a
caller that received a clipped payload without being told would treat it as the whole answer.

**6. The runner is a port, so the handler holds no identity.**

A remote call has no JARVIS run, no session, and no actor of its own, and only the daemon knows what one
should be attributed to. `ServedToolRunner` carries `(name, arguments, correlation_id)` and nothing else,
so the refusal rule is testable with **no daemon, no database, and no receipt**. The correlation identity is
minted in the handler rather than taken from the request: MCP's `_meta` trace-context keys are optional
OpenTelemetry conventions, and a client-supplied value is a client-supplied identifier.

**7. No annotations are advertised.**

`readOnlyHint`/`destructiveHint`/`idempotentHint` are exactly the facts a policy engine needs, and the
specification warns clients to treat them as untrusted. So a JARVIS tool's declared effects stay in the
JARVIS contract (ADR-0025) rather than being restated in a field the protocol says not to believe.

**8. The `server` feature is enabled, and the cost was measured before it was.**

Five new packages (`schemars`, `schemars_derive`, `serde_derive_internals`, `dyn-clone`, `pastey`),
nothing removed, and **no new duplicate** — the duplicate `name@version` sets before and after are
identical. `chrono`, `tower`, `sse-stream`, `http-body`, and `uuid` were already in the lock, and
`cargo deny` is ok in all four categories. This is recorded because the alternative — hand-rolling
JSON-RPC framing, header validation, the Base64 sentinel, and version gating — remains the higher-risk,
lower-value option, and because a dependency decision should cite measurement rather than preference.

## Consequences

- **A security property is now provable offline.** The refusal is pinned by a runner that panics when
  reached, and falsified by removing the check. Three falsifications in total: the served-name check, the
  version override (printed `2025-11-25`), and the `Unknown` mapping.
- **`CallToolResponse::Complete` is the only answer JARVIS gives.** The protocol allows `input_required` and
  `task`; an approval-shaped interaction belongs to the JARVIS approval path rather than being answered by a
  remote client, which is the rule ADR-0025 already records for MRTR. A response union that is always one
  variant is worth stating, because a reader would otherwise assume the others are reachable.
- **The SDK-default finding is now the third instance of the same pattern** in this project
  (`default_http_client`'s proxy, the server config's `Origin`/session/version fields, and now the
  handler's version and supported list). The reusable rule is in memory: read the `Default` impl's field
  list against the spec's MUSTs one at a time, and never inherit a permissive field.
- Still **not built**, and recorded rather than implied: **nothing is bound**. There is no loopback HTTP
  listener, no stdio serve, and no daemon wiring, so a remote client cannot reach this handler
  (`P3-009b`); no per-client allowlist or rate limiting (`P3-009c`); the runner has **no daemon
  implementation**, so a served call cannot yet pass the JARVIS pipeline and its actor, scopes, approvals,
  and call row are unexercised from this direction; and a remote call has **no `run_events` row** (`P3-012`).
- **Two of the three SDK-default findings are now pinned by tests that print the wrong value when the
  override is removed**, which is what makes them findings rather than preferences.

## Alternatives rejected

- **Filter `tools/list` and trust that callers only call what they were shown.** The protocol does not couple
  the two, so this is a catalogue rather than a control.
- **Refuse an unserved name in the runner instead of the handler.** Refusing after the call path has started
  is a report rather than a control, and it would put the security rule in the daemon where it cannot be
  tested without one.
- **Distinguish "you are not allowed" from "no such tool".** Enumerates the served set for a hostile caller.
- **Inherit `ServerConfig::default()`'s protocol version.** Advertises `2025-11-25`, the legacy era.
- **Inherit `supported_protocol_versions`.** Accepts every version the SDK knows, including the handshake
  semantics this build does not implement.
- **Return `Err(McpError)` for a refused call.** The SDK's documentation is explicit that clients render it
  opaquely, so the caller would not see the reason.
- **Map `Unknown` onto `Failed`.** A caller reading "failed" would reasonably retry, and a retry of a
  non-idempotent effect is a second effect.
- **Silently truncate oversized output.** A caller would treat a clipped payload as the whole answer.
- **Advertise `readOnlyHint`/`destructiveHint` from the declared effects.** Restates the JARVIS contract in
  a field the same specification tells clients not to trust.
- **Use the `#[tool]`/`#[tool_router]` macros.** They would put a tool's advertisement next to its execution
  with nothing checking they agree, which is exactly the `tools/list`-vs-`tools/call` coupling this ADR
  refuses to rely on.
