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
| [0023](0023-tool-pipeline-composition-root.md) | The tool pipeline is composed in the daemon over the adapter's own definitions | Accepted |
| [0024](0024-a-server-does-not-name-itself.md) | A server does not name itself, and two MCP tool names never become one identifier | Accepted |
| [0025](0025-mcp-effects-come-from-an-operator.md) | An MCP server's effects and risk come from an operator, never from the server | Accepted |
| [0026](0026-the-host-join-between-discovery-and-authority.md) | Discovery is joined to authority only where both halves are visible | Accepted |
| [0027](0027-a-remote-endpoint-is-a-validated-value.md) | A remote MCP endpoint is a validated value, and its HTTP client is built here | Accepted |
| [0028](0028-an-adapters-outcome-mapping-is-the-honesty-boundary.md) | An adapter's outcome mapping is the honesty boundary, and every row of it needs a test | Accepted |
| [0029](0029-the-mcp-host-role-is-a-narrow-configuration-surface.md) | The MCP host role is a narrow configuration surface, and one parser per document | Accepted |
| [0030](0030-the-daemon-dispatches-by-tool-identity.md) | The daemon dispatches by tool identity, and MCP servers live in their own document | Accepted |
| [0031](0031-a-dependencys-default-is-not-a-policy.md) | A dependency's permissive default is not a policy, and an origin is a tuple | Accepted |
| [0032](0032-a-transitive-tool-is-not-re-exposed.md) | A transitive tool is not re-exposed, and exposure is not the catalog | Accepted |
| [0033](0033-filtering-a-list-is-not-authorization.md) | Filtering a list is not authorization | Accepted |
| [0034](0034-a-delegated-check-that-cannot-express-the-rule.md) | A delegated check that cannot express the rule is not a control | Accepted |
| [0035](0035-a-documented-invariant-with-no-test-is-a-convention.md) | A documented invariant with no test is a convention | Accepted |
| [0036](0036-a-self-reported-name-is-never-a-permit.md) | A self-reported name is evidence or nothing, and never a permit | Accepted |
| [0037](0037-an-admitted-request-is-a-type.md) | An admitted request is a type, and the check order is a decision | Accepted |
| [0038](0038-a-network-request-is-never-a-local-caller.md) | A network request is never a local caller, and a policy refusal is a wire answer | Accepted |
| [0039](0039-a-remote-mcp-call-has-no-run.md) | A remote MCP call has no run, and its enforcement is the same enforcement | Accepted |
| [0040](0040-conformance-is-measured-against-the-protocol-schema.md) | Conformance is measured against the protocol's schema, not against the SDK | Accepted |
| [0041](0041-a-sandbox-guarantee-is-named-and-refused-when-unenforceable.md) | A sandbox guarantee is a named capability that is refused when it cannot be enforced | Accepted |