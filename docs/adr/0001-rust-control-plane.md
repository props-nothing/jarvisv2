# ADR-0001: Rust Owns The Durable Control Plane

- Status: Accepted
- Date: 2026-09-20

## Context

JARVIS must be installable without language runtimes, remain active as a long-lived service, enforce safety independently of models, handle concurrent streaming and background work, and support Windows, macOS, and Linux. External agent frameworks often require Python or TypeScript and evolve independently.

## Decision

Implement the durable JARVIS control plane in Rust using Tokio-based async services. Rust owns identity, sessions, context orchestration, canonical memory, tool policy/approvals, connectors, events, workflows, scheduling, APIs, secrets, audit, and state.

Use other languages only in isolated runtime/extension processes when their ecosystem provides clear value.

## Consequences

- Core installation can ship as native binaries.
- Authority and state-machine behavior remain strongly typed and independent of model frameworks.
- Rust expertise and native platform CI are required.
- Some AI SDKs will need protocol adapters and process supervision instead of direct library calls.
- The core must not attempt to rewrite every useful Python/TypeScript runtime in Rust.

## Alternatives

- TypeScript control plane: strong integration ecosystem but adds a runtime and makes native packaging/service behavior less self-contained.
- Python control plane: excellent AI ecosystem but weaker fit for a dependency-light installed daemon.
- Framework-owned core (OpenClaw/LangGraph/OpenAI Agents): accelerates one path while coupling canonical authority/state to that framework.

## Revisit When

Only if measured delivery/operational evidence shows Rust prevents the core product from meeting its requirements and a replacement preserves native installation, deterministic policy, durability, and adapter independence.