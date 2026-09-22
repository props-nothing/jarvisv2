# Architecture Decision Records

ADRs preserve why durable choices were made. Accepted records are historical; change a decision by adding a new ADR that marks the old one superseded.

## Statuses

- `Proposed`: under review; implementation should not depend on it yet
- `Accepted`: current decision
- `Superseded by ADR-NNNN`: replaced, retained for history
- `Deprecated`: still present only for compatibility
- `Rejected`: considered but not selected

## Required Sections

Each ADR includes status/date, context, decision, consequences, alternatives, and conditions that would justify revisiting it. Link implementation and migration evidence when available.

## Index

| ADR | Decision | Status |
| --- | --- | --- |
| [0001](0001-rust-control-plane.md) | Rust owns the durable control plane | Accepted |
| [0002](0002-daemon-client-topology.md) | Use a daemon with thin clients | Accepted |
| [0003](0003-sqlite-local-postgres-server.md) | SQLite local, PostgreSQL server | Accepted |
| [0004](0004-jarvis-owned-memory.md) | JARVIS owns canonical memory | Accepted |
| [0005](0005-canonical-tools-and-mcp.md) | Canonical tool gateway; MCP at the boundary | Accepted |
| [0006](0006-isolated-agent-runtimes.md) | External agent runtimes are isolated adapters | Accepted |
| [0007](0007-native-workflows-before-temporal.md) | Build a native durable workflow engine first | Accepted |
| [0008](0008-provider-neutral-voice.md) | Voice is provider-neutral; ElevenLabs is an adapter | Accepted |
| [0009](0009-clean-room-prototype-migration.md) | Migrate prototype behavior clean-room | Accepted |
| [0010](0010-model-based-turn-detection.md) | Turn detection and interruption are model-based capabilities | Accepted |
| [0011](0011-run-events-and-http-transport.md) | Run events are durable; HTTP is a first-class daemon transport | Accepted |
| [0012](0012-cli-runs-over-http.md) | The CLI reaches runs over HTTP with a loopback-only endpoint type | Accepted |
| [0013](0013-restart-settles-interrupted-runs.md) | A restart settles interrupted runs truthfully instead of resuming them | Accepted |
| [0014](0014-conversations-are-runs-in-a-session.md) | A conversation is runs sharing a session, and the session is a trust boundary | Accepted |
| [0015](0015-tool-contract-consistency-and-offline-schemas.md) | Tool contracts derive their source, refuse remote schema references, and validate cross-field consistency at construction | Accepted |
| [0016](0016-supplied-schema-documents-and-registry-boundaries.md) | A schema composes only against JARVIS-supplied documents; external manifests are self-contained | Accepted |
| [0017](0017-policy-evaluation-outcomes-and-ownership.md) | Policy evaluation is a pure adapter-crate function with three outcomes, deny overrides, and channel-capped authentication | Accepted |
| [0018](0018-approvals-bind-to-a-digest-and-store-no-bearer-token.md) | An approval binds to an intent digest and stores a nonce digest rather than a bearer token | Accepted |
| [0019](0019-tool-calls-are-a-durable-lifecycle.md) | A tool call is a durable lifecycle row; the idempotency ledger is a unique index and a terminal outcome is final | Accepted |
| [0020](0020-filesystem-confinement-is-a-handle.md) | Filesystem confinement is a directory handle rather than a validated path | Accepted |
| [0021](0021-an-authorization-receipt-derives-from-its-decision.md) | An authorization receipt is derived from its policy decision and cannot be built from a refusal | Accepted |
| [0022](0022-cancellation-carries-no-version.md) | A cancellation request carries no version, because operator intent cannot be stale | Accepted |