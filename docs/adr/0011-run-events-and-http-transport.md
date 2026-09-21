# ADR-0011: Durable Run Events And The HTTP Transport

- Status: Accepted
- Date: 2026-09-21

## Context

`P2-007` adds `POST /api/v1/runs`, run status, cancel, and an SSE activity/output stream.
Two things it needs do not exist, and neither is a coding detail.

**1. There is no durable run event record.** `docs/architecture/protocols.md` requires
that every stream event carry a sequence and event ID, that "clients reconnect with the
last observed event/sequence", and that "the server replays from durable records within
retention or returns an explicit resync requirement". No table can answer that. `RunEvent`
is currently a sketch in `docs/api/contracts.md` with no storage behind it, and
`docs/data/schema.md` schedules `events` / `event_inbox` / `event_outbox` for **Phase 6**.
A stream built on an in-memory counter would pass every test in a single process and lose
history on every daemon restart, which is exactly when a client reconnects.

**2. The HTTP surface's standing is stated inconsistently.** `docs/architecture/protocols.md`
introduces the HTTP API as an "Initial versioned surface" listing `/api/v1/runs`,
`/api/v1/runs/{id}/events`, approvals, memories, and more, and then describes the
authenticated loopback HTTP fallback as "required only if a platform/library constraint
makes native IPC unavailable". `docs/adr/0002` says local clients "prefer" Unix sockets and
named pipes while network endpoints "bind to loopback by default". `P2-008` has the CLI
using "the daemon API". It is not decidable from these whether the HTTP API is a peer
transport, a fallback, or a transitional artifact — and the answer decides whether adding
an HTTP server framework to `jarvisd` is architecturally sound or a shortcut.

`docs/architecture/overview.md` already fixes where routing code may live: the domain core
must not import Axum, and `repository-layout.md` scopes `jarvisd` handlers to translating
protocols into application commands. What is missing is the storage decision and the
transport's status.

## Decision

### Run events are a per-run ordered record, written with the transition

Add a `run_events` table. Each row is scoped to one run and carries a **per-run monotonic
`sequence`** with `UNIQUE (run_id, sequence)`. The event row is written **in the same
transaction as the state transition that produced it**, so an event cannot exist for a
transition that rolled back, and a transition cannot commit without its event being
recorded. Sequence assignment and the insert are one statement pair inside one
transaction, so two writers cannot interleave to a gap or a duplicate.

The record is the source for the SSE stream: a reconnecting client supplies its last
observed event ID, and the server replays from the durable record or refuses with an
explicit resync requirement. Sequence is never derived from a wall clock or an in-process
counter.

### `run_events` is not the Phase 6 event bus

Phase 6's `events` / `event_inbox` / `event_outbox` are a **cross-aggregate bus**: their
consumers are background workers, and their semantics are at-least-once delivery, leases,
attempts, dedupe keys, and dead letters. A run stream needs **per-run total ordering and
replay for one watching client**. These are different questions asked of different data by
different readers.

Reusing the Phase 6 tables was rejected on two grounds. It cannot be done at all without
moving Phase 6 work ahead of Phase 2, and coupling them would make the run stream inherit
delivery machinery it does not need while making the event bus responsible for per-run
ordering it does not care about. One table serving two consumers with different ordering
and retention needs is the shape that later forces an `if` on `event_type`.

### The HTTP API is a first-class, separately-enabled daemon transport

The HTTP API is a **peer transport to local IPC**, not a fallback and not a temporary
artifact. It is the transport for clients that cannot speak a named pipe or a Unix socket:
a browser, the Tauri web view, and provider callbacks such as an OpenAI-compatible voice
brain. Local IPC remains the **preferred** transport for the CLI and desktop because it is
OS-protected and needs no port.

Constraints, all enforced rather than documented:

- Loopback binding by default. Reaching a non-loopback interface is remote mode: explicit,
  TLS-terminated, and separately configured (`P10-004`).
- The **same** profile-bound credential as protocol v1, verified by the same code path. No
  second authentication implementation, and no third-party auth middleware.
- The **same** `WireError` envelope, so one error shape spans transports and a caller
  cannot infer a different trust model from a different status body.
- Both transports dispatch into the **same** application commands and repositories. The
  HTTP layer is a protocol adapter, never a second control plane (ADR-0002).
- Routing code lives in `apps/jarvisd`, and REST DTOs live in `jarvis-protocol`, matching
  `repository-layout.md`. `jarvis-core`, `jarvis-application`, and `jarvis-storage` gain no
  HTTP dependency.

### Framework selection is researched, not assumed

`docs/development/external-research.md` requires a dated record before an external
dependency is adopted. `docs/research/integrations/rust-http-server-and-sse.md` records
`axum` 0.8.9 (MIT, MSRV 1.80) pinned exactly, with an explicit minimal feature set, and
records that `hyper` 1.11.1, `http` 1.5.0, `tower` 0.5.3, and `tower-http` 0.6.11 are
**already resolved** in this workspace through `reqwest`. The reuse of an existing HTTP
core is the reason this framework was chosen over any other.

## Consequences

- `CURRENT_SCHEMA_VERSION` moves 3 -> 4 with a new migration, and a pre-migration backup
  path is exercised by the existing `initialize` flow.
- Every run transition now writes two rows instead of one. The transition and its event
  share a transaction, so the added cost buys atomicity rather than being an extra write
  that can drift.
- The run stream is replayable and sequence-gap-detectable across daemon restarts, which is
  what makes `A03` (durable conversation) and `A04` (cancellation) testable at a
  reconnect boundary rather than only in one process.
- `jarvisd` gains an HTTP listener, so `doctor` and `status` have a new failure mode to
  diagnose: a port that is occupied or a listener that dies. `P1-010`'s "occupied
  port/socket" case already exists as a contract to extend.
- `axum::serve` never returns an error and retries socket errors by sleeping, which is
  *not* the fail-fast behaviour of the local listener. The gateway must therefore detect a
  dead listener itself instead of inferring it from `serve` returning. Recorded in the
  research record as an unresolved question with a concrete recommendation.
- A second transport means two places to enforce authentication. The mitigation is that
  there is one credential and one verification function, and a test that both transports
  refuse the same bad credential.
- Run events accumulate. A retention policy is a real follow-up, not an omission: without
  one the table grows without bound on a long-lived local install. Tracked as `P2-007b`
  follow-up and deferred until a retention requirement is measured, consistent with how
  `tracing-appender` rotation was deferred.
- A schema bump leaves fixture literals behind. `P2-004`'s own history shows `cargo test`
  does not catch a stale `database_schema` literal, so the version change requires grepping
  the workspace for the old number, not just updating the crate that defines it.

## Alternatives

- **Serve an in-memory stream with no durable record:** rejected. It violates
  `protocols.md`'s replay requirement, and it fails precisely on daemon restart, which is
  the reconnect case the requirement exists for. It would also make a sequence gap
  unobservable, which `runtime-and-models.md` requires be detectable.
- **Reuse the Phase 6 `events` / `event_outbox` tables:** rejected; different consumers,
  different ordering guarantees, and it would pull Phase 6 forward. See above.
- **Derive sequence from the run's `version` column:** rejected. `version` advances on state
  changes only, so several events (an activity update and an output delta) can belong to one
  version. Overloading it would make `version` mean two things and would still leave no
  place to store the event payload.
- **Store events as one growing JSON document per run:** rejected. It cannot be appended to
  transactionally, cannot express `UNIQUE (run_id, sequence)`, and turns a replay into a
  full parse of every prior event.
- **Treat HTTP as a fallback only:** rejected. It contradicts the documented `/api/v1`
  surface, would force the browser and Tauri web clients onto a transport they cannot
  speak, and would leave the OpenAI-compatible voice endpoints with no host.
- **Make HTTP the primary and drop local IPC:** rejected. It gives up OS-protected
  transport and a machine-global namespace already solved (`P1-008`), and ADR-0002 records
  the preference for IPC on local clients.
- **Use a framework other than axum:** rejected without deep evaluation because axum's
  dependency graph is already resolved in this workspace. Adding any other framework would
  introduce a parallel HTTP stack for a loopback API.
- **Adopt a WebSocket for the run stream:** rejected. `protocols.md` reserves WebSocket for
  genuinely bidirectional low-latency sessions and requires SSE for one-way run/activity
  streams.

## Revisit When

- A run-stream retention or compaction requirement is measured, or event volume makes the
  table a storage concern rather than a log. That is a retention policy decision, not a
  schema defect.
- The Phase 6 event bus is implemented and the two records are shown to duplicate a
  payload shape worth unifying. Any merge needs a new ADR, because it changes ordering
  guarantees for both consumers.
- A browser or remote client needs push that SSE cannot express — for example a
  client-to-server control channel on the same connection. That reopens WebSocket, and
  `protocols.md` already frames it as a bidirectional-session decision.
- axum 0.9 is released. Its changelog already reworks `Router` fallback merging, changes
  `#[from_request(via(..))]` rejection types, alters `axum::serve`'s future output, and
  makes `serve` apply hyper's default `header_read_timeout`. Upgrading is a task with the
  research record re-verified, not a `cargo update`.
- A second transport is added that would make "one credential, one error envelope" untrue.
