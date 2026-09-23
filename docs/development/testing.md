# Testing Strategy

## Principle

Test at the boundary that owns the behavior. A synthetic model proves JARVIS orchestration; it does not prove a provider. A recorded provider fixture proves decoding; it does not prove current authentication or service availability. A live smoke proves one real path; it does not replace deterministic edge-case tests.

## Test Classes

### Unit

Pure domain invariants, state transitions, policy tables, ranking, parsing, scheduling, redaction, and error mapping. No network, wall-clock dependence, global profile, or real secret.

### Repository

Run each repository contract against SQLite and PostgreSQL where supported. Cover constraints, transactions, concurrency conflicts, migrations, pagination, deletion, leases, and outbox atomicity.

### Adapter contract

Exercise a model/runtime/tool/connector/voice adapter against an official schema, independent reference implementation, or sanitized recorded wire fixture. Verify normalization, not only happy-path deserialization.

### Integration

Run multiple JARVIS components together with controlled fakes: daemon/API/application/storage, policy/approval/tool, event/workflow, runtime protocol, and voice-session binding.

### Platform

Run on native disposable Windows, macOS, and Linux environments for paths, permissions, IPC, services, keychains, notifications, audio devices, installers, updates, and uninstall. Do not emulate these conclusions from one OS.

### Live provider

Opt-in, credential-gated, cost-labelled tests against real external services. They verify auth, current wire shape, permissions, quotas, and provider outcomes. They use dedicated test accounts/resources and clean up safely.

### End-to-end

Start from a packaged or release-like installation and complete a user journey through a real client. Assertions include durable records, visible output, policy, audit, restart, and cleanup.

#### Implemented: the Phase 1 acceptance gate

`tests/e2e` is a workspace member holding the process-level gate. It builds and runs
the real `jarvisd` and `jarvis` binaries against a temporary portable root, because a
library test proves libraries and the roadmap gate names an installation:

```text
cargo build --workspace
cargo test -p jarvis-acceptance --test phase1_gate
```

Set `ACCEPTANCE_REQUIRE_BINARIES=1` to make a missing build fail instead of
skipping. CI sets it. The variable deliberately avoids the `JARVIS_` prefix, because
the daemon rejects unknown `JARVIS_*` variables as configuration errors.

Two properties are worth preserving if this gate is edited:

- **Readiness is scoped to the process.** A hard-terminated daemon never records its
  `stopped` transition, so its last state stays `ready`. Matching any `ready` row
  accepts a stale row from a previous run; the gate matches the process ID it started.
- **Drain piped output only after termination.** Reading a child's piped stdout or
  stderr while it runs blocks on EOF that arrives only at exit, which hangs the gate
  instead of failing it.

## Deterministic Agent Tests

Build a scripted model/runtime that emits a sequence of normalized events. Use it to test:

- streamed text and activity
- one and multiple tool requests
- malformed/unknown tool calls
- approval pause, approve, deny, expire, and resume
- model retry and context overflow
- cancellation at every state
- runtime crash and fallback
- usage/cost accounting
- restart from each persisted boundary

Never call a real model in the default test suite.

## Falsification

Security and correctness guards require a control showing the test can fail. Examples:

- remove workspace filtering and prove the isolation test catches a cross-workspace row
- change an approval intent after decision and prove execution is rejected
- alter one webhook byte and prove signature verification fails
- simulate provider acceptance plus lost response and prove state is `unknown`, not failed/success
- reintroduce path traversal/symlink escape and prove the sandbox/filesystem suite detects it
- disable idempotency uniqueness and prove duplicate event/tool delivery creates an assertion failure

A test that can pass without observing the claimed boundary is not evidence.

## Fixtures

Every external wire fixture includes:

- provider/protocol and version/date
- official source or sanitized capture procedure
- operation represented
- removed/redacted fields
- expected normalized result
- license/terms note when relevant

No fixture contains real tokens, cookies, emails, names, phone numbers, calendar content, transcripts, audio, private URLs, or provider account IDs. Use structurally valid reserved example values.

Keep malformed, failed, paginated, rate-limited, empty, duplicated, out-of-order, and provider-upgrade fixtures alongside happy paths.

## Scratch Directories And Asynchronous Teardown

A fixture's scratch directory **must** be removed through `jarvis_core::remove_scratch_dir`, and its name **must**
contain `jarvis_core::scratch_tag()`. Both rules exist because violating either is invisible at runtime: a
violation leaks a directory and every test still passes.

**Naming.** A name built from a pid, a timestamp, or a counter that resets is reusable, so a directory left behind
by a killed run can be reopened by the next one — which is how a whole suite's fixtures began failing with
`UNIQUE constraint failed`. A `UUIDv7` is unique across processes and across runs.

**Removal.** `std::fs::remove_dir_all` in a guard's `Drop` **cannot** work for a fixture holding a database, and
the reason is worth knowing rather than rediscovering: `sqlx` releases the SQLite file late and on a spawned task,
so the failure is a race. Measured on Windows — drop then remove immediately fails **every** time, while drop,
wait 250 ms, then remove always succeeds. Two tempting fixes are worse than the problem:

- a bounded **blocking** retry still fails, because sleeping starves the current-thread runtime the pool needs;
- awaiting a close on a **fresh runtime inside a worker thread deadlocks**, and a deadlock in teardown is worse
  than a leaked directory.

Handing the removal to a **detached** thread that retries works: it does not starve the runtime, and it keeps
trying across the point where the runtime tears down. The window is **measured**: no retry at all left **124**
directories per full-workspace run, and the shipped version leaves **7–10**. Two designs measured *worse* or no
better and are recorded so they are not retried — a **ten-second** per-call window left the same handful, and an
**always-running** retry loop left **22**, because a thread competing for CPU across the whole suite delays the
runtime teardowns that are what actually release the handles. What remains are fixtures holding an
`Arc<SqliteDatabase>` — through a `ToolPipeline` — past the directory guard, so the handle is released at the end
of the test (which the window covers) or at process exit (which nothing in-process can reach). Such a fixture
should await its own close before the guard drops.

The rule is enforced by a source scan in `jarvis-core`'s testkit rather than by a behavioural test, because no
assertion can observe a directory *not* leaking without measuring the filesystem around a whole suite. The scan
requires the discarded-result form (`let _ = …remove_dir_all(&self.0)`), so it catches a bypass without flagging
its own explanation or an unrelated removal.

## Time And IDs

Inject clocks, UUID sources, randomness, and retry schedules. Tests use deterministic IDs and explicit time advancement. Scheduler suites cover daylight-saving gaps/overlaps and missed-run policy using real IANA zones.

## Concurrency And Crash Tests

- race two optimistic updates
- expire and reacquire a lease with fencing
- crash before/after each transaction commit and external effect
- deliver the same event concurrently
- cancel during model stream, tool call, approval wait, and workflow wait
- restart with orphaned external runtime process
- verify no terminal state regresses

Use model checking/property tests for small state machines where practical.

## Security Tests

Follow the list in [security.md](../architecture/security.md). Security fixtures are content-minimized. Secret redaction tests insert canary values into every structural location (headers, path, query, userinfo, JSON, errors) and assert no normal log/trace/diagnostic contains them.

## API And Protocol Tests

- OpenAPI compatibility diff
- local protocol handshake/version window
- SSE sequencing/reconnect/resync
- WebSocket auth, cancellation, backpressure, and frame bounds
- runtime worker protocol conformance for each language worker
- MCP Inspector plus independent SDK cross-tests
- ElevenLabs Custom LLM SSE fixtures for both supported endpoint shapes
- turn-event fixtures covering `EndOfTurn`, `EagerEndOfTurn`, `TurnResumed`, `FalseInterruption`, and `Backchannel`. A scripted pause-heavy utterance must produce a boundary at the model's pause, not at the earliest silence; a silence-window implementation passes every framing test and fails only this one.
- webhook raw-body signature and replay

## UI Tests

When the UI exists:

- component tests for rendering/state
- generated-client contract tests
- Playwright user journeys against a real test daemon
- desktop smoke on native OS
- screenshots for responsive desktop/mobile widths
- accessibility checks for keyboard, focus, labels, contrast, motion, and screen readers

Approval UI tests verify the exact target/effect shown matches the intent hash executed.

## Commands

The Phase 1 Rust baseline will be:

```text
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo deny check advisories bans licenses sources
```

The first three commands run on native Windows, macOS, and Linux CI. The dependency-policy command runs once on Linux against the same committed lockfile. Later commands must be added to a root task runner only after they exist. CI and this document stay synchronized. Live tests require explicit flags and never run merely because a developer happens to have a credential in the environment.

## Release Evidence

A release candidate needs:

- unit/repository/contract/integration suites
- native platform/install/update/uninstall lanes
- database upgrade and restore drill
- security/static/dependency/license/secret scans
- selected live provider smokes
- end-to-end acceptance scenarios
- signed artifact verification from a clean machine

Coverage percentage is a signal, not the release gate. Boundary and failure-path evidence is the gate.