# ADR-0007: Native Durable Workflows Before Temporal

- Status: Accepted
- Date: 2026-09-20

## Context

Long-running schedules, waits, retries, approvals, and event-triggered work need durability. Temporal provides mature distributed orchestration, but requiring it would break the zero-infrastructure personal install and add substantial operational complexity before scale is known.

## Decision

Build a database-backed native workflow engine for initial local/server use. Persist versioned definitions, runs, steps, attempts, waits, leases, retries, idempotency, and events. Keep a `WorkflowEngine` port so Temporal can become an optional implementation later.

Model calls and external APIs execute outside deterministic transition transactions.

## Consequences

- Local JARVIS remains installable with SQLite only.
- The team must implement and rigorously test restart, lease, timer, duplicate, and ambiguous-effect semantics.
- The native feature set stays intentionally bounded.
- Temporal adoption remains possible without changing client/tool/memory policy ownership.

## Alternatives

- Temporal from day one: mature but too heavy for default personal installation.
- In-memory tasks/cron: lose work and approvals on restart.
- Agent framework checkpoints as workflows: tie business durability to one runtime and mix reasoning with orchestration.

## Revisit When

Adopt a Temporal adapter after measured needs such as a worker fleet, cross-service ownership, very large timer volume, or operational guarantees the native engine cannot responsibly provide. Record migration and determinism rules in a new ADR.