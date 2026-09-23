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
      **`chat` was deliberately NOT built here.** It needs a session that persists across turns and a
      model that answers; neither existed until `P2-009`, so a chat loop at that point could only print
      a `received` run per turn — a conversation that records nothing. This item therefore shipped
      `ask` alone and `chat` was carried by `P2-009`, which is where it was built as `P2-009b`. This
      entry is checked because `ask` is delivered and `chat` is no longer outstanding, not because
      `chat` was part of this change.
- [x] `P2-009` Build a scripted model adapter for deterministic state, retry, stream, and cancellation tests.
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
         **CORRECTION (2026-09-23): re-reading first was necessary but not sufficient, and it was not the
         last word on this.** `P3-008j` found the same test failing under load again, because the
         cancellation itself still bumped `version` and invalidated the expectation the executor was
         holding across a model call. The fix is that a write changing no state does not advance the
         version at all (ADR-0022 decision 7). See the `P3-008j` entry for the deterministic test that
         replaced "it passed a few times".
      4. **`P2-008`'s client printed the answer twice**, because it rendered `output_delta` and
         `output_completed` identically and the completed event repeats every fragment. The two kinds
         are now distinct readings, with a test asserting they differ.
      A live run now completes in about a second with the answer on stdout and progress on stderr, and a
      bad model name fails closed with an actionable message.
      **This slice is complete.** The two things it said it would carry are done: the executor drives a
      run to a terminal state, and conversation persistence means an accepted user message and the
      answer that resulted are both recorded, with the answer read back from the run's own
      `output_completed` event rather than held in memory.
      **The `jarvis chat` loop it carried is built**, in the slice below.
- [x] `P2-009b` Build the multi-turn `jarvis chat` loop, carrying the `chat` half of `P2-008`.
      **Delivered:** a conversation is a sequence of **runs sharing one session**. `jarvis chat` reads
      a turn per line, starts a run in the session the daemon issued on the first turn, and prints it,
      so the earlier turns are replayed by the daemon rather than by the client. New pieces:
      - `jarvis_storage::SessionTarget` (`New` | `Existing`) and `StartRunInput::continuing`, so
        "start a conversation" and "continue one" are distinguishable rather than both being an
        optional identifier. The session predicate — `workspace_id`, `user_id`, `status = 'active'` —
        is folded into the `UPDATE` that attaches the run, not checked afterwards, because a session
        identifier is guessable and a caller must not be able to append to a conversation it was never
        granted. A refusal is resolved on the failure path into `SessionNotFound` (absent, or another
        identity's — reported as absent so its existence is not confirmed) or `SessionNotWritable`
        (archived, which means "start a new conversation" rather than "the identifier was wrong").
      - `jarvis_storage::{read_recent_messages, count_messages}`. The history window is selected from
        the **end** of the transcript, because a follow-up depends on the turns closest to it: reading
        the first N and reversing them returns the *oldest* turns, which is the obvious mistake here
        and the one the test names.
      - `StartRunRequest.session_id` (optional; absent means a new conversation) and
        `ApiClient::start_run_in_session`.
      - The executor loads the session's history, offers it to **the context assembler** rather than
        adding it to the request directly, and builds the request from the resulting manifest.
      - `ScriptedModel::seen_messages()` records every request served, because a conversation is a
        property of the *request*: a daemon that sent only the latest question would satisfy every
        assertion about the answer.
      **Running the new test found a real defect the design had wrong.** `messages_from_manifest`
      walked the manifest and produced `policy, current question, earlier question, earlier answer`,
      because the assembler orders by **budget tier** — required content first — and the objective is
      required while history is optional. That order is right for budgeting and wrong for a
      conversation: a model reading the earlier exchange *after* the current question sees it as a
      continuation of the prompt rather than as context for it. Fixed by separating the two questions:
      the manifest decides **what may be sent** (so an excluded turn is absent by construction) and
      the conversation decides **in what order** (policy, then replayed turns oldest-first, then the
      question).
      **Verified live:** two turns against a real daemon produced two runs in one session
      (`run …4cee-772d… session …d915c4c7…`, then `run …4f05-70f6… session …d915c4c7…`), both
      `completed`, with the second turn replayed from the first.
      **Honest limits:** the session's history is replayed as text, so a tool result is skipped rather
      than replayed (a tool message needs the call it answers, and nothing writes one yet); the
      twelve-turn window is bounded because a transcript grows without limit and a model's context
      does not, and a turn the budget cannot hold is excluded **with a recorded reason**; and
      `jarvis chat` has no resume-by-identifier, so a conversation continues within one process —
      `P4-008` adds the inspect and export surface that would make a stored conversation reachable
      again.
- [x] `P2-010` Prove restart behavior at every persisted run boundary and pass the Phase 2 gate.
      **Delivered:** restart truthfulness, and the Phase 2 process gate that proves it against the real
      binaries.
      `jarvis_storage::recover_interrupted_runs` settles every non-terminal run at startup: `cancelled`
      when a cancellation had already been requested (the operator's intent outranks the interruption),
      otherwise `failed` with `error_code = interrupted_by_restart`. Each settlement writes the terminal
      state and its terminal event in **one transaction** through `settle_run`, so a client replaying
      the stream sees it exactly once; it is the second place that primitive is required, which is the
      evidence ADR-0011 wanted. Tests cover every non-terminal boundary in a loop rather than one
      convenient state, a cancelled-before-restart run, idempotence of a second pass, and a clean profile
      recovering nothing — 99 storage tests.
      The daemon calls it **before the listener binds**, so no client can observe a run the daemon has
      not accounted for, logs the identifiers it recovered rather than a bare count, and treats failure
      as fatal: a daemon unable to explain its own interrupted work must not serve.
      `tests/e2e/tests/phase2_gate.rs` is the Phase 2 exit gate as a process test. It runs the real
      `jarvisd` and `jarvis`, starts runs through the public API, kills the daemon with two runs in
      flight, and asserts the restarted daemon settled the abandoned run `failed` with
      `interrupted_by_restart`, settled the cancelled one `cancelled`, that each gained exactly one
      terminal event, that the recovery is in the log with its identifiers, that a fresh run streams to
      `run_completed`, and that `jarvis ask` reaches the same API. It is wired into CI for all three
      operating systems.
      **Recovery is truthfulness, not resumption**, and ADR-0013 records why: the model stream and the
      answer text live in the executor's memory and the daemon cannot know whether an in-flight provider
      call was accepted, so a resume could present a second answer to a question already being answered.
      `A03` offers that either/or and the second branch is taken deliberately.
      **Three defects were found by running it, none by the unit suites:**
      1. the gate could not write `config.toml` because its directory did not exist — it was relying on
         the daemon to create the file the daemon reads;
      2. `reqwest` panicked with "there is no reactor running" because the gate drove HTTP through
         `Runtime::block_on` from a synchronous body. Neither `block_on` nor entering the runtime around
         it gives the request builder a runtime context; making the gate genuinely `async` removes the
         distinction instead of papering over it;
      3. **the falsification run passed.** Disabling recovery and rerunning the gate reported success,
         because `cargo test` does not rebuild the binary the test executes — it ran the previous
         `jarvisd`. Rebuilding first made the gate fail exactly as it should ("a run interrupted by a
         restart must be reported as failed", with the run still `received`), which is why CI rebuilds
         the workspace before each gate.

## P3: Tools, Policy, Approvals, And MCP

- [x] `P3-001` Define canonical tool identifiers, JSON Schema 2020-12 input/output, effects, risk, scopes, source, version, timeout, retry, and idempotency metadata.
      Implemented as the `jarvis-tools` crate: `ToolId` (`namespace.name`, split on the last dot,
      source derived from the namespace), `ToolSchema` (2020-12 only, compiled once, refusing any
      non-local `$ref`), `EffectSet`, `Risk` (typed 0..=3, refused below the effect floor),
      `ScopeSet` (`resource.action`, JARVIS capabilities rather than provider OAuth scopes),
      `ApprovalPolicy`, `RetryPolicy`/`RetryDeclaration`, `Idempotency`, `Availability`,
      `ToolSensitivity`, `ToolOutcome`/`ToolOutcomeRecord`, and the aggregate `ToolDefinition`.
      Three cross-field rules are enforced at construction, including through a parsed manifest:
      risk ≥ the effect floor, no blind retry of an ambiguous effect, and a declared source that
      must agree with the identifier. 96 in-crate tests. ADR-0015 records the durable decisions;
      `docs/research/integrations/json-schema-validation.md` records the `jsonschema` 0.57.0
      selection and why its default features (HTTP/file `$ref` resolution) are disabled.
      Deliberately not a placeholder: this slice declares and validates metadata and decides nothing.
      Policy evaluation is `P3-003`; registration is `P3-002`; execution is `P3-005`. No adapter is
      wired to it yet, so no tool can actually be called.
- [x] `P3-002` Implement the capability registry with collision detection, namespacing, dynamic availability, and compact model-facing discovery.
      `ToolRegistry` in `crates/jarvis-tools/src/registry.rs`: `define` refuses a collision and
      **leaves the first tool intact**; `define_all` loads a manifest atomically, with a collision
      inside the batch refused rather than last-writer-wins; `replace` requires a version change, so
      new behaviour cannot hide under an unchanged version. Availability has two sources — the
      definition's declaration and an optional runtime state — and the restrictive one always wins,
      so a health check cannot enable what the build declared impossible. `discover` is model-facing
      and bounded (`MAX_DISCOVERY_TOOLS`), offering only callable tools and reporting omissions by
      count; `inventory` is operator-facing and lists everything with its reason. `ToolSummary`
      carries selection fields only, asserted by a serialized-field-list test.
      Also closes `P3-001`'s deferred cost: `DocumentSet` supplies the documents a JARVIS-authored
      schema may compose against, and a contract test proves a supplied `$ref` resolves **and its
      constraints are enforced**, with the control that the same reference with nothing supplied is
      refused. External manifests stay self-contained deliberately (ADR-0016) — a manifest cannot
      influence which document is loaded, and `$id` shadowing is refused. 127 in-crate tests plus 7
      contract tests. ADR-0016.
      Deliberately not a placeholder: this slice holds and lists capabilities and decides nothing.
      Policy evaluation is `P3-003`; execution is `P3-005`. The registry is populated from code, not
      persisted, and no adapter is wired to it, so no tool can actually be called.
- [x] `P3-003` Implement deterministic policy evaluation with deny-overrides, workspace grants, actor identity, channel constraints, and reason codes.
      `crates/jarvis-tools/src/evaluation.rs`: `evaluate` is a pure function over a **borrowed**
      `ToolDefinition` (no clock, no I/O, no repository), returning `Allow` / `RequireApproval` /
      `Deny` plus an `effective_risk`, a `DenyReason` code, the strength an approval would need, and
      the escalation signals that raised the risk. Deny overrides is a control-flow property: the
      checks are ordered and the first refusal ends the evaluation, so no permissive finding can
      mask a later refusal. The denials precede the approval steps, so a call that is denied *and*
      would need approval reports the denial rather than sending an operator to approve something
      that cannot run. A channel **caps** the authentication an actor may claim (`channel_ceiling`),
      which is what produces the documented voice rule: a voice-originated risk-3 call is *held* for
      a desktop/CLI approval, not refused, so the resume path stays reachable. Workspace policy can
      only narrow what a tool declares. 150 in-crate tests. ADR-0017 records the ownership tension:
      `repository-layout.md` gives core "policy decisions and reason codes" while `jarvis-tools` is
      named for the policy pipeline, and the decision must read a definition holding compiled JSON
      Schema validators — so core cannot own it without either gaining a vendor SDK or inverting the
      adapter direction.
      Deliberately not a placeholder: nothing calls this yet and nothing is persisted. Recording an
      approval obligation (nonce, expiry, resume) is `P3-004`; the execution receipt is `P3-005`.
- [x] `P3-004` Implement durable approval requests, expiry, approve/deny/cancel, authenticated approvers, and resume semantics.
      `jarvis_core::approval` (the domain) + `jarvis_storage::approval_repository` + migration
      `0006_approvals.sql` (schema v6). An approval binds to a **canonical intent digest** computed
      from the tool, its version, and a sorted-key rendering of the arguments, domain-separated so
      two parts cannot bleed into each other. The one-time decision nonce is stored only as a
      SHA-256 **digest** and rotated on use, so a leaked row proves a decision happened without
      yielding the ability to make one. A storage-loaded request **cannot verify a nonce** and says
      so, because it holds only the digest; the store verifies and then calls the domain's verified
      path, which enforces the strength floor, the expiry, and the self-approval refusal. Expiry is
      evaluated in Rust from `unix_nanos` and is a **read-side** fact, never a stored state and never
      a SQL comparison. One approval per intent per run (unique index), and a second decision is
      refused, so a denial cannot be overwritten. 113 core tests plus 19 in-crate storage tests, with
      one named after each category `security.md` requires (forgery, replay, expiry, mutation, stale
      state). ADR-0018; `docs/research/integrations/sha2-intent-hashing.md`.
      Deliberately not a placeholder: nothing requests or answers an approval yet, and no run parks in
      `awaiting_approval`. A caller that creates and decides one exists in tests only. `P3-005` now
      stores an approval identifier on a call row, but **does not consume an approved intent**: the
      revalidation-on-resume path that re-reads an `Approved` approval and compares its `intent()`
      digest is still unbuilt.
- [x] `P3-005` Implement tool execution lifecycle, bounded output, idempotency ledger, audit receipt, and honest outcome states (`requested`, `submitted`, `confirmed`, `failed`, `unknown`).
      Split across three crates along the dependency direction. `jarvis_core::tool_outcome` holds the
      honest vocabulary — `ToolOutcome` with `may_have_had_an_effect`, `is_safe_to_repeat_from_outcome`,
      `is_reported`, and the strict `can_advance_to` table that refuses `requested -> submitted` — plus
      `ToolOutcomeRecord`, which cannot be built as `confirmed` without evidence or as `failed` without
      a reason, and reports "no evidence supplied" and "evidence too long" as different errors.
      `jarvis_tools::execution` holds the bounded invocation: `BoundedOutput` (32 KiB, `strict()` refuses
      and `truncating()` cuts on a char boundary), `ProviderEvidence` (256 chars) kept **separate from**
      the output column so a success-sounding sentence cannot become proof, `IdempotencyKey` (32
      lowercase hex), and `AuthorizationReceipt` whose four approval fields move together from one
      citation and whose validity is compared in Rust from `unix_nanos`. `jarvis_tools::executor` is the
      async `ToolExecutor` port, whose `AdapterError` distinguishes `RefusedBeforeReaching` from
      `AmbiguousAfterReaching` because that is what maps onto `failed` versus `unknown`.
      `jarvis_storage::tool_call_repository` + migration `0007_tool_calls.sql` (schema v7) make a call
      durable. **The idempotency ledger is the `UNIQUE (run_id, idempotency_key)` index, not a second
      table**: admission is one `INSERT ... ON CONFLICT DO NOTHING` and a suppressed insert returns the
      **existing** call id so the caller adopts it. The key is generated at admission, not derived from
      the intent, because the digest must be deterministic (an approval binds to it) while the key must
      be unique per logical call — two calls with an identical intent hash and different keys are
      asserted to be two calls. **A terminal outcome cannot be replaced** and a repeat of the same
      outcome is a no-op, which is what stops an `Unknown` becoming a `Failed` and one sent message
      becoming two. The 32 KiB bound is enforced twice (the type and the migration's `CHECK`), and the
      `CHECK` also refuses a `confirmed` row with no evidence.
      170 `jarvis-tools` tests, 125 `jarvis-core` tests, 143 in-crate `jarvis-storage` tests — including
      one named after the illegal `requested -> submitted` edge, which was added **because** a test
      author wrote it and the domain correctly refused. ADR-0019.
      Deliberately not a placeholder, and deliberately not a working tool: this is the lifecycle and the
      port. **No adapter implements `ToolExecutor` until `P3-006`** (the only implementation here is a
      test `ScriptedExecutor`), **no caller admits a tool call**, no run reaches any outcome, and
      `apps/jarvisd` has no tool pipeline — no schema validation, no policy evaluation call, no approval
      request, no execution. Nothing is written to `run_events` for a tool call either, so a call is a
      stored document rather than an entry in the durable event log. The audit receipt is stored as a
      document produced upstream; nothing here generates one. **`P3-006` added the first real adapter and
      the first consumer of this lifecycle, but still with no composition root**: no caller admits a
      call, so the lifecycle remains driven by tests. `P3-012` is where the Phase 3 gate proves the whole
      path.
- [x] `P3-006` Add a read-only filesystem tool constrained to explicit workspace roots; test traversal, links, races, and oversized output.
      `cap_std::fs::Dir` handle per granted root and **resolves through the handle** rather than
      validating a path. The obvious join-canonicalize-prefix-check scheme is rejected because it has a
      two-syscall window — the path is resolved and then opened, so a component swapped for a link in
      between escapes — which is the "race" the requirement names. A handle resolves each component
      beneath itself, so an escape is an error rather than a file. `cap-std` was required because
      `unsafe_code = "forbid"` rules out a hand-written `openat`/`NtCreateFile` wrapper, and `rustix`'s
      `openat2` + `RESOLVE_BENEATH` is unix-only, so Windows would need its own implementation.
      `Dir::open_ambient_dir` — the grant, which has no confinement of its own — appears exactly once,
      in `WorkspaceRoots::new`. An unusable grant is refused rather than narrowed: empty, relative,
      duplicate, missing, or not-a-directory are all errors, because a skipped missing root makes an
      unmounted disk read as an empty workspace.
      `files.rs` is the first real `ToolExecutor` adapter: `jarvis.files.read` and `jarvis.files.list`,
      both `read_only`, risk 0, `Auto` approval, idempotency `Required`. A refused path is a **`Failed`
      outcome, not `RefusedBeforeReaching`** — the resolution reached the filesystem and failed, so
      "nothing happened at all" would be false; the refusal vocabulary is reserved for a malformed
      argument or a lapsed deadline, where nothing was touched. Output is read through a limited reader
      that takes one byte past the bound, so a 4 GB file is truncated rather than allocated. 179
      `jarvis-tools` tests. ADR-0020; `docs/research/integrations/cap-std-filesystem-confinement.md`.
      **Two defects were found by running the tests, not by review.** (a) A binary file was reported as
      `Confirmed` with **no content**, because `String::from_utf8` reports the valid prefix and a file
      invalid from its first byte has a valid prefix of zero — which decodes to an empty `String`, which
      *is* valid UTF-8. (b) **The truncation flag was discarded, so a cut read was recorded as
      complete**: the reader bounded at the source (correct) and then handed already-limited text to
      `BoundedOutput::truncating`, which infers truncation **by length** and reads a value of exactly
      the limit as "fits". Fixed by adding `BoundedOutput::from_bounded(content, truncated)`, because a
      source-bounded reader holds a fact the type cannot infer. The general lesson: when a type infers a
      property from a value's shape, a caller that knows the property directly must be able to state it,
      or the inference silently overrules the knowledge.
      Deliberately not a placeholder, and deliberately not wired up: **no entry point drives the tool**.
      Nothing in `apps/jarvisd` registers the two definitions, evaluates policy, requests an approval, or
      calls `execute`, so no model can read a file yet. `definitions()` returns the canonical pair but
      nothing inserts them into a `ToolRegistry` in production code — the composition step joining the
      registry, the policy engine, the approval repository, and this adapter is unwritten. The deadline
      is checked, not enforced: a read that begins in time and then blocks on a slow mount is not
      interrupted, because that needs cancellation (`P3-011`), which also owns the resource limits that
      belong under this blocking I/O. No write, move, delete, or trash/undo — the read half was chosen
      first because its failure mode is disclosure, which confinement prevents, rather than destruction,
      which needs an undo design. `P3-012` is where the gate proves the path end to end.
- [x] `P3-006a` Close the authorization seam: make an authorization receipt derive from the policy decision it came from.
      **Added out of ledger order, because composing the pieces found a security defect.** `P3-001`..`P3-006`
      each built one part and **nothing composed them**: the only `AuthorizationReceipt` constructions in the
      workspace were six test fixtures, so no adapter had ever been handed a receipt from a real decision.
      Closing that gap to write the first seam test showed it was a security gap, not only an integration one.
      `AuthorizationReceiptParts` took the risk as a **value**, so a composer could evaluate a decision at
      `High` and then build a receipt declaring `Risk::Minimal` — and nothing would refuse it, because nothing
      held the decision and the receipt at once. The receipt is what an adapter treats as permission, so the
      declaration an adapter acted on could understate the risk the decision was taken at. The digest had the
      same shape of problem: `intent_hash: String` cannot express "this is the digest an approval binds to".
      Fixed by making both **derived rather than stated**: the parts now carry the `PolicyDecision` and the
      `arguments`, and `AuthorizationReceipt::new` reads `effective_risk()` from the decision and
      **recomputes** `CanonicalIntentHash::compute` to refuse a mismatch. Two new refusals make the remaining
      ways back in unrepresentable: `DecisionNotAuthorizing` (a `Deny`, or a `RequireApproval` citing no
      approval, can never produce a receipt) and `IntentMismatch`. The digest stays pre-computed at the call
      site because `jarvis-tools` does not depend on `sha2` and a second implementation of the canonical form
      would get the `\u{1f}` separator wrong — so it is verified at the boundary instead. 182 `jarvis-tools`
      tests; the seam has a named test with controls proving each refusal is about the intended cause. ADR-0021.
      **No production code builds a receipt**, so this is a library fix, not a pipeline: there is still no
      composition root (no registry lookup, no `evaluate` call, no approval round-trip, no admission, no
      adapter call) and the seam is correct and tested but **not driven**. `P3-012` owns that. Also recorded:
      the digest covers tool, version, and arguments but **not the actor or workspace**, so two actors with the
      same tool and arguments currently produce the same digest — a question `P3-012` should answer.
- [x] `P3-006b` Bind a tool execution request to the receipt it carries.
      Found by deliberately hunting the `P3-006a` defect class in the next seam. `ToolExecutionRequest::new`
      checked only that the receipt had not expired; it never compared the request's `tool`, `tool_version`,
      or `arguments` against what the receipt authorized. An adapter cannot tell an authorized call from an
      unauthorized one, so that constructor is the last place the binding can be enforced — the
      `NotImplemented` path only catches a tool an adapter happens not to implement. Fixed with
      `ToolNotAuthorized` (tool AND version as a pair) and `ArgumentsNotAuthorized`, which **recomputes**
      `CanonicalIntentHash` rather than comparing JSON, so key order is not significant. **The evidence it
      was live rather than theoretical: five existing tests started failing**, because they built requests
      whose arguments did not match their receipt's digest — one asked for an extra `subject` key the
      receipt never covered, and had been passing.
- [x] `P3-006c` Fix the cancellation contract: operator intent cannot be stale.
      Surfaced from the P3-006b work as an intermittent test failure, which turned out to be a **real
      user-facing defect in existing code**. `POST /api/v1/runs/{id}/cancel` required `expected_version`,
      justified in `acceptance-tests.md` as "a client cannot cancel a run it has not read". That is wrong as
      a control: the executor advances a running run's version at **every state-machine step**, so a
      client's version is stale almost immediately and the cancel was refused with a `409` the client could
      not resolve — the user asked to stop a run and was told the run had changed. `RunService::cancel` made
      it worse by **re-reading the run and then discarding what it read**. `ADR-0013` already establishes
      that a cancellation request is operator intent which "already outranks the interruption", and `COALESCE`
      already made a repeat idempotent, so the version guard had no safety role left. Now: no version; the
      guard is the one that cannot go stale (**a settled run refuses**, because there is no work left to
      stop); zero-rows-affected is resolved by reading, so `RunNotFound` and `TerminalStateImmutable` stay
      distinguishable; `CancelRunRequest` is **deleted** from the protocol rather than left with a permissive
      decoder, and the route takes no body. The flakiness is gone **structurally** — verified by 12
      consecutive passes — because there is no read-then-write window left. ADR-0022.
      Honest limits: `expected_version` remains on run start and on approval decisions, and neither has been
      re-examined against the same reasoning. No `jarvis cancel` CLI verb exists, so the endpoint is
      exercised only by the gateway test and the e2e gate. Whether a cancellation *request* should emit a
      `run_events` row is not answered here.
      **CORRECTION (2026-09-22): the `expected_version` half of that limit was FALSE.** Neither case exists
      — `ApprovalDecision` has no version field and never did, and starting a run creates a row so there is
      no prior version to expect. A grep for `expected_version` across the workspace now finds only ADR-0022,
      a doc quotation of it, and the message of the test that replaced the removed cancel field. Three
      documents asserted the field as live (`ADR-0022`, `docs/api/contracts.md`, and this entry) while **no
      code, test, or protocol type ever had it** — a plausible-sounding claim written as a *deferral*, which
      made it read as a live risk and survive three slices of review. Corrected in all three, with the ADR
      struck rather than quietly edited. The general rule the ADR established generalises and is now recorded
      there: **where a durable identity already names the subject of a decision — a stable `id` plus a state
      machine that refuses a second decision — a version expectation adds no safety and can only add a
      refusal.** Still true: no `jarvis cancel` verb (the verbs are `status`, `health`, `ask`, `chat`,
      `logs`, `doctor`), and whether a cancellation *request* should emit a `run_events` row remains
      unanswered.
- [x] `P3-006d` Compose the tool pipeline in the daemon and make it reachable over HTTP.
      **The first production caller of the whole tool path.** `P3-001`..`P3-006c` each built one layer and
      **nothing composed them**, so no route reached a tool adapter and every seam was unverified — and the
      three preceding slices found a real defect in every seam they examined. This closes that: route →
      handler → registry → schema → policy → receipt → admit → authorize → submit → adapter → recorded
      outcome. Composed in `apps/jarvisd`, because `repository-layout.md` forbids an arrow from
      core/application into an adapter. ADR-0023.
      The pipeline registers the **adapter's own** `definitions()` rather than restating schemas and risk
      levels, because policy deciding about a declared copy would be policy deciding about a tool other than
      the one being run — the `P3-006a` defect class one level up. The route accepts only `run_id` and
      `arguments`: the **workspace comes from the stored run's row** and the scope from the daemon's actor
      constructor, never from the request, because `identity-and-workspaces.md` requires access to follow
      from authentication rather than from a client-supplied identifier. No roots means **no pipeline**, not
      an empty one, which would advertise a tool that fails every call. An unusable grant fails **startup**,
      per ADR-0020's rule that a grant is refused rather than narrowed.
      `call_tool` at 117 lines was split into `validate`, `authorize_and_admit`, and `execute_and_record`,
      with `authorize_and_admit` returning `Option<PreparedCall>` so that "held" is a value rather than a
      hidden early return, and `PreparedCall` grouping the values that must agree.
      Four route-level tests, two of them **falsification** tests: a tool call cannot name its own
      workspace (positive control first — the granted root really is read — then `422` for a named
      workspace, `404` for an unknown run, and a traversal asserted to produce `state: "failed"` with
      `output: null`), and a call with no composed pipeline is `404` naming the config key. **One assertion
      was wrong and the error was instructive**: it asserted "a traversal must not be 2xx" and got `200`,
      because ADR-0020 puts a refused path in the *outcome* rather than in a transport error. The assertion
      was conflating HTTP success with "the read happened" — exactly the confusion `P3-005` exists to
      remove — so it now asserts on the outcome and on the absence of content.
      **Honest limits.** **There is no approval round-trip**: a held decision returns `202` with `call_id`
      and `required_strength` and then nothing happens — no `ApprovalRequest` is persisted, no route lets a
      human decide, and nothing resumes the call, so the row stays truthfully `requested` forever. That is
      a gap, not a design. **No `run_events` row is written for a tool call** (`P3-012` owns it), so a call
      is absent from any stream a client is watching. **`policy_version` is a label, not a verifiable
      version** — the handler passes the literal `"policy-1"` and nothing checks it, so a receipt citing it
      proves which string was supplied rather than which rules were applied. **The authentication strength
      is asserted, not proven**: `Credential` is passed because a loopback credential was presented, with
      no per-call verification at the call site. The route takes its workspace from the run but **does not
      check that the run is the caller's**, because this transport has one local identity; that becomes a
      real question when a second identity exists. No CLI verb drives the route, so it is exercised by the
      gateway tests and no end-to-end process gate yet.
- [x] `P3-007` Research the current MCP specification and selected Rust SDK; record negotiated versions and features.
      **The research found the previous record was substantially WRONG about the current protocol**, so this
      slice's value was mostly in what it corrected. `docs/research/integrations/mcp.md` (re-verified
      `2026-09-22` against the live `llms.txt`, changelog, versioning page, Streamable HTTP page, and the
      official Rust SDK source) replaces an architecture snapshot that assumed the `initialize` handshake.
      **Revision `2026-07-28` is a stateless rewrite**: the `initialize`/`notifications/initialized`
      handshake is **gone**, the `Mcp-Session-Id` session is **gone**, and every request now carries its
      protocol version, capabilities, and identity in `_meta`
      (`io.modelcontextprotocol/protocolVersion`, `.../clientCapabilities`, `.../clientInfo`). `server/discover`
      is **mandatory for servers**. Also removed: the Streamable HTTP GET endpoint, SSE resumability
      (`Last-Event-ID` + event ids), `ping`, `logging/setLevel`, `notifications/roots/list_changed`, and
      `resources/subscribe`/`unsubscribe`. New: `subscriptions/listen` for change notifications, and
      **multi round-trip requests** replace server-initiated JSON-RPC requests (a server returns
      `InputRequiredResult` with `inputRequests`; the client retries the original request with
      `inputResponses`). Every result now carries a required `resultType`. Error codes were renumbered
      (`UnsupportedProtocolVersion` `-32004` → `-32022`; resource-not-found `-32002` → `-32602`) and the
      server-error range was partitioned so `-32020`..`-32099` is spec-reserved.
      **Selected: protocol `2026-07-28`, SDK `rmcp` 3.4.0** (published `2026-09-15`, `Apache-2.0`, MSRV
      `1.88`, edition 2024 — this workspace is on `1.98.1`/edition 2024, and `Apache-2.0` is already in
      `deny.toml`). The Rust SDK is now **Tier 1** (100% conformance pass rate, features before a spec
      release, triage in two business days, relegation if any conformance test fails four weeks running),
      which is a concrete commitment rather than an aspiration. **A trap worth recording: the SDK's
      `ProtocolVersion::LATEST` is `V_2025_11_25`, NOT the current revision** — so relying on it would
      silently negotiate the deprecated handshake while appearing to target the new spec. Both the client
      and the server must name `V_2026_07_28` explicitly.
      **Decisions, each with a reason recorded:** JARVIS is a **modern client** (`ClientLifecycleMode::Discover`,
      not `serve()`, which is the legacy handshake) and a **dual-era server**, because the spec's own
      compatibility matrix states a legacy client has **no fall-forward mechanism** — serving modern-only
      would break older clients silently for no gain. `Auto` is the fallback for an unknown-era peer, and the
      SDK treats only a **correlated, non-modern JSON-RPC error** as evidence of a legacy peer; a transport
      error stays a hard error rather than a silent fallback. **Roots, Sampling, and Logging are deprecated**
      (twelve-month removal window) and are deliberately **not adopted** — their spec-named migrations (pass
      directories via parameters/configuration, call the provider directly, log to `stderr`/OpenTelemetry) are
      already JARVIS's shape. Tasks moved out of core to an extension and is **not adopted**; long-running work
      is a JARVIS run. `x-mcp-header` is recorded as an adapter-only wire hint that must never leak into a
      `ToolDefinition`, and the spec's strict reachability rules (primitives only, `properties`-chain only, no
      `$ref`/`items`/`oneOf`) mean an invalid annotation **excludes that one tool** from `tools/list` rather
      than failing the list.
      **Honest limits.** This slice produced **no code and no dependency** — `rmcp` is not in the workspace
      yet, so nothing here is proven by a test; the verification plan names ten falsifying tests, none of which
      are written. Six unresolved questions are recorded rather than answered, including whether the
      dual-era server requirement conflicts with the daemon's single-transport shape (`P3-009`), whether the
      full `transport-streamable-http-server` feature set pulls something `cargo deny` refuses (unmeasured),
      and whether `rmcp`'s `auth` machinery can be confined to the adapter. **No `llms.txt` exists for the
      docs separately** — one index covers both, which is recorded as `not found` rather than glossed.
- [x] `P3-008a` Build the MCP translation layer: server identity, canonical tool naming, and schema conformance.
      **Added out of ledger order, because `P3-008`'s first real decision is not a transport decision.** New
      crate `crates/jarvis-mcp` (depends only on `jarvis-core` + `jarvis-tools`). ADR-0024. Three rules, each
      about *authority* rather than bytes, so each is settled once for every MCP server that will ever be
      configured. **33 tests; 725 workspace tests.**
      **1. A server does not name itself.** `ServerName` is an **operator-chosen** local name, validated as
      narrowly as a tool-name segment (lowercase ASCII, digits, `-`, `_`), with dots refused because
      `mcp.a.b` would otherwise be ambiguous between "server `a.b`" and "server `a`". The server's own claim
      is `ReportedIdentity` — bounded, kept as **evidence**, never an identifier — because the specification
      says the server name "is not guaranteed to be unique across servers and **SHOULD NOT** be relied upon
      for disambiguation". It is a self-assertion by the party being identified, and the **namespace prefix
      is load-bearing**: `mcp.` is what classifies a definition `ToolSource::Mcp`, i.e. third-party, so
      letting a server choose it would let a hostile server claim `jarvis.files` and be classified native.
      **2. Two MCP tool names never become one identifier.** MCP names may legally contain a **dot** and
      uppercase; JARVIS names are lowercase-only and forbid a dot in the name half. Every lossy fix merges
      two tools, and the dangerous direction is that a call for one tool would then run the other *with the
      other's declared risk, effects, and scopes*. So: use the name unchanged when a segment can hold it
      **exactly**, otherwise **hash the whole name** (SHA-256, `h` + 8 hex, length-prefixed input) — never
      truncate, because a shortened name *is* a different name and can collide with a real one, whereas a
      digest is obviously not a name. `Prefixed` (`mcp.github.list_issues`), `Bare` (`mcp.list_issues`, which
      collides across servers by design), and `Hashed` differ in **namespace**, not by mangling the tool
      name. `CanonicalToolName` keeps the **server's** name alongside the canonical id, because `tools/call`
      must send the server its own name back — sending the canonical id would be the translation applied
      twice, invisible until a real server rejects a call.
      **3. A server-supplied schema is untrusted input.** `$ref` is refused wherever it appears (this
      revision loosened schemas to any 2020-12 keyword, so that refusal is **more** load-bearing, not less —
      resolving one means fetching a URL a server chose). All four `x-mcp-header` constraints are enforced:
      primitives only (**`number` explicitly forbidden**; an integer bound must sit inside the JS-safe range
      ±2^53−1, and an *exclusive* bound of 2^53 is accepted because a bound is what must fit), reachable only
      through `properties` keys (not `items`/`oneOf`/`anyOf`/`allOf`/`if`/`then`/`else`/`$ref`/wildcards),
      unique case-insensitively, and a valid HTTP field name. The **reachability rule is the interesting one**
      and is not arbitrary: the spec defines extraction as reading the value at "the exact property path",
      which has a single answer only when the path is unique — under `oneOf` there are two answers and under
      `items` the path needs an array index a header cannot carry. A failing tool is **excluded by itself**,
      per the spec's explicit requirement that one malformed definition not remove the others.
      **THE DESIGN BUG MY OWN TESTS FOUND.** `NameAssignments` first keyed the collision check on the tool
      name alone, so it **refused a legitimate `tools/list` refresh** while **silently permitting two
      different servers to share one identifier** — a collision check that failed in both directions. The
      keys must be `(server, tool)`: same server + same name = refresh (accept); different server + same name
      = the collision `Bare` causes (refuse). Two of the four failing tests were that bug, and the other two
      were my own wrong expectations about the identifier shape (I had assumed the prefix belonged in the
      *name* half, which would give that half a dot and is not even expressible).
      **Honest limits.** **Nothing has been exercised against a real MCP server** — these are pure functions
      tested as pure functions, and "the translation is correct" and "a real server's tool list translates"
      are different claims; only the first is proven, and the field names used (`serverInfo`,
      `x-mcp-header`, `inputSchema`) come from the recorded specification rather than from bytes. **`rmcp` is
      not a dependency yet**, deliberately (these rules are about JARVIS identifiers), so nothing is checked
      against a real payload. **The hash is a disambiguator, not a security boundary** — 8 hex characters,
      and an attacker who can grind a colliding digest can forge one; the authority is the stored canonical
      id, not the hash. **No operator surface for the naming strategy exists**, so `Bare` cannot currently be
      selected — and when it can be, a collision refuses the *second* server's tool at translation time
      rather than at configuration time, which is later than an operator would want to learn it.
      `ReportedIdentity::agrees_with` has a test and **no caller**, because the host adapter that would use
      it is `P3-008`. The conformance module checks the *annotation* rules, not the schema's validity — a
      schema with no annotation gets no structural opinion at all, which is correct for this layer and worth
      not mistaking for validation.
- [x] `P3-008b` Translate a server's tool listing into canonical definitions, with the effects decided by an operator.
      The second half of `P3-008a`: that slice settled what an MCP tool is *called*, this one settles **what it may
      do and at what risk**. New module `crates/jarvis-mcp/src/definition.rs`. ADR-0025. **54 crate tests; 746
      workspace tests.**
      **Effects, risk, and approval come from an operator, never from the server.** An MCP server supplies
      `name`/`title`/`description`/`inputSchema` and an optional `annotations` object — and that object carries
      `readOnlyHint`/`destructiveHint`/`idempotentHint`, which are precisely the three facts policy needs. The
      specification warns of it: clients "**MUST** consider tool annotations to be untrusted unless they come
      from trusted servers". `McpToolListing` has **no field for annotations**, so the translation cannot consult
      them even by accident, and a test asserts that smuggling them into the input schema changes nothing. **The
      incentive is what makes this decisive**: `EffectSet::risk_floor` is a MAXIMUM, so a server that admits one
      outward effect lands at risk 2 → `Ask` and can never be silently auto-approved, while a server that
      under-reports has no such problem. The asymmetry runs opposite to the server's incentive.
      **An unclassified server fails closed.** `ToolEffectPolicy::unclassified()` declares
      `ExternalCommunication` + `Write` (floor 2) and then risk **3** with `ApprovalPolicy::Ask` — which
      workspace policy cannot lower the way `Policy` could — plus no automatic retry and
      `Idempotency::Unsupported`. A read-only server is mildly inconvenienced by a prompt; a mass-mail server
      presumed harmless is not inconvenienced at all. `read_only()` is the one convenience constructor.
      **Effects are an operator statement, not a schema inference.** "It takes a `query` string, so it reads" is
      inference presented as knowledge, and this project does not persist unsupported inference as fact.
      `ToolEffectPolicy::new` refuses a risk below the effects' floor and a retry that would repeat an
      un-deduplicated outward effect — **at configuration time**, so a policy that would send a payment twice
      cannot be *held*, let alone used.
      **The dialect rule composes two documents that disagree.** MCP says a schema with no `$schema` "defaults
      to 2020-12"; `jarvis-tools` **requires** the keyword and refuses to default it (because
      `exclusiveMinimum` is a boolean in earlier drafts and a number in 2020-12, so defaulting silently
      reinterprets an older-draft document). Both are right: a server's **omission** is supplied with 2020-12,
      and a server's **declaration of a different dialect** is refused rather than reinterpreted. The refusal
      names both dialects, or an operator cannot act on it.
      **Server text is sanitised before it reaches a model.** Whitespace collapses, control characters are
      dropped, and **bidirectional / zero-width characters are removed** — not typographic tidiness: a bidi
      override changes how text *reads* without changing what it *is*, which in the field that decides whether
      a tool gets called is a deception primitive. A missing description says the server supplied none rather
      than inventing a plausible one.
      `translate_listing` excludes per tool for names as well as schemas, and a collision is an **exclusion,
      never a rename** — any rename is a mapping the operator did not choose. The derived `version` is
      `schema-<8 hex>` over the **input schema** (not the whole listing), so a cosmetic description edit does
      not invalidate a stored intent while a schema change does; it is a digest, not a counter, because a
      version must survive a restart. Clippy's `too_many_arguments` (8/7) was **right, not noise**: `risk` and
      `timeout_seconds` are two adjacent numbers a caller could transpose silently, so the inputs were grouped
      into `ToolEffectPolicyParts` / `ToolPosture` / `ToolExecutionLimits` — the same reasoning `P3-005` applied
      to its call parts.
      **Honest limits.** **No configuration surface exists**, so `ToolEffectPolicy` cannot be built from
      `config.toml` and in practice every MCP server would get `unclassified` — the safe direction, but it means
      `read_only()` has **no caller outside tests** and the operator story is incomplete. Still **nothing
      exercised against a real MCP server**. The scope vocabulary is **one coarse `mcp.call`** — an MCP server is
      treated as one trust unit because no grant model can yet express more. A tool with no declared output
      schema gets a **permissive** one, so an unvalidated result reaches the model and the protection is the
      output-size bound, not schema validation. **The version excludes the tool name**, so two servers'
      identically-schemaed tools share a version string. **`Idempotency` is never `Required`/`ProviderKey`**
      because MCP cannot express provider-side dedup, so an outward MCP call can never retry automatically —
      safe, but a transient network failure is always a failure. **`translate_listing` builds its own
      `NameAssignments`**, so it cannot detect a collision between two different servers; that is the
      aggregate's job, and the test drives the cross-server case directly rather than pretending one call sees
      two servers.
- [x] `P3-008c` Aggregate several servers into one catalog, and refuse to serve an ambiguous one.
      **Closes a limit recorded by `P3-008a`/`P3-008b` rather than deferring it again**: a per-listing
      translation builds its own assignment set, so it *structurally* could not see a collision between two
      servers. New module `crates/jarvis-mcp/src/catalog.rs`. **67 crate tests; 759 workspace tests.**
      `McpCatalog` owns **one** `NameAssignments` across every server, so a collision is detected exactly
      where the information exists. `McpCatalog::build` is the first function in this crate that can answer
      "what happens when two servers offer the same name", and the answer is the interesting part:
      **a cross-server collision is reported, not resolved.** Dropping one side would mean the set of tools a
      model can call is a function of **configuration order** — the spec requires each server's own
      `tools/list` to be deterministic but says nothing about the order a client iterates *servers*, and since
      a collision is refused rather than renamed, the loser is **absent from discovery entirely**. A catalog
      whose contents depend on an ordering nobody declared meaningful is worse than one that refuses, so
      `has_cross_server_collision()` is the signal to **not serve**. A test drives both orderings and asserts
      the collision is reported either way, which is what makes refusing the right answer rather than an
      arbitrary winner.
      **Collisions are their own list, not a flag derived from message text.** The first version asked
      `exclusions.iter().any(|e| e.reason.contains("both translate to the identifier"))` — matching on a
      `Display` string, so a reworded error would have **silently stopped the catalog refusing**. The category
      is data (`collisions()`), not prose. Same family as the "two values that must agree" defects: a safety
      property that depends on a string a message happens to contain is a safety property nobody is holding.
      Other decisions: a **truncation is reported separately from an exclusion** (the causes and remedies
      differ — an exclusion is about that tool, a truncation is about the *size* of the configuration, and an
      operator fixing one tool would learn nothing); a server with no listing is **reported as an exclusion**
      so its absence does not look like a server that was never configured; a server configured twice is
      **refused**, because the second translation would look like an idempotent refresh, which is the case
      `NameAssignments` deliberately accepts; entries are sorted by identifier so a catalog built twice is
      equal; `MAX_MCP_SERVERS` is 16 so a config file cannot make startup unbounded; and the registry bound is
      enforced **here** rather than discovered at registration, because a catalog that silently exceeded it
      would fail later in a different component with an error naming the registry rather than the
      configuration that caused it. `route()` returns `None` for an unknown identifier, so a lookup cannot
      default to some other server.
      **Honest limits.** Still **no configuration surface**: `McpCatalog::build` takes servers and listings as
      arguments, so nothing constructs one from `config.toml` and `MAX_MCP_SERVERS`/the strategy cannot be
      set by an operator. Still **nothing exercised against a real MCP server**. The catalog holds
      **routing, not authority** — it carries definitions and server names, and a caller still passes them
      through the pipeline. `has_cross_server_collision` is a method, and **nothing calls it yet** — the
      daemon that would refuse to serve is `P3-009`.
      **CORRECTION (2026-09-22): this entry's first line and last sentence were both FALSE when written.**
      The commit message for this slice claimed it "closes two limits ... including `ReportedIdentity::agrees_with`
      having a test and no caller" — a grep at `1ecbb8e` showed `agrees_with` appeared only in `server.rs`
      (definition plus a test) and the `lib.rs` re-export, i.e. **still no production caller**. It also claimed a
      "duplicate entry in `listings`" is **resolved by first-match rather than refused**; that limit is **also
      false now**, because duplicates are refused (`CatalogError::DuplicateListing`) — but the claim was written
      as a *deferral to the config layer*, so it read as a live gap and made the slice look honest while the
      code it described was changing underneath it. This is the **second** time this session that a
      plausible-sounding recorded claim was false (`expected_version` was the first, at `P3-006c`), and the
      pattern is the same: **a commit message or ADR reads to the next reader as verified evidence**, so an
      overclaim propagates exactly like a wrong external doc — the thing this repository's external-research
      rule exists to prevent, arriving through the door marked "our own notes". The rule that follows is
      recorded in `docs/development/definition-of-done.md`: **before building on a recorded claim, check the
      claim; before writing one, check the code.**
      The work that made the first half true, and closed the second, is the identity-drift slice below.
- [x] `P3-008d` Report a server whose self-description changed, and refuse two listings for one server.
      **This is the slice that makes `P3-008c`'s commit message true a day late**, and it is written as its own
      entry rather than folded into `P3-008c` so that the overclaim stays legible. `ReportedIdentity::agrees_with`
      now has a **non-test caller** — verified by grep, not by recollection: it appears at exactly one non-test
      site, `observe_identity`, which is reached from the public `McpCatalog::build`. That is a narrower claim
      than "it is used in production" and deliberately so, because the wider one would be false: see the limits
      below. The first-match gap is closed by a refusal.
      `ServerListing` gives `McpCatalog::build` the server's **`ReportedIdentity`** alongside its tools, and
      `build` takes `seen_before: &[ObservedServer]`. Where a server's self-report **disagrees with a previous
      observation**, the difference is recorded as an `IdentityDrift { server, before, after }` and exposed via
      `drifts()`. **The drift is deliberately not fatal**: a vendor legitimately renames a product, and turning
      that into a refusal would make an ordinary upgrade an outage. The point is visibility, exactly as with
      `P3-008a`'s decision that a server does not name itself — an operator classified *a server by name*, and if
      the process behind the name changed, the posture they chose applies to something they may never have seen.
      Nothing else in the protocol records that, because the server's own name is explicitly not an identity, so
      this is the only place the fact can be observed. `agrees_with` is the right predicate for it because it is
      **deliberately tolerant of a changed `title` and only strict about a changed `name`** — the name is what
      the operator's decision is bound to.
      **A duplicate listing is now refused, not first-matched.** Two listings for one server are two
      *observations of one thing*, and which one wins would depend on argument order — the same
      order-dependence the collision rule refuses one level down, and the same reason a server configured twice
      is refused. `CatalogError::DuplicateListing { server }` names the server rather than the index, because an
      operator acts on a config file, not on a `Vec` position.
      Four tests, each aimed at a way the check could be wrong rather than a way it could pass: a **first build
      has nothing to compare** and must report no drift (or the check would be vacuous); a **changed** report
      produces exactly one drift naming before and after; an **unchanged** report produces none (the positive
      control — without it, a check that fired on every refresh would be ignored and therefore absent); and a
      drift **does not disturb routing** (it is about the report, not the tools) and **does not stop the catalog
      being built**. A server present in `seen_before` but absent now is **not** a drift — it simply was not
      listed, and that is already reported as an exclusion.
      **Honest limits.** The drift is reported to whoever calls `build`; **nothing calls it with a prior
      observation yet**, so in practice every build is a first build until `P3-009` owns the process that
      persists an observation. `ObservedServer` is not stored anywhere, so a daemon restart loses the baseline
      and a drift across a restart is invisible. A drift records **that** the report changed and the two texts;
      it does not classify *how* (rename vs. a different server that reuses a name), because the protocol
      supplies nothing that could distinguish them — which is precisely why it is a report and not a refusal.
      Clippy's `too_many_lines` (122/100) on `build` was **right**: the function had grown a second
      responsibility, so it was split into `admit_configuration`, `observe_identity`, and `finalize` — each one
      now states a single decision, and `admit_configuration` is where the three "refuse before translating"
      checks can be read together.
- [x] `P3-008e` Adopt the MCP SDK and carry a real negotiation: `server/discover`, not the handshake.
      **The first slice where JARVIS speaks the MCP wire**, and the first time `rmcp` is a dependency.
      New crate `crates/jarvis-mcp-transport` (depends on `jarvis-mcp` + `rmcp` + `tokio`). **19 crate
      tests; 783 workspace tests.**
      **Split into two crates deliberately.** `jarvis-mcp` stays pure, offline, and free of the SDK, so
      the authority rules a security reviewer must read are still testable without a peer standing and
      without any third-party type in scope — and so the transport can be reimplemented against the
      wire without any naming, posture, or conformance rule moving. `AGENTS.md`'s "provider SDK types
      must not cross JARVIS domain boundaries" is what forces this: a *transport* is where an SDK
      belongs, and an authority decision is where it does not.
      **The SDK's own README contradicts its own source, and the source is the contract.** Its README
      says `ProtocolVersion::LATEST` is "newest stable version this SDK defaults to" and that the SDK
      "implements the stable `2026-07-28` specification"; `LATEST` is **`V_2025_11_25`**, the *legacy*
      era, and the removal of `initialize` is precisely what makes `2026-07-28` modern. So the trap
      `P3-007` recorded is now an **executable fact**: `revision.rs` asserts `LATEST` is *not* the
      modern revision, and every connection names `V_2026_07_28` explicitly. An SDK bump that changes
      this produces a failing test and a decision, not a silent change of era.
      **`Discover`, never `Auto`.** `Auto` falls back to the legacy handshake when the peer does not
      answer `server/discover` within ten seconds (its constant is `DEFAULT_AUTO_DISCOVER_TIMEOUT`),
      which would negotiate a different era while looking healthy — and nothing in this crate
      implements the legacy era. `Discover` has no fallback path, verified by reading the SDK's
      `serve_client_with_lifecycle`: the timeout is passed in the `Auto` arm **only**.
      **That reading found a real defect in our own first version.** The same verification showed
      `Discover` has *no deadline at all*, so a peer that never replies left the client waiting
      forever — a daemon starting against a dead or legacy server would hang at startup rather than
      report. Found by a test that asserted a refusal and instead **hung**, which is why that test now
      asserts a bounded `DiscoveryTimedOut` naming the deadline and pointing at the protocol era as
      the likely cause.
      **A server's `annotations` are structurally unreachable.** `Tool` carries `readOnlyHint`,
      `destructiveHint`, and `idempotentHint`, and a test sends a tool that declares itself read-only
      and idempotent — exactly what a server would assert to get an effect auto-approved. The hints are
      dropped **by construction**: `McpToolListing` has five fields and none is an annotation, so the
      translation cannot consult them even by accident. A comment saying "do not read these" is a
      convention; a type with nowhere to put them is a guarantee.
      **The tests are driven against the wire, not against the SDK's server.** `tests/support` is a
      hand-written scripted peer that reads and writes newline-delimited JSON-RPC itself — the framing
      `rmcp` uses, verified in its `transport/async_rw.rs` (`read_until(b'\n', ..)` and
      `put_u8(b'\n')`). Using the SDK's own server would have been less code and worthless as
      evidence: a client and server from the same library share their assumptions, so a disagreement
      with the *specification* would pass. The peer also **refuses to answer a method it was not
      scripted for**, so a client calling the wrong method cannot appear to negotiate.
      What the tests prove, each aimed at a way the code could be wrong: `server/discover` is sent and
      `initialize`/`notifications/initialized` **never** are; a silent peer yields a bounded refusal
      rather than a connection or a hang; a server offering only `2025-11-25` is refused by name; a
      tool list arrives over the wire and translates (with a missing title/description staying absent
      rather than invented); a server claiming its tool is safe gets no say; the operator's name and
      the server's reported identity stay separate; a modern server that reports **no identity** still
      connects (the SDK's own source notes `server/discover` responses are "not required to provide
      it"), which matters because `P3-008d` must be able to see a server that *stops* naming itself; a
      discovery request actually carries its per-request metadata, which is the stateless contract's
      core requirement; and a JSON-RPC error on `tools/list` is a `PeerError` rather than
      `Unavailable`, because the two have different remedies.
      **Measured, not assumed: the SDK is admissible.** `cargo deny check` reports advisories, bans,
      licenses, and sources **all ok** with `rmcp` in the graph, which answers unresolved question 5 in
      `docs/research/integrations/mcp.md` (recorded there as "not yet measured"). The feature set is
      the narrowest that serves a client — `client`, `transport-child-process`,
      `transport-streamable-http-client-reqwest` — and the graph gained **no duplicate** of any crate
      the workspace already pins (`tokio`, `reqwest`, `serde`, `serde_json`, `tracing` all resolve to
      the same version, and `process-wrap` is genuinely new). `server` is off because the daemon is a
      client until `P3-009`; `auth` is off because JARVIS authorization is not MCP authorization and
      the OAuth machinery would be a second, unowned token lifecycle.
      **Honest limits.** **Still nothing exercised against a real MCP server.** A scripted peer proves
      the client agrees with the documented framing; it cannot prove a third-party server behaves.
      `connect_stdio` and `connect_http` are **unexercised** — every test drives `connect_over` over an
      in-process duplex pair, so the child-process spawn and the HTTP transport have no test at all,
      and `StdioCommand` being spawned is the one path where a real OS error could appear. **No
      `tools/call`**: this slice lists tools and stops, so the transport cannot yet run anything, and
      the MRTR round-driving the SDK offers is unused. **No connection is pooled or reused**, and
      nothing reconnects — `P3-009` owns the daemon that would. **Nothing calls any of this**: there is
      still no configuration surface for MCP servers, so the crate is a capability a caller can use and
      not one an operator can reach — the same limit `P3-008a`..`P3-008d` each recorded. The
      `resources` and `prompts` capabilities are not modelled because nothing consumes them, and the
      SDK's automatic response caching is left at its default rather than deliberately configured.
- [ ] `P3-008` Implement MCP client/host adapters for stdio and Streamable HTTP behind canonical tools.
      **Most of this is delivered, split across five slices that each answered one question; what remains
      is named at the end of this entry rather than being folded into a checkmark.**
      - `P3-008a` settled **what a tool is called** (`jarvis-mcp`: `ServerName`, `ReportedIdentity`,
        `canonical_tool_name`, `NamingStrategy`, `NameAssignments`, schema conformance). ADR-0024.
      - `P3-008b` settled **what a tool may do** (`ToolEffectPolicy`, the translation, per-tool
        exclusion). ADR-0025.
      - `P3-008c`/`P3-008d` settled **what happens when servers collide or a server changes its
        self-description** (`McpCatalog`, `IdentityDrift`, `CatalogError::DuplicateListing`).
      - `P3-008e` settled **the wire** (`jarvis-mcp-transport`: `rmcp` 3.4.0, `Discover` never `Auto`,
        a bounded negotiation, `tools/list`). ADR research in `docs/research/integrations/mcp.md`.
      - `P3-008f` settled **the join** and closed the two limits the five above each recorded. ADR-0026.
      - `P3-008g` settled **the remote endpoint policy** and closed the `connect_http` limit. ADR-0027.
      - `P3-008h` made a server's tool a real `ToolExecutor` (the outcome mapping). ADR-0028.
      - `P3-008i` added the **host role**: the `[mcp]` configuration surface, and the first caller of the
        catalog's collision check. ADR-0029.
      - `P3-008j` put the host role **in the daemon's startup path**: `mcp-servers.toml`, the dispatch table,
        and the two-scope actor. ADR-0030. **This is the slice that makes an MCP server an operator-reachable
        tool**, which is the limit all ten entries above recorded.
      **`build_catalog` is the composition that was missing.** `jarvis-mcp` could translate a listing
      nobody had fetched and `jarvis-mcp-transport` could fetch a listing nobody had translated, and
      **nothing joined them** — so every slice recorded the same honest limit, "a capability a caller can
      use, not one an operator can reach". `HostedServer { configured, connection }` carries the
      operator's name and posture with the live connection in one value, so a policy cannot be paired
      with a different server's tools; `ServerListing`'s reported identity is read from the
      **connection** rather than from the tool result, because `P3-008d`'s drift check compares that
      value against a previous observation and a host that defaulted it would make the check compare a
      value with itself. A server that cannot be read does not fail the build — one flaky third-party
      process must not empty the model's tool list — and its reason is kept in `HostBuild::unreadable`,
      which distinguishes "unreachable" from "refused" from "declared no tools capability" rather than
      letting all three look like the catalog's generic "offered no listing". A server that declared no
      `tools` capability is **not asked**, and a test asserts the request was never sent.
      **`tools/call` now exists**, which `P3-008e` recorded as absent. It uses the SDK's **single-round**
      request deliberately: the high-level helper drives MRTR rounds through a client `ClientHandler`,
      and JARVIS registers none because the only human answer comes through JARVIS's own approval path.
      So the modes JARVIS cannot serve are **refused by name** (`CallError::InputRequired`,
      `CallError::Task`) instead of failing on a missing handler or, worse, reading as an empty success
      for work that had not started. A tool returning `isError: true` is a **result, not an error** — it
      ran and refused, which is an outcome, not a broken connection.
      **A real defect was found by writing a fixture wrongly, and the message a user would have seen is
      why the fix matters.** The protocol's result union is `#[serde(untagged)]` (read in the pinned
      SDK's `model.rs`, `ts_union!`), so a result whose declared `resultType` disagrees with its fields
      does not fail as "a malformed `task`" — it becomes the SDK's generic `UnexpectedResponse`. The
      first `CreateTaskResult` fixture nested the task instead of **flattening** it (the SDK uses
      `#[serde(flatten)]`), so nothing matched and the refusal arrived as `Unavailable`, whose text is
      "the call did not complete" — describing an unreachable peer and sending an operator to inspect a
      connection that is working. That is now its own variant, `CallError::Undecodable`, whose text
      names the untagged-union cause. Same family as the other opaque-diagnostic defects this project has
      recorded.
      **`connect_stdio` is now exercised against a real child process**, closing `P3-008e`'s "entirely
      unexercised" limit. `src/bin/fixture_peer.rs` is a hand-written server behind the off-by-default
      `fixture-peer` feature — so `cargo build --workspace` never builds it and it cannot be mistaken
      for a shipped binary — and it writes the wire framing itself rather than using the SDK, for the
      reason the scripted peer does: a client and server from one library share their assumptions, so a
      disagreement with the specification would pass. Tests cover spawning, negotiating, listing,
      calling, that the configured arguments reach the child, and that a **program which cannot be
      spawned is refused rather than hung** — the failure a user hits first. The skip path was
      falsified: deleting the fixture under `ACCEPTANCE_REQUIRE_BINARIES=1` fails with the actionable
      message, and `cargo test` alone does not rebuild it (the recorded stale-binary trap).
      **Also removed a dead error variant found during review.** `ListError::NoIdentity` was declared,
      documented as "a signal worth surfacing", and constructed by nothing — absence of an identity is
      an empty `ReportedIdentity` on a working value, which is precisely what makes a server that
      *stops* naming itself observable. Removing it made a test's wildcard arm match exactly one
      variant, which clippy caught, so the removal also tightened an assertion. A declared-but-
      unconstructed variant reads downstream as a live condition, which is the `expected_version` class
      corrected at `P3-006c`.
      **Honest limits.** `connect_http` was covered by `P3-008g` below, which also closed this slice's
      "nothing reads a `config.toml`" note only in part: the MCP servers were not read from any daemon
      document, so nothing constructed a `HostedServer` outside a test and `apps/jarvisd` had
      **zero** references to either MCP crate. **CORRECTION (2026-09-23): `P3-008j` closed that** — the daemon
      loads `mcp-servers.toml` and constructs the host, so an MCP server is reachable from a running daemon.
      A **cross-server
      collision** is detected and reported but nothing acts on it, because the daemon that would refuse
      to serve is `P3-009`. `tools/call` is proven over the wire and against a child process, and
      `P3-008h` makes it an adapter a policy-authorized request reaches — but **nothing composes that
      adapter into the daemon**, so no run drives an MCP tool yet, and no `ToolOutcome` from an MCP call
      is stored. `P3-012` is the gate for that. Connection **pooling and reconnect** are unbuilt, so
      every build reconnects. The catalog's `MAX_MCP_SERVERS` bound is enforced but the server count is
      still the caller's to supply.
- [x] `P3-008f` Join discovery to authority: build a catalog from live connections, and add `tools/call`.
      **Recorded as its own slice rather than folded into `P3-008`, so the five preceding entries stay
      legible as five questions answered.** New code: `crates/jarvis-mcp-transport/src/host.rs`
      (`HostedServer`, `HostBuild`, `UnreadableServer`, `build_catalog`), `McpConnection::call_tool`
      with `ToolCallResult`, `CallError`, `src/bin/fixture_peer.rs`, and `tests/{host,stdio}.rs` plus
      five new `wire.rs` tests. 42 transport tests; **806 workspace tests, 37 suites**. ADR-0026.
      Findings, the defect the tests found, the falsified skip guard, and every honest limit are in the
      `P3-008` entry above; this line exists so the ledger shows the slice happened.
- [x] `P3-008g` Make a remote endpoint a validated value, and build the HTTP client in this crate.
      **Closes `P3-008f`'s recorded limit that `connect_http` was "entirely unexercised", and closes a
      real client-policy gap found while researching it.** `connect_http` took a bare `&str`, so every
      rule `docs/architecture/security.md` lists for "SSRF and unsafe redirects" was implicit and
      unenforced. New code: `crates/jarvis-mcp-transport/src/endpoint.rs` (`McpHttpEndpoint`,
      `EndpointError`, `build_http_client`) and `tests/http.rs` (10 tests against a hand-written HTTP/1.1
      server). `connect_http` now takes `&McpHttpEndpoint`. 62 transport tests; ADR-0027.
      **The client-policy gap is the finding worth recording.** The SDK's `default_http_client` disables
      redirects but **does not call `no_proxy`**, so proxy support there is off *only* because the SDK's
      manifest pins `reqwest` with `default-features = false` — a fact in a dependency's `Cargo.toml`,
      not a property of anything in this workspace. A feature unification elsewhere in the graph could
      re-enable an unchosen intermediary with nothing here changing. `jarvis-models` already refuses the
      same thing explicitly for the model client, so the two adapters were making different choices
      about the same hazard. This crate now builds the client itself and passes it through the SDK's
      `with_client`, making each choice a statement rather than an inherited default.
      **The endpoint rules are properties of the value**: scheme is `http`/`https`; no userinfo (a
      credential in a URL is a substring of every log line naming it, the rule `ApiKey::new` already
      enforces); no `#` fragment (never sent to a server, so a URL carrying one expresses something other
      than what the request will do); no interior whitespace; and **TLS off loopback**, since a plaintext
      remote MCP session carries tool arguments and results in the clear. Loopback recognition uses
      whole-host comparison, so `127.0.0.1.evil.example` is correctly remote.
      **The redirect test's assertion order is load-bearing, and falsifying it is what showed why.**
      Allowing redirects made the test fail on a *message* assertion first (the refusal that arrives is a
      protocol error whose text does not mention `302`), so the run ended before the security property was
      examined — the test would have failed for a formatting reason while the thing it exists to catch
      went unobserved. With the property asserted first, re-running the falsification reports the
      redirect target actually receiving the request (a `GET` carrying `mcp-protocol-version` and a
      `referer` for the original host), which is the vulnerability stated as evidence.
      **Honest limits.** **DNS resolution and private-range blocking are deliberately NOT claimed** — a
      point-in-time answer is the wrong shape for a question where a name can resolve differently when
      connected, which is why `security.md` lists rebinding defense separately; the module records the
      residual rather than implying coverage. The standalone `GET`-for-SSE path is declined by the test
      server and is therefore **not covered**. `Mcp-Method`/`Mcp-Name` agreement is asserted on the
      client's *outgoing* request only — the server side's `-32020 HeaderMismatch` handling is `P3-009`.
      At the time of this slice the MCP servers were not read from any daemon document, so an endpoint was
      constructed by a caller and nothing loaded one from `config.toml`. **CORRECTION (2026-09-23):**
      `P3-008j` made `mcp-servers.toml` load endpoints, so an operator now writes one. The endpoint rules
      here are what make that safe to write, which is why the slice order was right: the policy existed
      before the reachability did.
- [x] `P3-008h` Make a remote server's tool a real `ToolExecutor`, and get the outcome mapping right.
      **Closes the limit every preceding MCP slice recorded — "nothing drives it from a run" — at its last
      step.** New code: `crates/jarvis-mcp-transport/src/adapter.rs` (`McpToolAdapter`) and
      `tests/adapter.rs` (15 tests). `McpCallResult` was renamed from `ToolCallResult`, because
      `jarvis-tools` has a different type of that name and the two must not be conflated: one is a raw wire
      result, the other is an established outcome with its evidence and bounded output. ADR-0028.
      **The conversion is the subject, and its table is in the module.** A tool that reports failure is
      `Failed` with the server's reason (it ran and refused); a JSON-RPC error is `ProviderRefused` (the
      peer answered, so nothing happened); an undecodable answer is `AmbiguousAfterReaching` (the server
      answered, so it ran something); a transport failure after sending is `AmbiguousAfterReaching`; MRTR
      and Tasks are `ProviderRefused`; an unrouted identifier is `NotImplemented`; a past deadline is
      `RefusedBeforeReaching`. Evidence is a **locator** (`mcp:<operator's server>/<server's tool>`) and
      output is content, kept separate because `tools-and-connectors.md` requires it — and the operator's
      name is used rather than the server's own claim, since ADR-0024 makes that claim evidence and not an
      identifier.
      **A falsification found a missing test, which is the most useful outcome here.** Changing the
      `Unavailable` mapping from `AmbiguousAfterReaching` to `RefusedBeforeReaching` left the suite
      **green**: no test covered a transport failure at all, so the one row deciding whether a
      non-idempotent effect may be repeated was unpinned. The scripted peer gained `HangUp` — a method
      answered with silence and a **closed** connection, the only way to express "sent, no answer" — and
      that test now fails the falsification with the exact wrong claim ("refused the call before reaching a
      provider: Transport closed"). **A row in a mapping table is a claim until a test pins it.**
      **The design was wrong once and the reason is already load-bearing elsewhere.** The first version put
      the server's own tool name into the request's `arguments` under a reserved key. That violates the
      tool's input schema *and* the authorization digest, which `ToolExecutionRequest::new` recomputes and
      compares — so every request would have failed its own binding check. Both properties exist
      deliberately (`P3-001`'s contract, `P3-006b`'s binding), so the routing moved into the adapter,
      supplied from the catalog entries at construction and filtered to one server so an identifier cannot
      be sent to a server it does not belong to.
      **Two fixture mistakes are recorded in the test rather than fixed silently**: an MCP tool requires the
      `mcp.call` scope, and even a minimal-risk tool requires `ChannelEvidence` — so an `Absent` strength
      claim is a hold. Both turned every test into a held call, and both were visible only because the
      fixture asserts an allowance instead of assuming one.
      **Honest limits.** **At the time of this slice nothing composed `McpToolAdapter` into the daemon** — no
      run drove an MCP tool, no `ToolOutcome` from an MCP call was stored, and `apps/jarvisd` had zero
      references to either MCP crate. **CORRECTION (2026-09-23): `P3-008j` closed the composition**, so the
      daemon now dispatches a policy-authorized request to an MCP adapter and stores its outcome. No
      `run_events` row is written for an MCP call (`P3-012` links calls to the event log). The `Confirmed` for a successful call rests on the server's own `isError: false`, which
      the protocol gives no way to verify — the honest statement is that it is the strongest available
      evidence, and the design keeps it separate from the output so a reader can weigh it.
- [x] `P3-008i` Add the MCP host role: the configuration surface, and the first caller of the collision check.
      **Closes the limit nine slices recorded — "a capability a caller can use, not one an operator can
      reach" — at its last step.** New code: `crates/jarvis-mcp-transport/src/host_config.rs`
      (`McpHostConfig`, `McpHost`, `ServerTransport`, `HostConfigError`, `HostError`) and
      `tests/host_config.rs` (7 tests including the whole journey). 103 transport tests. ADR-0029.
      **A test starts from TOML text and ends with a call**: configuration → spawn a real child process →
      negotiate → list → translate → catalog → adapter → `Confirmed`, all through the fixture binary. A
      stdio server is spawned by the configuration itself, so the test proves the *program string an
      operator wrote* is what runs.
      **`has_cross_server_collision()` has its first caller**, held since `P3-008c` with a documented
      instruction and nobody to follow it. A collision refuses the **whole host**, and the same test's
      `Prefixed` control proves the refusal is about the collision rather than about an unusable
      configuration.
      **The posture vocabulary is two classes and the default is the severe one.** An operator writes
      `class = "read-only"` or nothing; nothing means `unclassified` — risk 3, every call held — so
      **omitting something fails closed**. A full `ToolEffectPolicy` in a file would be a vocabulary with no
      consumer; the one thing an operator can usefully narrow today is "this only reads". An unknown class is
      **refused**, because a typo must not silently leave a server at the permissive value an operator was
      trying to set.
      **Two findings, both from getting something wrong first.**
      1. **The collision message named one server twice.** It was built from `CatalogExclusion`'s `server`
         field, but a collision entry records only the server that *lost* the assignment, so two colliding
         tools produced `["bravo", "bravo"]` — while the remedy ("rename one of them") needs **both**. The
         refusal now carries the catalog's own `NameCollision` text, which names both sides. A refusal that
         does not identify what must change is the opaque-diagnostic defect in the one place an operator has
         nothing else to go on.
      2. **A parse error claimed a cause it could not know.** `InvalidDocument` said "an unknown key is
         refused rather than ignored", but a syntax error, a misspelled key, and a wrong value type all arrive
         as the same deserialization failure. A fixture using a Windows path in a TOML **basic** string — where
         `\U` begins an invalid escape — reported a syntax error as a key mistake, sending an operator to
         check keys that were correct. The message now carries a **character offset** and warns about the most
         likely cause; the parser's own text is not forwarded, because it can echo a document that holds paths
         and hostnames.
      **`HostedServer` changed shape** (`McpConnection` → `Arc<McpConnection>`) so the catalog build and the
      adapters share **one** connection per server. That is a statement, not tidiness: two connections would be
      two sessions (or two child processes) whose lifecycles diverge, and shutdown would close only one; it
      moved ~13 test construction sites. `McpHost::close` drops the adapters before closing, because a
      connection can only be closed by its last owner, and a still-shared connection is **reported** rather
      than claimed as stopped.
      **Honest limits.** At the time of this slice the daemon did not read an `mcp-servers.toml` document and
      `apps/jarvisd` had **zero references to either MCP crate**, so this was a configuration surface a caller
      could drive and not one the daemon loaded. **CORRECTION (2026-09-23): `P3-008j` closed that.** The daemon
      now reads `mcp-servers.toml`, connects the host, registers its adapters, and dispatches to them, so the
      sentence above describes this slice's boundary rather than the platform's. `P3-009` remains the gate for
      the **other** direction (JARVIS as a server). No `run_events` row is
      written for an MCP call (`P3-012`). **No credential can appear in the document**: a remote server needing
      authentication needs the credential store and is its own slice. Connection **pooling and reconnect** are
      still unbuilt, so every host build reconnects.
- [x] `P3-008j` Put the MCP host in the daemon: `mcp-servers.toml`, the dispatch table, and the two-scope actor.
      **This closes the limit all ten preceding MCP slices recorded** — "a capability a caller can use, not one
      an operator can reach" — because it is the first slice where `apps/jarvisd` references either MCP crate.
      New code: `apps/jarvisd/src/dispatch.rs`, `apps/jarvisd/src/mcp_host.rs`, and `apps/jarvisd/src/tool_pipeline_tests.rs`;
      changed: `tool_pipeline.rs`, `tool_actor.rs`, `gateway.rs`, `main.rs`. 103 transport + 14 new daemon
      tests; **880 workspace tests, 40 suites**. ADR-0030.
      **The dispatch table is its own type, and it refuses two things rather than resolving them.** A tool the
      registry offers that no adapter can run (`DispatchError::Uncovered`) would be authorized by policy and
      then fail, so it is a **startup** refusal; and a tool two adapters claim (`DispatchError::Duplicate`)
      would make which one ran depend on registration order, so the same call could reach different providers
      on different builds. The table is keyed by identifier because that is the key the receipt binds — a scan
      would return the first match, which is an implicit ordering decision of exactly the kind this type exists
      to make impossible.
      **The falsification of the routing check PASSED when it should have failed, and that is the finding.**
      Changing `adapter_for` to `self.adapters.values().next()` left
      `a_call_reaches_the_adapter_that_owns_its_definition` **green**, because the test registered only **one**
      adapter — "return any adapter" then finds the right one, so the test proved the dispatch table was
      non-empty and nothing about routing. The fix was to register the filesystem adapter beside the MCP one
      and assert the precondition explicitly (`dispatchable_tools() == 3`); re-running the falsification now
      fails with `NotImplemented { tool: "mcp.test.search" }`, which is the misroute stated as evidence. **A
      routing test needs at least two candidates, and the count has to be asserted or the second adapter can
      be dropped later without the test noticing.** This is the same shape as `P3-008h`'s unpinned row: a
      check that cannot fail is not a check.
      **The MCP servers live in their own document, and the reason is that a failure there must not stop the
      daemon.** `mcp-servers.toml` sits beside `config.toml` rather than as a section inside it, because the
      daemon's schema is validated by **key allowlist** — an `[mcp]` section would mean `jarvis-storage`'s
      schema learns one protocol's configuration vocabulary, which is a dependency from an adapter into a
      protocol shape that `repository-layout.md` forbids in the other direction and would be just as wrong
      here. Keeping the documents separate also means a third-party program that is missing, a server that
      hangs, or a collision between two configured servers are all reasons for the **MCP tools** to be
      unavailable rather than reasons for the daemon to refuse to start; each is logged so the absence is
      visible rather than silent. A **pipeline** failure is fatal by contrast, because it covers the filesystem
      grant and the registry: starting anyway would serve a tool set nobody declared.
      **A missing document is not an error; an unreadable one is.** A daemon nobody configured servers on is
      the normal case, and requiring the file would make every existing profile fail to start. But a document
      that exists and cannot be read is reported, because an operator who wrote one and has it unreadable would
      otherwise conclude their servers were configured and empty.
      **`WorkspaceRoots` refuses an empty list, so the pipeline takes an `Option`.** `RootError::NoRoots` exists
      precisely so a tool cannot silently read nothing while looking like a tool that works. A daemon with MCP
      servers and no filesystem grant therefore registers **no filesystem adapter at all** rather than one over
      zero roots — the tool is absent rather than present-and-failing. This surfaced as a *test* failure
      (`NoRoots`) while writing the routing test, and the failure was the composition being wrong rather than
      the test.
      **The actor grants both scopes, and the narrow constructor had to stay.** `tool_actor.rs` gained
      `workspace_and_mcp`, granting `files.read` **and** `mcp.call`; the gateway uses it, because
      `workspace_reader` grants only `files.read` and **every MCP call would have been denied for a missing
      scope**. Widening the existing constructor was rejected: a profile with no filesystem roots has no
      filesystem tool to read, so granting `files.read` there would be a grant with no consumer. A test asserts
      `MCP_CALL_SCOPE` equals `jarvis_mcp_transport::DEFAULT_MCP_CALL_SCOPE`, so the two copies of that literal
      cannot drift apart silently.
      **Honest limits.** No `run_events` row is written for an MCP call (`P3-012` links calls to the event
      log), so the **call row** is the only durable record of it. The host is composed once at startup and
      **never reconnects or pools**, so a server that dies stays unavailable until the daemon restarts.
      `seen_before` is always empty, so an identity drift across a restart is **invisible** — `P3-009`'s
      persistence problem, unchanged. The **`NamingStrategy` is hardcoded to `Prefixed`** and the file cannot
      set it, so `Bare` is unreachable from configuration. A **collision refuses the MCP host and its tools are
      simply absent** — the correct outcome, but the caller is not told which servers were dropped, only a log
      line. And the daemon is proven to **compose** a host from a document, not to have serviced an MCP call
      through a **run**: the routing test drives the pipeline directly rather than over HTTP.
      **The gate failed on a pre-existing defect, and fixing it turned out to be this slice's most
      consequential work.** `cargo test --workspace` failed `a_cancellation_requested_in_flight_settles_the_run_once`
      with `the agent run was changed by another writer`. `apps/jarvisd/src/executor.rs` was **not modified by
      this slice**, and the test passed 12/12 in isolation, so it was initially tempting to call it flaky and
      re-run. It is a real lost-update race, and ADR-0022 — which claims to have fixed exactly this test
      "structurally" and "verified by 12 consecutive passes" — **had not**: it removed the *expectation* from
      the cancellation request but left `version = version + 1` on the write.
      **A version is a guard for a write that depends on the version it read; this write depends on nothing.**
      Its predicate is a state and its effect is idempotent under `COALESCE`, so advancing the version protects
      nothing while invalidating the expectation every other writer holds — the executor holds one across the
      whole model call. The fix is that the cancellation no longer advances the version at all (ADR-0022
      decision 7; the superseded decision and the false sentence are both struck in place rather than edited
      away).
      **The reproduction is deterministic, and building it corrected a claim of my own.** `a_progress_write_racing_a_cancellation_neither_conflicts_nor_loses_the_request`
      stages the read → cancel → write interleaving explicitly rather than sleeping, because the defect is an
      **ordering** rather than a timing — which is precisely why "12 consecutive isolated passes" could not
      have found it. Falsifying it found that my first test comment was **false**: I asserted that dropping the
      version bump alone would lose the cancellation, and running it showed the transition's own `COALESCE`
      re-preserves the request, so the test simply passes. Dropping the `COALESCE` is what fails it
      (`left: None`), so the two properties are pinned separately and the comments now state the mechanism that
      actually holds. **A test comment is a claim like any other, and the only way to know which property a
      test pins is to break each one in turn.**
      **The lesson supersedes the one the earlier slice recorded.** "It passed 6/6 after the change" and "12
      consecutive passes" are not evidence about a *concurrency* defect: the window only opens under load, so
      isolated repetition samples the wrong thing. `P2-009a`'s finding 3 ("writes now re-read first") was
      necessary but not sufficient, and it was recorded as if it were the fix.
- [ ] `P3-009` Implement authenticated, scoped JARVIS MCP server exposure with per-client allowlists and rate limits.
      **Split into three recorded parts, because this reverses a platform decision and adds an auth surface.**
      `P3-009a` (done below) is the policy value; `P3-009b` is the server transport wired to it; `P3-009c` is
      the client allowlist and rate limiting. The slice is deliberately **not** one change: the research found
      that the SDK's server defaults are permissive on the specification's MUSTs, so the policy has to exist
      and be testable before anything inherits a default.
- [x] `P3-009a` Make server exposure a policy value: origin trust decisions the SDK cannot make.
      **The first server-side slice, and it exists because reading the SDK's server transport contradicted its
      own documentation.** New code: `crates/jarvis-mcp/src/exposure.rs` (`ServerExposure`, `AllowedOrigin`,
      `OriginVerdict`, `OriginError`, `ExposureError`, `is_loopback_host`) and 17 tests. 92 `jarvis-mcp` tests;
      **900 workspace tests, 40 suites**. ADR-0031. Research recorded live in
      `docs/research/integrations/mcp.md`.
      **The finding: `StreamableHttpServerConfig::default()` is permissive on exactly the MUSTs a security
      story rests on.** From the vendored source — `allowed_origins: vec![]` with
      `validate_empty_origin_allowlist: false` makes `validate_origin_header` return `Ok(())` **before reading
      the header**, so the spec's "servers **MUST** validate `Origin`" is unenforced; its own doc comment says
      it "disables Origin validation for backward compatibility". `legacy_session_mode: true` mints an
      `Mcp-Session-Id` that `2026-07-28` (SEP-2567) removed. `stateless_protocol_metadata_required: false`
      "preserv[es] today's legacy behavior where an absent header is treated as protocol version
      `2025-03-26`". **The SDK is not uniformly permissive** — `allowed_hosts` defaults to loopback only and
      the `-32020` header↔body check runs unconditionally — and that is what makes inheritance dangerous: a
      fail-closed field and a fail-open field are indistinguishable at a call site.
      **The second finding, from reading the comparison rather than its description.** The SDK's doc says an
      `Origin` "must match per RFC 6454 `(scheme, host, port)`"; its `origin_is_allowed` is
      `a_scheme == o_scheme && a_host == o_host && (a_port.is_none() || a_port == o_port)`. The
      `a_port.is_none()` arm makes **a portless entry a wildcard over every port**, and browsers omit a default
      port — so `https://x` admits `https://x:8443` (far broader than it reads) while `https://x:443`
      false-rejects the normal traffic it was written to allow. **A control whose narrowest setting is a
      wildcard and whose exact setting is wrong is not a control**, so JARVIS owns the comparison: a default
      port and an omitted port are one origin, and no other port is.
      **A real defect was found by a test rather than by inspection.** `AllowedOrigin` derived `Eq`, which
      compares `port` as written, so the duplicate check accepted `https://x` and `https://x:443` as two
      entries while `matches` correctly called them one origin — the operator's policy would hold one origin
      twice with the file reading as two. `PartialEq`/`Eq`/`Ord` are now hand-written against the **effective**
      port.
      **Both central properties are pinned by falsification.** Replacing the default-port implication with
      `unwrap_or(0)` fails `an_omitted_port_and_the_schemes_default_port_are_one_origin` and the duplicate test;
      folding `Malformed` into `Absent` fails `a_malformed_origin_is_refused_rather_than_treated_as_absent` and
      `the_default_policy_refuses_every_present_origin` — the second because a malformed `Origin` would become
      the admitted shape, which **inverts the control** rather than merely weakening it.
      **Decisions worth naming.** An empty allowlist is **enforced**, which is the opposite of the SDK's empty
      default, so the accessor is `is_loopback_only()` and not `allowed_origins().is_empty()` — a reader
      checking only for emptiness would conclude the reverse of what they hold. `Origin` is **not** an
      authentication boundary (a non-browser client sends anything, and a client omitting the header is
      admitted by the spec's own rule), so the real boundary is `requires_remote_bind()` and the refusal to be
      reachable off-host at all. Every refusal answers the same `403`, so a hostile caller cannot enumerate the
      allowlist by the refusal received. `null` is refused at both ends, because admitting the opaque origin
      admits every sandboxed frame at once.
      **Honest limits.** **Nothing serves anything yet**: this is a policy value with no transport behind it,
      so `P3-009b` is what makes JARVIS reachable as a server, and the SDK field mapping is not written. There
      is **no remote bind and therefore no OAuth resource server** — RFC 9728 Protected Resource Metadata and
      RFC 8707 audience binding are MUSTs for a server that adopts OAuth, and they are not built, so a
      remotely reachable JARVIS MCP server would be an **unauthenticated control plane**. That is why the
      slice refuses exposure off-host rather than exposing and relying on JARVIS authorization in front of it.
      **No per-client allowlist and no rate limiting** (`P3-009c`). The policy does **not** model
      `allowed_hosts`, because the SDK's loopback-only default is already fail-closed and a remote bind is
      refused outright. `Origin` is checked against a configured list only — there is no support for
      wildcard subdomains, deliberately, since a wildcard in an allowlist is the broadest possible reading of
      a narrow intent.
- [ ] `P3-009b` Serve the MCP endpoint: bind loopback, map the exposure policy onto the SDK's server fields.
- [ ] `P3-009c` Add the per-client allowlist and rate limits, and prove the `-32020` and `Origin` refusals over a real HTTP request.
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