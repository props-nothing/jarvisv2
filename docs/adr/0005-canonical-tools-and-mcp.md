# ADR-0005: Canonical Tool Gateway With MCP At The Boundary

- Status: Accepted
- Date: 2026-09-20

## Context

Capabilities may come from native Rust, first-party connectors, MCP servers, HTTP services, external runtimes, or sandboxes. Provider/framework tool types differ and do not encode all JARVIS effect, risk, scope, approval, idempotency, and audit requirements.

## Decision

Define one canonical JARVIS tool contract and execution gateway. Every call passes schema/semantic validation, actor/workspace/account authorization, effect/risk policy, approval, idempotency, bounded execution, normalized outcome, and audit.

Support MCP as client/host/server interoperability. Translate MCP definitions and calls at the edge; do not use MCP objects as internal domain types.

## Consequences

- Native, connector, and MCP tools receive uniform safety and observability.
- External MCP authentication does not imply JARVIS authorization.
- Provider/tool metadata translation and compatibility tests are required.
- JARVIS can expose a narrow tool set to ElevenLabs or another agent without exposing its control plane.
- MCP protocol evolution is isolated in adapters.

## Alternatives

- Give models raw provider/MCP tools: inconsistent policy and confused-deputy risk.
- Make MCP the internal tool model: couples domain semantics to a changing interoperability protocol.
- Let each runtime execute its own tools: bypasses canonical approvals, secrets, outcomes, and audit.

## Revisit When

The canonical metadata may evolve by version. The single policy/execution path remains unless a formally equivalent enforcement architecture is proven.