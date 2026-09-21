# ADR-0006: External Agent Runtimes Are Isolated Adapters

- Status: Accepted
- Date: 2026-09-20

## Context

OpenClaw, OpenAI Agents, LangGraph, ACP-compatible agents, and future frameworks provide useful specialized execution. They have independent languages, dependencies, lifecycle, native tools, memory, and release cadence. Loading them into `jarvisd` would expand its failure and authority boundary.

## Decision

Define a versioned JARVIS runtime protocol and run third-party runtimes out of process by default. `jarvisd` owns selection, supervision, canonical state, context eligibility, policy, tool execution, memory, and audit. Runtime-native session/checkpoint IDs are opaque bindings.

JARVIS Native Runtime remains available without optional workers.

## Consequences

- A runtime crash or dependency conflict cannot directly crash the daemon.
- Python/TypeScript/Go workers can evolve independently.
- Protocol negotiation, event normalization, cancellation, health, resource limits, and orphan cleanup are required.
- Some latency and packaging complexity is accepted for isolation.
- Native runtime tools are disabled or explicitly sandboxed/mediated.

## Alternatives

- Link every runtime into one process: impossible across languages and unsafe for daemon reliability.
- Pick one framework as core: sacrifices replaceability and canonical ownership.
- Reimplement all runtimes in Rust: wastes mature ecosystem capability and creates long-term maintenance burden.

## Revisit When

A small trusted Rust runtime may execute in process after security review. Foreign or independently updated runtimes remain isolated.