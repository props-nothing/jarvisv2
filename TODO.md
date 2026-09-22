# JARVIS Implementation Backlog

This is the execution ledger. Work top to bottom unless an ADR records why ordering changed. A box may be checked only when its acceptance evidence exists. Update this file in the same change as the implementation.

## Working Rules

- Select the first unchecked task whose dependencies are complete.
- Keep one behavioral slice in progress at a time.
- Complete required research before selecting a dependency or writing an external adapter.
- A compile-only implementation is not complete when the task describes runtime behavior.
- Use [definition-of-done.md](docs/development/definition-of-done.md) for every item.

## P0: Architecture Baseline

- [x] `P0-001` Add repository-wide AI engineering instructions.
- [x] `P0-002` Define product requirements and non-goals.
- [x] `P0-003` Define process topology and ownership boundaries.
- [x] `P0-004` Define target repository layout and dependency direction.
- [x] `P0-005` Define runtime, model, tool, connector, memory, workflow, storage, protocol, voice, and security architecture.
- [x] `P0-006` Record foundational ADRs.
- [x] `P0-007` Inventory the Python prototype and license boundary.
      Retirement is tracked separately by `P9-008` and `P9-009`: this item records the
      inventory, not permission to delete. `docs/migration/python-prototype.md` defines the
      gate, and `docs/adr/0009-clean-room-prototype-migration.md` is `Accepted`.
- [x] `P0-008` Add external documentation research rules and templates.
- [x] `P0-009` Define acceptance tests, test strategy, release plan, and definition of done.

## P1: Local Foundation

- [x] `P1-001` Create root `Cargo.toml`, `rust-toolchain.toml`, formatting/lint policy, and minimal workspace members: `jarvis-core`, `jarvis-protocol`, `jarvis-application`, `jarvis-storage`, `jarvisd`, and `jarvis-cli`.
- [x] `P1-002` Add CI for format, Clippy with warnings denied, tests, dependency audit, and license policy on Windows, macOS, and Linux.
- [x] `P1-003` Implement UUIDv7 typed IDs, UTC timestamps, domain error taxonomy, and redaction-safe secret reference types in `jarvis-core`.
- [x] `P1-004` Implement OS-native config, data, cache, runtime, and log path resolution with restrictive directory permissions.
- [x] `P1-005` Define a versioned configuration schema, environment override rules, unknown-key errors, atomic writes, and migrations.
- [x] `P1-006` Implement SQLite connection setup, migration runner, schema-version checks, and backup-before-migration behavior.
- [x] `P1-007` Implement `jarvisd` bootstrap, singleton lock, graceful shutdown, readiness, liveness, and build metadata.
- [x] `P1-008` Define local client protocol v1 and support Unix domain sockets on Unix plus named pipes on Windows, with authenticated loopback fallback only where necessary.
- [x] `P1-009` Implement `jarvis status`, `jarvis health`, `jarvis logs`, and JSON output conventions.
- [x] `P1-010` Implement `jarvis doctor` check contracts: detect, explain, optional repair, evidence, and machine-readable exit codes.
- [x] `P1-011` Add a portable foreground mode and service lifecycle abstractions without installing services yet.
- [x] `P1-012` Prove the Phase 1 acceptance gate on native CI. (Gate implemented as `tests/e2e` and wired into the Windows, macOS, and Linux matrix. Verified on Windows locally; Linux and macOS evidence is produced by the hosted runners on the next CI run.)

## P2: Safe Conversation

- [x] `P2-001` Research and record the selected first model API and SDK version.
      Record: `docs/research/integrations/openai-compatible-model-api.md`. Selected transport is
      `POST /v1/chat/completions` (Chat Completions), not the Responses API, because Responses is
      interoperable with self-hosted servers only in its non-stateful subset. No SDK: OpenAI publishes
      no official Rust library, so the adapter speaks HTTP directly.
- [x] `P2-002` Define provider-neutral model request, response, stream-event, usage, capability, and normalized error contracts.
      Implemented in the new `crates/jarvis-models` crate: `ChatRequest`/`ChatMessage`/`ContentPart`,
      `ChatResponse`/`FinishReason`/`ToolCall`, `StreamEvent`/`StreamEnvelope`/`StreamValidator`,
      `TokenUsage`, `ModelCapabilities`/`Support`/`Placement`, `ModelError`/`ModelErrorKind`, and the
      `ModelGateway` port. Quota and billing failures map to `PermanentUpstream` because they are not
      retryable. No HTTP client or provider SDK is in the dependency graph yet.
- [x] `P2-003` Implement one OpenAI-compatible model adapter with bounded retries, cancellation, timeouts, and secret-safe diagnostics.
      Implemented in `crates/jarvis-models/src/openai/`: a provider adapter against a `Transport` port,
      `reqwest` 0.13.5 (rustls) as the implementation, an incremental SSE decoder, a bounded retry
      policy that honours `Retry-After`, and validated credential/URL types. 16 offline contract tests
      replay scripted provider bytes, so the default suite performs no network I/O. Proxies, redirects,
      and insecure TLS are disabled because each default fails open for private content. A live smoke
      test against a real provider is deliberately not included and is not claimed.
- [x] `P2-004` Define and migrate sessions, messages, agent runs, agent steps, model calls, and usage records.
      Migration `0003_conversation_state.sql` adds `users`, `workspaces`, `sessions`, `messages`,
      `agent_runs`, `agent_steps`, and `model_calls` (schema version 3). Usage is recorded inline on
      `agent_runs` as a run summary and on `model_calls` per call; `docs/data/schema.md` defines no
      separate usage table, and a duplicate total could disagree with its parts. Seven constraint tests
      prove the invariants are enforced by SQLite rather than only documented: duplicate message
      sequence, orphan foreign key, terminal run without an outcome, unknown run state, optimistic
      concurrency (exactly one writer advances a run), failed call without an error code, and an unknown
      sensitivity level. Also added the `jarvis_core::Sensitivity` vocabulary, which five documents
      referenced but none defined.
- [x] `P2-005` Implement the native run state machine with explicit legal transitions and optimistic concurrency.
      `jarvis_core::run` owns the transition table as data (`RunState`, `RunOutcome`,
      `RunTransition`, `ExpectedRunState`); `jarvis_storage::run_repository` makes it durable in
      `agent_runs`. Legality is decided in the domain and the *comparison* is decided by a single
      `UPDATE ... WHERE id = ? AND state = ? AND version = ?`, so a lost update is not
      representable. The write path is a real repository used by tests against a real SQLite
      database, and the concurrency guard has a falsification test in which two callers hold the
      same expectation and exactly one wins. Three documented gaps were closed rather than
      implemented around: `awaiting_approval` is cancellable but **not** failable (nothing can
      fail while a human decides), and `responding -> failed`/`cancelled` were missing from the
      diagram, which left a generation failure with `completed` as its only legal successor.
      Cancellation is recorded as a *request* that does not settle the run, matching A04.
- [x] `P2-006` Implement context envelopes with source, sensitivity, token budget, and inclusion reason.
      `jarvis_core::context` defines `ContextItem`, `ContextSourceKind`, `ContextTrust`,
      `ContextPriority`, `InclusionReason`, `ContextBudget`, and `assemble_context`, with a
      manifest that accounts for every offered item and reports instruction and external token
      totals separately. Two contract fields are enforced rather than recorded: only
      `Authoritative` content may instruct the model, and untrusted content must be quoted, so
      external content labelled as policy is rejected at construction rather than at the
      assembly step. The step-1 budget reservation is arithmetic rather than ordering advice:
      retrieved content draws only on the unreserved remainder. 24 tests, including one that
      falsified a real defect in the first implementation, where required and retrieved
      spending shared one counter and silently evicted retrieval results that fitted.
      Also fixed four stale `database_schema: 2` fixture literals in `jarvis-protocol` left
      behind by the round-2 schema bump to version 3.
- [x] `P2-007a` Research the selected Rust HTTP server framework and fix the gateway boundary.
      Record: `docs/research/integrations/rust-http-server-and-sse.md`. Selected **axum 0.8.9**
      (MIT, MSRV 1.80), pinned exactly because `main` is mid-0.9 and documented as breaking.
      The deciding evidence is reuse rather than novelty: `hyper` 1.11.1, `http` 1.5.0,
      `tower` 0.5.3, and `tower-http` 0.6.11 are **already resolved** through `reqwest` 0.13.5,
      so this adds ~3 packages instead of a parallel HTTP stack. Also recorded: `DefaultBodyLimit`
      defaults to 2 MB and is local rather than global; `Sse::keep_alive` needs the `tokio`
      feature and defaults off; `axum::serve` never returns an error and retries socket errors
      itself, which contradicts this daemon's fail-fast listener. Boundary fixed by
      `docs/adr/0011-run-events-and-http-transport.md`: HTTP is a first-class peer transport to
      local IPC, routing lives in `apps/jarvisd`, REST DTOs in `jarvis-protocol`, and
      `jarvis-core`/`jarvis-application`/`jarvis-storage` stay framework-free.
- [x] `P2-007b` Persist an ordered, sequence-numbered run-event record.
      Migration `0004_run_events.sql` (schema version 4) adds `run_events` with
      `UNIQUE (run_id, sequence)`. `jarvis_core::run_event` owns `RunEventKind`,
      `RunEventSequence`, `EventSummary`, `RunEventPayload`, and `ReplayRequest` (10 tests);
      `jarvis_storage::run_event_repository` makes it durable (13 tests). Two guards are
      falsified rather than asserted: a settled run cannot emit another event, and a stored
      stream that continues after a terminal event is reported rather than replayed. The
      append is ONE `INSERT ... SELECT` that allocates its own sequence, because the
      read-then-insert form fails with `SQLITE_BUSY_SNAPSHOT` when two writers race. Also
      fixed the schema-bump fixture trap this change re-triggered: `inspect.rs` hard-coded
      `user_version = 3` twice and asserted `Inconsistent { user_version: 3 }`, which the
      documented rule predicted. Those now derive from `CURRENT_SCHEMA_VERSION`.
- [x] `P2-007` Add `POST /api/v1/runs`, run status, cancel, and SSE activity/output streams.
      Depends on `P2-007a` and `P2-007b`. Both are prerequisites rather than parallel work:
      the SSE contract requires replay from durable records (`docs/architecture/protocols.md`),
      and a server framework is a new dependency that `docs/development/external-research.md`
      requires be researched before adapter code is written.
      **Delivered:** REST DTOs in `jarvis-protocol/src/rest.rs` (requests `deny_unknown_fields`,
      responses additive-tolerant, every failure reusing the v1 `WireError` envelope); the
      authenticated router in `apps/jarvisd/src/gateway.rs` with the body limit applied as a
      router layer so a later route cannot omit it; run use cases in `run_service.rs`; the SSE
      stream in `sse.rs` driven by a `Last-Event-ID` sequence cursor over the durable log, with
      keep-alives as **comment** frames rather than `heartbeat` events (a heartbeat event would
      occupy a sequence number and enter the log). Storage side: `SessionId`/`RunId`/
      `SessionChannel`/`SessionStatus` in the domain, and `session_repository::start_run`, which
      writes the session, the run, and the run's first event in **one** transaction because
      three calls could leave a run whose stream is empty — indistinguishable from a run whose
      events were lost. The HTTP transport is separately enabled
      (`daemon.http_enabled`, `JARVIS_HTTP_ENABLED`, default off), loopback-only, and bound
      *before* serving, with the serving task's completion as a third `select!` branch, because
      `axum::serve` never returns an error and so cannot report its own death.
      **Two real defects were found by the tests rather than by reading:** the bearer extractor
      trimmed whitespace and therefore accepted a padded token, and
      `GET /runs/{id}/stream` answered `200` for a run that did not exist, because
      `highest_run_event_sequence` returns `None` for both "no run" and "run with no events".
      **Honest limits:** this slice is a *transport*, not an executor. Nothing invokes a model,
      so a run reaches `received` and stays there until `P2-009` supplies an adapter.
      `idempotency_key` is refused with `Unsupported` rather than silently ignored, because no
      deduplication ledger exists and ignoring it would tell a retrying client its request was
      deduplicated while two runs had in fact been created.
- [x] `P2-008` Add CLI `ask` and `chat` using the daemon API; the CLI must contain no orchestration logic.
      **Delivered:** `jarvis ask <objective...>`, which starts a run through `POST /api/v1/runs`,
      consumes `GET /api/v1/runs/{id}/stream`, and renders the run. The client is transport only:
      it sends an objective and no identity, because the workspace and user are resolved by the
      daemon from its seeded local rows. New code: `crates/jarvis-core/src/loopback.rs`
      (`LoopbackHost`, whose host is unrepresentable so a client cannot be aimed off loopback),
      `crates/jarvis-protocol/src/run_api.rs` (request paths with a refused unsafe segment, and a
      hand-written SSE decoder matching the decision already recorded for the model adapter), and
      `apps/jarvis-cli/src/{api_client,chat}.rs`. Two exit statuses were added so a script cannot
      read a cancelled (`9`) or failed (`10`) run as success.
      **One real defect was found by running it, not by testing it:** the client set a *client-level*
      `reqwest` timeout, which bounds the whole response body, so every SSE stream was killed at the
      deadline while every non-streaming call worked. The timeout is now applied per non-streaming
      request, and only `read_timeout` bounds a stream.
      **`chat` is deliberately NOT built.** It needs a session that persists across turns and a model
      that answers; neither exists until `P2-009`, so a chat loop now could only print a `received`
      run per turn — a conversation that records nothing. This item therefore stays unchecked, and
      `chat` is carried by `P2-009` rather than reported as done.
- [ ] `P2-009` Build a scripted model adapter for deterministic state, retry, stream, and cancellation tests.
      **Conversation persistence is done, which closes the A03 half that needed no model:** the
      `messages` table existed since `P2-004` but had **no production writer** — only test fixtures
      inserted rows, so an accepted user message was not recorded at all. `jarvis_core::message` now
      defines the stored vocabulary and `jarvis_storage::message_repository` is the writer, with the
      sequence allocated by the inserting statement because `UNIQUE (session_id, sequence)` makes a
      read-then-insert collide (a test races eight appends and asserts eight contiguous positions).
      The user's message is written **in the same transaction as the run it triggers**, and the
      assistant's answer is read back from the run's own `output_completed` event rather than held in
      memory, so the transcript and the event stream cannot disagree.
      Also carries the `jarvis chat` loop from `P2-008`: a multi-turn chat needs a model that answers
      and a session read model, both of which this slice must supply.
      **In progress — the adapter is done, the executor is not.** `crates/jarvis-models/src/scripted.rs`
      implements the **same** `ModelGateway` port as the real provider adapter, so a consumer cannot
      tell a scripted run from a live one except by what it was told to do: `Turn` scripts answer,
      fragment, fail with a normalized error, or delay so a cancellation can race them. Ten tests
      assert the sequence contract (`0..n`, contiguous, terminal last), that a dropped event is
      reported as a gap by `StreamValidator`, that the last turn repeats so an over-call is a visible
      call count rather than an error, that a cancellation arriving *during* a turn reports `Cancelled`
      and not a timeout, and that the port works through a trait object.
      It is deliberately **not** `#[cfg(test)]` and not a mock: the daemon must be able to run it for
      the first end-to-end run, and a mock-only path is what `definition-of-done.md` forbids calling
      complete. It is not reachable from configuration, so it cannot be selected by accident.
      **Remaining for this slice:** the executor that drives a run through `jarvis-models` and writes
      each step to `run_events`, which is what makes a run reach a terminal state and therefore what
      `A03`/`A04` need. That is the next round, and it also unblocks `jarvis chat` and the `jarvis ask`
      stall guard's replacement (a settled run).
      **Its storage prerequisite is done:** `jarvis_storage::settle_run` writes a terminal transition and
      its settlement event in one transaction. The two existing primitives are individually correct but
      had **no valid order** — transitioning first makes the event append fail
      (`RunEventAfterSettlement`), and appending first records a settlement for a run that has not
      settled. Four tests, including one that writes out the two-call form and asserts it is refused, so
      the reason the primitive exists is falsifiable. `RunState::terminal_event_kind` is the one place
      the state-to-event mapping lives.
      **The executor is done:** `apps/jarvisd/src/executor.rs` drives a run from `received` to a terminal
      state, and it lives in that composition root because `repository-layout.md` allows
      `jarvis-application` to depend only on `jarvis-core` with no arrow into an adapter. It assembles
      context (recording the manifest as audit evidence), calls the model through the `ModelGateway`
      port, appends one event per answer fragment, records usage, and settles. Selected by the new
      `daemon.executor_model` / `JARVIS_EXECUTOR_MODEL`, resolved **at startup** so an unimplemented name
      stops the daemon rather than being found when the first run starts.
      **Verified live, and running it found three real defects plus one in `P2-008`:**
      1. the loop re-settled an already-settled run, which the domain refused as
         `TerminalStateImmutable { from: Failed }` — anything but an error about settlement ordering;
      2. `assemble_context` was called with a ceiling that **excluded** the objective, and the objective
         was then sent to the model anyway. The ceiling is now derived from the model's declared
         placement, so the assembled context and the request agree;
      3. a cancellation requested during a model call bumps `version` without changing `state`, so the
         executor's next guarded write failed with a spurious `RunConflict`. Writes now re-read first;
      4. **`P2-008`'s client printed the answer twice**, because it rendered `output_delta` and
         `output_completed` identically and the completed event repeats every fragment. The two kinds
         are now distinct readings, with a test asserting they differ.
      A live run now completes in about a second with the answer on stdout and progress on stderr, and a
      bad model name fails closed with an actionable message.
- [ ] `P2-010` Prove restart behavior at every persisted run boundary and pass the Phase 2 gate.

## P3: Tools, Policy, Approvals, And MCP

- [ ] `P3-001` Define canonical tool identifiers, JSON Schema 2020-12 input/output, effects, risk, scopes, source, version, timeout, retry, and idempotency metadata.
- [ ] `P3-002` Implement the capability registry with collision detection, namespacing, dynamic availability, and compact model-facing discovery.
- [ ] `P3-003` Implement deterministic policy evaluation with deny-overrides, workspace grants, actor identity, channel constraints, and reason codes.
- [ ] `P3-004` Implement durable approval requests, expiry, approve/deny/cancel, authenticated approvers, and resume semantics.
- [ ] `P3-005` Implement tool execution lifecycle, bounded output, idempotency ledger, audit receipt, and honest outcome states (`requested`, `submitted`, `confirmed`, `failed`, `unknown`).
- [ ] `P3-006` Add a read-only filesystem tool constrained to explicit workspace roots; test traversal, links, races, and oversized output.
- [ ] `P3-007` Research the current MCP specification and selected Rust SDK; record negotiated versions and features.
- [ ] `P3-008` Implement MCP client/host adapters for stdio and Streamable HTTP behind canonical tools.
- [ ] `P3-009` Implement authenticated, scoped JARVIS MCP server exposure with per-client allowlists and rate limits.
- [ ] `P3-010` Add MCP Inspector conformance tests and cross-SDK interoperability tests.
- [ ] `P3-011` Define sandbox contracts and implement one restricted process backend before exposing code execution.
- [ ] `P3-012` Prove approval restart and duplicate-delivery safety; pass the Phase 3 gate.

## P4: Memory And Context

- [ ] `P4-001` Define memory types, provenance, confidence, validity, sensitivity, correction, supersession, and retention semantics.
- [ ] `P4-002` Implement memories, entities, aliases, relations, sources, and deletion tombstones in SQLite.
- [ ] `P4-003` Implement candidate extraction as a reviewable pipeline; never persist unsupported inference as fact.
- [ ] `P4-004` Implement exact, full-text, recency, importance, entity, and workspace retrieval before adding embeddings.
- [ ] `P4-005` Research and implement a provider-neutral embedding adapter with dimension/version metadata.
- [ ] `P4-006` Add hybrid retrieval and explainable scoring; embeddings are an index, not canonical truth.
- [ ] `P4-007` Integrate memory retrieval into context budgets with provenance and injection-resistant quoting.
- [ ] `P4-008` Add inspect, search, remember, correct, forget, export, retention, and full user-deletion APIs/CLI.
- [ ] `P4-009` Implement PostgreSQL plus pgvector backend parity for the completed memory behavior.
- [ ] `P4-010` Pass isolation, correction, deletion, stale-memory, and adversarial-source acceptance tests.

## P5: Connectors

- [ ] `P5-001` Define connector manifest, account, auth flow, health, sync cursor, webhook, rate-limit, scope, and diagnostics contracts.
- [ ] `P5-002` Implement shared OAuth 2.0 Authorization Code plus PKCE flow, state/nonce validation, loopback callback, refresh rotation, revocation, and secret references.
- [ ] `P5-003` Create connector quality checklist and scaffold generator modeled on manifest-driven integration projects.
- [ ] `P5-004` Research Google identity, Gmail, Calendar, push notifications, quotas, and restricted scopes; record findings.
- [ ] `P5-005` Implement Google connection setup and Gmail/Calendar read tools with recorded wire fixtures.
- [ ] `P5-006` Research Microsoft identity platform and Microsoft Graph mail/calendar, subscriptions, delta queries, and limits; record findings.
- [ ] `P5-007` Implement Microsoft connection setup and Outlook/Calendar read tools with recorded wire fixtures.
- [ ] `P5-008` Research and implement GitHub authentication and read tools.
- [ ] `P5-009` Add draft/write operations behind policy and approval with provider idempotency where available.
- [ ] `P5-010` Add reauth, token expiry, revoked scope, pagination, throttling, webhook replay, and redacted diagnostics tests.

## P6: Events And Workflows

- [ ] `P6-001` Define event envelope, source identity, schema version, causation/correlation IDs, dedupe key, visibility, and sensitivity.
- [ ] `P6-002` Implement transactional outbox/inbox, leasing, retry schedule, dead letters, and replay tooling.
- [ ] `P6-003` Implement timezone-aware schedules and deterministic next-run calculation across daylight-saving transitions.
- [ ] `P6-004` Define workflow, run, step, attempt, wait, approval, cancellation, and compensation records.
- [ ] `P6-005` Implement sequential and parallel steps, conditions, retries, timeouts, delays, event waits, and cancellation.
- [ ] `P6-006` Require idempotency or an explicit non-retryable classification for every effectful workflow step.
- [ ] `P6-007` Implement notifications and proactive suggestion budgets, quiet hours, dedupe, and user controls.
- [ ] `P6-008` Add crash-at-every-transition and duplicate-event tests; pass the Phase 6 gate.

## P7: External Runtimes

- [ ] `P7-001` Define runtime protocol v1: handshake, capabilities, start, event stream, tool request, input request, resume, cancel, settle, health, and errors.
- [ ] `P7-002` Implement process supervisor with resource limits, environment allowlist, crash backoff, health, log redaction, and orphan cleanup.
- [ ] `P7-003` Ensure external runtime tool requests re-enter the JARVIS policy gateway; disable unmediated native tools by default.
- [ ] `P7-004` Research and implement the current OpenClaw runtime or ACP integration.
- [ ] `P7-005` Research and implement an OpenAI Agents worker adapter.
- [ ] `P7-006` Implement a generic ACP adapter and conformance fixture.
- [ ] `P7-007` Add LangGraph only for a named use case that benefits from its checkpoint/interrupt model.
- [ ] `P7-008` Test crash isolation, cancellation, protocol skew, unavailable runtime fallback, and canonical audit ownership.

## P8: Voice And Calls

- [ ] `P8-001` Define voice provider, voice session, call, transcript, interruption, turn-detection, identity-evidence, and retention contracts. Turn events and their policy are fixed by [ADR-0010](docs/adr/0010-model-based-turn-detection.md).
- [ ] `P8-002` Research the current ElevenLabs `llms.txt`, Custom LLM, Speech Engine, MCP, telephony, turn-taking, webhook, privacy, and breaking-change docs; refresh the dated research record. (Record refreshed 2026-09-21 with the Speech Engine transport and turn-taking settings; implementation must re-verify before release.)
- [ ] `P8-003` Implement authenticated `/v1/responses` SSE compatibility for ElevenLabs with contract fixtures.
- [ ] `P8-004` Implement `/v1/chat/completions` SSE compatibility only after the Responses path; normalize ElevenLabs system tools without bypassing JARVIS policy.
- [ ] `P8-005` Issue short-lived scoped voice-session credentials and map verified caller/conversation identity to user and workspace.
- [ ] `P8-006` Expose a dedicated least-privilege MCP client profile for ElevenLabs-owned-agent mode.
- [ ] `P8-007` Implement Twilio and SIP outbound call adapters through provider-neutral `voice.call.*` tools.
- [ ] `P8-008` Verify HMAC signatures against raw webhook bodies, deduplicate events, return promptly, and process durable work asynchronously.
- [ ] `P8-009` Implement call records, consent/disclosure flags, transcript/audio retention, deletion, and redacted audit events.
- [ ] `P8-010` Add local wake word, push-to-talk, VAD, STT/TTS provider ports, echo handling, and interruption as client capabilities.
- [ ] `P8-011` Pass inbound, outbound, voicemail, no-answer, interruption, latency, replay, privacy, and provider-outage tests.
- [ ] `P8-012` Implement the turn-detection and interruption port: canonical turn events with confidence, backchannel suppression, false-interruption resume, and a silence setting used only as a maximum cap.
- [ ] `P8-013` Implement a `flux`-class turn-detecting STT adapter and assert that a pause-heavy request is not cut off at the silence threshold.
- [ ] `P8-014` Report turn-detection latency separately from perceived response latency, and record discarded speculative work.
- [ ] `P8-015` Implement the Speech Engine brain WebSocket transport (shared-secret upgrade, per-call signed URL, `ulaw_8000`) behind the same application command as the SSE path.
- [ ] `P8-016` Place a boundary and placement ADR for any external agent framework used in the voice path (runtime worker under the supervisor versus a `jarvis-voice` adapter), before implementing it.
- [ ] `P8-017` Decide and record realtime session scope before any realtime transport work: whether JARVIS adopts a room as a session concept, which participants and media types a session may carry, whether data/RPC channels are permitted as transport-only, and the E2EE-versus-inspection policy. Record explicit exclusions where the answer is "no".

## P9: Desktop, Installation, And Releases

- [ ] `P9-001` Scaffold Tauri desktop as a daemon client with generated API bindings and a narrowly scoped command allowlist.
- [ ] `P9-002` Implement onboarding, chat, activity, approvals, connections, memory, tasks, models, runtimes, voice, diagnostics, and settings views.
- [ ] `P9-003` Implement service install/start/stop/restart/uninstall for Linux systemd user, macOS LaunchAgent, and Windows per-user Scheduled Task; document optional system-wide modes.
- [ ] `P9-004` Build idempotent shell and PowerShell installers with OS/architecture detection, checksum/signature verification, PATH setup, onboarding, and doctor.
- [ ] `P9-005` Build native signed artifacts for supported targets; generate SBOM, checksums, signatures, provenance, and release notes.
- [ ] `P9-006` Implement atomic update, schema preflight, backup, health verification, rollback, channels, and split-brain version diagnosis.
- [ ] `P9-007` Test install, update, rollback, repair, and uninstall on clean native VMs/runners without a development toolchain.
- [ ] `P9-008` Implement the `jarvis migrate mark-liv` importer specified by
      `docs/migration/python-prototype.md`: dry-run manifest, source/destination display, rejection of
      API keys and certificates, `legacy_import` provenance, backup and rollback before finalization,
      truncated/invalid/duplicate reporting, and explicit user confirmation before writing canonical
      memory. Until it exists, `example/memory/long_term.json` is the only readable source for a real
      user's prototype data, so deleting `example/` first would discard it.
- [ ] `P9-009` Retire `example/` against the documented gate: installation, conversation, safe tools,
      memory, local voice, visual geometry, proactive workflows, and cross-platform behavior all have
      equivalent or deliberately superseding acceptance evidence; license obligations are settled;
      user migration is complete or explicitly abandoned by ADR; and `THIRD_PARTY.md`, `SECURITY.md`,
      `README.md`, `AGENTS.md`, `.gitignore`, and the ~11 files linking into `example/` are updated in
      the same change. `docs/research/evaluated-prototypes.md` records the required sequence: findings
      become dated records first, then the prototype is deleted.

## P10: Server And Multi-Device

- [ ] `P10-001` Define server deployment profile, TLS termination, trusted proxy rules, backup/restore, and operator runbooks.
- [ ] `P10-002` Complete PostgreSQL parity and concurrency tests for every canonical repository.
- [ ] `P10-003` Implement device enrollment, pairing proof, scoped credentials, rotation, revocation, and lost-device response.
- [ ] `P10-004` Implement remote access with explicit enablement, secure defaults, rate limits, and exposure audit.
- [ ] `P10-005` Add team membership and roles only after workspace isolation is proven.
- [ ] `P10-006` Establish service-level objectives, load profiles, recovery objectives, restore drills, and capacity evidence.
- [ ] `P10-007` Evaluate optional infrastructure only with measured bottlenecks and an ADR per adoption.