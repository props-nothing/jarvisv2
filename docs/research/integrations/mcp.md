---
integration: model-context-protocol
status: researched
last_verified: 2026-09-20
owners: []
selected_spec_version: 2026-07-28 candidate for implementation negotiation
selected_sdk: modelcontextprotocol/rust-sdk rmcp, exact crate version undecided
---

# Model Context Protocol

## Scope

Architecture research for JARVIS as MCP client/host/server, using local stdio and remote Streamable HTTP. Exact crate version and negotiated compatibility range will be selected in `P3-007`.

## Official Sources

| Source | URL/version | Accessed | Purpose |
| --- | --- | --- | --- |
| Documentation index | https://modelcontextprotocol.io/llms.txt | 2026-09-20 | current/versioned docs discovery |
| Current specification | https://modelcontextprotocol.io/specification/2026-07-28/index.md | 2026-09-20 | protocol contract |
| Versioning | https://modelcontextprotocol.io/specification/2026-07-28/basic/versioning.md | 2026-09-20 | negotiation/compatibility |
| Transports | https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/index.md | 2026-09-20 | stdio and Streamable HTTP |
| Authorization | https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization/index.md | 2026-09-20 | remote auth |
| Security guidance | https://modelcontextprotocol.io/docs/2026-07-28/tutorials/security/security_best_practices.md | 2026-09-20 | threat controls |
| Official Rust SDK | https://github.com/modelcontextprotocol/rust-sdk | 2026-09-20 | `rmcp` features and examples |
| Inspector | https://modelcontextprotocol.io/docs/2026-07-28/tools/inspector.md | 2026-09-20 | conformance/debugging |

## Verified Contract

### Transport And Lifecycle

- The current official docs/spec expose stdio and Streamable HTTP as standard transports.
- The official Rust SDK has server-side stdio, client child-process stdio, Streamable HTTP client/server feature flags, protocol negotiation, and stateless-friendly HTTP behavior.
- SDK examples show cross-language tests against the JavaScript SDK.
- Protocol versions are negotiated; JARVIS must not serialize only one unversioned shape.
- The current spec includes progress, cancellation, subscriptions, discovery, tools/resources/prompts, and extensions; support is capability-negotiated.

### Authorization

- Remote MCP authorization follows the current specification's OAuth-oriented discovery and security requirements.
- The Rust SDK includes auth/client credential features, but exact feature completeness and crate version require re-check during implementation.
- Stdio child process authorization relies on launch/configuration isolation and must not receive the daemon's whole environment.

### Volatility

- The `llms.txt` index contains current, older, and draft specification versions plus extensions and SEPs.
- The Rust SDK includes deprecation notes and a conformance assessment; current feature support must be checked rather than inferred from the latest spec.
- Legacy SSE appears in older integrations while current server transport is Streamable HTTP. Keep legacy support adapter-specific.

## JARVIS Mapping

- MCP tool schemas translate into canonical `ToolDefinition`; MCP is not the internal tool model.
- Server identity/configuration, negotiated version, tool-list snapshot/hash, and health belong to the MCP host adapter.
- Every call receives JARVIS actor/client/workspace grants, policy, approval, idempotency, output limits, and audit.
- External MCP clients receive purpose-built scoped profiles; no inherited operator authority.
- Local stdio servers run under the runtime/process supervisor with environment, path, resource, and output limits.
- Remote URLs pass SSRF/TLS/redirect policy and use dedicated credentials.

## Decisions

1. Use the official Rust SDK if its selected release passes required conformance and security checks; hide it behind `jarvis-tools` adapters.
2. Implement stdio and Streamable HTTP first.
3. Negotiate compatible versions and test at least the selected current version plus one supported prior version if the SDK permits.
4. Use the official Inspector and an independent SDK for interoperability.
5. Do not make MCP authentication equivalent to JARVIS authorization.

## Rejected Alternatives

- MCP as the JARVIS runtime protocol: it does not by itself model the complete run/supervision lifecycle JARVIS needs.
- Raw MCP tools exposed directly to models: bypasses canonical risk, policy, and naming.
- Arbitrary local MCP commands with inherited environment: leaks credentials and host authority.
- WebSocket as the primary MCP transport: not one of the standard transport pairs identified by the current SDK/spec.

## Verification Plan

- Run official Inspector CLI/TUI recipes against the JARVIS server.
- Cross-test Rust client with official JavaScript test server and Rust server with JavaScript client.
- Test negotiation, unknown versions, capability changes, pagination, cancellation, progress, subscriptions, reconnect/sessionless behavior as selected.
- Test OAuth discovery/invalid audience/scope/refresh and unauthenticated denial for remote mode.
- Test stdio environment stripping, command identity, stderr bounds, crash, timeout, and cancellation.
- Test malicious schemas, duplicate names, oversized outputs, prompt injection, and server URL SSRF.

Cheapest discriminator: stand up the official SDK counter Streamable HTTP fixture and prove JARVIS can negotiate/list/call it while replacing its metadata with a canonical scoped tool definition.

## Unresolved Questions

- Exact `rmcp` crate version/features and current SDK tier at implementation time.
- Compatibility window JARVIS will promise to external clients.
- Which optional current extensions (tasks, skills, subscriptions) belong in the first release.
- Whether any required ElevenLabs account path still needs legacy SSE rather than Streamable HTTP.

## Verification Log

| Date | Versions checked | Relevant change or no-change evidence | Researcher |
| --- | --- | --- | --- |
| 2026-09-20 | spec/docs 2026-07-28, official Rust SDK main | Initial architecture verification; exact release selection deferred | GitHub Copilot |