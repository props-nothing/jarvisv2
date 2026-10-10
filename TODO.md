# JARVIS Implementation Backlog

This is the execution ledger. Work top to bottom unless an ADR records why ordering changed. A box may be checked only when its acceptance evidence exists. Update this file in the same change as the implementation.

## Next Slices (reviewed 2026-10-10)

The ledger below is in phase order; the work is not. This is the order to pick from, and why (the same list, shorter, is in [ROADMAP.md](ROADMAP.md)). The first unchecked task rule below applies *within* a slice, not across this list.

1. **Live checks of what only a stand-in has tested** (no new code): Gmail drafts (`P9-081`, needs a Google sign-in again for the new permission), a real OCR run (`P9-082`, needs `tesseract` installed), the fallback at a second provider (`P9-083`, needs a second key), the ntfy push (`P9-065`), a reply sent from another sender (`P9-074`), a provider switch saved from Settings (`P9-085`). Each has "NOT run live" in its entry; record the result there.
2. **Answer-quality harness** (`P9-011`): labeled prompts from the owner's real work (prospecting, mail, research) run against the configured model, scored over time, so every later change can be judged better or worse. Without it, prompt and tool changes are guesses.
3. **Browser tool for JavaScript pages** (`P9-069`): the largest gap in research quality; needs a research record and a trust-boundary ADR (a page is hostile input; the browser must not reach the owner's files or sessions).
4. **Model routing** (`P9-069`, `ROADMAP.md` item 4): a cheaper model for sub-agents and background work, now that token usage is recorded (`P9-087`) and a second provider can be configured (`P9-083`). Do not remove tools to save tokens: capability is the goal.
5. **Coding-agent runtimes** (`P7-001` to `P7-004`, via ACP) and delegation as durable workflow steps (`P6-010`, `P7-010`).
6. **Decide the Google push and watch framework** (`P5-005b`): compose it or retire it; polling from a schedule is what works today.
7. **Spend guard in the run**: stop a run midway at a token limit (`ADR-0165` stops only new scheduled runs); a workspace-wide cap.
8. Then local voice and telephony (`P8`), distribution (`P9-003` to `P9-007`, blocked on a licence choice) and the server profile (`P10`).

Housekeeping: `P5` was condensed on 2026-10-10 (the notes are kept word for word in `docs/history/todo-p5-connectors-notes.md`). `P3` (about 1,800 lines) and `P4` (about 1,300 lines) hold finished work with long engineering notes and are the next candidates for the same treatment; open items inside them (`P3-008`, `P3-009`, `P3-012`, `P3-021`, `P4-014`) are partly done and say what remains.

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
      `Deny` plus an `effective_risk`, a `DenyReason` code, and
      the escalation signals that raised the risk. Deny overrides is a control-flow property: the
      checks are ordered and the first refusal ends the evaluation, so no permissive finding can
      mask a later refusal. The denials precede the approval steps, so a call that is denied *and*
      would need approval reports the denial rather than sending an operator to approve something
      that cannot run. Workspace policy can
      only narrow what a tool declares. 150 in-crate tests. ADR-0017 records the ownership tension:
      `repository-layout.md` gives core "policy decisions and reason codes" while `jarvis-tools` is
      named for the policy pipeline, and the decision must read a definition holding compiled JSON
      Schema validators — so core cannot own it without either gaining a vendor SDK or inverting the
      adapter direction.
      Deliberately not a placeholder: nothing calls this yet and nothing is persisted. Recording an
      approval obligation (expiry, resume) is `P3-004`; the execution receipt is `P3-005`.
- [x] `P3-004` Implement durable approval requests, expiry, approve/deny/cancel, authenticated approvers, and resume semantics.
      `jarvis_core::approval` (the domain) + `jarvis_storage::approval_repository` + migration
      `0006_approvals.sql` (schema v6). An approval binds to a **canonical intent digest** computed
      from the tool, its version, and a sorted-key rendering of the arguments, domain-separated so
      two parts cannot bleed into each other. A decision records its outcome, channel, instant and approver and
      enforces the expiry. Expiry is
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
      and the reason and then nothing happens — no `ApprovalRequest` is persisted, no route lets a
      human decide, and nothing resumes the call, so the row stays truthfully `requested` forever. That is
      a gap, not a design. **No `run_events` row is written for a tool call** (`P3-012` owns it), so a call
      is absent from any stream a client is watching. **`policy_version` is a label, not a verifiable
      version** — the handler passes the literal `"policy-1"` and nothing checks it, so a receipt citing it
      proves which string was supplied rather than which rules were applied. The route takes its workspace from the run but **does not
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
      **A fixture mistake is recorded in the test rather than fixed silently**: an MCP tool requires the
      `mcp.call` scope, and an empty grant turned every test into a refusal, visible only because the
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
      set it, so `Bare` is unreachable from configuration — **CLOSED by `P3-008k` below.** A
      **collision refuses the MCP host and its tools are
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
- [x] `P3-008k` Make the naming strategy the document's, so an operator can choose it and a collision is
      reachable from configuration.
      **Closes the limit `P3-008j` recorded** — "the `NamingStrategy` is hardcoded to `Prefixed` and the file
      cannot set it, so `Bare` is unreachable from configuration" — and with it three defects rather than one
      gap: an operator could not express the choice, **a caller could pass a strategy the document disagreed
      with**, and `HostError::Collision` was **unreachable through the daemon's own path** because `Prefixed`
      cannot collide. New: `NamingDocument`, `StrategyName` and the `naming_strategy()` accessor in
      `crates/jarvis-mcp-transport/src/host_config.rs`; `connect` lost its `strategy` parameter and eight call
      sites were updated. `ADR-0127`.
      - **`connect` takes no strategy, so the disagreement is unrepresentable rather than refused.** That is the
        shape of the fix: `McpHostConfig` owns the value and reads its own field, so two builders of one
        configuration cannot produce two tool sets — the property `dispatch.rs` refuses for adapters and the
        catalog refuses for duplicate names.
      - **An absent section means `Prefixed`, and the default is stated on `StrategyName` rather than derived
        onto `NamingStrategy`.** `NamingStrategy` already derives `Deserialize`, so reusing it was shorter — but
        a `#[serde(default)]` on the domain enum would be **that type choosing a security-relevant default for
        one consumer**. The default belongs where the consequence is, and the consequence is which tools a model
        is offered. `StrategyName` is also written out rather than derived, for the reason every vocabulary here
        is: the domain's `as_str` is its wire form, and a rename there must not silently change what a document
        means.
      - **The daemon's test changed from asserting a constant to asserting a read.** The old
        `the_daemons_naming_strategy_is_the_one_that_avoids_collisions` passed a document and checked that two
        unreachable servers composed — which **a daemon that ignored the section would also pass**. The new pair
        is `the_daemon_reads_the_configured_naming_strategy` (the parse, asserted directly, because an
        unreachable server cannot collide so the collision path cannot distinguish the strategies) and
        `a_configured_bare_strategy_can_collide_and_is_then_refused`. **A test that asserts a hardcoding is a
        test of a constant.**
      - **The collision test now writes the strategy into its document**, and asserts both halves: `bare` with
        two servers offering one name is a **refusal**, and the same two servers under the default **compose**.
        A test of the refusal alone would pass if every document composed.
      - **Both directions falsified, in both layers.** Replacing the parsed strategy with `Prefixed` in `parse`
        fails the transport's collision test *and* the daemon's read test (`left: Prefixed, right: Bare`);
        making `Bare` the default fails `a_configured_stdio_server_becomes_a_callable_tool` — the identifiers
        lose the prefix (`["mcp.fetch", "mcp.search"]` where `["mcp.local.fetch", "mcp.local.search"]` is
        required) — and the absent-section assertion in the collision test.
      - **⭐⭐ The same over-correction mistake as `P4-016`, made again, and caught by the compiler this time.**
        Restoring mutation 1 with a **global** replace rewrote *both* `naming:` initializers, so `none()` — which
        has no document — tried to read `parsed`. `P4-016` recorded this exact lesson ("a mutation restore must
        be scoped to its occurrence") and it recurred, because the anchor was a line that legitimately appears
        twice. The compiler caught it here; in `P4-016` a silent narrowing of three queries did not. **The rule
        that works: when the anchor appears more than once, count the occurrences first and refuse to replace
        unless the count is the one you expect.**
      - **Limits:** `Hashed` is expressible but **not exercised against a real server**, because no fixture
        reports a name that cannot be represented. The strategy is **per host, not per server**, so a setup
        where one server's names need hashing has to hash both or be split across two profiles — the section
        shape is where a per-server override would go.
      - Gates: fmt, clippy `-D warnings` over the workspace, the workspace suite, `cargo deny check` ok.
- [ ] `P3-009` Implement authenticated, scoped JARVIS MCP server exposure with per-client allowlists and rate limits.
      **Split into three recorded parts, because this reverses a platform decision and adds an auth surface.**
      `P3-009a` (done below) is the policy value; `P3-009d` (done below) is the served surface, recorded under
      its own number because "which tools may a caller reach" is a different question from "from where";
      `P3-009b` is the server transport wired to both; `P3-009c` is the client allowlist and rate limiting. The
      slice is deliberately **not** one change: the research found that the SDK's server defaults are
      permissive on the specification's MUSTs, so the policy has to exist and be testable before anything
      inherits a default.
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
- [x] `P3-009d` Decide what JARVIS serves: the served surface, with a transitive tool refused and named.
      **Answers the question that actually decides whether exposure is safe** — which of our tools may a
      remote client call — after `P3-009a` answered *from where*. New code:
      `crates/jarvis-mcp/src/served.rs` (`served_tools`, `ServedTool`, `ExposureExclusion`, `ServableName`,
      `MAX_EXPOSED_TOOLS`) and 11 tests. 103 `jarvis-mcp` tests; **911 workspace tests, 40 suites**.
      ADR-0032.
      **The central rule: a tool whose source is third-party code is never re-exposed.** The test is
      `ToolSource::is_third_party()` (`Mcp`, `Runtime`, `Extension`). A **connector** tool *is* servable,
      because JARVIS wrote the adapter and our own operator declared its risk — which is why the rule is not
      "anything but `Native`". The reason is worth stating: our exposure decisions (origin allowlist,
      loopback bind, `P3-009c`'s per-client allowlist) describe **this daemon**. A caller reaching
      `mcp.github.search` through us has driven a call through our policy and then through a third party's
      tool that policy never classified, in a context we did not choose — the `ToolEffectPolicy` governing it
      was written for *our* use of that server on *this* machine. **Nested exposure is deliberately not
      built**: consent, whose credentials execute the call, and which audit record owns it are unanswered
      questions, and a federation feature must not arrive as a side effect of "expose my tools".
      **Exposure gets its own type rather than reusing `McpCatalog`, because the rules are the opposite
      direction's rules in three places.** Naming: outbound, rule 1 says a server does not name itself so we
      invent the name; inbound, the only name a remote operator can be sure of is the **canonical identifier
      transmitted verbatim**. Posture: outbound it is an operator's declaration about a third party
      (ADR-0025); inbound it is irrelevant, because what matters is whether the source is code this project
      wrote. Absence: outbound a server being unreachable excludes its tools; inbound a **known but
      unavailable** tool is withheld, because advertising it offers a tool that fails on every call. One
      plausible reuse would have given three wrong answers.
      **A constructible tool that cannot be transmitted is excluded, not renamed.** The protocol
      `SHOULD`-constrains a tool name to `A-Za-z0-9_.-`, and `ToolId`'s name segment legally contains `:` — so
      `jarvis.files:read` is a real, registerable tool the wire cannot carry. Renaming would give a remote
      caller a name the daemon's own operator cannot find in their configuration. The check is **stricter**
      than the wire's alphabet: it admits exactly what the canonical rules produce, so uppercase is refused
      too and a future identifier vocabulary cannot become transmittable by accident.
      **The most consequential reason wins, and that ordering was falsified.** A transitive tool that is also
      unavailable is reported as `Transitive`. Checking availability first makes the reason "the account is
      not connected", so an operator reconnects an account and fixes nothing — the tool would still not be
      exposed.
      **Three falsifications, one per rule.** Removing the transitive guard serves `mcp.github.search`;
      checking availability before source reports the unfixable reason; removing the bound serves 35 tools
      instead of 32. The bound is checked **after** every eligibility check, so `dropped` means "withheld for
      size" rather than "withheld", and the truncation is reported once as a **list** problem with `tool()`
      returning `None` — an operator fixing one tool's availability would learn nothing from a size bound.
      **Also fixed from a real inconsistency:** the name check's first version admitted uppercase while its
      documentation claimed to enforce the canonical lowercase alphabet. The test asserting the documented
      property failed, and the check was tightened to match its own description.
      **Honest limits.** **Nothing serves anything yet** — `P3-009b` wires the transport to this value and to
      `ServerExposure`, so these are two policy values with no listener behind them. **The served schema is
      transmitted verbatim**, so a schema the protocol's own `x-mcp-header` rules reject is **not filtered on
      the server side**; `check_tool_schema` is the client-side model for it and the server-side placement is
      `P3-009b`'s. No per-client allowlist and no rate limiting (`P3-009c`). The posture attached to a served
      tool is the **local** declaration unchanged, which is honest only because a third-party-sourced tool is
      excluded outright — so there is no case where a posture about someone else's code is published.
- [x] `P3-009e` Serve the handler: refuse an unadvertised tool before anything runs, and name one revision.
      **The first place JARVIS is the one being called, and the slice whose title is its central rule.** New
      code: `crates/jarvis-mcp-transport/src/serve.rs` (`JarvisMcpServer`, `ServedToolRunner`,
      `call_result`, `SERVED_PROTOCOL_VERSION`) and 11 tests. 47 transport tests; **922 workspace tests, 40
      suites**. ADR-0033.
      **Filtering a list is not authorization, and that is the finding.** An MCP client may call **any** tool
      name whether or not the server advertised it — nothing in the protocol couples `tools/list` to
      `tools/call`. A server that only filtered the list would have a **catalogue, not a control**. So the
      handler holds the served set and `invoke` refuses an unserved name **before the runner is consulted**,
      and the test's runner **panics if it is reached** — because "the call was refused" and "the call was
      refused before anything ran" are different claims and only the second is the security property.
      **Falsified: removing the check fails with `the runner must not be reached for mcp.github.search`.**
      **Two SDK server defaults are wrong for this revision, and the research had recorded both before the
      code existed.** `ProtocolVersion::default()` is `LATEST`, and `LATEST` is **`V_2025_11_25`** — the legacy
      handshake era — so `ServerConfig::default()` advertises a revision this build does not implement; the
      falsification prints `left: ProtocolVersion("2025-11-25")` as evidence. And
      `supported_protocol_versions` defaults to `KNOWN_VERSIONS`, i.e. **every** version the SDK knows
      including the legacy ones, so a server inheriting it would accept an `initialize` handshake whose
      semantics it does not drive and the client could not tell. Both are narrowed to `V_2026_07_28`.
      **A refused call travels as a result, not as a protocol error.** The SDK's own documentation says
      `Err(McpError)` is rendered **opaquely** — the caller sees "Tool result missing due to internal error" —
      so a refusal that travelled that way would be unreadable. The outcome table is the honesty boundary one
      level above `McpToolAdapter`'s, and its important row is **`Unknown`**: it must *say* the outcome is
      unknown rather than report a failure, because a caller reading "failed" would reasonably retry and a
      retry of a non-idempotent effect is a second effect. Falsified by mapping `Unknown` onto the failure
      text. A **truncation** is likewise stated in the content rather than silently applied.
      **No annotations are advertised.** `readOnlyHint`/`destructiveHint`/`idempotentHint` are exactly what a
      policy engine wants, and the specification warns clients to treat them as untrusted — so a tool's
      declared effects stay in the JARVIS contract (ADR-0025) rather than being restated where the protocol
      says not to believe them.
      **The `server` feature's cost was measured before enabling it.** Five new packages (`schemars` 1.2.2,
      `schemars_derive` 1.2.2, `serde_derive_internals` 0.30.0, `dyn-clone` 1.0.20, `pastey` 0.2.3), nothing
      removed, and **no new duplicate** — the duplicate `name@version` sets before and after are identical
      (`chrono`, `tower`, `sse-stream`, `http-body`, and `uuid` were already locked). `cargo deny` reports all
      four categories ok with the features on. Recorded in `docs/research/integrations/mcp.md` because a
      dependency decision should cite measurement rather than preference.
      **A fixture mistake was caught by the type it was testing.** The first `served` fixture declared
      `ToolSource::Native` for `gmail.messages.send`, and `ToolDefinition::new` **refused** it because the
      namespace implies a connector — so a test cannot construct a definition the production path would
      reject. The fixture now derives the source from the identifier.
      **Honest limits.** **Nothing is bound.** There is no loopback HTTP listener, no stdio serve, and no
      daemon wiring, so a remote client cannot reach this handler — `P3-009b` is the transport and `P3-009c`
      is the per-client allowlist and rate limiting. `ServedToolRunner` has **no daemon implementation**, so a
      served call cannot yet pass the JARVIS pipeline: its actor, scopes, approvals, idempotency key, and
      durable call row are all unexercised from this direction, and that wiring needs a run-versus-remote
      attribution decision a slice must make explicitly. `CallToolResponse::Complete` is the only variant
      JARVIS answers, because approval-shaped interaction belongs to the JARVIS approval path rather than a
      remote client (ADR-0025) — so the protocol's `input_required` and `task` paths are deliberately
      unreachable rather than unimplemented. No `run_events` row for a served call (`P3-012`).
- [x] `P3-009b` Build the serving configuration, and drive the handler through the real SDK service.
      **`P3-009a`'s design argument became a functional one, and a gap `P3-009e` recorded is closed.** New code:
      `crates/jarvis-mcp-transport/src/serving.rs` (`ServingConfig`, `ServiceError`, `MCP_ENDPOINT_PATH`,
      `MAX_REQUEST_BODY_BYTES`) and `tests/serving.rs` (11 tests driving the **real** service). 56 transport
      tests; **942 workspace tests, 41 suites**. ADR-0034.
      **The finding: origin validation cannot be delegated at all.** The SDK's `validate_origin_header`
      compares against `allowed_origins` directly and has **no extension point** — `with_allowed_origins` is
      the whole surface — so the comparison `P3-009a` rejected cannot be replaced. `ServingConfig::sdk`
      therefore calls `disable_allowed_origins`, and `origin_check` is the decision; **enabling both would be
      worse than disabling the SDK's**, because two checks with different rules would disagree about a
      portless entry and the permissive one would be part of the answer, while a populated `allowed_origins`
      would read as the control. A disabled field with the reason recorded is honest; an enabled field whose
      rule is wrong is a false assurance.
      **Every permissive SDK default is a stated value, and a test reads the SDK's `Default` alongside it**, so
      an upgrade that changes a default fails rather than silently changing the policy: `legacy_session_mode`
      `false`, `stateless_protocol_metadata_required` `true`, the body bound stated, `allowed_hosts` loopback,
      and `NeverSessionManager` in place of an in-memory store for a protocol with no sessions.
      **A configuration allowing a public origin is refused outright** rather than warned about, because serving
      off-host needs RFC 9728 metadata and RFC 8707 audience binding that are not built, and a remotely
      reachable MCP server without them is an unauthenticated control plane. The test is on the **host**, so
      `http://localhost:3000` is served and `https://jarvis.example.com` is refused.
      **Three facts were discovered by driving the real service rather than assumed.** The `Accept` header must
      list both `application/json` **and** `text/event-stream` — sending only the first is answered
      `406 Not Acceptable`, so `P3-009a`'s client obligation is enforced server-side too. A refused tool call is
      **`200` with `result.isError: true`**, not a `4xx`, which is `P3-009e`'s decision seen from outside. And a
      `GET` is `405` while an oversized body is `413`, both now pinned.
      **This closes the gap `P3-009e` recorded**: `list_tools`, `call_tool`, and `server/discover` were
      compiled but **never executed** through the SDK, so a response-shape mismatch had nowhere to fail. They
      are now driven with a real `http::Request` and `http::Response`, which is the path a client's POST takes.
      `server/discover` also confirms the modern revision is advertised and `2025-11-25` is not.
      **A falsification corrected a test's own claim, for the second time this phase.**
      `a_request_without_a_protocol_version_header_is_refused` was written as evidence for
      `stateless_protocol_metadata_required(true)`; removing that line did **not** fail it, because the refusal
      comes from the SDK's body-versus-header agreement rule — a body `_meta` version requires the matching
      header whatever the flag says. `a_request_with_no_protocol_signals_at_all_is_refused` was added to isolate
      the flag, and the first test's comment now names the property it actually pins.
      **Honest limits.** **Nothing is bound**, deliberately: `ServingConfig` produces the SDK's service and
      binding a socket is the daemon's, because `repository-layout.md` gives network listeners to the
      composition root — so this is reachable from a test and not yet from a client. **Origin enforcement over
      a real request is `P3-009c`'s** (the decision is here, the layer owning the request is there). No daemon
      wiring, no per-client allowlist, no rate limiting. The `server` feature's five packages are recorded in
      `docs/research/integrations/mcp.md` with the measurement that admitted them.
- [x] `P3-009f` Enforce the SDK-boundary invariant, and remove the leak `P3-009b` added.
      **Written because reviewing `P3-009b` against `repository-layout.md` found that its public `sdk()` and
      `service()` returned SDK types, which the docs had explicitly forbidden.** New code:
      `crates/jarvis-mcp-transport/src/boundary_tests.rs` (5 tests) and `src/serving_transport_tests.rs`, which
      is the SDK-facing transport proof moved **inside** the crate. **947 workspace tests, 40 suites**. ADR-0035.
      **A documented invariant with no test is a convention, and this one had been wrong for two phases.**
      `connect_over` (`P3-008e`) is public with `rmcp::transport::IntoTransport` and `rmcp::RoleClient` in its
      signature, and `revision.rs` exposes three functions returning or taking `ProtocolVersion`. So the
      sentence "no provider SDK type appears in this crate's public surface" was **not merely violated by
      `P3-009b` — it had been false since `P3-008e`**, and every reader in between was misled in the reassuring
      direction: someone asking "does an SDK type cross this boundary?" would have concluded no.
      **The first version of the test did not catch the violation it was written for, and falsifying found it.**
      It searched for the literal crate name `rmcp`, which finds a *fully-qualified* path and misses an
      *imported* name — so `pub fn sdk(&self) -> StreamableHttpServerConfig` was reported clean, and restoring
      that signature left the test green. The scan now collects the names a file imports from the SDK and checks
      both forms, with unit tests for each: a public declaration naming the SDK in either form must be reported,
      and a private one must not.
      **The justified exposures are an exception list with reasons, and a companion test keeps it honest** —
      each entry must still match a real declaration, so a rename or removal fails and deleting an exception is
      a deliberate act. `connect_over` is a documented test seam; the three revision helpers exist to make the
      `LATEST = V_2025_11_25` trap executable, and `describe_negotiated` takes the SDK's type deliberately so a
      caller cannot pass a bare string that disagrees with what was negotiated.
      **`P3-009b`'s own leak was removed rather than recorded.** `sdk()` and `service()` are private;
      `tool_list`/`invoke` are `pub(crate)` because their return types are the SDK's `Tool` and `CallToolResult`,
      with `serves()` as the public `bool` statement of the same rule; `served_protocol_version()` returns the
      **wire string** rather than `ProtocolVersion`. Four dead items went with it (`bounded_reason`,
      `method_not_found_code`, `SdkSurfacePosture`, and a `ServedEndpoint` wrapper that existed **only** to reach
      an SDK type publicly — so removing the leak removed its reason to exist).
      **The SDK-facing transport proof moved in-crate** to `src/serving_transport_tests.rs`, because an
      integration test sees only the public surface and the accessor it needed would have been the violation. A
      crate-internal module compiling against the SDK is not the same as the SDK being part of this crate's
      contract, and only the second is what the invariant forbids.
      **The one not-yet-consumed configuration carries `#[cfg_attr(not(test), expect(dead_code, ..))]`**, because
      `sdk`/`service` are reached only from tests while nothing binds. An `expect` fails once the lint stops
      firing, so the binding slice must delete it; the scoping is needed because the claim is true in only one
      configuration, and an unconditional `expect` is unfulfilled in a test build.
      **Honest limits.** `connect_over`'s visibility should move behind an off-by-default feature so the
      **shipped** surface is SDK-free; that is recorded as a follow-up rather than half-built. The four recorded
      exceptions remain, with their reasons and their guard test. Nothing is bound, so `ServingConfig` is still
      reachable from a test and not from a client (`P3-009c`).
- [x] `P3-009g` Decide who may call: an allowlist whose subject is a credential, never a label.
      **The inbound half of `ADR-0024`, and the design was decided by reading the protocol's shape before
      drawing it.** New code: `crates/jarvis-mcp-transport/src/admission.rs` (`CallerAdmission`, `Fingerprint`,
      `CallerLabel`, `AdmittedCaller`, `AdmissionVerdict`, `AdmissionError`) and 13 tests. **960 workspace tests,
      40 suites**. ADR-0036.
      **The finding: `clientInfo` is entirely client-chosen, so an allowlist keyed on it is a control a client
      names itself into.** `Implementation` in the pinned SDK is `{name, title, version, description, icons,
      website_url}`, decoded from per-request `_meta`, and nothing verifies any field — so a caller sending
      `name = "vscode"` would be admitted **as VS Code** with no credential at all. That is *a server does not
      name itself* arriving inbound, where it is worse because the label decides **permission** rather than
      identity. The rule is now stated in the type: **a self-reported name is evidence or nothing, never a
      permit** — the inbound mirror of `jarvis_mcp::ReportedIdentity`.
      **A label mismatch is reported, never refused** — falsified by forcing `label_matches` to `true`, which
      fails `a_label_mismatch_is_reported_rather_than_refused` with `left: Some(true)`. Refusing would make the
      label a **second permit after the module says it is not one**, and would break every call from a valid
      credential on a client version upgrade. A mismatch is worth *seeing*, so the operator decides.
      **A pasted credential is refused locally as a fingerprint**, because `Fingerprint::parse` accepts only the
      digest alphabet — a JWT (`eyJ…eyJ…`) or a `Bearer …` value fails with "it is probably not a fingerprint"
      rather than being stored, compared, formatted into diagnostics, and committed. `Display` prints a
      **prefix and a length**, so a log line names the caller without carrying a collectable digest.
      **`local_only` is the default and admits nobody remotely**, which is the same decision `ServingConfig`
      already makes for the bind — stated **twice on purpose**, because a control that depends on another
      control having worked is not a control (a proxy or a changed bind would otherwise be enough). The two empty
      states are opposites, so the accessor is `is_local_only()` rather than an emptiness check.
      **Rate limiting is a bound here and the counting is the daemon's**: `decide` takes `spent_budget: bool`,
      because a policy that mutates per request cannot be compared, logged, or reused. The budget is a **rate**
      (twelve per minute), so a burst is not punished for the rest of a window. Refusals answer **`401`** for a
      missing or unknown credential rather than `403` — the two are the same situation to a caller, and
      distinguishing them would reveal which fingerprints exist — and `429` for a spent budget.
      **Honest limits.** **Nothing binds**, so no request reaches this policy yet; binding plus applying both
      decisions at the request layer is the next slice. **The token itself is unvalidated**: audience binding
      (RFC 8707) and Protected Resource Metadata (RFC 9728) are the OAuth slice, and until they exist a remote
      caller cannot be admitted at all — which is why `is_local_only` is the default rather than a warning.
      `spent_budget` is **supplied by the caller**, so a daemon that never sets it has a rate limit that never
      fires — recorded rather than implied, because a bound nothing enforces reads exactly like one that works.
      The clock is not modelled, because a budget window needs one and that belongs with the counting.
- [x] `P3-009h` Fix `local_only` admitting nobody, and make a local caller's origin explicit.
      **A review of `P3-009g`'s shipped policy against its own docs found that its central default refused
      everybody.** `decide` took only a credential fingerprint, so `None` meant "no credential" and was refused —
      but a **local** caller is precisely the case the credential apparatus does not apply to. So the default
      **refused the operator on their own machine**, while its doc said "the only admitted caller is a local
      one". Fixed in place rather than in a new slice: `CallerOrigin` is now a parameter, the falsification
      reproduces the original bug exactly (`left: NoCredential, right: Admitted`), and the ADR's claim is
      corrected with the reason. 16 admission tests; **963 workspace tests, 40 suites**.
      **`CallerOrigin` is supplied by the request layer and derived from nothing a caller sends**, which is the
      rule `CallerLabel` exists for one level down: there is deliberately no `Deserialize` and no constructor
      from a header or body, so a remote caller cannot admit itself by claiming to be local. A test asserts the
      type has exactly two states, so adding a deserializer is a compile failure rather than a silent widening.
      **The rate limit applies to a local caller too**, and that ordering is load-bearing: `decide` checks the
      budget *before* the origin, because an early return for `Local` would make the bound reachable by anyone
      who could reach the socket.
      **The lesson is about what a test asserts.** The original tests checked that a *remote* caller is refused —
      which the bug satisfied perfectly. Nothing asserted the doc's own words about a local caller, so a shipped
      document described behaviour the code did not have. That is the same defect class this phase has found five
      times now, and the remedy is the same: **write the test that asserts the claim, in the claim's terms.**
- [x] `P3-009i` Apply both inbound decisions to one request, in one place, and make an admitted request a type.
      **Closes the limit every slice above recorded: "a policy value nothing enforces".** New code:
      `crates/jarvis-mcp-transport/src/enforcement.rs` (`RequestGate`, `RequestAdmission`, `RequestRefusal`) and
      11 tests. 99 transport tests; **974 workspace tests, 40 suites**. ADR-0037.
      **The gate holds both policies and consults both**, so "both were applied" is structural rather than a
      convention a caller keeps: there is no way to hold one policy and call it a gate.
      **`RequestAdmission` has private fields and no public constructor**, so the only way to hold one is
      `decide` returning `Ok`. A `bool` carries the same information and none of the guarantee — the caller
      decides what `true` means, and `true` is also what a default-initialized field holds. This is `P3-009e`'s
      "a catalogue is not a control" applied to admission rather than to tools.
      **`Origin` is checked first, and that order is the specification's**: the revision says servers **MUST**
      validate `Origin` on **all** incoming connections, so a request failing **both** is reported as an origin
      refusal (`403`) rather than a `401` that tells the caller about its credential while the origin went
      unexamined. Swapping the two blocks fails `a_hostile_origin_is_refused_before_the_credential_is_considered`
      **and nothing else**, which is what makes that test the one holding the rule. The refusals keep different
      statuses because they carry different remedies.
      **`RequestAdmission` carries *how* the origin was decided, not only that it was** — `Absent` and `Allowed`
      are different justifications for the same outcome, and an audit record that could not tell them apart
      could not answer "was this a browser request", the question a loopback-only deployment most needs answered.
      **The gate takes values, never a request.** A gate reading a `HeaderMap` could not be tested without one,
      and deriving those values is the daemon's because the listener is.
      **No consistency check between the two policies.** A loopback-only `ServingConfig` beside an allowlist with
      remote entries looks contradictory and is not: the bind is one control, the admission policy another, and a
      control that depends on another having worked is not a control (`P3-009g`'s rule). Reconciling them would
      remove the second.
      **Two bugs were found by this slice's own tests rather than by review.** `also_refused_admission` returned
      `false` whenever the allowlist was `local_only`, conflating "the allowlist is empty" with "this request
      carried nothing to check" — the same confusion `decide` had before `CallerOrigin`, reproduced within one
      commit of being fixed; its test failed immediately. And a fixture used `https://jarvis.example.com` as an
      allowed origin, which `ServingConfig::new` correctly refuses, so the fixture moved to a loopback origin —
      the check working one layer up.
      **The falsification needed two attempts and the first was wrong.** Disabling the admission refusal *and*
      swapping the order left the order test **passing**, because that request was never refused on admission.
      The first attempt proved "the pair is broken" rather than "the order is reversed"; only the second
      isolates the property the test names. Worth remembering as a general shape.
      **Honest limits.** **Nothing binds**, so no request reaches this gate — the binding is `P3-009c`, and the
      four values are derived from nothing yet. `spent_budget` is still supplied by the caller, so a daemon that
      never sets it has a rate limit that never fires. The **token remains unvalidated**: no RFC 8707 audience
      binding and no RFC 9728 Protected Resource Metadata, so a remote caller can only be admitted against a
      fingerprint an operator configured by hand.
- [x] `P3-009c-a` Bind the endpoint: a tower service that derives both decisions from a real request.
      **The layer `P3-009i` recorded as missing — "nothing binds, so no request reaches this gate."** New code:
      `crates/jarvis-mcp-transport/src/binding.rs` (`ServedEndpoint`, `ResponseBody`, `CREDENTIAL_HEADER`,
      `BEARER_SCHEME`, `MAX_ORIGIN_HEADER_CHARS`, `REFUSAL_CODE`) and 12 tests. 112 transport tests.
      ADR-0038.
      **It is a `tower` service wrapping the SDK's own service, not a route handler.** The SDK's Streamable HTTP
      service *is* a `Service<Request<Body>>`, so wrapping it is one `impl Service` rather than a second routing
      layer and a second body type; `axum::Router::fallback_service` accepts any `Service`. The wrapping is also
      what makes the layer provable — a real request is the only evidence the four values were **derived** rather
      than handed over.
      **A network request is `CallerOrigin::Remote`, always, and that is the slice's central decision.** `Local`
      means the operator on their own machine, established by the daemon's **transport** (a named pipe, a socket
      it holds). A loopback *bind* is not that evidence: any local process, and a browser on the same machine,
      can reach `127.0.0.1`. Deriving `Local` from a peer address would be `CallerOrigin`'s own defect one layer
      down — a property of the request deciding admission — and a daemon wanting a local caller admitted does so
      through the **allowlist**. Falsified: changing the literal to `Local` fails three tests, including the
      anonymous remote caller being **admitted**.
      **An oversized `Origin` is truncated, never treated as absent** — because absent is *admitted* by the spec's
      own rule, so collapsing an oversized value into `None` **inverts** the control. Falsified by adding a
      `.filter(…)`: the response became **`200 OK` with the full tool list served**.
      **The credential requires its scheme, compared case-insensitively.** A bare digest in `Authorization` is not
      valid HTTP. Falsified in both directions: removing the scheme check admits the bare digest (`200` where
      `401` is required); removing the case-insensitivity would refuse a client sending `bearer`.
      **The refusal names the policy class and never the verdict**, because on the wire a verdict helps a hostile
      caller enumerate the allowlist. `RequestRefusal::reason` names it and is for the log.
      **The boundary test was extended to the shape that would leak next, and the probe found it.** `P3-009c`
      adds a `pub type` alias (`ResponseBody`), and a `pub type` is a signature-like public declaration with no
      `fn` — which the existing scan's `pub fn` probe would not have exercised. A `pub type Leaked =
      StreamableHttpService<…>` was added to the **real source** as a live probe and the scanner reported it by
      line; the probe was removed and replaced with a test over the same text. **That test failed on its first
      run**, because the scan keys imported names off the `use rmcp::…` line *in the same file* and the fixture
      had none — correct behaviour, discovered rather than assumed.
      **Honest limits.** **The daemon does not mount this yet** (`P3-009c-b`, which now does), so at the time this
      slice closed the layer was proven by tests and not by a socket. `spent_budget` is still a literal `false`,
      recorded at the call site rather than left to inference: the rate limit the gate could apply never fires.
      The admitted `RequestAdmission` is logged on a span and dropped — an admission is observable but **not
      attributable**, because attributing an MCP call
      needs a correlation id the request does not carry (`P3-012`). The **token remains unvalidated** (no RFC
      8707 audience binding, no RFC 9728 Protected Resource Metadata), so a remote caller can only be admitted
      against a fingerprint an operator configured by hand.
- [x] `P3-009c-b` Mount the endpoint in the daemon: choose the loopback port, wire the policies from configuration, and prove a real socket refuses an anonymous remote caller.
      `apps/jarvisd/src/mcp_serve.rs` (the runner, the endpoint, the bound listener), `mcp_serve_tests.rs`, plus
      `ToolActor::remote`, `ToolPipeline::call_remote_tool`/`definitions`, `Running`/`start`/`run`/`wait_for_exit`
      in `apps/jarvisd/src/main.rs`, and `mcp_serve_port` in `crates/jarvis-storage/src/config.rs`. ADR-0039.
      **The endpoint is bound on loopback on its own port and served under both inbound policies.** The REST
      transport's flag is separate: the two listeners have different trust boundaries and different answers to "who
      may call", so sharing a port would make one admission policy govern the other's traffic. `Some(0)` and a port
      with no granted workspace roots are refused at configuration time, because either would report a daemon ready
      on a listener that cannot answer.
      **A remote call has no run, and the schema is the authority.** `0007_tool_calls.sql` declares
      `run_id TEXT NOT NULL REFERENCES agent_runs (id)`, so a call with no run cannot be written as a `tool_calls`
      row. Inventing a run identifier to satisfy the column was the tempting shortcut and the foreign key refuses
      it. `call_remote_tool` therefore writes **no** row, and that is recorded as a limit rather than presented as
      completeness.
      **A second entrypoint rather than a `record: bool`**, so "may this call write a row" is a property of which
      function was called and cannot be set wrongly by an argument whose name does not say what is lost.
      **Parity is by calling the same code**: the same `validate`, the same `evaluate` over the same definition,
      workspace policy, and dispatcher. The differences are exactly two — the audit record and the approval outcome.
      **An approval is refused, not bypassed, and there are two independent checks**: the engine's
      `Decision::RequireApproval` (a *workspace* threshold) and `definition.approval() != ApprovalPolicy::Auto`
      (a *tool* declaration), both refused with `mcp_call_cannot_hold_an_approval` — because reporting the
      workspace's own code would tell a client an approval is obtainable when this path cannot hold one.
      **⚠ The second check had no test, and that was measured rather than suspected: mutating it to `false && …`
      left the suite green at 13/13.** A test now registers a real `ToolDefinition` declaring
      `ApprovalPolicy::Ask` at risk 0 — so the default workspace threshold of `Moderate` cannot hold the call,
      making the declaration the only thing under test — beside a **counting** adapter that must not be reached.
      The mutation now fails exactly that one test.
      **⚠ The caller policy was not covered either**: replacing `build_endpoint`'s `caller_policy` argument with
      `CallerAdmission::local_only()` left 14/14 green, because every other test already passes `local_only()` and
      a parameter that is always given one value is indistinguishable from one that is ignored. A test that passes
      an *admitted caller* now pins the pass-through, and it asserts its own fixture is not the default so it
      cannot silently become a duplicate of its neighbours.
      **The remote actor's scopes are the union of the served definitions' `required_scopes`** — exactly what the
      advertised tools demand. Never `mcp.call`, which is this daemon's grant to call someone else's server: on the
      inbound direction that is a scope with the right name and the wrong direction. **The first version granted
      nothing and every call was refused with `missing_scope`**; its own test caught it, and reverting to
      `ScopeSet::none()` fails four tests.
      **An adapter's own error is propagated unchanged.** `map_pipeline_error` returns `AdapterCall(error)` as-is,
      preserving `AdapterError`'s three-way claim — `RefusedBeforeReaching`, `ProviderRefused`, and
      `AmbiguousAfterReaching`. Rewriting all three into "nothing was reached" would tell a client to retry a call
      whose effect is unknown, and for a non-idempotent tool the retry is **a second effect**. Every other pipeline
      error is raised before the dispatcher is consulted, so mapping those to `RefusedBeforeReaching` is a
      statement the code supports. Tested by **constructing** the ambiguous value, since no served adapter returns
      one. Also `into_service` now pins `Future = ResponseFuture` with `Clone + Send + Sync + 'static`, because an
      opaque `impl Service` says nothing about `Service::Future` being `Send` and the omission otherwise surfaces
      at the **mount site** in an error naming its generic parameter rather than this layer.
      **Five mutations were run, one property each**, and each failed exactly the test naming it: the scope union
      (4 tests), the held-decision refusal (1), the tool-declaration refusal (1), the empty-surface refusal (1), and
      the caller policy (1).
      **Honest limits.** A remote call is **observable but not attributable**: it is logged and has no `tool_calls`
      row, because that row needs a run — `P3-012` owns the durable link. `spent_budget` is still a literal
      `false`, so the rate limit the gate can apply never fires. The origin list is **not configurable**:
      `build_endpoint` builds `ServerExposure::loopback_only()` through `ServingConfig::new` rather than reading a
      configured list, because a configurable list is a value that can make a startup either fail or serve
      something nobody chose; the caller allowlist is where a deployment says *who* may call. The **token remains
      unvalidated** (no RFC 8707 audience binding, no RFC 9728 Protected Resource Metadata), so a remote caller can
      only be admitted against a fingerprint an operator configured by hand.
- [x] `P3-013` Close the test-scratch directory leak: a fixture's `Drop` cannot await the close that releases the file.
      **Found while running the `P3-009c-a` gate suite, and it is a real defect in the test fixtures rather than
      in shipped code.** Two causes were behind one symptom, and both are now fixed. New code:
      `jarvis_core::remove_scratch_dir` plus the `MAX_SCRATCH_REMOVAL_ATTEMPTS`/`SCRATCH_REMOVAL_INTERVAL`
      constants in `crates/jarvis-core/src/testkit.rs`, all 27 guard `Drop` impls converted, and a source scan that
      keeps future fixtures from reverting. Also `docs/development/testing.md` (the rule) and
      `docs/architecture/repository-layout.md`.
      **Fixed: the naming collision.** ~19 scratch-directory helpers built a path as
      `temp_dir() / format!("jarvis-<what>-{pid}-{sequence}")` with `static …: AtomicU64 = AtomicU64::new(0)`
      beside it — so the sequence starts at 0 in every process and a pid is **reusable**. A directory survives
      whenever a test fails or the suite is killed (no `Drop` runs), and the next run with the same pid reopened
      the previous run's database. Measured: `jarvis-storage` passed **144/144 alone** and produced ~20
      `UNIQUE constraint failed: sessions.id` failures inside a full-workspace run. Now one helper,
      `jarvis_core::scratch_tag()` (a `UUIDv7`), used by all 22 sites, with a test asserting 1,000 tags are
      distinct — because two distinct values would have passed for the broken scheme too.
      **Fixed: the directory is never removed — and the recorded mechanism was wrong in two places.** The entry
      above used to say `sqlx-core-0.9.0/src/pool/mod.rs` has **no `impl Drop for Pool`** and that `Pool::close` is
      an `async fn`. Both are false: `PoolInner` **has** a `Drop` (`src/pool/inner.rs`) and `PoolConnection`
      **has** one (`src/pool/connection.rs`), and `Pool::close` is a **sync fn returning a lazy future**
      (`pool/mod.rs:441`). The real behaviour: `PoolInner::close()` calls `mark_closed()` and closes the
      connections **in the future's body**, `PoolInner::Drop` **never awaits** that future, and
      `PoolConnection::Drop` hands the connection back by **spawning a task**. So the file is released **late and
      off-thread**, and the teardown failure is a **race**, not a permanent hold.
      **Six shapes were measured, and each obvious fix fails for a different reason** — this is the part worth
      keeping:
      - drop then `remove_dir_all` immediately ⇒ **fails 5/5**; drop, `await` 250 ms, remove ⇒ **always Ok**;
      - a bounded **blocking** retry (50 × 10 ms) ⇒ **still fails**: `std::thread::sleep` starves the
        current-thread runtime, so the spawn that returns the connection never runs;
      - a **background thread** retrying `remove_dir_all` for 1 s ⇒ **still fails**, for the same reason: it only
        *waits* for a release that needs the runtime to make progress;
      - dropping the **runtime** first, or `shutdown_timeout(5s)` after ⇒ **fails**; no runtime alive + a real 1 s
        delay ⇒ **Ok**;
      - driving `database.close()` on a **fresh runtime inside a worker thread** ⇒ **DEADLOCKS**, because the close
        future waits on a semaphore only the *original* runtime can release. **A deadlock in teardown is worse
        than the leaked directory**, so that shape must never be shipped.
      **What is shipped:** a **detached thread** that retries, spawned from `Drop` (which cannot await). It does not
      starve the runtime, and it keeps trying across the point where the runtime tears down. The window is
      **measured**: no retry left **124** directories per full-workspace run and this leaves **7–10**. Two variants
      measured worse or no better and are recorded so they are not retried — a **ten-second** per-call window left
      the same handful (a window covers a *delay*, not a release scheduled after it has closed), and an
      **always-running** retry loop left **22**, because a thread competing for CPU across the whole suite delays
      the teardowns that actually release the handles.
      **What remains, honestly:** the residual handful are fixtures that hold an `Arc<SqliteDatabase>` — through a
      `ToolPipeline` — past the directory guard, so the handle is released at the **end of the test** (covered by
      the window) or at **process exit** (which nothing in-process can reach). Those directories **are** removable
      once the process has exited, which is how the race was confirmed rather than inferred. The old "28,226"
      figure is also misleading as a measure of the current suite: **546 of 615** sampled directories came from
      three fixture families (`fnd007`, `fnd013`, `fnd014`) that **no longer exist in the source**, so a historical
      total answers "how bad did it get" rather than "what does the current code do".
      **The guard is a source scan, not a behavioural test**, because no assertion can observe a directory *not*
      leaking without measuring the filesystem around a whole suite. It requires the **discarded-result** form
      (`let _ = …remove_dir_all(&self.0)`), so it catches a bypass without flagging its own explanation or an
      unrelated removal — and it **names the offending file and line** when it fires (falsified by reverting one
      fixture, which it reported at `crates/jarvis-tools/src/workspace.rs:303`). The scan initially matched its own
      prose and read every file in `target/`, which cost **85 s**; skipping comments and reading only `.rs` files
      brought it to **0.4 s**.
      **Test-only**: no shipped path relies on a pool dropping.
- [x] `P3-010` Add MCP Inspector conformance tests and cross-SDK interoperability tests.
      **Split into what is provable now and what is blocked, and the provable half found a real defect.**
      New code: `crates/jarvis-mcp-transport/src/conformance.rs` (4 tests), a vendored derived schema slice at
      `crates/jarvis-mcp-transport/tests/spec/` (11 definitions, 41 KB, with provenance and an extraction
      script), `jsonschema` as a dev-dependency, and `JarvisMcpServer::tools_list_result`. ADR-0040.
      **The defect: `tools/list` was emitting a document the revision's schema rejects.** Read from the official
      **machine-readable** schema rather than the human-readable pages: `$defs.ListToolsResult` declares
      `"required": ["cacheScope", "resultType", "tools", "ttlMs"]`, and `DiscoverResult` requires the same two
      among five. `rmcp` 3.4.0 models both fields and **deliberately leaves them unset** —
      `with_all_items` sets `ttl_ms: None, cache_scope: None`, with the field doc saying *"Required by spec
      version 2026-07-28, but optional here to maintain compatibility with older spec versions"*, pinned by the
      SDK's own `cache_hints_are_omitted_when_absent` test. That default is correct for a **multi-era** server,
      because a field this revision requires would be a spurious field on an older wire. This server narrows to
      one era, so the compatibility argument does not apply and the omission was simply non-conformant.
      **Two asymmetries show it is a per-type default rather than a policy, and they are why the defect was
      isolated rather than systemic:** `DiscoverResult::from_server_info` **does** set `ttl_ms: 0` and
      `cache_scope: Private`, so `server/discover` always conformed; and `CallToolResult` requires only
      `["content", "resultType"]`, so `tools/call` was never affected. The requirement is on *cacheable*
      results, which is why only the list method was wrong.
      **Both values are posture, not tuning.** `cacheScope: "private"` because the endpoint is admission-gated:
      the schema's own distinction is whether a response "does not contain user-specific data" and may be cached
      "across authorization contexts", so answering `public` would tell a caching proxy it MAY serve one
      caller's tool list to another. `ttlMs: 0` because the served set derives from a policy an operator can
      change, so a cached list would keep offering a tool this server had stopped serving — `0` is the schema's
      own wording for "immediately stale".
      **The authority is the specification, not the SDK, and that is the point.** The check has three layers
      that share no assumption: the revision's own schema slice, `jsonschema` (a general-purpose validator with
      no MCP knowledge), and JARVIS's serialization. An MCP-aware validator would have agreed with the bug,
      because the bug **is** an SDK default — the same reasoning `P3-007` recorded for negotiation fixtures,
      one level up.
      **⚠ The first version of the conformance test was wrong and the falsification attempt found it.** It
      assembled its own JSON from `tool_list()` plus the two constants, so **removing the builder call changed
      nothing** — the test never read the builder. The construction was extracted into
      `JarvisMcpServer::tools_list_result`, reachable by both the trait method and the test, so the validated
      document is the value the transport actually sends. *A test that restates the code it checks is not
      checking it*, and a falsification that does **not** fail is a finding about the test rather than a
      reprieve. Removing `.with_ttl_ms(..)` now fails with `"ttlMs" is a required property`, and the slice
      carries a **negative control** proving it rejects the omitted shape, so the check is not vacuous.
      **Honest limits — the live client-level test is blocked, and the reason is recorded rather than glossed.**
      The official `modelcontextprotocol/conformance` suite (Apache-2.0, `npx`) and the MCP Inspector are both
      **third-party clients**, and `jarvisd` builds its endpoint with `CallerAdmission::local_only()`, which
      `P3-009g` made *enforce*: a remote caller is refused even on loopback. So neither can reach an MCP method
      against this daemon until audience-bound tokens (RFC 8707) and Protected Resource Metadata (RFC 9728)
      exist, which remain unbuilt. The live run is therefore named as the **next step** rather than implied to
      have happened. What was researched and kept: the suite's `--requirements <revision>` flag is the form a
      tier claim needs (`--suite`/`--spec-version` describe the suite as it grows, while
      `requirements/<revision>.yaml` is frozen at release), only *scored* scenarios affect the exit code
      (`extension`, `added-after-release`, and `pending` run and are reported but cannot fail), and a scenario
      shared between revisions must run **twice** — once per era — because `2025-11-25` and earlier use the
      stateful `initialize` handshake while `2026-07-28` is stateless with per-request `_meta`.
      Cross-SDK interoperability is likewise blocked for the same reason: it needs a foreign client to connect.
- [x] `P3-011` Define sandbox contracts and implement one restricted process backend before exposing code execution.
  - **Shape of the crate.** `crates/jarvis-sandbox` with `policy.rs` (the request, the guarantees, and the
    refusal), `backend.rs` (the `SandboxBackend` contract, `GuaranteeSupport`, `backend_for_host`, the launcher
    seam), and `linux.rs` (the cgroup v2 backend, `#[cfg(target_os = "linux")]`). `docs/adr/0041` records the
    decision; `docs/research/integrations/os-process-sandboxing.md` records the live documentation evidence.
  - **A guarantee is an enum, not a boolean, and it is refused when it cannot be enforced.** `Guarantee` names
    `TreeTermination`, `ProcessCountCeiling`, `MemoryCeiling`, `CpuRateCeiling`, and `CpuTimeCeiling`;
    `SandboxPolicy::new` resolves a request's `required` list against the backend **once, at construction**, and
    `SandboxError::UnsupportedGuarantee` names the first guarantee the backend cannot provide. There is no
    "applied but weaker" outcome, because a caller who declared a requirement has already stopped checking — so a
    silently weaker sandbox is the one configuration nobody audits.
  - **`unsafe_code = "forbid"` decided the platform split.** Every Windows job-object call is `unsafe` FFI, so
    the job-object backend **cannot exist** in this workspace; Linux cgroup v2 is a **filesystem** interface
    (`pids.max`, `memory.max`, `cpu.max`, `cgroup.kill`), so it needs no FFI and is the backend that exists.
    `backend_for_host()` **probes the running host** rather than reporting the compiled target, and reports an
    **empty** support set on Windows and macOS, which makes every requirement a refusal there.
  - **`CpuTimeCeiling` is deliberately absent from the Linux backend**, and the omission is the load-bearing
    assertion. cgroup v2 accounts CPU time in `cpu.stat` but has **no limit file for a cumulative total** — only
    the rate form in `cpu.max` — while a Windows job object has `PerJobUserTimeLimit` and no rate form. That
    asymmetry is why CPU is two `Guarantee` variants rather than one, and `cgroup_v2_guarantees()` lives in the
    always-compiled `backend.rs` so a **non-Linux** host can still falsify the omission.
  - **`RLIMIT_NPROC` is rejected as a fallback, by construction.** It is counted **per user id**, so a child that
    reaches its own limit can still `fork` — each new process gets a fresh budget under the same uid. Only a
    tree-scoped counter makes a fork bomb fail. Reporting `ProcessCountCeiling` on its strength would be a
    guarantee a fork bomb walks straight through.
  - **Confined before the child exists, with the one unclosable window stated.** Limits are written before the
    spawn (which is what `memory.max` needs, since a limit applied afterwards is applied after the allocation),
    and the spawn is **injected** by the caller so only it decides about pipes. cgroup v2 has no atomic
    spawn-into-cgroup, so the migrate window is recorded as a limit rather than described as closed, and a test
    reads `/proc/<pid>/cgroup` to prove the child really landed in the launch's cgroup.
  - **First caller: `doctor`.** `check_sandbox` reports `sandbox.available` / `sandbox.unavailable` with the
    **facility and the guarantee list** as evidence — the list, not a boolean, because a boolean would let a host
    enforcing one of four read identically to one enforcing all four. Both are `Severity::Info`, following
    `ServiceNotApplicable`: a capability absent **by design** is informational, and `sandbox.unavailable`'s
    remediation names the `systemd Delegate=yes` action that would change the answer.
  - **Falsified.** 24 mutations across the crate and the doctor check, applied one at a time and each restored in
    a `try/finally`: 13/13 for the sandbox crate, 11/11 for the doctor check, with the tree re-verified clean and
    the restored suite re-run green afterwards. Two tests were **strengthened because a mutation survived them**:
    the kill test asserted only `Ok`, which a no-op `kill` also satisfies, so it now asks the operating system
    whether the pid is still alive; and the working-directory test now asserts the temp directory differs from the
    daemon's first, so dropping `current_dir` cannot pass.
  - **Honest limits.** (1) The Windows/macOS backend **does not exist**, so no guarantee is available there and
    enabling code execution needs either a lint relaxation for a job-object crate or a separate helper process,
    with its own ADR. (2) **No network policy, no child filesystem confinement, and no privilege reduction** —
    `Isolation::Restricted` bounds *resources*, and a confined child can still open any file and reach any host
    this process can. (3) The Linux tests are **compiled but have not been executed** in this development
    environment; they skip loudly when no cgroup is delegated, printing what could not be checked. (4)
    `connect_stdio` in `jarvis-mcp-transport` is **not wired** to this crate: that needs a trait seam so the
    transport does not depend on `jarvis-sandbox`, and the slice's phrase "before exposing code execution" is
    satisfied by the contract existing with refusal working, not by an execution path using it.
  - **`cargo clippy --target x86_64-unknown-linux-gnu` is part of this crate's gate**, and earned its place: it
    found six defects invisible on Windows, including an import unused only on Linux, two collapsible `if let`
    chains, identical `match` arms, an error-type mismatch in the kill path, and a duplicated `linux` module from
    declaring `mod linux;` in both `lib.rs` and `backend.rs` (a `#[path]` module declared twice includes the file
    **twice**, as two modules with distinct copies of every type).
- [x] `P3-014` Split the overloaded acceptance skip-guard, because one variable described two artifacts and made CI red on every OS.
      **CI had been red for four consecutive completed runs, and the cause was a configuration value rather than a
      code defect.** `ACCEPTANCE_REQUIRE_BINARIES` was read by two guards that require *different artifacts produced
      by different commands*: the process-level phase gates need the application binaries (`jarvisd`, `jarvis`), and
      the MCP stdio tests need the `fixture-peer` child process. Commit `42f7017` set that single variable on the
      *Test workspace* step, whose command is `cargo test --workspace --all-features`. That builds each package's test
      harness and — because `--all-features` enables the `fixture-peer` feature — the fixture binary, but it **never
      builds `target/<profile>/jarvisd`**, which is one directory above `target/<profile>/deps` where the gate looks.
      So the skip cannot legitimately happen for the fixture and **always** happened for the application binaries,
      and the phase 1 gate inside that step asserted on all three runners.
      **Measured rather than reasoned:** with the application binaries removed, `cargo test -p jarvisd --no-run`
      compiles `jarvisd` but produces **no `target/debug/jarvisd.exe`** — `cargo test` does not place a package's
      binary beside its test harness. The failing assertion was reproduced locally by deleting the two binaries and
      running the gate with the variable set, and it printed the variable's own message ("requires built jarvisd and
      jarvis binaries") while the real question — which binary, and which command produces it — stayed unstated.
      **The fix is per-artifact variables.** The MCP guards now read `ACCEPTANCE_REQUIRE_FIXTURE_PEER`; only the two
      phase-gate steps, which run `cargo build --workspace` first, set `ACCEPTANCE_REQUIRE_BINARIES`. The workspace
      test step sets the fixture variable only. A skip-guard that turns absence into a failure must name the one
      artifact it is about, because the failure it reports is otherwise about a build step the reader cannot see.
      **Both directions were falsified, one property each:** with the fixture binary moved aside and
      `ACCEPTANCE_REQUIRE_FIXTURE_PEER=1`, the stdio suite fails with its actionable message; with the *old* variable
      set and the fixture absent, it skips — so the rename is effective rather than merely documented. The phase gate
      still fails closed when `ACCEPTANCE_REQUIRE_BINARIES=1` and the applications are missing, and skips when it is
      unset.
      **Two failure messages were also made diagnostic.** Each phase gate now names the specific absent binary
      (`jarvisd` or `jarvis`), because the gate cannot know *why* a build is missing and should not imply it does.
      Docs updated: `docs/development/testing.md` states the per-artifact rule and that `cargo test` does not produce
      an application binary in `target/<profile>` at all, and
      `docs/research/integrations/github-actions-rust-supply-chain.md` records the observed failure.
      **Gates:** `cargo fmt --check`, `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`,
      `cargo test --workspace --all-features --locked` **with the application binaries absent** (the exact state the
      workspace step runs in — 0 skips), both phase gates with `ACCEPTANCE_REQUIRE_BINARIES=1`, and `cargo deny check`
      all green.
- [ ] `P3-012` Prove approval restart and duplicate-delivery safety; pass the Phase 3 gate.
  - [x] `P3-012a` A held call persists the durable approval it is waiting on.
        `ToolPipeline::authorize_and_admit` returns `Admission::{Runnable,Held}`; a held decision writes an
        `ApprovalRequest` bound to the call's own canonical intent, and `AwaitingApproval` carries the `approval_id`.
        A held call has no receipt, because a receipt is what an adapter treats as permission and none exists yet.
        The requester is the run; the answer comes from the owner.
  - [x] `P3-012b` Let the owner answer a stored approval. (Simplified by `P3-037` / `ADR-0136`: an answer needs only
        the authenticated request. The decision route and the resumption of the held call are `P3-012c`.)
  - [x] `P3-012c` Link calls to approvals and events, and pass the Phase 3 process gate.
        Delivered as four slices, because each step exposed the next gap:
        - `P3-016` **linked a held call to its approval** (the `approval_id` column existed since `0007` with
          no writer) and wrote `tool_requested` / `approval_requested` into the run's stream.
        - `P3-017` made the **decision carry its approver** — `decode_approval` parsed `decided_by`,
          validated it, and discarded it, so no resume could build an `ApprovalCitation`. `ADR-0043`.
        - `P3-018` built **the resume path and its route**: `ToolPipeline::resume` and
          `POST /api/v1/calls/{id}/resume`.
        - `P3-019` built **`tests/e2e/tests/phase3_gate.rs`**, the roadmap exit gate as a process-level test,
          and wired it into CI.
        **The gate drives the real platform end to end**: it configures one real MCP server over stdio (the
        hand-written `fixture-peer`) with the daemon's **unclassified** posture — risk 3, effects
        `Write`+`ExternalCommunication`, `ApprovalPolicy::Ask`, which is what an operator gets by naming a
        server and saying nothing else — holds a call,
        **kills the daemon**, restarts it, decides the approval, resumes the call, and asserts the
        second resumption is refused. So the hold comes from the product's own configuration path rather
        than from a writable test adapter compiled into `jarvisd`.
        **Falsified end to end**: configuring the server `read-only` instead of unclassified makes the gate
        fail at the hold step (`409` where `202` is required), because a read is risk 0 and is not held.
        **⚠ Two recorded limits, and the gate names them rather than papering over them:**
        1. **An external MCP client cannot yet be admitted.** The daemon builds its served endpoint with
           `CallerAdmission::local_only()`, and `CallerAdmission::new` — the allowlist that would admit a
           credential — has **no production caller**: nothing reads one from configuration. `ADR-0038` makes
           every request over a network listener `Remote`, so no external client can authenticate at all.
           The gate therefore asserts the half that exists — an unauthenticated remote caller is refused
           `401` **through the real listener**, the fail-closed direction — and says in its module docs that
           it is not asserting a `200`. `ADR-0038` records why this is deliberate: a remotely reachable MCP
           server needs **audience-bound** credentials (RFC 8707), so the allowlist's credential vocabulary
           belongs to the OAuth slice, and inventing one here would pre-empt a documented decision about a
           trust boundary.
        2. **A resume does not re-evaluate policy.** The approval was decided against the policy in force
           when the call was held. Whether a policy tightened afterwards should refuse a resume is a real
           question — re-evaluating makes an approval lapse silently; not re-evaluating lets a decided
           action run under a superseded policy — and it is deliberately unsettled rather than settled by
           accident.
        **A falsification that found defence in depth.** Removing the resume's "still `requested`" check
        left the route test **green** with the adapter still reached once, because `execute_and_record`
        advances the call before the adapter and `record_tool_outcome` refuses to replace a terminal
        outcome. So the single-effect property is defended **twice**, and a test written as if one guard
        carried it would be claiming a guarantee two mechanisms provide. The assertion that carries it is
        the adapter's own call **count**, which both the route test and the pipeline test now assert.
        Gates: fmt, clippy `-D warnings`, 44 suites with the application binaries absent and **zero skips**,
        all three phase gates with `ACCEPTANCE_REQUIRE_BINARIES=1`, `cargo deny` ok.
- [x] `P3-020` Add a restricted-execution sandbox backend behind the `P3-011` contracts (a container or an
      equivalent isolated worker), naming the guarantees the backend can actually enforce per `ADR-0041`.
      No new capability surface: the existing code-execution tool gains a backend, and a guarantee the
      backend cannot enforce is refused rather than declared. The `P3-011` backend stays available so a
      deployment without a container runtime degrades to it rather than to nothing.
  - **Shape.** `crates/jarvis-sandbox/src/container.rs` (`ContainerBackend`, `container_arguments`,
    `container_guarantees`), `Support::Container`, `SandboxRequest::image`, and a `wait` method added to
    `LaunchedProcess`. `backend_for_host()` now prefers a **probed** container where the host-process backend
    has nothing. `docs/adr/0128` records the decisions and `docs/research/integrations/container-sandboxing.md`
    the evidence, written before the code.
  - **Three of five guarantees, and the two refusals differ in kind.** `TreeTermination`,
    `ProcessCountCeiling` (`--pids-limit`, container-scoped), and `MemoryCeiling` (`--memory`). `CpuTimeCeiling`
    is refused because the only flag (`--ulimit cpu`) is `RLIMIT_CPU`, **per process**, so a tree of N
    processes gets N budgets — the same argument `ADR-0041` used to reject `RLIMIT_NPROC`. `CpuRateCeiling` is
    refused because the `--cpus` translation is **not written**, which is a fact about this file rather than
    about the facility, and is recorded as such.
  - **A memory ceiling has a floor, and zero is refused separately.** `--memory` below 6 MiB is rejected **by
    the daemon** (exit 125), so it is refused by value rather than passed on as an opaque launch failure; and
    `--memory=0` is *accepted* and read as **unlimited**, the inverse of the field's meaning. Found by probing
    the boundary. A mutation that deleted the explicit zero test **survived**, because zero is arithmetically
    below the floor — so the distinction now lives in the refusal's explanation and is asserted on its own.
  - **The environment boundary is `env -i`, and the first design for it was a no-op.** `-e` **adds** to an
    image's environment (`PATH` and `HOME` survive; probed), while `SandboxRequest::environment` is documented
    as the complete one. A `/bin/sh -c 'export -n -p; …'` wrapper was written and believed correct; probed,
    BusyBox `ash` ignores `export -n` and `-p` only *prints*, so the image's environment survived — and in
    `dash` it would have worked, making confinement a property of the image. The same argv also dropped the
    program, because `shift` skipped a marker `"$@"` had already excluded.
  - **The kill path removes the container, not the CLI.** `Child::kill` on the `docker` CLI leaves the container
    running in the daemon, so `kill` runs `docker rm -f <name>` and then reaps the client. The generated name is
    therefore a **handle**.
  - **Two defects the acceptance test found that the unit tests could not.** (1) The launcher started
    `request.program` — an **in-image** path — instead of the runtime, failing with a bare `entity not found`.
    (2) Because of that, the container argv was built **twice**, so the name generator ran twice and the
    container was launched under one name while `kill` removed another; the leftover sat in state `Created`
    while the kill reported success. Both are why the injected launcher is now `stdio_launcher(piped)` — a
    closure that only decides whether to capture the streams — and the backend builds the command. The ordering
    property is unchanged: the command is built *after* the confinement exists, so `refusing_launcher` still
    proves no process was created.
  - **`LaunchedProcess` gained `wait`.** It had `pid()` and `kill()` and no way to observe a child, so no live
    property of **any** backend was assertable — a handle that could be destroyed but never read. Consuming, for
    the `'static` future; the limitation that a consuming wait cannot be cancelled is recorded on the trait.
  - **The live tests are serialised.** A container runtime keeps one container list for the whole host, so
    parallel tests saw each other's containers and read an absence as a removal. A `tokio::sync::Mutex` is held
    across each live test, and `wait_for_owned_containers` polls because a container is created by the daemon
    *after* `spawn` returns. Both were observed failures, not precautions.
  - **Limits recorded, not closed.** A daemon that ignores `--pids-limit` is undetectable from here (it depends
    on the daemon's cgroup driver and kernel support); `--network none` and `--read-only` are enforced but are
    **not** `Guarantee`s, so a caller cannot require them and `doctor` does not report them — which means a
    container is confined *more* than the guarantee list says; `--security-opt no-new-privileges` is deliberately
    not passed, because unmodelled hardening enforces something a caller cannot see. A request with an `image`
    sent to a host-process backend is refused **by name** rather than served, because confining the `docker` CLI
    would apply `pids.max` to a client while the container's processes ran unbounded — silently, with `doctor`
    reporting four guarantees.
  - **Verification.** Live on this host: `doctor --json` reports `sandbox.available` with
    `facility: container` and `guarantees: tree_termination,process_count_ceiling,memory_ceiling`, where the
    same host previously reported `unconfined`/`none`. Eight tests launch real containers (read-only refusal
    observed as `Read-only file system`, `exit 7` propagating as **7**, kill leaving nothing in `docker ps -a`,
    the image's `PATH`/`HOME` cleared, empty-environment control, both refusals before a container starts,
    argument boundaries intact), and they skip **loudly** when no runtime or image is present. Mutations proved
    caught: `--read-only` removed; the removal skipped (leaves the container, named in the failure); the
    environment boundary skipped; the memory floor disabled; the zero explanation removed.
  - Gates: `cargo fmt --all --check` clean; `cargo clippy --workspace --all-targets --all-features --locked --
    -D warnings` clean; `cargo test --workspace --all-features --locked` **1992 passed / 51 suites**; `cargo
    deny check` ok (advisories, bans, licenses, sources).
- [ ] `P3-021` Add a remote or disposable sandbox backend (a hosted or short-lived worker) behind the same
      contracts, and prove resource limits, crash isolation, output bounding, and cleanup on the adapter
      boundary. The `P3-011` port is unchanged, so this is an adapter slice and not a protocol change.
- [x] `P3-022` Make the model a caller of the tool pipeline: offer it the registry's tools and drive the
      documented tool round trip, so the pipeline's model-facing consumer exists.
      **THE GAP THIS CLOSES WAS RECORDED BY `P2-009` ITSELF.** `MAX_MODEL_CALLS` was `1` with the comment
      *"One, because `P2-009` does not plan or use tools"*; `ChatRequest` had **no `tools` field**, so
      `ToolRegistry::discover()` — the model-facing tool surface — had no consumer; and the `OpenAI` adapter
      deliberately did not decode streaming `tool_calls`. The platform could serve tools over HTTP and as an
      MCP server and could run a tool a caller named, but **the model could not ask for one**: a user asking
      JARVIS to read a file received an answer generated without it. The whole policy/approval/audit pipeline
      was a capability a caller could use and not one the agent could reach.
      **Delivered in two crates, split along the dependency direction:**
      - `jarvis-models` gained the provider-neutral contract: `ToolSpec` (name, description, input schema),
        `ChatRequest::with_tools`, an assistant turn that carries `tool_calls`, and the `OpenAI` wire
        mappings (`WireToolDefinition`, outgoing `WireToolCall`, `WireMessage.tool_calls`). Streaming
        tool-call fragments are reassembled by `WireToolAccumulator` — grouped by the provider's `index`, not
        arrival order — and emitted **once**, at the turn's finish, so a partially written invocation never
        reaches the domain. `tool_choice` is deliberately **not** sent (`auto` is the server default with
        `tools` present, and the research record excludes the field as non-interoperable).
      - `apps/jarvisd` turned `execute_run` into `execute_run_with_tools`: a bounded loop over the documented
        state machine. The offered set is re-derived from `ToolRegistry::discover()` on every call, so it is
        always the current registry and the tool `ToolSpec` schema is the definition the pipeline validates
        against — one statement of the contract. Each invocation goes through `ToolPipeline::call_tool`, and
        the actor is built from the **stored run** (workspace + run id), never from the model.
      **⭐⭐ A TEST FOUND THE RE-ENTRY DEFECT, WHICH IS THE POINT OF WRITING THE TEST FIRST.** The first
      version of the loop ran the tool, observed it, and then went `Observing → Responding` — answering from
      a transcript the model had **not yet seen**. The tool ran and the answer ignored it. The test asserting
      two model calls failed with `left: 1, right: 2`, and the fix is the planning re-entry
      (`Executing → Observing → Planning`) that makes the loop a loop. **A tool round trip has three edges,
      not two, and only the missing third produces the defect** — the run looked successful either way.
      **⭐ THE OFFERED TOOL SURFACE IS A PROPERTY OF THE REQUEST, SO IT IS ASSERTED ON THE REQUEST.**
      `ScriptedModel::seen_tools()` was added beside `seen_messages()` for exactly the reason the messages
      one exists: an executor that attached no tools would satisfy every result-shaped assertion, so "the
      model was offered the registered tools" is unassertable without recording the request's own tool list.
      **⭐ A REFUSAL IS FED BACK, A CONFIGURATION FAULT IS NOT.** A policy denial, schema violation, unknown
      tool, or adapter fault becomes the tool **result** the model reads (with its reason code) so the loop
      continues and the model can recover — a test asserts an unknown tool does not fail the run. A **missing
      tool surface**, by contrast, fails the run, because an answer claiming to have used a tool nothing ran
      is the failure the system prompt warns against. **⭐ A HELD CALL PARKS THE RUN** at `AwaitingApproval`
      and stops; the approval and resume routes complete it, so the executor does not hold a promise
      `security.md` assigns to those routes.
      **Preserved deliberately:** the old contract test asserted a *fragment* is never surfaced; that
      property is kept and strengthened — an invocation missing its id or name is still dropped, while a
      complete one is now surfaced exactly once. **`ADR-0119`.** Gates: `cargo fmt --check`,
      `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`, `cargo test
      --workspace --all-features --locked` (adds 20 model tests and 3 daemon loop tests, including the
      end-to-end "the model calls a real filesystem tool and answers from its result"), `cargo deny check` ok.
      **Limits, recorded rather than glossed:** the executor does not itself resume a decided call (the routes
      do); `MAX_TOOL_CALLS` is a fixed bound rather than policy-derived; the tool surface is offered whole
      with no selection rule (fine at the current tool count, named in `ADR-0119` as the condition for a
      selection or pagination rule); and no live provider has been called, so the loop is proven against the
      scripted adapter and the offline wire fixtures, not a real streaming provider.
- [x] `P3-023` Continue the run a decided approval released: a paused conversation must produce its answer.
      **THE LIMIT `P3-022` RECORDED, CLOSED.** `P3-022` parked a run at `AwaitingApproval` when a tool call
      was held; the approval and resume routes executed the decided effect, but **nothing advanced the run**
      — it stayed at `AwaitingApproval` with its effect already made and its user still waiting. A
      conversation that paused for approval never finished.
      **Delivered:** `apps/jarvisd` gains `resume_run_with_tools` (a second entry point sharing one loop
      body) and `resume_held_call` (reads the decided call's outcome, feds it back as a **fenced**
      observation, and re-enters the model call); `gateway.rs`'s `resume_call` now spawns
      `continue_parked_run` so the run answers after the route executes the effect. Context assembly was
      split into `assemble_context_messages` (returns the messages) from the transition that moved the run,
      so the resume reuses it.
      **⭐⭐ A STATE-MACHINE VIOLATION, FOUND BY A TEST.** The first `park_for_approval` advanced
      `Executing → AwaitingApproval` and was refused by the domain (`run: the agent run transition was
      refused`). **The documented table has `Planning → AwaitingApproval` and NOT `Executing →
      AwaitingApproval`** — a hold is decided when the run *plans* a step, not while one is *running*, and a
      model call runs in `Executing`. Fixed by walking the legal path and narrating the hold honestly:
      `Executing → Observing → Planning → AwaitingApproval`. **A shortcut to a legal state by an illegal
      path is not a state-machine fix; the table encoded the semantics.**
      **⭐⭐ THE RUN CONTINUATION MUST NOT RE-RUN THE CALL.** The resume **route** owns the effect
      (`ToolPipeline::resume` refuses any call past `requested`, `P3-012c`). The first version resumed the
      call from inside the executor too — a test caught it as *"the approved effect must happen exactly
      once — left: 0"*, because the route had not run yet in the test. The fix separates the two: the route
      executes, the run continuation **reads** the stored outcome. Two execution paths is how a single
      effect becomes two; the test asserts the adapter's own call count, not a status.
      **⭐ A RESUMED RUN RE-ASSEMBLES ITS CONTEXT.** A resumed run begins with an empty loop state, so
      without re-assembly the model call would be sent *only* the observation — no policy, no objective, no
      history — and it would answer a message about a tool with no idea what was asked. The test asserts all
      three are present.
      **⭐ THE OBSERVATION IS FENCED, NOT A PROVIDER `tool` MESSAGE.** `tool_calls` stores no arguments
      (`0007`), so the assistant turn that requested a call is not reconstructible and a `tool` result
      cannot be correlated. The outcome is reported through `IsolatedText` (`ADR-0049`) — the vocabulary
      for content originating outside JARVIS. **A recorded limit, not an oversight.**
      **⭐ A RUN PARKED WITH NO PENDING CALL STAYS PARKED.** `recover_interrupted_runs` settles every
      non-terminal run at startup, so a parked run does not survive a restart; reaching the arm without a
      pending call means a caller re-drove a run it has no decision for. Advancing there would act on a
      decision never made. And the route's continuation spawns only when the run **is** parked, so a
      resume of a settled run is a no-op. **`ADR-0120`.** Gates: fmt, clippy `-D warnings`, `cargo test
      --workspace --all-features --locked` (adds 2 daemon park/resume tests, 112 daemon tests total), `cargo
      deny check` ok.
      **Limits, recorded rather than glossed:** the continuation is **in-memory**, so an approval decided
      *after a daemon restart* finds no parked run — persisted approval across a restart is the `P6`
      workflow slice; a resumed run **answers** over the observation rather than re-planning (so it cannot
      chain a second tool call within the same resume); and the observation is fenced data, not a provider
      `tool` message, per the `0007` decision.
- [x] `P3-024` Compose the live model provider from configuration: a configured profile must answer a run
      with a real model instead of the scripted one.
      **THE `P3-022`/`P3-023` LIMIT, CLOSED.** Both slices recorded the same limit — *"no live provider has
      been called"*. The `openai-compatible` adapter existed and passed offline wire fixtures, but **nothing
      in a running daemon could select it**: `executor_model` was compared against one scripted constant, so
      a user could install JARVIS, hold a conversation, and receive a deterministic scripted answer with no
      configuration key that changed that.
      **Delivered:** `jarvis-storage`'s `DaemonConfig` gains `executor_base_url`, `executor_model_name`, and
      `executor_api_key_ref` with matching environment variables and three validation rules;
      `LIVE_PROVIDER_MODEL_NAME` (`"openai-compatible"`) is the single spelling that selects the adapter,
      exported so configuration validation and the composition root cannot disagree. `jarvisd` gains
      `compose_executor` and `compose_model_provider`; `Executor::build(name, provider)` dispatches on the
      name and validates each coordinate through the adapter's own constructors. The configured `model_id`
      is threaded through every executor entry point rather than hardcoded.
      **⭐⭐ THE KEY IS A FILE PATH, NEVER A VALUE.** `to_toml` round-trips a profile and `Debug` is derived,
      so a key **field** would be republished by the very operations that exist to *inspect* configuration
      — and a committed config would contain a live credential. `executor_api_key_ref` is a `PathBuf` read
      **once** at composition; the file's contents never become a `DaemonConfig` field, so they cannot reach
      `to_toml`, the derived `Debug`, or the SQLite profile row. **`ADR-0121`.**
      **⭐ A PARTIAL PROVIDER IS REFUSED, NOT COMPLETED FROM DEFAULTS.** Each field is independently optional,
      which is what makes a half-configured provider *start cleanly and fail on the first run with a vendor
      error instead of a local one*. `IncompleteModelProvider` refuses the live implementation without all
      three coordinates; `ModelProviderWithoutImplementation` refuses coordinates with no implementation —
      the opposite error, a settings block with no effect that makes an operator believe their setting is
      used. The default is "refuse to start", because a daemon whose provider is half-configured is a daemon
      whose answers cannot be trusted.
      **⭐ THE RELATIVE PATH IS REFUSED AT LOAD.** Resolved against a service's working directory, "which
      file?" would depend on the process; `RelativeModelApiKeyRef` answers it once, where the operator can
      see the error.
      **⭐ AN ERROR ABOUT A CREDENTIAL IS A PLACE A CREDENTIAL GETS PRINTED.** `InvalidProviderField` names
      the **field** and not the value, and `Executor`'s `Debug` is hand-written to redact it, so `{:?}` on
      the composed executor is safe by construction rather than by discipline. **Two tests pin this with
      distinct canaries**: a key that is actually a URL is refused while the error renders no part of it,
      and `format!("{executor:?}")` on a composed live executor contains the model id and no credential —
      the latter fails if the hand-written `Debug` is ever replaced by a derive.
      Gates: fmt, clippy `-D warnings`, `cargo test --workspace --all-features --locked` (adds 7 executor
      composition tests and 4 configuration tests; 119 daemon tests), `cargo deny check` ok.
      **Limits, recorded rather than glossed:** **no live provider was called by any test** — composition is
      proven against the scripted adapter and the adapter's own constructor validation, so a live smoke
      against Ollama or OpenAI remains opt-in and credential-gated; the key file is read **once at startup**
      and a rotation on disk needs a restart; there is **no keyring integration** (the path is a plain file
      path); there is **no per-model or per-session routing** (one composed executor, one credential); and
      `executor_model` selects the transport while `executor_model_name` is the id sent to the provider, so
      an operator must set both. Two stale statements were corrected alongside: the scripted model's answer
      text and the `SCRIPTED_MODEL_NAME` doc comment both told the reader to set `executor_model` "once a
      provider adapter is available", which this slice makes false.

## P3-025: Configurable tool approval policy

- [x] `P3-025` Let an operator configure which tools need approval, and per tool.
      **THE VOCABULARY EXISTED AND THE SURFACE TO USE IT DID NOT.** The engine already honored a per-tool
      `ApprovalPolicy::Ask` (`P3-015`), a workspace could deny a tool by identifier, and `WorkspacePolicy::new`
      refused a threshold above its ceiling — but `WorkspacePolicy::default()` was constructed at the
      composition site, `denying()` had **no production caller**, and `config.toml` had no policy field at all.
      So every install decided every tool by the same compiled-in posture, and the "producer with no consumer"
      shape this repository removes was sitting in the authorization path.
      **Delivered:** `jarvis-core` gains `Risk` and `ApprovalPolicy`; `jarvis-tools` re-exports both and keeps
      the effect-floor binding; `WorkspacePolicy` gains `requiring(id, policy)`, `effective_approval`, and
      `approval_overrides`; `evaluate` applies the effective policy at the tool-policy step; `jarvis-storage`
      gains a `[policy]` section (`max_risk`, `approval_threshold`, `deny`, `[policy.approval]`); and
      `compose_workspace_policy` in `jarvisd` translates the document into the policy the pipeline decides
      with. **`ADR-0122`.**
      **⭐⭐ A SHARED DOMAIN TYPE NEEDED BY TWO ADAPTER CRATES BELONGS IN CORE, NOT IN EITHER.** `Risk` and
      `ApprovalPolicy` moved to `jarvis-core` because `repository-layout.md` allows an adapter to depend on
      core and **not on another adapter** — so `jarvis-storage`, which parses the operator's `max_risk`, could
      not name `jarvis-tools`'s type. The workaround would have been a second risk vocabulary in storage:
      **two values that must agree with nothing holding both**, where a drift would compile and an operator's
      configured ceiling would be silently ignored. Core already held `Sensitivity` for the same reason.
      `Risk::declared_for` now takes the floor as a **number** (an `EffectSet` is `jarvis-tools`'s type), and
      `jarvis-tools::declared_for_effects` is the single binding that supplies `EffectSet::risk_floor()`, so
      the rule keeps exactly one home.
      **⭐⭐ AN OVERRIDE IS A `max` APPLIED AT EVALUATION, NEVER AT CONSTRUCTION.** `requiring` stores what the
      operator wrote; `effective_approval(id, declared)` returns `declared.tighter(override)` inside
      `evaluate`. Applying the override when the policy is built would make the stored value the effective
      value, so the override would **replace** the declaration and a looser override would remove a guard —
      exactly what `ADR-0017` rejects by name. The rule lives with the policy, so a future runtime editor
      inherits it rather than reimplementing it.
      **⭐⭐ MY "SAFETY" TRANSLATION WAS ITSELF THE DEFECT.** The first `requiring` rewrote `Ask` into `Deny`,
      reasoning that an override must be unable to lose to a tool declaring `Deny`. But because the override
      is a `max`, `tighter(Deny, Ask)` is **already** `Deny` — no translation was needed — and the rewrite
      broke the common case, turning "hold this tool for approval" into "refuse this call outright". A test
      named for the *other* direction caught it: *"a stricter override must win, or the override does
      nothing"*. **A second guard that also changes the meaning is not a guard.**
      **⭐ A PREDICATE'S DIRECTION IS NOT INFERABLE FROM ITS NAME.** `tighter` reads correct whether it
      returns the stricter or the looser operand. `ApprovalPolicy::strictness()` (Auto 0 → Policy 1 → Ask 2 →
      Deny 3) is the mechanism, and the tests state the adversarial case **in their names** (*"a workspace
      override must not relax a tool that declares Ask"*, *"an override cannot make a denying tool
      runnable"*) plus a **sweep over every (declared, override) pair**, because a single hand-picked
      assertion written the wrong way round survives an inverted comparison. Both complements are asserted:
      the sweep proves nothing relaxes, and *"a stricter override wins"* is the control that stops "always
      return the declaration" from passing.
      **⭐ THE DOCUMENT IS A SEED, AND ITS ABSENCE IS THE DEFAULT.** `[policy]` is `#[serde(default)]`, so a
      document written before the section existed still loads — the safe direction, because an absent section
      means "no operator opinion" rather than a refusal to start. Two mistakes are refused as
      self-contradictions: a threshold above the ceiling (every risk that could be approved is already
      refused) and a blank identifier (applies to nothing while reading as a restriction). An entry naming
      an **unregistered** tool is deliberately **not** refused: MCP servers are discovered at startup and one
      may be down, so an inert entry is honest whereas a refusal would make a valid policy unable to start.
      Gates: fmt, clippy `-D warnings`, `cargo test --workspace --all-features --locked` (adds 8 core
      vocabulary tests, 6 direction-rule and override tests, 4 config tests, 5 composition tests; 124 daemon
      tests, `jarvis-tools` at 187), `cargo deny check` ok.
      **⭐⭐ A COMPOSITION-ORDER DEFECT, FOUND BY ASKING WHERE THE NEW CODE IS REACHED.** `compose_tool_pipeline`
      returned `Ok(None)` early when no roots and no MCP servers were configured, and the policy was built
      **after** that return — so a document with an unusable policy was **accepted and silently ignored** by
      exactly the daemons most likely to have a stale one. Building the policy before the return fixes it, and
      the test for it was proven to falsify: reintroducing the early return made the test fail. **A new
      validation on one path is not a validation unless you check which paths reach it.**
      **Limits, recorded rather than glossed:** the document is read **at startup** and there is **no runtime
      policy surface** — a CLI or control-plane UI that edits policy while the daemon runs needs a durable
      workspace row and a reload path, and that is the next slice; the tool-identifier vocabulary is split
      (`PolicyConfig` holds `String`s and the composition root parses them, because `ToolId` belongs to
      `jarvis-tools`), so a malformed identifier is a **startup** error rather than a parse error; an override
      can never relax a tool's declaration by design; and `[policy.approval]` cannot express "ask, whatever
      the tool declares" for a tool that declares `Deny`, because `Deny` is absolute.

## P3-026: The tool control plane

- [x] `P3-026` Let an operator read the policy in force and preview a decision, from HTTP and the CLI.
      **THE QUESTION `P3-025` CREATED AND COULD NOT ANSWER.** `P3-025` made a tool's approval policy
      configurable, which made *"did my configuration take effect?"* answerable in principle and
      unanswerable in practice: a configuration document is the **input**, while the workspace policy is what
      `evaluate` enforces. An override is applied as a `max` at evaluation time, a denial short-circuits
      before several checks, and the actor's scopes are derived rather than configured — so a correct document
      still does not describe the posture in force, and the only way to learn it was to attempt a call and
      read the refusal. There was also **no `GET /tools` at all**: `ToolRegistry::discover()` existed with no
      route, so a CLI or control-plane UI could not enumerate what was configurable.
      **Delivered:** `jarvis-protocol` gains `tool_api` (`ToolReply`, `ToolListReply`,
      `ToolPreviewRequest`, `ToolPreviewReply`); `ToolPipeline` gains `policy_inventory`, `preview_call`, and
      `workspace_policy`; `jarvisd` gains `GET /api/v1/tools` and `POST /api/v1/tools/{tool}/preview`; the CLI
      gains a `jarvis tools <list|preview>` group with `--escalation`, `--channel`, and
      `--json`. **`ADR-0123`.**
      **⭐⭐ A PREVIEW IS A DECISION, NOT A PREDICTION, AND IT WRITES NOTHING.** `evaluate` is a pure function
      of declared facts, so `preview_call` calls the **same** function the tool-call path calls over the same
      workspace policy and definitions — the answer is exact for the context supplied rather than an estimate.
      It takes no `run_id` and touches no table, because a preview that recorded a call would let an operator
      fill the ledger by looking at it, and one that consumed an idempotency key would make the real call a
      duplicate. The property is asserted by **counting `tool_calls` rows** before and after, because a
      `requested` row would look identical in the preview's own response — which is why
      `jarvis_storage::count_tool_calls` was added: an assertion on the response cannot see the write.
      **⭐⭐ THE LIST REPORTS THE DECLARED AND THE EFFECTIVE POLICY, AND NAMES AN OVERRIDE AS ONE.** Both values
      are reported because they differ for a reason worth seeing: a tool held by the workspace *threshold*
      looks identical to one held by its own *declaration* unless both are visible. `overridden` is **derived**
      rather than stored, so the flag cannot disagree with the two policies it describes. The reply also
      carries the workspace ceiling and threshold, so one tool's posture is readable against the ceiling it
      sits under. **Falsified**: making the projection report `declared` as `effective` fails with
      `left: Auto, right: Ask`, so the assertion is not vacuous.
      **⭐ THE CLOSED SETS ARE `jarvis-core`'s TYPES ON THE WIRE, WHICH IS `ADR-0122`'s PAYOFF.** This crate
      depends on `jarvis-core` and **not on `jarvis-tools`**, so a `String` field plus a hand-written
      membership check was the only alternative — and three such checks were written and then **deleted** once
      the move made `Risk`, `ApprovalPolicy`, and `EscalationSignal` nameable. A mistyped policy is now a `422`
      naming the field rather than a default, and a default here would be the *most permissive* reading of a
      value the caller mistyped. `EscalationSignal` moved to core in this slice for exactly that reason.
      **⭐ THREE STATUSES STAY DISTINCT BECAUSE THE REMEDIES DIFFER.** An unknown tool is `404` rather than a
      refusal, because a typo and a policy denial have opposite remedies. No tool surface at all is `404`
      rather than an empty list, because "nothing is configured" and "this daemon cannot serve tools" are
      different deployment facts. A **denied** tool is still `callable: true`, because the denial is a policy
      fact rather than an availability one — conflating them would send an operator to fix a grant when the
      remedy is a configuration line.
      **⭐ A CALLER SUPPLIES CONTEXT; THE DAEMON SUPPLIES AUTHORITY.** A preview body carries only `channel` and
      `escalation` — the parts of the call only the caller knows. Scopes, the workspace
      policy, and the definition come from the daemon, and `deny_unknown_fields` makes an attempt to name them
      a `422` rather than an ignored value. A test asserts a body naming `scopes` is refused, so the preview
      cannot become an authorization oracle.
      **⭐⭐ A TEST CAUGHT AN OFF-BY-ONE IN THE CLI FLAG PARSER.** `preview_request` scanned from index 2, which
      is the **tool identifier** rather than the first flag — so every preview would have failed with
      `unknown tools option "jarvis.files.read"`. It was caught because the parser is tested **directly**
      rather than through the rendered output: a rendering test would have shown a plausible error and the
      cause would have looked like a bad identifier. Two more CLI tests assert that an unknown escalation,
      or channel is a **usage error** rather than an ignored flag, because silently dropping an
      escalation computes the preview for a call *without* it and reads as more permissive than the user asked
      about — with a `--json` control so a parser that refused every flag cannot pass.
      Gates: fmt, clippy `-D warnings`, `cargo test --workspace --all-features --locked` (adds 10 gateway
      route tests, 3 CLI parser tests, 3 core signal tests, and the storage count; 134 daemon tests, 50
      suites), `cargo deny check` ok.
      **Limits, recorded rather than glossed:** the surface is **read-only** — editing policy at runtime needs
      the durable workspace row and reload path `ADR-0122` records as absent; the list is **unbounded**,
      relying on `MAX_REGISTERED_TOOLS` rather than a page parameter; a preview **cannot express a
      hypothetical authority**, deliberately, since accepting scopes or a workspace policy would make it an
      authorization oracle; and a preview does **not** read a target out of the tool's arguments, so the
      escalation signals are the caller's to supply — the same limitation `call_remote_tool` records for the
      same reason. Two CLI observations checked rather than assumed: an **unknown sub-verb** reports daemon
      unreachability before naming the sub-verb, because the client is built before `tools::run` is called —
      **verified to match `jarvis memory` exactly**, so this is the established convention rather than a bug in
      the new group, and changing it here alone would make the two groups inconsistent; and `jarvis tools list`
      against a daemon with **no tool surface** exits through the `Denied` path, which is the documented
      mapping for a refusal (the `404` body's remedy text is what distinguishes it).

## P3-027: Web fetch tool

- [x] `P3-027` Give the model a way to read the public web, behind the same policy gate as every other tool.
  - Delivered: `crates/jarvis-web` (new) with `jarvis.web.fetch`, composed in `jarvisd` beside the memory tool. A
    bare `GET` of a public `http`/`https` URL; the body comes back as fenced untrusted text. 29 tests in the crate,
    plus 2 through the real pipeline in `jarvisd` (held by default; unattended is still refused by the address guard).
  - Research: [web-fetch.md](docs/research/integrations/web-fetch.md). Decision: [ADR-0129](docs/adr/0129-a-fetch-is-checked-on-the-address-it-connects-to.md).
  - Boundary: addresses are checked after resolution (allowlist of global unicast, embedded IPv4 judged by what it
    embeds), the connection is pinned to the checked answers, every redirect hop is checked again, only ports 80/443,
    no credentials/headers/cookies/proxy/decompression, body read stops at 256 KiB.
  - **Held for approval by default** (`read_only`, risk 2, approval `policy`). Unattended use is the operator's opt-in
    (`policy.approval_threshold = "high"`).
  - Falsified: the address rule mutated to accept everything fails 7 tests, including the one asserting a loopback
    server received **zero** connections. Gates: fmt, clippy `-D warnings`, `cargo test --workspace` (2,023 tests),
    `cargo deny check`.
  - **Limits:** a model-chosen URL can still carry data out once approved or opted in (not solvable by the tool);
    the model sees at most 1,800 characters because the executor truncates every tool result to 4,000 (follow-up:
    per-tool result budget and an `offset` argument); non-UTF-8 charsets are decoded lossily; no live-internet test;
    `jarvis tools list` and the CLI were not exercised against a running daemon with this tool.

## P3-028: Approve from the CLI, and the defects that flow exposed

- [x] `P3-028` Let a person see, decide and finish a held action from the shipped product. Found by driving a real
  model (Ollama `glm-5.3:cloud` through a local Ollama) and the real network through `jarvisd` and the CLI.
  - **Live result:** "fetch https://example.com" → model requests `jarvis.web.fetch` → run parks (CLI exit 11) →
    `jarvis approvals approve` → the page is really fetched → the model answers "Example Domain". Also verified live:
    deny, a refused loopback URL, and kill-and-restart of the daemon while an approval was pending.
  - **Four defects found by that walk, each fixed with a test that failed first or asserts the fix:**
    1. `jarvis-models`: **a stream whose events arrive in one network chunk was reported `incomplete`.** The SSE
       decoder returns one event per call and the adapter never drained it before awaiting the next chunk, so the
       finish reason and `[DONE]` sat unread. Every fixture sent one event per chunk. Test:
       `several_events_in_one_network_chunk_are_all_delivered`. *Every live run against Ollama failed before this.*
    2. **A denied approval left its run parked forever.** Now the executor tells the model it was declined and the
       run answers (`resume_run_declined`; test asserts the tool ran zero times).
    3. **A daemon with a parked run would not start** (recovery tried `awaiting_approval → failed`, which the state
       table forbids). Recovery now leaves a parked run alone. Amends `ADR-0013`.
    4. **A person could not see what they were approving, or release a held call without a client-side copy of its
       arguments.** A pending approval now holds its arguments until decided ([ADR-0130](docs/adr/0130-a-pending-approval-holds-what-it-is-waiting-on.md),
       migration `0013`, schema version 13).
  - Delivered: `GET /api/v1/approvals`; `jarvis approvals list|approve|deny`; `jarvis ask` exits 11 with the approval
    id instead of "stalled"; docs (`getting-started.md` was also stale about what exists).
  - **Limits:** an expired pending approval keeps its payload until a retention sweep exists; a declined call's
    `tool_calls` row stays `requested`; `approve` resumes the call from the arguments the CLI was shown, so a daemon
    restart *between* the decision and the resume left the call approved but not run (closed by `P3-030`); SQLite only (no Postgres `approvals` migration
    exists yet).

## P3-029: The model can run code, in a disposable container

- [x] `P3-029` Give the model a way to calculate, behind the sandbox and the approval gate.
  - Delivered: `jarvis.code.run` (`apps/jarvisd/src/code_run.rs`), `LaunchedProcess::wait_for` (bounded wait that
    stops the work), operator config `daemon.code_sandbox_image` / `daemon.code_sandbox_interpreter`, a shared
    per-result budget (`MAX_MODEL_FACING_RESULT_CHARS` 12,000, from 4,000; the fetch cap rose from 1,800 to 4,000),
    and multi-line programs shown as lines by `jarvis approvals`. [ADR-0131](docs/adr/0131-model-authored-code-runs-in-a-disposable-container.md).
  - **Live:** asked for the sum of the first 100 primes, the model (Ollama `glm-5.3:cloud`) wrote JavaScript, I read and
    approved it, it ran in `node:22-alpine` with no network, and the answer (24,133) was right.
  - **Always held:** `code_execution` at risk 3 with `ApprovalPolicy::Ask`, which a workspace cannot relax
    (`ADR-0122`); test raises ceiling and threshold to `High` and still gets a hold.
  - **Falsified against a real runtime:** network blocked, root and `/tmp` read-only, host environment absent, a
    runaway snippet stopped and its container *removed* (asked of `docker ps`), unbounded output cut and still inside
    the budget, output unable to close its own fence. Tests skip loudly without a runtime or image.
  - **Limits:** the cgroup backend has no `wait_for` yet (the default refuses rather than waiting unbounded); no files in
    or out and no state between runs; limits (30 s, 64 processes, 256 MiB) are constants; a program too large for the
    8 KiB approval payload cannot be approved from the CLI; `jarvis.code.run` only exists when an image is configured,
    pulled, and a runtime is reachable.

## P3-030: An approved call survives a daemon restart

- [x] `P3-030` Close the limit `P3-028` recorded: a daemon that died between a decision and the release left a call that
  was approved, never run, and impossible to finish.
  - Approved approvals keep their arguments until the call runs (denials clear them at once); the decision route takes
    an opt-in `resume: true` and releases the call from the held arguments; `POST /approvals/{id}/resume` and
    `jarvis approvals resume ID` finish one that was not released; `GET /approvals` lists `approved` ones. The CLI no
    longer re-sends any payload. Amends [ADR-0130](docs/adr/0130-a-pending-approval-holds-what-it-is-waiting-on.md).
  - **Live:** approve, kill the daemon, restart, `list` shows approved-not-run, `resume` fetches the page, run completes.
  - Tests: 3 gateway tests (release exactly once and arguments cleared; finish later without re-sending; a denial can
    never be released), 2 storage tests (approved keeps, denied clears).
  - **Limits:** nothing releases an approved call *automatically* at startup (a person runs `resume`); an approved
    approval whose call is never released keeps its payload until a retention sweep exists.

### P3-031: First run — `jarvis init` and `jarvis start`, and a multi-call run bug found by using it

- [x] A fresh install reaches a conversation in three commands. `init` detects a local Ollama, refuses a model it does
  not list, writes the key file and a configuration validated by the daemon's own parser, never overwrites without
  `--force`, and grants no folder unless one is named. `start` spawns the sibling `jarvisd` detached and waits until it
  listens, or reports that it exited. (`apps/jarvis-cli/src/init.rs`, 6 unit tests.)
- [x] **Defect found by the live run:** a second tool call in one run failed with "failed to admit a tool call". The
  call's identity is its correlation identifier, and the executor passed the run's single one to every call, so the
  second collided on `tool_calls.id`. Every earlier test and live run made one call per run. Fixed in
  `executor::run_tool_call` (a fresh identifier per call); regression test `a_run_can_make_two_tool_calls`,
  falsified by restoring the shared identifier.
  - **Live:** `init` → `start` → `ask` with no environment variables; the model listed a folder and read a file in one run.
  - **Limits:** `init` needs an existing `--root` directory; a hosted provider is checked only for a readable key file,
    not a live call; no Windows service or autostart.

### P3-032: Less asking — approval is for what can hurt, and the owner can decide once

- [x] `jarvis.web.fetch` is risk 1 and runs under the default workspace (`ADR-0133`, amends `ADR-0129`). Residual
  exfiltration-by-URL risk is bounded (2,048-character URL cap, address rule, audit) and documented in `security.md`;
  `"jarvis.web.fetch" = "ask"` restores the hold.
- [x] `[policy] trust = [...]` / `WorkspacePolicy::trusting`: the owner's advance decision waives a tool's `Ask`, the
  risk threshold — and not the ceiling, a denial, scopes or the external-communication
  rule; never consulted for externally communicating tools; a denial or approval override wins in either order.
- [x] `jarvis init --code-image IMAGE [--code-interpreter "node -e"] [--trust-code]` writes the code sandbox and, only
  on request, the trust.
  - Tests: `standing_trust_waives_the_ask_and_nothing_else` (7 limits), `a_trusted_code_tool_runs_without_a_hold`,
    `a_web_fetch_runs_by_default_and_an_ask_override_holds_it`, a composition test, 2 init tests.
  - **Live:** with `--trust-code`, one prompt fetched example.com and ran a prime-sum snippet in `node:22-alpine`
    (answer 5117, correct) with **no approval requested**.
  - **Limits:** no "always allow" at the approval prompt (needs a writable policy); trust is per tool, not per argument.

### P3-033: The kill switch — `jarvis cancel`, and a parked run that actually stops

- [x] `jarvis cancel RUN` (identifier prefix; ambiguous or finished refused) and `jarvis cancel --all` (everything not
  yet settled). Reports the request, not a stop that has not happened.
- [x] **Defect found by the live run:** cancelling a run parked at `awaiting_approval` recorded a request nobody read (a
  parked run has no driver), so the run stayed in the approvals list for ever. `settle_parked_run_cancelled` now settles
  it directly (`awaiting_approval → cancelled`, refused for a run with a driver) and
  `withdraw_cancelled_run_approvals` expires its pending approvals (payload gone) and drops approved-but-
  unreleased arguments, so `resume` cannot run an action for a cancelled run. Restart recovery runs the same sweep, which
  also repaired the orphan the live run had already left. Amends `ADR-0013`.
  - Tests: storage `cancelling_a_parked_run_settles_it_and_a_running_one_is_refused`, gateway
    `cancelling_a_parked_run_withdraws_its_approval` (cancelled, unlisted, undecidable, tool never ran), 2 CLI selection tests.
  - **Limits:** a run in the middle of a model call stops at its next step boundary, not instantly; no `jarvis stop`
    for the daemon itself; no sub-agent supervision yet (nothing to supervise).

### P3-034: Sub-agents — delegation to a staff of ordinary runs

- [x] `jarvis.agent.delegate` (optionally `background`, for parallel work) and `jarvis.agent.result`
  (`apps/jarvisd/src/delegate.rs`, `ADR-0134`): a sub-agent is an ordinary run with the same policy, approvals, audit and
  budgets, visible in `jarvis runs` as `[sub-agent]` and stoppable with `jarvis cancel`; delegation is one level deep (the
  executor withholds `jarvis.agent.*` and the scope from a run whose objective carries the notice); at most 4 active; the
  answer is fenced untrusted data; `result` refuses any run that is not a sub-agent of the workspace; a parked
  sub-agent is reported as waiting for the person.
  - Tests (7): delegate end to end with a fenced answer, a sub-agent is offered and granted no delegation, `result`
    refuses an ordinary run, a parked sub-agent is reported waiting and not moved, definitions, marker, objective fit.
  - **Live:** two sub-agents in parallel with `background: true` (fetch a page; compute with code), both collected by the
    parent and reported correctly.
  - **Limits:** the parent is not notified when a parked sub-agent is later approved; sub-agents share the parent's
    model and tool set minus delegation; cost scales with the number of sub-agents (cap 4).

### P3-035: Visual presence, first step — `jarvis watch`

- [x] `jarvis watch [--once] [--interval N]` (`apps/jarvis-cli/src/watch.rs`): one screen of the work, not the transcript —
  **waiting for you** (each approval with its tool, risk, arguments and the exact approve/deny commands), **working**
  (runs and sub-agents, state and age, with the cancel commands), **scheduled** (next fire and cadence) and **recent**
  (outcome and answer). Reads `/runs`, `/approvals` and `/schedules` and keeps no state of its own, so it cannot disagree
  with `jarvis runs`, `jarvis approvals` or `jarvis schedule`. Plain ASCII; prints once when output is not a terminal.
  - Tests (4): spans and clipping, idle screen, and the section order and content of a busy screen.
  - **Live:** during a parent run with two background sub-agents the screen showed all three working with ages and
    `[sub-agent]` labels, then the parked code run under WAITING FOR YOU with its approve/deny commands.
  - **Limits:** a terminal view, not yet the heads-up display the product is aiming at (no browser or overlay surface);
    no keypress approvals; no per-run event detail.

### P3-036: Visual presence — the heads-up display (`jarvis hud`)

- [x] The daemon serves a static display at `/` (alias `/hud`) (`apps/jarvisd/src/hud.rs`, `ADR-0135`): an orb that is idle, working,
  or amber and pulsing when something **needs you**, plus waiting-for-you (with the exact approve command and a copy
  button), working (with **Stop** per run and **Stop everything**), scheduled and recent. `jarvis hud` opens it with the
  credential in the URL fragment, which the page moves to session storage and removes from the address bar. No
  dependency, no build step.
  - Tests: public assets are exactly two `GET` paths and nothing else became public (route test, both sides, including a
    `POST` and sibling paths), the script never inserts markup / inline script / query credential, the fragment form.
  - **Live in a browser:** the page rendered the parked code approval and its run, Stop cancelled the run and the
    approval vanished, the orb returned to idle; a reload kept working from session storage.
  - **Limits:** polling every 1.5 s rather than a
    push stream; one workspace; no per-run event detail yet.
- [x] **Upgraded to a full console** (same slice): animated canvas orb whose waveform follows the mic or the spoken
  answer, a streaming conversation over the run's SSE (tool chips, safe markdown, session kept across turns), the four
  panels, keyboard shortcuts (`/`, `M`, `Esc`), `/hud.css` with `style-src 'self'` and a microphone-only
  `Permissions-Policy`. **Voice (first slice of `P8-010`, browser-native):** push-to-talk, wake word "Jarvis", spoken
  answers, interruption, and the hands-free kill switch ("Jarvis, stop"). Tests: no markup insertion, no outside
  origin, no `MediaRecorder`, no inline style. **Live:** a streamed answer with tool chips and two parallel sub-agents
  visible in the panels; the stale-cursor and raw-markdown-in-panel defects found by looking at it were fixed.
- [x] **The face** (2026-10-06): the orb became a film-style interface (concentric ticked rings, segmented arcs, name
  ring that fills one turn, radar sweep, radial voice spectrum, arc-reactor core, sight lines and brackets) in a
  three-column HUD layout with framed panels and a telemetry panel; state shown by colour and tempo. **Live:** idle and
  working states checked in a browser while two sub-agents ran; a name-ring overprint and an undersized face found by
  looking at it were fixed. See `ADR-0135`.
  - **Limits:** the microphone path could not be exercised here (no audio device); browser recognition may use the
    vendor's cloud (Chrome/Edge); no file attach; no spoken approval; Firefox has no speech recognition.

### P3-037: Light approvals — an approval is a yes or no from the owner

- [x] `ADR-0136`: deciding whether to ask stays deterministic policy (risk threshold, per-tool `ask`/`deny`, always-ask for
  tools that talk to other people, `trust`); answering is one click, one command or one spoken word from any surface that
  holds the local credential. `POST /api/v1/approvals/{id}/decision` takes `{ decision, resume, channel? }` and the bearer
  credential alone; an old-style body that still sends a code is refused with a validation error.
- [x] Removed from the code and the docs: the one-time decision code and its delivery file, the authentication-strength
  levels with the per-channel ceiling, and the distinct-approver rule. Kept: the digest that binds an approval to the
  exact arguments shown, arguments held until the call runs, expiry, withdrawal on cancel, restart survival, and a decision
  that records the outcome, the surface, the instant and a plain approver label. Three older database columns stay in the
  table (they cannot be dropped safely) and are written with fixed values in one isolated place.
- [x] Console: **Approve** / **Deny** buttons on the waiting panel, a spoken announcement when something new needs an answer
  (spoken answers on), and a bare "yes"/"no" that answers the single waiting question (several waiting: it asks you to use
  the buttons). A conversation that no longer exists starts a fresh one instead of failing.
  - Tests: storage approval tests restored and adapted after review found a dozen deleted tests of behaviour that stays;
    gateway tests for deciding with the credential alone, 401 without it, and refusing an old-style body; policy and
    pipeline tests updated. 2,103 tests pass.
  - **Live, real model:** a held fetch appeared amber with its exact arguments; one click on Approve ran it and the answer
    came back; a second held fetch was denied from the CLI and the model said it had not fetched the page. No approval-code
    directory exists on disk any more.
  - **Limits:** a spoken answer is as reliable as the browser's recognition (the microphone path could not be exercised
    here); with several approvals waiting a word cannot pick one; no per-surface restriction on who may answer, deliberately.

### P3-038: Write and edit files in granted folders

- [x] `ADR-0137`: `jarvis.files.write` (create or append, never replaces; runs by default) and `jarvis.files.edit`
  (replace one exact text; asked about once, waived by `trust`), both confined by the same directory handles as reads,
  content capped so a held call stays decidable, and not served to remote MCP callers.
- [x] A failed tool call now tells the model why (also after an approval), and a repeated identical request that was already
  answered is refused with a sentence instead of a database error (both found live).
  - Tests: create/parents/no-overwrite/append, traversal + absolute + link escapes for write and edit, oversize, unique
    replace, not-found/ambiguous/identical, CRLF, no temp file left, MCP exclusion, repeated request.
  - **Live, real model:** asked to create `shopping/list.txt` it ran without asking; an edit was held with its exact
    arguments, approved from the CLI and applied with no temp file left; an edit of text that is not there came back as
    "the text to replace was not found" and the model reported exactly that.
  - **Limits:** no delete/move/overwrite; one approval per edit; 4,000 characters per write call.

### P3-039: The console gets a real head, a "can do" panel, and opens on start

- [x] The face is now a real human head: MediaPipe's canonical face mesh (Apache-2.0, from upstream, recorded in
  `docs/research/integrations/mediapipe-canonical-face-model.md`; not copied from `example/`) drawn as a hologram in plain 2D
  canvas inside the existing rings, with a wire skull and neck. Jaw opens with speech, eyes glow and follow the pointer,
  brows lift when something needs you, it leans in while listening, offline it dims with eyes shut.
- [x] A "Can do" panel lists every tool as **runs**, **asks** or **off**. The daemon says which (`asks_first` on
  `GET /api/v1/tools`); a test pins it to `evaluate` over 180 effect/risk/approval/posture combinations (it caught a
  divergence: a workspace denial is checked apart from the approval policy).
- [x] `jarvis start` opens the console (`--no-open` skips it); an already-running daemon just opens it.
  - Tests: head asset is public and `GET` only, pinned to 468 vertices / 898 in-range triangles with attribution kept,
    no outside origin or markup insertion; `asks_first` parity.
  - **Live, real daemon:** the head renders in all states (mouth open, amber brows, green lean); the panel showed 8 tools
    with only "Edit a file" asking.
  - **Limits:** the head is a mask plus wire skull (no hair or shoulders); speech mouth motion is a synthetic envelope, not
    phoneme-driven; no settings editing in the console yet (policy and folders are still `config.toml` / `jarvis init`).

### P3-040: A real voice (neural speech through the daemon)

- [x] `ADR-0138`: `crates/jarvis-voice` (ElevenLabs text-to-speech adapter, verified against the live docs), `GET/POST
  /api/v1/speech` behind the credential, `daemon.speech_api_key_ref` / `speech_voice_id` / `speech_model`, and
  `jarvis init --elevenlabs` (key from `ELEVENLABS_API_KEY`) / `--voice-key-file` / `--voice-id` / `--voice-model`.
- [x] The page plays the daemon's audio with the next sentence group made while one plays, interruption aborts it, the head's
  jaw follows the real audio level, and the browser voice is the fallback, now ranked sensibly (Natural/Online first,
  British male preferred, voices re-read as they load, no pitch hack) and honest when it only has a basic one.
  - Tests: adapter contract tests against a local fake provider (request shape, key never in errors or `Debug`, status
    mapping, bounds, malformed settings); route tests (401 without the credential, 404 when not configured, audio proxied,
    key never reported); config validation; init rendering and flags.
  - **Live:** the page path ran end to end with the speech endpoints mocked in the browser (violet speaking state, mouth open
    with the audio, a new message aborting the in-flight request); later against a real account (below).
- [x] Streamed speech (found by the user: the face "spoke" before any sound, and the voice lagged the text). The daemon passes
  the provider's `/stream` chunks through instead of collecting them (`SpeechStream`); the page speaks each sentence as soon
  as it is complete, plays progressively (Media Source, whole-blob fallback), keeps two pieces loading ahead, sends the previous
  sentence as `previous_text` (not for `eleven_v3*`, which refuses it), and shows "speaking" only while sound plays ("preparing
  voice" before). Measured live: first sentence complete at 2.6 s, request 2.64, first byte 2.92, playing 2.96, face "speaking"
  3.03 s. `eleven_v3` (set by mistake while chasing a stale tab) was 2.4 times slower and was put back to `eleven_v4_turbo`.
  Tests: first chunk available before the provider finishes; `previous_text` sent only to a model that accepts it, trimmed.
  Not done: the WebSocket input-streaming endpoint (a candidate, not a bottleneck).
  - **Limits:** text spoken leaves the machine when a key is set; per-character cost with no cap yet; regional endpoints are
    not configurable; speech to text is still the browser's.

## P4: Memory And Context

- [x] `P4-001` Define memory types, provenance, confidence, validity, sensitivity, correction, supersession, and retention semantics.
      `crates/jarvis-core/src/memory.rs`, built from `docs/architecture/memory-and-context.md` rather than
      invented, and the module's central claim is the document's own sentence taken literally: **a memory is a
      sourced claim with lifecycle metadata, not an unqualified string**. `MemorySource` is a **required**
      field and `MemoryRecord::new` refuses a record without one, so the admission lifecycle's rejection step
      ("not supported by source" → do not persist as fact) is enforceable rather than a review note — there is
      no way to build the value the rule forbids.
      **The invariants are constructor rules, not advice.** Three are worth naming because each closes a path
      a caller could otherwise take:
      - **A model inference cannot exceed `Unverified`.** "Never persist unsupported inference as fact" is a
        refusal at construction, so a candidate the model produced cannot be recorded as `Confirmed` — the
        acceptance invariant "an inferred preference never appears as confirmed fact" has no way to be
        violated by a caller that tried.
      - **A provider record cannot back a preference.** The document's own example of trust that does not
        transfer between claim kinds ("authoritative for an event timestamp but not for a person's
        preference"), stated once at the constructor instead of left for each reader to apply.
      - **A source cannot claim a trust class its kind does not carry.** `MemorySourceKind::permitted_trust`
        is an equality, so `ExternalContent` cannot claim `Authoritative` — the injection boundary is a
        property of construction rather than of every consumer. The default is `ExternalContent`/`Untrusted`,
        because a default is what a deserialiser fills in when a field is absent and an unrecorded origin
        must not read as something the user said.
      **Deliberate design choices, each with a rejected alternative:**
      - **Confidence is four named levels, not a float.** A `f64` invites arithmetic that means nothing (two
        confidences averaged are not a third) and lets a caller threshold at `0.6` with a meaning that changes
        across builds. The levels are ordered for a **floor** comparison and never for arithmetic.
      - **`Expired` is not a stored status.** Same split as `ApprovalState`: a stored `expired` would need a
        sweep job for correctness and would make a row restored from a backup wrong.
        `MemoryStatus::effective_at` derives it, and `EffectiveMemoryStatus` distinguishes `Expired` (lapsed)
        from `Superseded` (replaced) because those are different answers to "why is this not being used".
      - **Both supersession directions are stored.** `supersedes` alone would make "is this claim still
        current" a scan of every later memory, and that question is on the retrieval path.
      - **`Proposed` exists.** A candidate deterministic code cannot support as fact is stored as a proposal
        rather than dropped, which is what makes "the model may propose candidates" produce something.
      - **`Relationship` memories start as proposals.** Derived from the type at construction rather than
        taken from the caller, so "high-impact identity and relationship inferences require explicit user
        confirmation" cannot be skipped. `Semantic` is deliberately **not** flagged: a semantic claim can be a
        medical or financial fact and the type alone cannot say, so flagging it would refuse an ordinary fact
        about a city — the per-claim classification is `P4-003`'s, where the content is read.
      - **Deletion clears the content in the domain as well as in storage.** A value already held cannot be
        returned by a caller that kept a reference, so the domain — not only the repository — is unable to
        produce deleted text. The row's existence is retained so a source link and an audit record resolve.
      - **A repeated entity keeps the weakest match basis.** A reader of one memory cannot tell which mention
        it is looking at, so one confident mention must not launder a later guess about the same entity.
      - **The search key normalizes case, whitespace, and word order only.** No stemming, no stop words, no
        synonyms: each would make two *different* claims collide, and over-collapsing loses information the
        user gave, while under-collapsing a duplicate costs one row.
      **Falsified, one guard each:** forcing the model-inference check false made
      `a_model_inference_cannot_claim_confidence` fail at `Uncertain`; forcing the provider-preference check
      false made `a_provider_record_cannot_back_a_preference` fail. Both restored and re-run green —
      **155 `jarvis-core` tests.**
      **⚠ Naming defect found and fixed while writing the tests:** the supersession transition and its
      accessor were both `superseded_by`, distinguishable only by arity. A reader could not tell an act from a
      query, and the act is the one that changes state — renamed to `replace_with`.
      Gates: fmt, clippy `-D warnings`, 45 suites with the application binaries absent and **zero skips**,
      `cargo deny` ok.
      **Recorded as a limit:** nothing persists a memory yet. This slice is the vocabulary and its invariants;
      `P4-002` owns the tables, and the embedding fields in the canonical record are `P4-005` — a memory is
      fully usable without them, because "embeddings are one signal" and not a requirement.
- [x] `P4-002` Implement memories, entities, aliases, relations, sources, and deletion tombstones in SQLite.
      `crates/jarvis-storage/migrations/sqlite/0009_memories.sql` and
      `crates/jarvis-storage/src/memory_repository.rs`. Six tables — `memories`, `entities`,
      `entity_aliases`, `entity_relations`, `memory_entities`, `memory_tombstones` — and the slice's real
      work was deciding **where each rule is enforced**, which turned out to be three schema decisions and
      one domain-constructor one. Recorded as `ADR-0044`.
      **A structured claim is three columns, not one JSON document.** `docs/data/schema.md` says JSON stores
      "versioned provider payload fragments or flexible metadata, **not core relationships that need
      constraints**", and a subject/predicate/object triple is exactly such a relationship. The first cut
      stored one bounded document, which permitted an 8000-byte subject and an empty object; three columns
      make each part boundable, make "all three or none" unrepresentable rather than a reader's check, and
      turn a lookup by predicate into an index seek.
      **The tombstone stores `SHA256(search_key)`, not the key.** The acceptance invariant is "deleting it
      removes text **and derived indexes**", and a search key *is* derived text — it holds the claim's
      words, case-folded and word-sorted. Keeping it would have satisfied "resurrection is blocked" while
      violating "the text is gone", and the violation would be invisible because no retrieval reads the
      tombstone table. The same technique `ADR-0018` uses for an intent digest.
      **"A relationship memory starts as a proposal" is deliberately *not* a table `CHECK`.** A `CHECK`
      applies to every write, not to the insertion, so a constraint reading "a relationship row is never
      `active`" would forbid the *confirmation* it exists to require. The invariant is about creation, and
      only the constructor can express that — the same distinction `ADR-0041` draws about a sandbox
      guarantee: a rule enforced in the wrong place is not a stricter rule.
      **The stored status had to travel into the constructor.** The first cut decoded a record and then
      stamped the status on, which could not work: the content rule refused an empty content *before*
      anything could say that emptiness is what `deleted` means, so a tombstone could not be read back at
      all. `MemoryRecord::from_stored(parts, state)` now takes the status with the parts, which makes both
      halves of one rule expressible — empty content is permitted **only** in `Deleted`, and a `Deleted` row
      that still carries text is `InvalidMemory::DeletedRetainsText`, the decode-side counterpart of the
      schema's `CHECK`. Two independent enforcers, which is the argument the approval and tool-call
      repositories already make.
      **A compiler finding worth recording:** `read_entity_memories`'s statement takes a third parameter,
      and the shared runner had an `entity_id` argument it **never bound**. The query still ran and
      silently ignored the entity filter; only the arity check surfaced it. The runner now binds it as an
      `Option`, so a caller cannot supply the entity-scoped statement without its entity.
      **Falsified, one guard each:** forcing `is_tombstoned` to return `false` made
      `deletion_removes_text_and_blocks_a_re_ingest` fail; removing the workspace scope from the read
      statement made `a_memory_cannot_be_read_from_another_workspace` fail. Both restored, re-run green.
      **Two test-fixture defects found and fixed by the failures:** the seeded workspace insert omitted
      `mode`/`data_policy`, and `a_replacement_cannot_be_rewritten_in_storage` recorded rows with fresh
      random identifiers and then linked two different ones — so it asserted a link to nothing and passed
      only because the foreign key refused it. A fixture that cannot satisfy its own precondition is worse
      than no test, because its failure reads as a defect in the code.
      **Recorded as limits:** nothing retrieves or ranks a memory yet (`P4-004`; the reads are recency-ordered
      only, and the embedding columns are `P4-005`), duplicate detection is exact-search-key only, the entity
      links are written after the row rather than in one transaction with it (the safe direction, which is
      not the same as atomic — `P4-008` needs the atomic form), and `entity_relations` has no cycle check.
      Gates: fmt, clippy `-D warnings`, 45 suites with the application binaries absent and **zero skips**
      (1095 tests), all three phase gates with `ACCEPTANCE_REQUIRE_BINARIES=1` and
      `ACCEPTANCE_REQUIRE_FIXTURE_PEER=1`, `cargo deny` ok. `jarvis-core` 156 tests, `jarvis-storage` 167.
- [x] `P4-003` Implement candidate extraction as a reviewable pipeline; never persist unsupported inference as fact.
      `crates/jarvis-core/src/candidate.rs` + `candidate/tests.rs` (19 tests). The document's admission
      lifecycle as stages, and the slice's central finding is that **one of its stages cannot be
      implemented the way it reads**. Recorded as `ADR-0045`.
      **`MemoryCandidate` is not a `MemoryRecord` with fewer fields.** It is the model's *proposal*, so it
      must be representable with a confidence its source cannot support, a sensitivity below the floor, and
      an entity set the store has not resolved — each of which a stage resolves or refuses. A record cannot
      be built until every rule passes (`P4-001` made that structural), so the two types cannot be merged
      without making every consumer defend against a half-valid memory.
      **⭐ THE FINDING: a correction cannot be inferred from a key match.** The first cut compared contents
      and called a same-key/different-content pair a correction. That branch is **unreachable**, because
      `P4-002`'s search key *is* the sorted, case-folded word set: two records sharing a key have the same
      words, so a key lookup can only return a memory whose words match, and `normalized_equal` agreed with
      the key by construction. The test asserting a `Correction` got `Duplicate`, and the tempting repair —
      "make the fixture differ more" — was wrong: the fixture was already as different as a shared key
      permits, which is not different at all. Inferring from "same entity, different words" was the next
      candidate and is worse: "Alice is my sister" and "Alice lives in Rotterdam" share an entity, so it
      would retire a memory for every second fact about anyone.
      So **supersession is declared** (`MemoryCandidate::supersedes`), checked before the comparison because
      it asserts the candidate's own history rather than observing the store, and it outranks the
      fact-or-proposal decision — a corrected *relationship* claim must supersede, not become a proposal.
      `compare`'s `Correction` branch is kept as a **diagnostic**: a caller supplies `existing`, so a caller
      passing a row from a wrong lookup would otherwise have it reinforced as though it held the candidate's
      claim; returning `Correction` makes that bug surface as a visible supersession instead of a silent
      merge.
      **A second finding from the same test run:** `MemorySearchKey` bounds each key *word* at 64 chars as
      well as the content at 4096, so a 4096-character single word is within the content bound and
      unkeyable. The first cut reported `Content` for both, which would send an operator shortening text
      that already fits — now `CandidateRefusal::Unkeyable`, with a test asserting both bounds (a
      single-word fixture conflates them, which is how the bug was found).
      **Confidence is lowered, never raised.** A model inference is admitted at `Unverified` **and** as a
      proposal whatever it claimed; a document claim caps at `Likely`; a caller claiming *less* certainty
      than its source could support is not overruled. The invariant is therefore a property of the stored
      value rather than a check a later reader must remember.
      **Sensitivity is the maximum of three floors** — the type's (a relationship claim is `Confidential`),
      the content's (a credential or health term raises it to `Restricted`), and the extractor's. Only one
      direction is safe to correct automatically, because exclusion from a remote model is decided from the
      *stored* level.
      **Falsified, one guard each:** removing the confidence cap failed exactly
      `a_model_inference_is_never_admitted_as_fact` and `an_overconfident_claim_is_lowered_rather_than_refused`;
      making the proposal branch unreachable failed exactly the two tests about inferences and relationships.
      Both restored, re-run green.
      **Recorded as limits:** nothing extracts a candidate from anything yet (the model-facing half is
      `P4-007`, where context assembly and the provider boundary live), **nothing writes an admission** so
      the end-to-end property is unproven, nothing reads the store (`CandidateContext` carries borrowed
      values because entity resolution needs `jarvis-storage`), the restricted-content check is a keyword
      list rather than a classifier, `supersedes` is a single link with no chain walk, and a candidate
      carries no identifier so the domain's self-supersession refusal belongs to `P4-001` where the
      identifier does.
      Gates: fmt, clippy `-D warnings`, 45 suites with the application binaries absent and **zero skips**
      (1114 tests), all three phase gates with `ACCEPTANCE_REQUIRE_BINARIES=1` and
      `ACCEPTANCE_REQUIRE_FIXTURE_PEER=1`, `cargo deny` ok. `jarvis-core` 175 tests.
- [x] `P4-004` Implement exact, full-text, recency, importance, entity, and workspace retrieval before adding embeddings.
      `crates/jarvis-core/src/retrieval.rs` + `retrieval/tests.rs` (21 tests, `jarvis-core` 196). The
      document's two retrieval stages as pure functions over records. `ADR-0046`.
      **Eligibility is a filter, not a low score** — and three of the six rules could not be ranking signals
      at all. A foreign workspace's memory is not a poorly-ranked result; ranking it and dropping it later
      would make one user's score depend on what another user stored. A superseded or expired claim is not
      "less relevant" but "not current", and the document requires it stop being retrieved *as current* — a
      ranker that surfaced it would need every consumer to re-check. A memory above the destination's
      ceiling must not be admitted and then redacted, because a redaction transforms content already inside
      the assembly step.
      **"Actor authorization" and "workspace policy" are one check here, recorded rather than silently
      merged.** In this platform actor authorization *is* workspace membership — `P4-002`'s reads all bind
      `workspace_id` — so a separate actor rule would always pass, which is worse than an absent rule
      because a reader would count it. The two *are* separate in a server deployment, so the merge is the
      thing to look for when `P4-008`/`P4-009` add sharing.
      **The weight table is capped at a quarter of the total per signal**, which is "no single signal may
      dominate by accident" as arithmetic: the eight weights sum to 1000, none exceeds 250, and the test
      asserts the comparison directly — a memory good on several signals outranks one perfect on a single
      signal. Semantic similarity, active project/task relevance, and relationship overlap are
      **deliberately absent** from the table rather than present as zeros, because a zero row is the table
      claiming a signal it has no data to produce.
      **⭐ A test caught the explanation not explaining.** The first total scaled the whole weighted sum once
      while each contribution was scaled separately, and **integer division is not distributive** — so they
      disagreed by up to one per term (521 vs 522). The total now *is* the sum of the contributions, which
      makes an operator's displayed breakdown the arithmetic that produced the total rather than a parallel
      calculation. Integer rather than float for the same reason the weights are one table: a float score
      would make the ranking depend on platform rounding, so a stored score would be unreproducible.
      **⭐ The reason precedence was decided by an array literal.** `max_by_key` returns the **last** maximum
      on a tie, and a tie is the common case — a question naming a memory's entity usually also shares its
      words, scoring full on all three matching signals — so the reported reason depended on the order the
      arms were written. Now explicit: identifier, then overlap, then keyword.
      **A shared scaling helper, because the obvious integer form is always zero.** `numerator /
      denominator` truncates to nothing for every partial case, so a memory matching two of three words
      would score as if it matched none — and the same shape appears in four signals. One `scaled` helper
      multiplies before dividing, so eight call sites cannot each forget it.
      **Diversification is a prefix of the ranking, and a drop is not a refusal.** Caps applied in rank
      order; selecting by category first would override the ranking with a category order the scoring never
      expressed, and a category order imposed afterwards is a signal by another name. A capped and an
      excluded memory are **separate lists** because the actions differ — collapsing them would report "you
      have seen enough preferences" as "this preference is not available to you". A zero cap is refused
      because "none of this category" and "no limit" are opposite readings and the silent one is restrictive.
      **Falsified, one guard each:** disabling the destination-sensitivity rule failed
      `a_memory_above_the_destination_is_excluded`; raising a weight above the cap failed
      `no_single_signal_can_dominate` (on both the sum and the domination comparison). Both restored, green.
      **Recorded as limits:** no semantic similarity (that is `P4-005`), no project/task relevance, no
      relationship-graph traversal; the recency window is a constant rather than a per-query parameter; the
      **weights are reasoned and not calibrated** against a real result set (`P4-006`'s evaluation is where a
      measurement belongs, and the weights are one table so it is a single edit); **no function here issues a
      query**, so nothing yet turns a `MemoryQuery` into a candidate set; and nothing calls the module —
      `P4-007` is the integration and until then it is a complete, tested, uninvoked component.
      Gates: fmt, clippy `-D warnings`, 45 suites with the application binaries absent and **zero skips**
      (1135 tests), all three phase gates with `ACCEPTANCE_REQUIRE_BINARIES=1` and
      `ACCEPTANCE_REQUIRE_FIXTURE_PEER=1`, `cargo deny` ok. `jarvis-core` 196 tests.
- [x] `P4-005` Research and implement a provider-neutral embedding adapter with dimension/version metadata.
      `docs/research/integrations/embeddings.md` + `crates/jarvis-models/src/vector.rs` (+`vector/tests.rs`) +
      `crates/jarvis-models/src/embedding.rs` (+`embedding/port_tests.rs`) +
      `crates/jarvis-models/src/openai/{embedding.rs,wire_embedding.rs}`. `ADR-0047`. 106 `jarvis-models`
      tests. **Research first, because this is an external integration**: the API index, the embeddings guide,
      and the create-embeddings reference were fetched live and recorded with the URL, the access date, the
      limits, and the unresolved questions before any code was written.
      **The central decision is that a vector is not comparable by itself.** `docs/architecture/storage.md`
      says "never compare vectors with incompatible metadata" and then lists the fields, which reads as a
      checklist — and a checklist fails, because the two fields easiest to forget are the two that produce a
      *plausible* wrong answer. A normalization mismatch does not fail, it ranks wrongly (a dot product over
      an unnormalized vector returns a number in the right range often enough to look like a score); a
      dimension mismatch fails as a length error far from the comparison, which reads as a corrupt vector.
      So `Embedding` holds the vector **and** its metadata, `Embedding::new` is the only constructor and
      refuses a vector whose length contradicts its declared dimension, and `ensure_comparable_with` is the
      guard — a method on the pair, returning `IncompatibleMetadata { field }` naming the first field that
      differed. The rejected alternative is the tempting one: `cosine_similarity(a: &[f32], b: &[f32])`,
      whose signature cannot see either field. This is the same defect class `ADR-0044` recorded as "two
      values that must agree with nothing holding both"; here something holds both and the constructor is the
      only way to make one.
      **`Normalization::Unknown` is the default, deliberately.** The researched provider documents
      length-1 vectors, but the port is what a *second* provider has to satisfy, so the dot-product shortcut
      is taken only when both sides declare `Normalized`. A default is what a caller who did not think about
      the field gets, and the permissive assumption on a distance is the one that goes wrong quietly.
      **Vectors are reassembled by the provider's `index`, never by position.** The wire defines `index` as
      "the index of the embedding in the list of embeddings"; zipping positionally would hold for an in-order
      response and silently pair every text with somebody else's vector if the provider ever reordered. An
      out-of-range index and a **duplicated** index are both refused — the duplicate was found while writing
      the ADR, which claimed the refusal the code did not yet perform; taking the later vector is the same
      defect class as positional pairing.
      **`input_hash` and `chunker_version` are stored but deliberately not compared.** The document's list
      is a set of properties a stored vector must carry, not a set of fields that must be equal for a
      comparison — two vectors of *different* text are exactly what a search compares, so requiring the hash
      to match would forbid the only meaningful comparison.
      **⭐ Three falsifications, all caught.** Disabling `ensure_comparable_with` failed
      `incompatible_metadata_is_refused_field_by_field`, `a_comparison_across_models_refuses`; disabling the
      length check in `Embedding::new` failed `a_vector_length_that_contradicts_its_metadata_is_refused`
      **and** `a_vector_of_the_wrong_length_is_refused` through the adapter path; disabling the
      duplicate-index refusal failed `a_duplicated_index_is_refused_rather_than_overwritten`. All restored,
      green.
      **A fourth thing the compiler caught:** the first scripted transport modelled a *streaming* response,
      which needed `Box<dyn ResponseBody>`, was not `Send`, and could not be constructed at all. The
      embedding call is never sent with `streaming: true`, so a test double that could produce one would
      suggest a reachable path that does not exist — the streaming half is gone and the double returns only a
      buffered response.
      **Recorded as limits:** **no provider has been called**, so the record stays `researched` and not
      `live-verified` — the provider's normalization claim, the per-model default dimensions, and the
      empty-string refusal are read from documentation, and the live smoke test in the record's verification
      plan is described rather than run; **nothing writes an embedding to storage** (there is no
      `memory_embeddings` table and no repository function — vector search and pgvector parity are `P4-009`);
      **no chunking**, so the 8192-token bound is the caller's problem; no token-count pre-check for the
      2048-element/300,000-token batch bounds either, because that needs a tokenizer this crate does not have
      and an estimate used as a *limit* would refuse valid requests while looking like a check; no retry
      inside the adapter (one attempt, as the Chat Completions record establishes, with the extra note that a
      retry costs tokens); `chunker_version` is `None` on the memory path where nothing chunks yet; `base64`
      is unsupported as an unmeasured decode path for provider-supplied data; and **nothing calls the port** —
      no daemon route, no memory path, no CLI verb — until `P4-006` consumes it.
      Gates: fmt, clippy `-D warnings` across the workspace, 45 suites with the application binaries absent
      and **zero skips** (1162 tests), all three phase gates with `ACCEPTANCE_REQUIRE_BINARIES=1` and
      `ACCEPTANCE_REQUIRE_FIXTURE_PEER=1`, `cargo deny` ok after `sha2` was added to `jarvis-models`.
- [x] `P4-006` Add hybrid retrieval and explainable scoring; embeddings are an index, not canonical truth.
      `crates/jarvis-core/src/retrieval.rs` (+`retrieval/semantic_tests.rs`, 10 tests; `jarvis-core` 205).
      `ADR-0048`. The ninth signal, and the one the previous slice deliberately left out because it had no
      data. **Research first**, because `P4-005` was the external integration; this slice is the domain logic
      over it.
      **The vectors cannot cross the crate boundary, and the domain does not need them to.** `jarvis-models`
      depends on `jarvis-core`, so the domain naming `Embedding` would be a cycle. `jarvis-core` defines its
      own borrowed `SemanticVector` carrying provider, model, version, normalization, and a `&[f32]` — which
      is the whole contract — plus a `ScoringContext` that holds the query's vector and a lookup together.
      They are one optional value rather than two arguments because they must agree: separate arguments would
      make "a query vector with no index" and "an index with no query vector" both representable.
      **⭐ The mutation that was itself a no-op.** Zeroing the semantic signal to prove the ranking tests
      depended on it, the tests still passed — so a comment was written claiming the test had been vacuous
      and the coincidence was the tie-break. It was not: the mutation was written as
      `if false { 0 } else { real }`, which **selects the real branch**, so nothing had been changed and the
      green result carried no information. With the mutation written as a plain `0`, both ranking tests fail
      — `left: 0, right: 1000` on the signal and `Some(0)` vs `Some(1000)` on the total. The lesson is that a
      falsification which passes is first a claim about the mutation, not about the test, and the mutation has
      to be shown to compile into something different before a green run means anything.
      **⭐ The orthogonal pair found a real defect in a doc comment and a real defect in the code.** A test
      asserting an orthogonal pair scores half the scale failed with `left: 0`. The **test was wrong** — an
      orthogonal cosine is exactly `0.0`, not `0.5`, and the code was right. But the failure also exposed
      that `cosine <= 0.0` returned `absence: None`, which conflates an orthogonal pair with an opposed pair
      and with a genuine refusal; the four states that produce a zero score (no index, no memory vector,
      incomparable, zero magnitude) plus real arithmetic all collapsed into "zero, no reason" for two of
      them. `SemanticAbsence::NoSimilarity` now names the arithmetic case, which is the same principle
      `ADR-0046` applied when it made the total *be* the sum of the contributions.
      **The comparability guard is the point of the feature.** A cosine over two vectors from **different
      models** is a number with no meaning that still lands in the usual range, so a missing check does not
      look like a bug — it looks like a slightly worse ranking. Provider, model, and version are compared
      before any arithmetic and the differing field is named; the vectors' own lengths are compared too, which
      makes a declared dimension field unnecessary and would have repeated a check `P4-005`'s constructor
      already performs. A zero-magnitude vector refuses rather than dividing by zero, because a `NaN` in one
      signal would poison a whole ranking.
      **Nine weights that must still sum to `TOTAL_WEIGHT`, so the table was rebalanced and not extended.**
      Every ordering the document states is kept, and the cap that no weight exceeds a quarter of the total is
      unchanged, so `ADR-0046`'s "no single signal may dominate by accident" comparison still holds. The
      *reason* precedence is deliberately **not** the weight order: the document lists semantic similarity
      above entity overlap and the weights keep that, but a reason is what a user is told, and "this memory
      means something close to what you asked" is an inference where "this memory repeats your words" is
      present in the text — so text wins the reason and meaning keeps the weight.
      **A memory without an embedding is not a worse memory.** It scores zero on one of nine signals and is
      ranked on the other eight, which is the document's "embeddings are one signal" as arithmetic rather
      than as policy — and no index is a zero **with a recorded reason**, so a caller can tell a zero from an
      unconfigured provider, from a memory the index never saw, from an incomparable pair, and from two
      genuinely orthogonal directions.
      **Falsified, one guard each:** disabling the provider/model/version comparison and the length check
      failed `a_vector_from_another_model_is_refused_and_names_the_field` and
      `vectors_of_different_lengths_are_refused`; disabling the zero-magnitude guard failed
      `a_zero_magnitude_vector_refuses_a_comparison`; zeroing the signal in `signals_for` failed both ranking
      tests. All restored, green.
      **Recorded as limits:** **nothing produces the vectors** — the `P4-005` port is wired to nothing, no
      route embeds a memory, no job populates a lookup, and nothing writes a vector to storage, so what is
      proven is the scoring and the guards rather than an end-to-end retrieval with embeddings; **nothing
      calls `rank`** either (`P4-007` is the integration); the **weights are reasoned, not calibrated**, and
      this slice makes that limit slightly larger by moving eight values rather than only adding one; the
      lookup is a closure, so the module cannot say how a vector is *found* (`P4-009` owns pgvector);
      the cosine is `f32`, so cross-platform bit-identity is not claimed even though the stored score is an
      integer; relationship overlap and active project/task relevance remain absent as in `P4-004`; and a
      text-free query is untested because no caller constructs one yet.
      Gates: fmt, clippy `-D warnings`, 45 suites with the application binaries absent and **zero skips**
      (1172 tests), all three phase gates with `ACCEPTANCE_REQUIRE_BINARIES=1` and
      `ACCEPTANCE_REQUIRE_FIXTURE_PEER=1`, `cargo deny` ok. `jarvis-core` 205 tests.
- [x] `P4-007` Integrate memory retrieval into context budgets with provenance and injection-resistant quoting.
      `crates/jarvis-core/src/isolation.rs` (+`isolation/tests.rs`, 10 tests) +
      `crates/jarvis-core/src/memory/context_tests.rs` (11 tests) +
      `MemoryRecord::{context_trust, context_item}` + `RetrievedMemory` + `memory_context_introduction` +
      `jarvis-storage::read_retrievable_memories` + `apps/jarvisd`'s `load_memories`/`context_summary` and
      the `messages_from_manifest` memory path (4 daemon tests). `ADR-0049`.
      **`jarvis-core` 227 tests** (up from 205), **1198 workspace tests** in 45 suites.
      **⭐ Isolation is not detection, and the module says so.** "Detect and remove instructions" is not
      implementable — deciding whether a sentence is an instruction requires understanding it, and any pattern
      list is defeated by rephrasing or by a language it does not cover. So the module makes the text have
      three properties an attacker cannot undo instead: **format characters are removed** (a bidi override
      changes how text *reads* without changing what it *is*, so in a prompt it is a deception primitive); the
      **fence token cannot appear in the payload**, so the region cannot be closed from inside; and the
      **framing is authoritative text this platform wrote**, placed before the payload. Property three is why
      the fence is not itself a security claim — properties one and two hold regardless of what the model
      decides to do.
      **⭐ The model may still obey an instruction inside the fence, and that is recorded as the design.**
      Prompt injection is *mitigated* here, not solved, and no fencing scheme solves it. What protects the
      system is that the *effect* of any instruction still passes through schema validation, authorization,
      risk classification, approval, and audit: the model may ask, and deterministic Rust decides. Stated in
      the ADR so the fence is not read as a guarantee it is not.
      **⭐ Two real defects the tests found, and the second is the more interesting.**
      (1) The fence-token check **silently did nothing for every payload, including the real marker**: it
      stripped separators from the text and then searched for a token that still had its own hyphens in it, so
      the comparison could never match. An assertion written in terms of the same normalisation had reproduced
      the bug and passed — what caught it was counting opening markers in the *rendering*. A guard that cannot
      fire reads exactly like a guard that finds nothing.
      (2) **Neutralisation removed every control character, and `\n` and `\t` are control characters** — so a
      multi-line memory silently became one run-on line, and a list rendered as a sentence is a different
      claim. Line structure is kept; a carriage return is not, since a lone one is a line-overwrite primitive.
      The failure mode was a *quiet reshaping* rather than a loss, which is why the test asserts the exact
      text.
      **⭐ A third defect: the token estimate was two bytes short of what would be sent.** It added the two
      marker lengths and forgot the newlines `render` inserts. That is the one direction a budget must not err
      in. The estimate now takes the rendering itself, so there is no length arithmetic left to get wrong, and
      the test asserts an inequality over four payload shapes rather than an equality against one hand-computed
      length.
      **Eligibility is split between the read and the conversion, deliberately.** SQL excludes what only SQL
      can decide (deleted/proposed/superseded rows, **model inferences**, task-shaped predicates); conversion
      excludes what needs the clock or a policy (not current truth at this instant, a type the use case does
      not allow). Two rules deserve their reasons recorded: a **model inference is filtered by source kind,
      not by status**, because a *confirmed* inference is `active` and is still the model's own claim — the
      self-feeding loop the inference boundary exists to prevent; and **`Working`/`Conversation` are excluded
      by the allow-list**, the first because a plan replayed into a prompt reads as an instruction to continue
      it, the second because the history replay already provides it and offering it twice would make one turn
      look like independent corroboration of itself.
      **The trust mapping is deliberately not the identity.** A model inference is `Untrusted`, never
      `Derived`, or the model could reach a future prompt by first writing a memory. An unconfirmed claim from
      an authoritative source is `Derived`, not `User`, because a user statement recorded `Uncertain` is this
      platform's uncertain reading of something rather than the person speaking. Nothing maps to
      `Authoritative`, because a memory is never JARVIS's own policy — and only `Authoritative` is
      instruction-bearing.
      **Retrieved records go in one message, after the history and before the question.** One message per
      record would interleave untrusted text with the conversation's turns, so a record could be read as a
      turn; appending them to the user's own turn would make them indistinguishable from something the user
      typed. Membership comes from the manifest and order from the conversation, and a record the manifest
      included but which cannot be found is an **error** rather than a skip, or the request would disagree
      with its own audit record.
      **Falsified, one guard each:** disabling the newline exemption failed
      `line_structure_survives_but_a_carriage_return_does_not`; restoring the un-normalised needle failed two
      isolation tests; sending `isolated().body()` instead of `.render()` failed three of the four daemon
      memory tests; neutralising the `source_kind` filter failed `a_model_inference_and_a_task_claim_are_not_offered`.
      All restored, green.
      **A test weakness the falsification exposed and fixed:** `a_stored_memory_reaches_the_request_inside_a_fence`
      *passed* with the fence removed, because the introduction itself names both markers, so "the message
      contains the marker" was satisfied by prose about the fence. Extracted a shared `fenced_record_message`
      helper that requires the marker **on its own line**, which is the structural property `render` produces
      and prose cannot satisfy.
      **Recorded as limits:** no ranking, scoring, or diversity is applied — `P4-004`'s eligibility and
      `P4-006`'s ranking exist and are tested and **nothing calls them on this path**, so the read is a recency
      window rather than a relevance one and `MAX_MEMORIES_LOADED` is the only bound; `retrieval_count` is
      **never incremented** and `last_accessed_at` stays null (`reinforce_memory` exists, nothing calls it), so
      the reinforcement signal is zero for every memory; **no vector search**, because nothing writes an
      embedding, so the hybrid ranking is not yet hybrid; `read_entity_memories` exists and is unused, so
      "what do I know about this person" is still answered by ranking rather than an index seek; **expiry is
      evaluated in Rust**, so expired-but-active rows still fill the window (`ADR-0034`'s timestamp comparison
      defect would have to be fixed first); the format-character range set is **duplicated** between
      `jarvis-core` and `jarvis-mcp` because the dependency runs one way; the event says how many memories were
      offered/included/altered but **not which**, beyond the manifest's references not being stored; and no
      CLI or route exposes any of it (`P4-008`).
      Gates: fmt, clippy `-D warnings`, 45 suites with the application binaries absent and **zero skips**
      (1198 tests), all three phase gates with `ACCEPTANCE_REQUIRE_BINARIES=1` and
      `ACCEPTANCE_REQUIRE_FIXTURE_PEER=1`, `cargo deny` ok. `jarvis-core` 227 tests, `jarvisd` 93.
- [x] `P4-008` Add inspect, search, remember, correct, forget, export, retention, and full user-deletion APIs/CLI.
      `crates/jarvis-protocol/src/memory_api.rs` (the wire DTOs) + `apps/jarvisd/src/memory_service.rs`
      (8 tests) + six routes in `apps/jarvisd/src/gateway.rs` (3 tests) + `apps/jarvis-cli/src/memory.rs`
      (seven verbs) and `api_client.rs`'s seven memory methods. `jarvis-storage`'s `purge_memory`,
      `read_all_memories`, `find_memory_including_deleted`, `count_memory_entity_links`, and
      `write_tombstone_on`. `ADR-0050`.
      **`jarvisd` 105 tests** (up from 93). **45 suites, 1210 workspace tests, 0 failed, 0 ignored.**
      **⭐ This slice was mostly wiring, and the wiring is what found the defects.** The domain, storage,
      pipeline, retrieval, and embedding port all existed and were tested, and **nothing could reach any of
      them**. Every defect below was found by *connecting* a tested component to a caller — which is the case
      for these tests existing at all.
      **⭐ A guard that was unreachable because its input was never supplied.** The deduplication check lives in
      the pipeline's `compare(context.existing, …)`, and `remember` passed `existing: None` — so the entire
      duplicate path was dead code and a re-statement fell through to the unique index and surfaced as
      `MemoryDuplicate` → `409` "the memory was changed by another writer". **A user restating their own claim
      was told another writer had edited it.** The gateway test asserting `200` for a repeated claim found it.
      The mapping had even carried a comment saying the variant "is reachable only as a race": a comment
      explaining why an error is unlikely is how a bug gets a justification instead of a fix.
      **⭐ The pipeline's outcome and the fact of a write are different answers.** Fixing the above exposed that
      the reply was built from the admission alone, so a caught duplicate reported `outcome: "remembered"` —
      right shape, wrong claim. `Stored { memory, wrote }` now travels out of the store, and `outcome_of`
      reconciles the two: nothing stored means nothing was remembered, whatever the pipeline decided. Pairing
      them makes the divergence unrepresentable rather than merely documented.
      **⭐ Three reads silently skipped a join, and the refusal it caused looked like the caller's fault.**
      `find_memory_including_deleted`, `read_all_memories`, and `read_memories` all decoded the memory row
      without loading `memory_entities`. A correction inheriting `existing.record().entities()` inherited
      **nothing** and was refused with *"a memory must name between 1 and 16 distinct entities"* — a message
      about the caller's request for a defect in a read. A missing join is invisible: the row decodes, every
      field is present and typed, and the empty vec looks like a legitimate "about nothing". Only `find_memory`
      had loaded the links.
      **⭐ SQLite in WAL fails a deferred transaction that reads then writes across connections.** The purge
      read its row inside a transaction, wrote the tombstone through the **pool** (a second connection), then
      tried to `DELETE` on the transaction — `SQLITE_BUSY_SNAPSHOT`, reported as a bare "failed to purge a
      memory the SQLite database". `write_tombstone_on` takes the `Transaction` rather than the database, so
      splitting the operation across connections is now a **compile** error.
      **⭐ An operation's own doc described a path the code does not take.** The export's exclusion text said
      "deleted claims appear as an empty record", which describes the `Delete` **transition** (clear text, keep
      the row). The only deletion verb this surface has **purges**. The doc and the text agreed with each other
      while both described something no verb does — an assertion that the purged id is *absent* from the export
      found it, and agreement between two statements by the same author is not evidence.
      **⭐ A correction cannot inherit the original's window, and the domain is right to refuse it.** A memory's
      `valid_from` may not precede its `created_at`: a window that opens before the record existed is a
      backdated claim, and it is how a correction would fail to outrank the thing it corrects. So a correction
      takes `valid_from` from its own creation instant and carries `valid_until` over unchanged. The limit is
      recorded rather than papered over: correcting a claim that had **not yet taken effect** makes the
      correction effective from now, so the pair reads as overlapping; and a claim whose window has **already
      closed** cannot be corrected at all (the correction would be born expired), refused with `lapsed` rather
      than stored.
      **⭐ A tombstone was reported as an infrastructure failure.** `DatabaseError::MemoryTombstoned` fell
      through to `Storage` in the error mapping, so a client that restated a deleted claim was told *"the local
      database is not available"* — sending an operator to check a daemon that is running.
      **⭐ Deleting is two operations, not one with a flag.** `Delete` clears the text and keeps the row, because
      a source link has to keep resolving and the audit trail has to survive; `purge_memory` deletes the row and
      `memory_entities` cascades. Collapsing them would make "delete" mean whichever the caller intended, and
      the one it did not mean would be unrequestable.
      **The tombstone is written *before* the delete**, so a crash between the two leaves a tombstone for a
      memory that still exists (resolvable by deleting again) rather than a deleted memory with no tombstone
      (resurrected by the next ingest). The safe failure direction is the one that **over-blocks**.
      **`allow_relearn` is an explicit undo**, not a skipped write: it removes any existing tombstone *and*
      skips writing a new one, because a flag that merely suppressed the write would leave an older tombstone
      in place and the undo would appear to work and then not.
      **`expected_version` is required on correct and forget, and that is the opposite of `ADR-0022`.** A run
      cancellation carries none, because a run's terminal-state rule already refuses a second one; correct and
      forget change **what will be retrieved as current truth**, so the write's subject matters. The version is
      on the **reference**, not only on a reply, because the caller obtains an expectation by reading and a
      reply that omitted it would leave a client re-reading and hoping. Its absence from the CLI is a usage
      error rather than an implicit re-read. **Two staleness reasons exist and both are tested**: a version the
      caller never saw, and a version read *before a correction archived the claim* — which advanced that
      claim's own version.
      **A search hit reports version `0`, and a purge guard refuses it.** The ranking holds a `MemoryRecord`,
      which carries no concurrency version, so `hit_of` can only fabricate a value; `0` cannot match a stored
      version (they start at one), so it is safe as "I do not know" instead of dangerous as a plausible guess.
      Recorded limit: a search result cannot be corrected without a `show` first.
      **No scope in a request body**, and `deny_unknown_fields` is what makes it structural: a `workspace_id`
      in a body is a `422` naming the field, so "client A's memory cannot enter client B's context" is a
      transport property rather than a per-handler check. The read-side check is a comparison **after** the
      read, and a claim in another workspace is `404`, never `403` — "not yours" would confirm that something
      exists.
      **A reference never carries content**; `show` and `export` are the two exceptions for different reasons
      (one claim deliberately requested vs. `GDPR`-shaped portability). The export lists its **exclusions**,
      because each entry is a different reason a reader might assume completeness.
      **An empty entity list is refused rather than filled with a placeholder** — a claim attached to a
      placeholder *looks* resolved, so a later question about the real subject will not find it and nothing in
      the store says why.
      **Falsified, one guard each:** setting the tombstone write to `if false` failed
      `a_forgotten_claim_does_not_return_unless_relearning_was_allowed` (the claim came back with a fresh id);
      replacing `load_scoped`'s workspace comparison with `false &&` failed
      `a_claim_is_only_readable_under_the_workspace_that_holds_it`. Both restored, green.
      **Recorded as limits:** **no entity resolution**, so a remember must name an entity identifier the caller
      already holds; `importance` defaults to `2`, the middle of the range, so a default cannot outrank explicit
      user statements; **no retention policy is implemented** — the verbs exist and the sweeper does not, so
      nothing expires on its own; **no provider-side deletion** is possible and the receipt says so; the CLI
      verb module has no test, so its argument parsing is unverified (the daemon service and the routes are
      tested); and an entity-less remember is refused by the service rather than resolved.
      Gates: fmt, clippy `-D warnings` over the workspace, 45 suites with the application binaries present and
      `ACCEPTANCE_REQUIRE_FIXTURE_PEER=1` (**1210 tests, 0 failed, 0 ignored**), all three phase gates with
      `ACCEPTANCE_REQUIRE_BINARIES=1`, `cargo deny` ok.
- [x] `P4-009` Implement PostgreSQL plus pgvector backend parity for the completed memory behavior.
      **DONE. The backend exists, was measured against a live server, and its limits are recorded.**
      `crates/jarvis-storage/migrations/postgres/0001_memory_embeddings.sql` +
      `crates/jarvis-storage/src/embedding_repository.rs` (+`embedding_tests.rs`, 9 tests) +
      `crates/jarvis-storage/tests/postgres_embeddings.rs` (14 live tests) + the SQLx `postgres` feature +
      `docs/research/integrations/postgres-pgvector.md`'s "Live-Server Findings". `ADR-0053` (the backend) and
      `ADR-0051` (the text codec, which this slice reuses unchanged).
      **`jarvis-storage` 191 tests** (up from 182), **47 suites, 1252 workspace tests, 0 failed, 0 ignored.**
      **The codec's own findings, from `ADR-0051` and unchanged here.** SQLx 0.9.0 has **no `vector` type
      mapping** — verified against the driver's own type table — so the value crosses as a text literal and
      `pgvector.rs` is the code that has to be right. The round trip is asserted **by bit pattern** (`-0.0 ==
      0.0` is true, so a value comparison cannot see a sign lost in the text form) **and** against the
      documented form literally (`encode(&[1.0, 2.0, 3.0]) == "[1,2,3]"`), because a round trip alone cannot
      tell a correct form from a self-consistent wrong one. **Falsified:** with the dimension cap removed
      `encode` accepted 2,001 dimensions; with the **encode-side** finite check removed
      `encode(&[1.0, f32::NAN])` returned `Ok("[1,NaN]")`, because `NaN` **formats successfully** and
      `"NaN".parse::<f32>()` **succeeds** — so a read-only guard would let this platform **write a row it
      cannot read**. **A guard on one side of a codec is not a guard on the pair.** The codec takes `&[f32]`
      rather than `EmbeddingVector` because `repository-layout.md` forbids an adapter-to-adapter edge and
      `jarvis-storage` and `jarvis-models` are both adapters.
      **⭐ This slice closed because the ENVIRONMENT changed, and the change is worth recording.** `P4-009`
      was partial because Docker Desktop was installed but **its daemon was not running** and no `psql` or
      Postgres service existed — so the live-server tests could not run and the slice honestly stayed open.
      Starting the daemon (`Docker Desktop.exe`, ~45 s) made `pgvector/pgvector:pg17` available:
      **PostgreSQL 17.11, vector 0.8.6** — the exact version the research record had selected. **A blocked
      slice can be blocked by a daemon, not by a decision**, and the fix was to try the thing that was
      assumed unavailable.
      **⭐ The README does not mention the finding the schema depends on.** A cast-expression HNSW index
      **without a predicate** over an unconstrained `vector` column creates successfully and then **refuses
      every insert at any other dimension** (`expected 1536 dimensions, not 3`). Measured: a table of
      1536-dimension rows became **entirely uninsertable**. Adding `WHERE vector_dims(embedding) = 1536`
      fixes it, because a partial index only covers rows matching its predicate: **3-, 4-, 1536-, 2000- and
      16001-dimension vectors then all stored in one column**, and the query still produced
      `Index Scan using ...` — the plan a fixed `vector(1536)` column gets. So the predicate is load-bearing
      in the index **and** in the query, and the control test shows what it earns: the same query without the
      predicate does not name the index at all.
      **⭐ Three more measured facts the documentation does not state plainly.** (1) `vector(n) <=> other(m)`
      across widths is an **error** (`different vector dimensions 1536 and 3`), not an empty result, so the
      cast in the query is not an optimisation. (2) All three distance operators return **`double
      precision`**, not `real` — binding the distance as `f32` **rounds rather than failing**, so
      `SimilarMemory::distance` is `f64` and the one narrowing happens at the retrieval boundary. (3) A row
      declaring `dimensions = 1536` while holding a 3-component vector **inserted cleanly**, and adding the
      constraint afterwards failed *on that row*; `vector_dims` is `IMMUTABLE` (verified in `pg_proc`), which
      is what makes it legal in the CHECK and in the index predicate.
      **⭐ A defect this slice found in ITSELF, via a live test.** The first migration bounded `dimensions` to
      **16,000** (pgvector's *storage* cap) while `pgvector::encode` refuses above **2,000** (the *index*
      cap) — so the schema allowed 2,001..16,000, a range **no code path can produce**, and a direct insert
      there would create a row the partial index cannot cover. The live test that tried to store a
      2,001-dimension vector is what surfaced it. Both bounds are now 2,000, and an offline test asserts the
      migration does not contain the storage-cap clause. **A bound nothing can reach reads as capability
      while being an unverified claim — and the two caps belong to different things, so "the server accepts
      it" was never the question.**
      **⭐ The live tests skip without a server and can be made to fail.** `ACCEPTANCE_POSTGRES_URL` supplies
      the connection; **`ACCEPTANCE_REQUIRE_POSTGRES=1`** turns the absence into a failure naming the
      `docker run` that fixes it — the same shape as `ACCEPTANCE_REQUIRE_FIXTURE_PEER` and
      `ACCEPTANCE_REQUIRE_BINARIES`. **Both directions falsified:** no server and no variable → 14 tests pass
      by skipping; no server and the variable set → the run fails with an actionable message. Each live test
      gets **its own schema** (cargo runs integration tests in parallel) and **keeps `public` on the search
      path** — measured: a path holding only the scratch schema made the migration fail with
      `type "vector" does not exist` while the extension was installed in `public`, because an extension
      lives in a schema.
      **⭐ The migration is verified by executing it, not by reading it.** The live test applies the embedded
      file and then reads `pg_indexes` for the index definition, asserting `USING hnsw`, `vector_cosine_ops`
      and the `WHERE (vector_dims(embedding) = 1536)` clause. **What the server built is the evidence, not the
      text that asked for it.**
      **Metadata guard, asserted with a positive control:** four rows identical as vectors and differing in
      exactly one metadata field each (model, normalization, workspace, dimension). The comparable row must be
      found, or the refusals prove nothing.
      **⭐ The SQLx `postgres` feature cost, measured before enabling.** `sqlx-postgres` was **already in the
      lock file**; the feature adds `hmac 0.13`, `md-5 0.11`, `stringprep 0.1.5`, `whoami 2.1.3` and a
      **second `sha2` (0.11.0)** beside the pinned 0.10.9 — `cargo tree --invert sha2@0.11.0` names
      `sqlx-postgres` as the only consumer. `cargo deny` → advisories, bans, licenses, sources **all ok**; the
      duplicate is policy `warn`, as the pre-existing eight are. `tls-*` stays **off** deliberately: a
      networked TLS server mode is `requirements.md`'s multi-device story and no record covers its
      authentication, so a plaintext **loopback** connection is the only one this slice can justify.
      **Every `sqlx` statement that must interpolate is `AssertSqlSafe`-wrapped, and the compiler refuses it
      otherwise.** An operator is a token and `vector(1536)` is a *type* in a cast, so neither can be a
      placeholder; `sqlx` 0.9 rejects a `format!`-built statement with "dynamic SQL strings should be audited"
      and `AssertSqlSafe` is the assertion that gets past it. The audit: `operator()` returns one of three
      fixed tokens from an enum match and `dimensions` is an `i64` from a shared helper, so neither can carry
      a quote or a statement separator, and every caller-supplied value is bound.
      **Recorded as limits:** the extension is **not trusted** (`pg_available_extension_versions.trusted =
      false`), so `CREATE EXTENSION` needs a superuser and an incapable role fails there — with the downstream
      symptom `type "vector" does not exist`, which does not say an extension is missing, and no pre-flight
      check gives a better message. **No server-mode composition**: nothing in `apps/jarvisd` opens a
      `PgPool`, so this repository has **no production caller**. No migration of existing SQLite data, no
      `memories`/`entities` tables in PostgreSQL, and **no foreign key** from `memory_embeddings.memory_id`
      (a `REFERENCES` naming a table nobody creates would fail for a reason unrelated to embeddings).
      **`hnsw.ef_search` is left at its default (40)** and measured as a real recall bound: a 10%-selective
      filter over 400 rows returned at most 40. **One index per width is hand-managed** — `index_ddl` makes
      the second correct-by-construction, but nothing calls it when a new model appears and nothing detects a
      width whose rows are unindexed. **`Normalization` is not recomputed**, only stored and required to
      match. **No `halfvec`, binary quantization, subvector indexing or dimensionality reduction**, so a
      model returning more than 2,000 dimensions cannot be used at all — the refusal names all four.
      Gates: fmt, clippy `-D warnings` over the workspace `--all-targets --all-features --locked`, 47 suites
      with the application binaries present and `ACCEPTANCE_REQUIRE_FIXTURE_PEER=1`, the Postgres suite with
      `ACCEPTANCE_POSTGRES_URL` and `ACCEPTANCE_REQUIRE_POSTGRES=1`, all four phase gates with
      `ACCEPTANCE_REQUIRE_BINARIES=1`, and `cargo deny check` ok.
- [x] `P4-010` Pass isolation, correction, deletion, stale-memory, and adversarial-source acceptance tests.
      `tests/e2e/tests/phase4_gate.rs` (2 process-level cases) + `crates/jarvis-storage/src/workspace_repository.rs`
      (+`workspace_tests.rs`, 5 tests) + `jarvis_storage::record_workspace` + the entity guards in
      `apps/jarvisd/src/memory_service.rs` (2 daemon tests). `ADR-0052`.
      **`jarvis-storage` 182 tests** (up from 177), **`jarvisd` 107** (up from 105).
      **46 suites, 1229 workspace tests, 0 failed, 0 ignored.**
      **⭐ The gate found three defects, and the first is security-relevant.** All three were **invisible to the
      library suites**, which were green throughout:
      **(1) `remember` invented any entity the caller named.** `resolve_entities` only *parsed* identifiers, and
      the link step then called `record_entity` — so `POST /api/v1/memories` naming a subject that did not exist
      **created that subject** and answered `201`. Two consequences: a claim about a fabricated entity is
      **indistinguishable from a claim about a real person**, so a typo produced a memory nobody could find or
      correct; and the **identity vocabulary became caller-controlled**, which
      `identity-and-workspaces.md` forbids (an entity must be established through resolution — verified provider
      IDs, exact identifiers, user confirmation, or a probabilistic match recorded as such). `ADR-0050` had
      refused the placeholder *subject* while the placeholder *mechanism* stayed one layer down.
      **(2) A claim could name another workspace's entity.** Nothing compared the entity's workspace against the
      caller's, so a caller could **write** a claim into its own workspace pointing at another workspace's
      subject — a cross-workspace link dressed as a local claim.
      **(3) Nothing could create a second workspace**, which `A09` requires; every profile has one, seeded by
      migration `0005`. So this slice added `jarvis_storage::record_workspace` — and it is **not** a multi-tenant
      feature: no session, no credential, no route, and no actor can name a workspace.
      **The fix is a read, not a check.** `resolve_entities` now reads the entity and refuses it unless it
      exists, belongs to **this** workspace, and is **usable** (not merged, not deleted — a merged entity's
      claims belong to the winner and a deleted one's to nobody, so a new claim against either attaches itself
      to a name that no longer denotes anything). `is_usable` is the domain's own predicate rather than a
      comparison restated at the edge. `apps/jarvisd` no longer calls `record_entity` at all: the
      `memory_entities` rows are written by `record_memory` in the same call as the memory row, so a claim
      cannot exist without the subjects it named and there is no second place an entity can appear.
      **⭐ Every refusal is paired with an independent second fact**, because a status-code-only assertion can
      pass for the wrong reason: the unknown-entity `422` is paired with a store read proving the entity was
      **not created**; the cross-workspace entity `422` with a search proving no claim **matched**; and the
      cross-workspace read `404` with a **positive** read of this workspace's own claim, so "refuses everything"
      cannot pass.
      **⭐ The gate's first run failed on a real wire-shape change, which is what it is for.**
      `superseded["reference"]["status"]` was `null` — `MemoryDetailReply` **flattens** its reference, so the
      fields are top-level. A client written against the nested shape would have broken in production, and the
      gate reads raw JSON rather than the product's DTOs precisely so a field rename or re-nesting is caught.
      **⭐ Two assertions the gate got wrong, and both were corrected rather than relaxed.** (a) A search for the
      refused wording returned the *stored* claim for **recency** — `is_a_match` is the field that says whether
      something answered the query, and `ADR-0046` deliberately includes recent claims that matched nothing, so
      the assertion now filters on `is_a_match` rather than asserting an empty result. (b) The duplicate check
      would have masked the entity bug if the refused wording had matched a stored claim, so the negative cases
      use **distinct words**: with identical text the claim is refused whatever the entity check does.
      **Falsified, one guard each:** with the entity-workspace comparison disabled the gate stored a claim naming
      another workspace's entity and answered `201`; with the memory scope comparison disabled a read of another
      workspace's claim answered `200` with its content. Both restored, green.
      **Recorded as limits.** **No entity-creation surface exists**, so a remember is still unreachable by a user
      of the shipped product — the gate reaches `record_entity` and then asserts the refusal, so the gap is
      measured rather than hidden. No multi-tenant surface: `record_workspace` has no route and its doc says so.
      No `memory_embeddings` table. `A08` does not exercise the model's context (that needs a configured
      executor model and a provider; it is covered by the daemon's own tests), and `A09` does not cover "context
      build", a model call, a tool, a trace, or diagnostics — those surfaces either take no workspace or do not
      exist.
      Gates: fmt, clippy `-D warnings` over the workspace, 46 suites with the binaries present and
      `ACCEPTANCE_REQUIRE_FIXTURE_PEER=1`, all four phase gates with `ACCEPTANCE_REQUIRE_BINARIES=1`, and
      `cargo deny check` ok (advisories, bans, licenses, sources — eight `duplicate` warnings, all pre-existing
      and none an error).
- [x] `P4-011` Define the skill format and its lifecycle. **`ADR-0117` was recorded first** (accepted): a
      skill is a procedure naming already-granted tools, loading authorizes nothing, a self-authored skill is
      `Derived` context fenced by `ADR-0049`, and promotion is a durable approval that names its approver
      (`ADR-0043`).
      **Recorded as its own slice because the gap is an unmade trust decision, not a missing feature.**
      `requirements.md` lists `procedural` among the memory types and `ROADMAP.md` names "event-triggered
      skills", and **nothing connected them** — no shape, no creation step, no statement of what a skill *is*.
      The reason that matters: "skill" is the name two other designs in this class give to **text that is
      loaded and then acted on**, injected beside the tool list with its effects gated by a pattern list or a
      coarse approval. That step is incompatible with `AGENTS.md`'s boundary — *the model may request an
      effect; deterministic Rust policy decides whether it may happen* — and a skill is a new route by which
      an instruction reaches the model.
      **Delivered:** `jarvis-core` gains `skill` (`SkillRevision`, `SkillStep`, `SkillState`, `DropReason`,
      `SkillDroppedField`, `InvalidSkill`, the `MAX_SKILL_*` bounds) and `SkillId`; a migration
      (`0010_skill_revisions.sql`) makes three of `ADR-0117`'s rules properties of a row; and
      `jarvis-storage` gains `skill_repository` (record, find, three scoped reads, promote, archive, restore,
      supersede). `CURRENT_SCHEMA_VERSION` moves 9 → 10.
      **⭐⭐ AUTHORITY IS UNREPRESENTABLE, WHICH IS THE POINT OF THE WHOLE SLICE.** There is deliberately **no
      field** for a grant, a scope, a pre-approved effect, or a chosen approver — no constructor accepts
      authority and there is nowhere one could arrive. That is the `P3-006c` shape: the rule is enforced by
      what the type can *express* rather than by a check every reader must remember. A skill is an `SkillId`,
      a source, a version, a step list, and prose.
      **⭐⭐ THE PROMOTION RULE HAS THREE ENFORCERS, AND THEY ARE NOT REDUNDANT.** `ADR-0117` §4 makes
      promotion an approval. So (a) `SkillRevision::new` refuses a **model-authored** revision recorded
      `Active`; (b) `promote_skill_revision` writes under `WHERE state = 'proposed'`, so a second promotion
      affects zero rows and is *reported* rather than silently overwriting the approver a first decision named
      (`ADR-0043`); and (c) the **schema** requires that an active model-authored row names its approver —
      `CHECK (state <> 'active' OR source_kind <> 'model_inference' OR promoted_by_actor_id IS NOT NULL)`.
      **(c) is the only layer that can state it.** A promoted revision legitimately decodes as `active`, so a
      decode must bypass the construction check — which is why `SkillRevision::from_stored` exists — and the
      gap that opens is closed in SQL, where a row that reached that state some other way is refused at write.
      A test writes SQL directly to reach it, because a repository-mediated write cannot produce the row.
      **⭐⭐ A FIXTURE PUT ITS DATABASE IN THE WRONG DIRECTORY AND EVERY TEST TOOK FIVE MINUTES.** The first
      version wrote each test's database **directly into the system temp directory**, and one test took
      **306 seconds**. The cause is not the code under test: `SqliteDatabase::open` calls
      `prepare_private_directory` on the database's *parent*, so the fixture was re-securing `%TEMP%` — a
      shared directory with many entries and a broad ACL — on every open. Moving the file into a private
      subdirectory its own `Drop` removes brought sixteen tests from 763 s to 2.6 s. **It looked like a hang
      rather than a slowdown, and the cost was in where the fixture put its file.** The approval fixture
      already used a subdirectory, which is why it was fast; the new fixture did not copy that.
      **⭐ A DECODE RE-APPLIES THE RULES, AND THE TOOL-IDENTIFIER RULE ARRIVES AS A PARAMETER.** A step names a
      tool, and the rule for a valid identifier lives in `jarvis-tools`, which `jarvis-storage` may not depend
      on (`repository-layout.md`: an adapter may depend on core and **not on another adapter**). So every
      function that builds a `SkillStep` takes a `validate_tool` function — the rule keeps **one** home in the
      crate that owns it rather than being restated as a second pattern here. It also means a decode
      **re-applies** the rule: a row written by another build, restored from a backup, or hand-edited is
      refused on read exactly as on write, asserted with the real validator so the failure is the rule rather
      than a parse error. The **provenance equality** is re-applied the same way, and restated in SQL, because
      a model inference that could claim authoritative trust would be a self-authored procedure read as the
      user's own instruction.
      **⭐ A DROPPED EXTERNAL FIELD IS RECORDED WITH A REASON.** `ADR-0117` §7 requires any field that would
      grant authority, preselect an ungranted tool, or pre-approve an effect to be dropped **and the drop
      recorded**, because a field accepted-then-ignored is worse than one never accepted — a reader of the
      stored skill cannot otherwise tell the format's intent from this platform's behaviour. `DropReason`
      carries the four authority-bearing categories plus `Unrepresented`, and `is_authority_bearing()`
      partitions them so a reviewer can see how much of a document was refused.
      **⭐ REPLACEMENT IS DECLARED, SO BOTH LEGS ARE THE CALLER'S.** `ADR-0117` §5 and `ADR-0045`: nothing
      infers a supersession from comparing prose. The successor names what it replaced at insert time and
      `supersede_skill_revision` writes the forward pointer — and it **checks the successor exists first**,
      because a dangling `superseded_by` would make the chain unwalkable and the failure would only surface
      when something tried to follow it.
      **⭐ A STEP'S VERSION IS EVIDENCE, NOT A GRANT.** `SkillStep::tool_version` records the version a
      revision was authored against, so "which version of the tool did this procedure address" is answerable
      from the record (`ADR-0117` §5) rather than reconstructed from today's registry. Whether the actor's
      *current* grant covers a step is decided when the step runs, never at load — a grant revoked between the
      two must refuse the step, and only an execution-time check can see that.
      **⭐ `procedural` MEMORIES AND SKILLS ARE SEPARATE TABLES, RECORDED AS A DEPARTURE.** A skill *could*
      have been a `memories` row with a JSON body, since `procedural` is a valid `memory_type`. It is not,
      because a skill is not a claim: a claim has confidence, decay, retrieval ranking, and a search key,
      while a procedure has versioned steps, a promotion, and a supersession chain. Putting one in `memories`
      would make every retrieval query carry a type predicate to skip the non-claims and would let a skill
      silently acquire a claim's semantics — and collapsing two different revision bodies onto one
      `search_key` would refuse the second as a duplicate.
      Gates: fmt, clippy `-D warnings`, `cargo test --workspace --all-features --locked` with the binaries
      present (adds 20 core tests and 16 repository tests; `jarvis-core` at 262, `jarvis-storage` at 215),
      `cargo deny check` ok — one `yoke-derive` version had to be updated because the advisory database began
      reporting the pinned `0.8.3` as yanked, which is the database moving rather than this change.
      **Limits, recorded rather than glossed:** **no external skill format is read yet.** The *mechanism* for
      recording a drop exists and is tested, but no parser for any external document does — and adopting one
      requires the research `.github/instructions/external-integrations.instructions.md` mandates, including
      deciding which fields a real format carries and which of them are authority-bearing. So `P4-011`'s
      "an external skill format is readable only under `ADR-0117` §7" is **unimplemented by choice**, and the
      drop vocabulary is the seam a format would plug into. Also: there is **no creation surface** (no route
      and no tool writes a skill, so a skill is reachable only from a test), which `P4-013` owns; `steps` and
      `dropped_fields` are JSON columns with `json_valid` bounds rather than child tables, recorded in the
      migration with the reasoning; and no `P4-012` retrieval selection existed at the time of this slice —
      selection and envelope inclusion were delivered later (see `P4-012`), and **the execution path still does
      not**, so nothing has run a skill's steps and nothing in the daemon calls `select_skills`.
- [x] `P4-012` Retrieve and use a skill: selection by relevance to the task, inclusion in the context envelope
      as derived content, and an execution path in which **every step is an ordinary tool request** through
      the full gateway. There is deliberately **no "skill execution" path**, so a step whose grant was revoked
      between load and run must refuse at execution — the check can only be an execution-time one.
      **SELECTION AND ENVELOPE INCLUSION ARE DELIVERED; THE EXECUTION PATH IS NOT, AND THE REMAINDER IS NAMED
      AT THE END RATHER THAN FOLDED INTO A CHECKMARK.**
      **Delivered:** `jarvis-core::skill` gains `SkillQuery`, `SkillIneligibility`, `ExcludedSkill`,
      `SelectedSkill`, `SkillSelection`, `SkillSelectionReason`, `select_skills`, `matches_text`,
      `estimate_skill_tokens`, and `skill_context_item`; `SkillRevision` gains a **`sensitivity`** (with the
      migration column, repository threading, decode rule, and a `CHECK`); and `ContextSourceKind` gains
      **`Skill`** so a procedure's trust rule is representable.
      **⭐⭐ THE `Skill` SOURCE KIND EXISTS BECAUSE THE RULE IS OTHERWISE UNREPRESENTABLE.** Its allowed trusts
      are `[Derived, Untrusted]` — **`User` and `Authoritative` are deliberately absent** — because
      `ADR-0117` §3 says a self-authored procedure must not re-enter a prompt as the person's own
      instruction. **`Memory` does permit `User`**, since a memory can be the person speaking, so reusing the
      memory kind would have made the skill rule unexpressible: a table that allows `User` cannot refuse it.
      The rule is enforced at construction (a test builds a skill item with `User` trust and asserts
      `TrustNotAllowedForSource`), and falsified — widening the arm to include `User` fails the test.
      **⭐ A SKILL IS NEVER POLICY, EVEN WHEN THE PERSON WROTE IT.** `skill_context_item` maps
      `MemoryTrust::Authoritative` **down** to `ContextTrust::Derived`, which is the one place a skill's rule
      is *stricter* than a memory's: a procedure the person wrote is their earlier writing rather than the
      person speaking now. Both directions are asserted, so an implementation that mapped everything to
      `Untrusted` fails too — the distinction between "trusted enough to be derived" and "untrusted" is real.
      **⭐⭐ MY FIRST RANKING DESIGN WAS INCOHERENT, AND THE TEST THAT REPLACED IT RECORDS WHY.** The first
      `select_skills` ranked by **how many query words a revision matched**. That cannot work: eligibility
      already requires **every** query word to appear, so every offered skill matches the whole query and every
      score is identical. **A conjunctive filter cannot also be graded.** The design only appeared to function
      because the test exercising it used a query a *candidate* did not fully match — which the eligibility
      rule refuses outright, so the case could never arise through the function at all. The ordering now
      answers the question a caller actually has when two procedures both apply: **which is current**. So
      revisions order by **creation instant, newest first**, with the identifier as a tie-break so the order is
      total and stable across runs. A memory is still ranked by nine weighted signals and that remains right
      *there*: a claim's eligibility is a set of **floors** rather than a conjunction, so "which claim is most
      relevant" is genuinely graded. **The two retrievals differ because their questions differ.**
      **⭐ THE CLASSIFICATION IS WHAT MAKES THE DISCLOSURE RULE ENFORCEABLE.** `is_eligible` refuses a revision
      above the destination's ceiling and names **both** values in the reason, and the projection feeds the
      same field into `ContextItem`, so `assemble_context` applies it a second time. The schema column is
      `NOT NULL` with **no default**, deliberately: a default would be a value nobody chose applied to every
      pre-existing row, and the only safe default — the most restrictive — would silently refuse to offer every
      skill written before the column existed. A round-trip test asserts each of the four levels survives,
      because a column that silently defaulted would make the rule refuse nothing.
      **⭐⭐ THREE GUARDS AND A PRE-EXISTING LIMIT, FOUND BY ASKING WHERE THE RULE WAS REACHED.** Writing
      `a_skill_cannot_be_required_context` I expected `RequiredMustBeTrusted`, and the test reported, in order:
      `PriorityReasonMismatch` (a retrieved reason implies `Optional`), then `ReasonSourceMismatch` (three of
      the four required-implying reasons name their own source kind), then — for `ReservedPolicy`, which names
      no kind — **`None`, meaning the item is constructible**. The cause is that `RequiredMustBeTrusted` fires
      on `ContextTrust::is_external()`, so it refuses **`Untrusted` rather than every non-authoritative
      class**. A hand-built `Derived`-trust `ReservedPolicy` item on a skill source therefore **can** reserve
      policy budget for a procedure. **That is a pre-existing limit in `context.rs`, not one this slice
      introduced** — the identical construction works with a `Memory` source — and it is now an **asserted
      finding** rather than a latent surprise, with the production route recorded as unaffected
      (`skill_context_item` always supplies `RetrievedMatch`). **Asserting the behaviour found beats asserting
      the behaviour assumed**, and the assumed version would have left the code looking stricter than it is.
      Gates: fmt, clippy `-D warnings`, `cargo test --workspace --all-features --locked` (adds 16 selection and
      projection tests in `jarvis-core`, 2 repository tests, and 2 context-kind tests; `jarvis-core` at 278,
      `jarvis-storage` at 217), `cargo deny check` ok.
      **⚠ A STALE BINARY MADE THE PHASE-1 GATE FAIL WITH A MIGRATION ERROR.** Editing migration `0010` after a
      previous `cargo build` left `jarvisd` embedding the **old checksum**, so SQLx's checksum guard refused to
      re-migrate the database the stale daemon had written (`open database: failed to migrate the SQLite
      database`). `cargo build --workspace` resolved it, and the gate went green with no code change.
      **SQLx checksums embedded migrations, so editing an applied one without rebuilding every binary that
      embeds it produces a failure that reads like a schema bug** — the recorded trap, met again.
      **⭐⭐⭐ A PROMOTION BY THE REVISION'S OWN AUTHOR IS REFUSED, AND THAT RULE HAD NO ENFORCEMENT AT ALL
      UNTIL IT WAS WRITTEN.** `ADR-0117` §4 states the boundary in words — *"an agent that could author a
      procedure and promote it would have authored its own effect"* — and the schema states half of it as a
      `CHECK` (a model-authored **active** row must record who promoted it). **Neither was enforcement of the
      second half.** The construction check refuses a model-authored revision *recorded* `Active`, but its own
      author could simply call `promote` to put it there, and the `CHECK` was satisfied because the row now
      named an approver — the author. `SkillRevision::promote` now refuses `approver == created_by_actor_id`
      with `InvalidSkill::PromotionSelfApproval`, and refuses a blank approver with
      `InvalidSkill::PromotionUnattributed` — **separate variants on purpose**, because the remedies differ: one
      needs an approver *named*, the other needs a *different* actor to decide, and collapsing them sends an
      operator looking for a missing field that is not missing. The comparison happens **after trimming**, and a
      test asserts a whitespace-padded self-promotion is still refused, because the guard exists to be bypassed
      and a padded string is the first bypass an author would reach for.
      **This is deliberately the same rule `jarvis_core::ApprovalRequest` applies to a tool call**
      (`InvalidApprovalField::SelfApproval`), including its scope: there the rule is **unconditional**, so a
      *user* cannot answer an approval they requested either — checked against `approval.rs` rather than
      assumed before mirroring it, because a guard copied at the wrong scope is a guard that refuses the
      legitimate case. `promoted_by_actor_id` is also bounded by `MAX_APPROVER_ID_CHARS` — **the `decided_by`
      column's own bound**, imported rather than restated — because a promotion *is* an approval and one
      identity must not have two lengths; a domain that accepted any length would report a revision as valid
      and then fail on write. The bound test asserts **at** the limit is accepted and **one past** it is not, so
      an implementation that refused every approver fails the first half rather than passing on the second.
      **⭐ EVERY TRANSITION REACHES THE DOMAIN, WHICH IS WHY THE GUARD CANNOT BE BYPASSED FROM STORAGE.**
      `promote_skill_revision`, `archive_skill_revision`, `restore_skill_revision`, and
      `supersede_skill_revision` each `find` the row, call the domain transition, and only then write — so no
      repository function can move a revision to `Active` without constructing the value that holds the rule.
      The `UPDATE`'s own `WHERE state = 'proposed'` is **defence in depth against a concurrent writer**, not a
      second copy of the rule, which is the shape this repository prefers: the domain decides, the schema and
      the predicate catch a race.
      **⚠ THE LESSON IS THE `ADR-0035` SHAPE, MET AGAIN: a decision that names a rule in prose is not
      enforcement.** §4's words were quoted in the migration, in the constructor, and in `promote`'s own
      doc comment — three places that *described* the boundary — while nothing anywhere *checked* it. **Read
      each boundary a decision states as reasoning and ask which check owns it**; a sentence explaining why
      something is forbidden is evidence that it was considered, never evidence that it is refused.
      **⭐⭐ `select_skills` IS NOW CALLED BY THE DAEMON, AND A STORED PROCEDURE REACHES A REAL PROMPT
      FENCED.** `apps/jarvisd`'s `assemble_context_messages` reads the run's own workspace's **active**
      revisions through `read_usable_skill_revisions`, builds a `SkillQuery` whose **text is the objective**
      and whose **destination is the model's placement ceiling**, selects, and offers each as an item from
      `skill_context_item` at **`Optional`** priority — a procedure that does not fit must be dropped rather
      than failing the run, exactly as a conversation turn is. The message builder renders each included
      skill's **prose and every step instruction** inside the same fence the memories use, under the same
      introduction, because a procedure is retrieved content for the same reason a claim is (`ADR-0117` §3);
      the introduction's own promise — *"the policy and the request win"* — is exactly what a procedure
      needs. The run event's summary gained `skills_offered`/`skills_included`, so "why was this procedure not
      used" is answerable from the record without storing the procedure. Deliberately **no `skills_altered`**:
      a count that is always zero is a field a reader learns to skip.
      **⭐⭐⭐ THREE DEFECTS FOUND BY WIRING IT, AND THE THIRD WAS INTRODUCED BY FIXING THE FIRST.** (1)
      **`ORDER BY created_at` is wrong for these timestamps** — they are RFC 3339 text and a whole second
      **omits its fraction**, so `'…:20Z'` sorts *after* `'…:20.5Z'` (`.` is 0x2E, `Z` is 0x5A). With a window
      applied the read dropped a **middle** row and kept the oldest. (2) **`julianday` does not fix it** — it
      resolves to milliseconds, so the boundary case still ties or mis-orders. (3) **⭐⭐ THE TIE-BREAK'S
      DIRECTION WAS WRONG, AND IT ONLY APPEARS ONCE (1) IS FIXED.** `ORDER BY unixepoch(created_at) DESC, id
      ASC` orders a **tie** — two rows inside one second — **oldest-first**, which is the exact opposite of
      what a newest-first read is for. It was introduced by adding the tie-break to make the order total, and
      no fixture of evenly-spaced whole seconds can see it: **the tie-break is part of the same ordering, not
      a separate concern, and an ordering has ONE direction.** Every newest-first statement in `jarvis-storage`
      is now `unixepoch(<col>) DESC, <tie-break> DESC`: three in `skill_repository` and four in
      `memory_repository`, the latter including the **paginating** `read_all_memories` export.
      **⭐ THE ASSERTION THAT FOUND ALL THREE WAS "WHICH ROW IS ABSENT", NOT "HOW MANY CAME BACK".** Two of
      the three defects returned the **right count with the wrong set**, so a count-only assertion is satisfied
      by every one of them. `the_usable_read_returns_at_most_the_limit_newest_active_revisions` and
      `the_retrieval_window_returns_the_newest_memories` both assert identity, and the skill one was **proven
      to falsify** against the original ordering. Its fixture is deliberately awkward: three instants **inside
      one second** with the **oldest carrying a whole-second timestamp**, and identifiers whose milliseconds
      match their instants (as `Uuid::now_v7` writes them) so the tie-break agrees with the row.
      **⚠ The sub-second limit is recorded rather than hidden.** `unixepoch` resolves to whole seconds and
      `julianday` only to milliseconds, so genuine nanosecond ordering needs an **integer-nanoseconds column**
      — a migration across every timestamp column, which is `ADR-0034`'s decision and is still **open**;
      `jarvis-core/src/timestamp.rs` already pins the underlying trap with
      `the_stored_form_is_not_lexicographically_sortable`.
      (2) **A conjunctive match is brittle by construction, and the run looks successful either way.** A user
      asking about *"my notes"* is not offered a procedure that says *"the user's notes"*, because
      `matches_text` requires **every** word. Both behaviours are asserted — the positive case (the procedure
      arrives fenced, prose **and** step text) and the negative (`an_objective_with_an_unmatched_word_offers_no_skill`),
      because the failure mode of this rule is **silence**. The fence assertion was falsified by sending the
      item's reference instead of the rendered procedure; the test failed, then passed after restoring.
      `ToolValidator` also gained `Send + Sync`, because these reads are `async` and the daemon calls them from
      a **spawned task** — the bound belongs in the alias, or it surfaces as a call-site error.
      **Limits, recorded rather than glossed:** **the execution path is not built**, which is the half of this
      slice's own acceptance that says every step must re-enter the ordinary gateway — no "skill execution"
      path exists, so nothing runs a skill's steps at all, and no model-facing surface tells the model what to
      call first; there is still **no creation surface** and no `P4-013` inspection verbs, so a skill remains
      reachable only from a test; text matching is whole-word containment over the prose and step
      instructions, deliberately **not** the nine-signal ranking a memory gets, for the reason recorded above.

      **⭐ PART 2 — a procedure reached the model with NO TOOL IN IT, and three artefacts hid it.** `P4-013`'s
      creation route made a stored procedure reachable for the first time, so I checked what a *selected*
      procedure actually sends. It sent the prose and each step's **instruction** and **dropped the tool**:
      - **One cause, three silent consequences.** The executor concatenated the body itself; `estimate_skill_tokens`
      counted the same fields; `skill_haystack` indexed the same fields. So (1) the prompt had no tool, and
      `P4-012` is literally "every step is an **ordinary tool request**"; (2) the budget **under-measured** the
      text it sent, so a skill reporting itself as fitting could overflow; (3) `matches_word`'s own doc comment
      — "what makes a query naming a tool find a procedure that calls it" — was **false**, because an objective
      containing `jarvis.files.read` splits into segments the haystack did not contain.
      - **The fix is one function, `jarvis_core::render_procedure`**, which the executor, the estimate, and the
      haystack all derive from. Three artefacts that must agree can no longer disagree.
      - **The e2e test asserted three fields of the rendering and never the tool**, which is why it passed: a
      test that checks several fields is not a test of the rendering, and the absent field had nothing
      asserting it. It now asserts `jarvis.files.read@1.0.0` reaches the prompt.
      - **Falsified at both layers** — reverting the renderer failed the exact-string test, the tool-matcher
      test, **and** the `jarvisd` prompt test.
      - **⭐ THE RENDERING MADE A LATENT INJECTION REACHABLE: a step's `tool_version` was validated by LENGTH
      ONLY.** It is interpolated into the body, so a version containing a newline forges a second step that the
      author never wrote — in a body a model then follows as a procedure. The rule was length-only in
      `SkillStep::new`, length-only in `0010`'s `CHECK`, and the migration's own comment claimed the two
      "cannot disagree about what a version looks like" while **neither looked at a character**. Now validated
      by its characters (`[A-Za-z0-9.\-_+]`, the set `ToolId::validate_version` accepts, restated because
      `jarvis-core` cannot depend on `jarvis-tools`). The decode goes through the same constructor, so a row
      that arrived another way is refused on read too.
      - **And the §6 property is now asserted:** a procedure naming a tool that **declares** an approval still
      parks the run with zero adapter calls. There is deliberately no "skill execution" path to test — the step
      is an ordinary tool request through the whole pipeline, and that is how the property is true.
      - **Still not built, and the remainder is now narrowed:** nothing gives a model a *first step*. The
      procedure now says which tool each step needs, so a model following it can issue the call; what no surface
      does is tell the model to begin, and a procedure whose step names a tool the model is not offered cannot
      be performed by it. The ADR's "a grant revoked between load and run must refuse the step" is also
      **unasserted** because nothing revokes a grant while a run is live and the registry is fixed at
      composition — recorded as a limit rather than claimed.
      - **Verified live:** a procedure created through `jarvis skills create` is `active`, and
      `jarvis skills show` renders `1. jarvis.memory.propose (1.0.0)`, so the tool and version survive the store.
- [x] `P4-013` Add skill inspection and control: list, inspect (what it is, what it names, its source), correct,
      forget, export, and disable — the `FR-MEM-005` lifecycle surface, applied to a stored procedure.
      **Delivered:** `jarvis-protocol::skill_api` (14 DTOs), `apps/jarvisd/src/skill_service.rs`,
      **8 routes** (`GET`/`POST /skills`, `GET /skills/export`, `GET`/`DELETE /skills/{id}`,
      `POST /skills/{id}/{promote,disable,enable}`), `apps/jarvis-cli/src/skills.rs` (8 verbs), and the storage
      operations they need (`find_skill_revision_state`, `read_workspace_skill_revision_states`,
      `delete_skill_revision`, and an optimistic-locking guard on all four transitions). **The creation surface
      `P4-012` recorded as missing is now the `POST /skills` route**, so a skill is no longer reachable only
      from a test — which is what the retrieval path needed to have a producer at all.
      **⭐⭐⭐ THE CREATION SURFACE IMMEDIATELY FOUND A DEFECT THAT MADE `enable` DESTROY A USER'S PROCEDURE.**
      `SkillRevision::restore` derived the restored state from the **promotion record alone**, so a
      user-authored revision archived and re-enabled came back `Proposed` — and **no actor could ever fix it**,
      because promotion refuses an approver equal to the revision's author and that author *is* the user. A
      disable/enable pair permanently destroyed a procedure the person wrote, and nothing in the domain, the
      storage layer, or the earlier tests could see it: the shape was only reachable once a route could create a
      user-authored revision. The derivation now uses the same three facts construction does (a model-produced
      revision is usable only if promoted; a user-authored one needs no promotion), tested both ways so a fix
      that returned everything to `Active` fails.
      **⭐⭐ A SUPERSESSION CHAIN COULD LEAVE ITS OWN PROCEDURE.** `supersede_skill_revision` confirmed the
      successor **exists** (`acknowledge_skill_revision`) and never confirmed it was **related** — so
      `superseded_by` could point from one procedure to a revision of a **different** one, and "what replaced
      this" would answer with another skill's revision. The `ON DELETE SET NULL` columns made the deletion case
      equally wrong: deleting either end quietly **blanks the other's link**, erasing one leg of the declaration
      `ADR-0117` §5 exists to make readable. The transition now requires both ends to share a `skill_id`, checked
      in **one statement** rather than a read plus a comparison (a property of the *pair*, so two round trips
      leave a window), and `delete_skill_revision` refuses a revision any other revision links to. Both refused
      **before** any write, asserted on the row so a check that reports an error and writes anyway fails.
      **⭐⭐ `promote` HAD TWO STATE DEFECTS AND ONE `SelfReference` REUSE.** It reported
      `ModelAuthoredTrust` — the *provenance* error — for an already-active revision, sending an operator to
      inspect a procedure's provenance for a state problem; and it **accepted an archived revision**, making
      `promote` a second undocumented route to `Active` that bypassed `restore`. Meanwhile `archive`/`restore`
      reused `InvalidSkill::SelfReference` ("a value referring to itself") to mean "wrong state". All three are
      now one vocabulary: `InvalidSkill::WrongState { transition, state }`, with the verb as a field because the
      verb differs and the fact does not. The storage mapping keeps a **per-verb reason** (`already active`,
      `archived; restore it instead`) because each remedy is a different action.
      **⭐⭐ A `version_counter` WITH NO READER, AND THE GUARD IT WAS FOR.** The column was written by
      `record_skill_revision` and incremented by every transition, and **no read returned it** — so the
      optimistic guard a control surface needs had no way to obtain the value it must present. Added
      `StoredSkillRevision`, a narrow wrapper (`revision` + `version_counter`) used by the **control** reads
      only: the retrieval path deliberately does not return it, because a counter has no meaning to a selection
      that is a pure function of the revision set, and widening a hot read would put a value with no reader on
      it (`ADR-0092`). All four transitions now guard on `version_counter` **and** the state they start from.
      **⭐ ZERO ROWS HAS TWO CAUSES AND THE REMEDIES ARE OPPOSITE.** A guarded `UPDATE` affecting nothing is
      either a **stale counter** (the row exists and changed — re-read and retry) or a **missing row** (stop),
      so `classify_skill_write_failure` resolves which by reading, and both are asserted separately; an
      implementation reporting one for both fails one of them.
      **⭐ THE LISTING CARRIES NO PROCEDURE TEXT, AND THE FENCE DISCIPLINE FROM `P4-012` IS WHY.** A skill's
      prose and step instructions both reach a prompt, so a reference that leaked them would be a second, less
      careful path into a model's context than retrieval's. The test asserts the prose is **absent from the
      listing and present on the detail read**. `detail_of` also reports usability from the **domain's own**
      `is_eligible` rather than from `state` + `superseded_by` — the first version re-derived it and **disagreed
      with retrieval** on a revision that is both archived and superseded.
      **Verified live, not only in tests:** `jarvis skills create/list/show/disable/enable/forget` were run
      against a real `jarvisd`, and the `enable` output (`returned to active from its promotion record`) is the
      user-authored stranding defect observed as **fixed** on a real profile. A stale `--version` returned the
      `409` conflict with the re-read remedy, an already-active `promote` returned the state refusal rather than
      a provenance one, and the two CLI step-pairing refusals named the actual problem.
      **⚠ A HINT THE DAEMON REFUSES.** The client's own message said *"set `JARVIS_HTTP_ENABLED=1`"*, and the
      daemon parses that variable as a **strict `bool`** — so following the hint produced
      "invalid value for configuration environment variable" with no indication of what would be valid. The hint
      now spells out `true`. **A hint that names a value the daemon rejects sends an operator to look for a
      problem that is in the hint.**
      **⚠ AND THE DAEMON'S OWN MESSAGE HAD THE SAME GAP, which cost two failed attempts to find.** While
      verifying against a live daemon I set `JARVIS_HTTP_ENABLED=1` and read
      `invalid value for configuration environment variable JARVIS_HTTP_ENABLED` — naming the variable and
      nothing about what would be accepted. `ConfigError::InvalidEnvironmentValue` now carries an `expected`
      field, and every site states the vocabulary **its own parser** accepts: `true or false`,
      `one of trace, debug, info, warn, error`, `a whole number of seconds`, `a port number from 1 to 65535`,
      `valid Unicode text`. The real daemon now answers `…: expected true or false`.
      - **The expected string is bounded by what that check actually parses, not by the rule that matters most.**
        A value like `JARVIS_SHUTDOWN_TIMEOUT_SECONDS=0` parses fine and is refused later by
        `InvalidShutdownTimeout`, whose own message names the range — so writing "1 to 300" in the parse refusal
        would claim coverage it does not have. Both refusals are asserted, because they arrive from one variable
        and are easy to conflate.
      - **The rejected value is never echoed**, and that is asserted: an environment value can be a credential,
        and a startup log is read by more people than the shell that set it. The two properties are one test with
        two assertions because either alone is satisfiable by the wrong message.
      - **Class of defect worth naming: a message that identifies the problem and not the remedy.** This is the
        third instance in this slice (the `JARVIS_HTTP_ENABLED=1` hint, this message, and the absent-route
        report), and the shape is always the same: the text says *what* failed and leaves *what would work* to
        be guessed, and the obvious guess is the one the code rejects.
      **Limits, recorded rather than glossed:** **the execution path is still not built**, so nothing runs a
      skill's steps and the lifecycle ends at "this is available"; `correct` is expressed as a creation with
      `--supersedes` + `--skill` rather than a dedicated route, and a correction's `skill_id` must be **stated**
      by the caller because the domain compares the predecessor against the skill and neither is derivable
      without the store; the export takes no offset; `sensitivity` is fixed at `internal` on creation (no flag
      to classify a procedure, so a `public` one cannot be authored here yet); and the tool-membership check
      needs a composed pipeline, so with no tool surface only the structural identifier rule applies.
- [ ] `P4-014` Add agent-proposed memory admission: the model may submit memory candidates, a candidate that
      deterministic code supports becomes a `Proposed` record, and admission is a decision that names its
      approver. The inference boundary is unchanged (`P4-001`, `ADR-0049` §6): a model inference is never
      admitted above `Unverified`.
      **⭐ PARTIALLY DONE — the "becomes a `Proposed` record" half was BROKEN and is now fixed, and the
      decision that names its approver now exists.** Starting the slice, I checked the boundary's *claim*
      against the code rather than adding to it, and the two layers that are supposed to agree disagreed:
      - `MemoryCandidate::admit` decides `MemoryAdmission::Proposal` for a model inference. Its own comment
        says one "is never written as current truth", and `CandidateClassification::requires_proposal` is the
        predicate. `MemoryAdmission`'s doc says writing a `Proposal` and reading it back "is what proves the
        two layers agree rather than merely intending to" — **nothing did that**.
      - `MemoryRecord::build` derived the status from `memory_type.requires_confirmation()` **alone**, so it
        answered `Active` for every inference. A `source_kind` of `model_inference` sent to
        `POST /api/v1/memories` was answered `201` with status `active`: the model's own claim stored as the
        workspace's current truth, contradicting `ADR-0049` §6 and the module comment in both crates.
      - **The harm is concrete, not a labelling nicety.** `compare` reports a candidate whose text differs
        from the current claim at its key as a `Correction`, and a correction **supersedes**. So one model
        inference silently retired an earlier claim — an effect produced by the disagreement.
      - **The fix is one condition**, `|| source.kind().is_model_produced()`, which is the rule
        `jarvis-core/src/skill.rs` already applies to procedures in two places — so the memory layer was the
        one that lacked it. The confidence ceiling (`Unverified`) and the status were two different rules, and
        only the first was enforced: bounding the level is not the same as refusing the claim as current truth.
      - **Three tests encoded the old behaviour, and one had the invariant as its NAME.** `memory/tests.rs`'s
        inference test asserted a raised confidence is refused and that `Unverified` is `is_ok()` — so a
        derivation returning `Active` passed it. `retrieval/tests.rs` used an inference as a convenient
        `Derived`-trust fixture and was actually refused by the **trust** rule, so the currency rule was never
        reached; it now uses a tool observation and the inference has its own test asserting the exclusion
        holds even against a query relaxed on every requirement. `jarvis-storage`'s test named
        `a_model_inference_cannot_be_recorded_as_a_fact` while asserting, in a comment, that the claim **was**
        `is_current_truth()` and that the *presentation* predicate was the thing keeping it from being
        asserted — "which is why the two are separate". Two layers disagreeing about one question is not a
        separation of concerns.
      - **Falsified at both layers.** Reverting the one condition failed the new `jarvis-core` test
        (`a claim nothing can support is a proposal, not a fact`) **and** the new gateway test
        (`a claim no source can support must not be the workspace's current truth`), each in the binary a
        developer would run. A control asserts a user-stated `Semantic` claim is still `Active`, so the rule
        is not satisfied by proposing everything.

      **Delivered for the second half — the decision that names its approver:**
      - **Migration `0011_memory_admission.sql`** adds `admitted_by_actor_id` and `admitted_at`, modelled on
        `skill_revisions.promoted_by_actor_id`/`promoted_at`. `CURRENT_SCHEMA_VERSION` 10 → 11. The backfill is
        deliberately **empty**: rows that predate admission were confirmed by a route that recorded nobody, and
        inventing an approver would fabricate an authorization for a decision nobody made.
      - **`MemoryRecord::confirm_by(approver, at)`** records the decision; `confirm` is kept as the **decode**
        companion, because a row written before admission decodes to `active` with no approver and re-applying
        the stored status must not invent one.
      - **`MemoryTransition::Confirm { approver_actor_id }`** — the approver is part of the variant, not an
        argument beside it, so a state change cannot be separated from the decision that justifies it.
      - **`POST /api/v1/memories/{id}/confirm`** plus `ConfirmMemoryRequest` (which carries **only**
        `expected_version`) and a `jarvis memory confirm <id> --version N` verb. `MemoryTransition::Confirm`
        existed from `P4-002` and **no route called it**, so a `Proposed` claim could be created over HTTP and
        never accepted — a claim the workspace held, offered to nobody, with no way to make it current.
      - **A defect found on the way: the author identity was fabricated.** `remember` hardcoded the string
        `"local-user"` while every other write path uses `LOCAL_USER_ID` (`0198f000-…-0000000000b1`, what
        `0005` actually seeds). It now reads the identity, as `SkillService::actor_id` does.

      **⭐ A GUARD I WROTE AND THEN DELETED, BECAUSE IT BLOCKED THE ARCHITECTURE'S OWN FLOW.** I first added a
      self-admission refusal mirroring `ADR-0117` §4 — an approver must not be the claim's own author. It made
      the live route answer `503` and, once that was diagnosed, `422` on the *intended* case:
      `memory-and-context.md` says a high-impact inference "requires explicit user confirmation", so the person
      confirming **is** the person whose statement produced the candidate. With one seeded identity the author
      and approver are always the same value, so the guard refused every legitimate confirmation. The rule worth
      wanting — *an agent must not admit what it authored* — needs an actor vocabulary that distinguishes a
      model from a person, and this build has one seeded human; even a run's model-produced candidate is stamped
      with that human's identifier. **`ADR-0117`'s rule is about a procedure's promotion, where the author can
      be a model; copying it onto a memory, where the author is a person, inverts it.** Two tests now assert the
      *non*-refusal so the symmetry cannot be restored without seeing why.
      - **Also fixed: a corrupt row answered `422`.** `decode_memory` mapped a domain refusal to
        `InvalidMemoryRequest`, which names a **request** field for a value the caller never sent — sending an
        operator to inspect their request instead of their data, and telling a caller to change what they ask
        for when what they need is a backup. It now reports `StoredMemoryInvalid`, and the field table is shared
        between the two mappings rather than duplicated.
      - **Falsified:** binding `None` for `admitted_at` (the "did not write the column" bug) failed the new
        storage round-trip test **and** both gateway tests. The half-decision rule catches it too — the mutant
        produces a row the decoder refuses, which is the property that makes the pair safe to store.

      **Still unbuilt, and now the whole of the remaining work:** no **tool** touches memory at all (every
      module in `jarvis-tools` is files/documents/policy/registry), so the model has no submission path; the
      route therefore needs a `source_kind` a model cannot set itself, and the submission path is what makes
      "a model may propose" true. `SkillService` documents a `propose` path for an agent-authored revision that
      still does not exist. The admission columns carry only their own bounds — SQLite cannot add a
      column-level `CHECK` mentioning `status`, and a table-level one needs the twelve-step rebuild that
      recreates four indexes and two self-referencing foreign keys — so the cross-column rule is enforced on
      the read path, which is the only enforcer; see `ADR-0124`.

      **⭐ PART 3 — the model's submission path now exists, and building it exposed the reachability defect the
      slice would otherwise have shipped.** `apps/jarvisd/src/memory_propose.rs` is the adapter:
      `jarvis.memory.propose`, a `write` at risk 1, approval `auto`, scope `memory.propose`.
      - **The arguments are narrower than `RememberRequest`, and every omission is load-bearing.**
        `additionalProperties: false` refuses `source_kind`, `confidence`, and `supersedes` — each of which
        exists on the HTTP request and each of which would defeat the boundary **through a field** rather than a
        bug: a claim labelled `user_statement` at `confirmed` is a model's output recorded as something the
        person said. The source is set in code, the confidence is `Unverified`, the locator is derived from the
        correlation identity, and supersession is impossible.
      - **Asserted through the pipeline's own schema validation, not the adapter's parser**, because the parser
        never sees those fields — testing it alone would prove nothing.
      - **Falsified:** switching the source to `UserStatement` failed the test, which reported the claim was
        stored as **`remembered`** — a status only a confirmed user statement can have. The mutant is caught by
        the outcome string, not by a provenance check.
      - **A judged candidate is a completed call, not a failure.** An unresolved entity or a refused rule is
        reported as `refused` with the rule's name, because telling a model the platform broke when the platform
        judged its input produces a retry loop. `UnknownValue` — the *common* case for a model naming a subject
        it only inferred — is in this arm rather than treated as an adapter fault.

      **⭐⭐⭐ THE TOOL SHIPPED REGISTERED, OFFERED, AND DENIED — because the actor's authority was a hand-written
      scope list.** `ToolActor` had `workspace_and_mcp` granting `files.read` + `mcp.call`; no constructor
      granted `memory.propose`, so every call would have been refused `MissingScope`. Nothing catches this:
      `Dispatch::verify_covers` checks the opposite direction (a registered tool with no adapter), and the
      adapter's own tests pass because they call the adapter directly.
      - **I then made the same mistake a second time within one change.** Adding `workspace_and_memory` with
        `files.read` + `memory.propose` *replaced* `mcp.call` at the HTTP site, and `phase_3_gate` failed:
        **`a write tool must pause for a human decision, got 403 missing_scope`**. I had reasoned that the
        surface served only native tools — it serves MCP tools too, and I had not checked.
      - **The fix is the pattern `ToolActor::remote` already used**: derive the authority from the **composed
        tools' own declarations**. A tool that is composed is callable; a tool that is not cannot be addressed;
        so the union *is* the daemon's authority over its own surface. No list, and a new adapter is reachable
        the moment it is registered.
      - **`workspace_and_memory` was then deleted**, because its only caller was the test I had just written.
        `for_composed_tools` superseded it, and a public constructor nothing calls is the shape this file's own
        comment warns about. `workspace_and_mcp` and `MCP_CALL_SCOPE` became `#[cfg(test)]` — the established
        pattern — because they survive only as the fixture proving the grants are distinct.
      - **And `jarvis tools preview` had never worked.** Its path used `path_segment`, the **run** identifier
        rule, which permits only alphanumerics and hyphens — so it rejected **every** tool identifier, since
        `namespace.name` contains dots, and the error named a *run*. Added `dotted_path_segment`, whose
        permitted set is the tool grammar (`[a-z0-9_\-.:]`, lowercase only, which the first attempt got wrong in
        both directions: it refused `list_issues` and accepted `Jarvis.memory`). `..` and the leading/trailing
        dot are refused as path-traversal and grammar cases — the traversal one found by writing the
        adversarial case into the test's name and watching it pass when it should have failed.
      - **Why the preview mattered more than it looks:** it answered `deny / missing_scope` for a tool the daemon
        was offering. A preview that lies in the refusals direction is worse than no preview, because it sends
        an operator to change a policy that is already correct.
      - **Verified live:** `jarvis tools list` → `jarvis.memory.propose … callable`; `tools preview` → `allow /
        allowed`; a run reaches `outcome=succeeded` with the tool present. Both defects were found by running the
        CLI against a real daemon, not by the suite.
      - **Limits:** with one seeded identity and no entity-creation surface, no end-to-end propose through a live
        model was possible — the adapter's behaviour is proven by its tests and the registration and preview by
        the daemon. The tool is `auto` because the claim is stored `proposed` and excluded from retrieval at any
        status; that reasoning is asserted beside the declaration, so changing the status rule forces a decision
        about the approval.

- [x] `P4-015` Add session summarization as derived memory: a compressed summary of a session is stored as a
      `Derived` claim with provenance and a retention rule, and is never presented as user-authored fact.
      **Domain vocabulary** in `crates/jarvis-core/src/summary.rs` (`SessionSummary`, `SummarySpan`,
      `SummaryLoss`, `spans_overlap`, `is_offerable_as_context`, **15 tests**); **storage** in
      `crates/jarvis-storage/src/summary_repository.rs` + migration `0012` (**14 tests**); **surface** as
      `POST|GET /api/v1/sessions/{id}/summaries` and `POST …/summaries/retire`, with **4 route tests**.
      `ADR-0125`.
      - **A summary is a memory plus a span table, and the split is the decision.** The memory half is what every
        memory rule already covers (workspace, entity links, tombstones, sensitivity), produced by
        `into_record` as `Conversation` + `Document` → `Derived`. The span half is what no `memories` column can
        hold, in a table whose **primary key is the memory identifier**, so "this row is a summary" and "this row
        has a span" are one fact rather than a nullable pair meaningless for six of seven types.
      - **Its own rules live in the writer, because SQLite cannot state them.** `session_summaries` cannot see
        `messages`, so it cannot refuse a span naming turns that do not exist; and "two intervals in one session
        must not intersect" needs a trigger. Both are checked in `record_summary`, which reads the message count
        and the session's spans on the **same connection** as its two inserts — a deferred read-then-write over
        the pool is the `SQLITE_BUSY_SNAPSHOT` failure `purge_memory` already records.
      - **The retention rule had a predicate and no caller.** `MemoryType::is_durable` states that `Working` and
        `Conversation` do not survive, and only a test named it; `P4-008` recorded "no retention policy is
        implemented — the verbs exist and the sweeper does not". The sweep now **generates** its SQL predicate
        from `is_durable` rather than listing type names, so the rule and the sweep cannot drift when a type is
        added. Archive rather than delete, because `Archived` is "retained for audit, not retrieved as current
        truth" — and deleting would additionally clear the search key and write a tombstone, turning a retention
        decision into a *forget*.
      - **A predicate whose answer never varied was a decision that decided nothing.** `is_offerable_as_context`
        first returned `false` unconditionally, which reads as a policy and encodes none; it now takes the spans
        a caller is **replaying** and answers the real rule, which also gave `spans_overlap` the production
        caller it lacked.
      - **Two defects the tests found, both by mutation rather than by review.** An archived summary was still
        returned by `read_session_summaries` — it filtered `deleted` and admitted `archived`, the same
        admitted-but-never-current shape `P4-007` recorded — so the read now filters `active` while the *span*
        read deliberately does not, or archiving a summary would re-open its turns to a second claim.
      - **A test named for the wrong guard.** `the_same_sequences_in_another_session_do_not_overlap` claimed to
        catch a missing query filter; mutating `spans_overlap` to ignore session identity **survived** it, and
        only `jarvis_core`'s `overlapping_spans_are_detected_in_both_directions` caught that. Mutating the
        query's `session_id` filter *did* fail it. Two guards, two detectors, and the comment now says which is
        which — a test named for a rule nothing checks reads as coverage.
      - **A fixture that promised a count it did not deliver.** `seed_session` appended N messages, but starting
        the run already writes the objective as message 0, so every session held N+1 and every expected span was
        shifted by one while still looking plausible. It now asserts the count it promises.
      - **Verified live** against a real daemon (`database_schema=12` in the readiness line, so the migration
        ran): `GET /sessions/{id}/summaries` → `{"summaries":[],"returned":0,"limit":128}`; a summary whose
        subject is a fabricated entity → `404` (the service's subject check, reached); a malformed session
        identifier → `422`; `?limit=0` → `422`; `POST …/summaries/retire` → `{"archived":0}`.
      - **Limits:** with no entity-creation or entity-listing route (the `P4-008` gap, unchanged), a real
        end-to-end **store** over the wire was not possible — the write path is proven by 14 storage tests and
        the routes by 4, and everything before the subject check is proven live. Nothing yet moves a history
        window past a span, so `is_offerable_as_context`'s `true` branch is reachable only from a caller
        supplying non-overlapping spans: the rule is complete and falsified, and what is missing is a windowing
        component. The summary route is HTTP-only — no CLI verb, matching `P4-014`'s tool, because both need an
        entity identifier the CLI has no way to obtain.

- [x] `P4-016` Add the entity surface: an operator can create, list, and look up the entities a memory is
      `about`, and attach an alias to one so a later lookup resolves by name rather than by identifier.
      **The gap three slices recorded.** `P4-008`'s note is that "**No entity-creation surface exists**, so a
      remember is still unreachable by a user of the shipped product"; `P4-014` and `P4-015` each record the
      same limit from a different direction. The measurement behind this slice: seven entity-repository
      functions had **no production caller at all** — `merge_entities`, `record_alias`, `resolve_alias`,
      `read_alias_candidates`, `read_entity_memories`, `record_relation`, `read_subject_relations` — and the
      last-named two are exactly the "entity resolution" that note names as missing.
      **New:** `crates/jarvis-protocol/src/entity_api.rs` (13 DTOs), `apps/jarvisd/src/entity_service.rs`,
      `apps/jarvis-cli/src/entity.rs` (**3 tests**), 5 routes with **6 route tests**, 4 storage reads and one
      counter. `ADR-0126`.
      - **Its own surface, not a sub-resource of `memory`.** An entity is not a memory: different lifecycle
        (created, aliased, merged versus proposed, confirmed, corrected, forgotten), different owner (the
        operator's vocabulary, not the model's), and a different role — it is what a claim is *about*. As a
        sub-resource it would put "create the subject" behind the verb that requires one, which **is** the
        recorded ordering problem.
      - **A lookup returns every candidate with its evidence, and the CLI renderer is where that rule could be
        undone.** `resolve_alias` documents it — "ambiguous aliases remain separate candidates" — so the reply
        is a list and each match says whether it was `verified`. A client printing `matches[0]` would present a
        guess as an identity at the last step, so the renderer has its own test and the daemon exposes **one**
        route for both lookup kinds rather than two that could diverge.
      - **The verdict is the conjunction**: an entity holding a verified name *and* a probabilistic one for the
        same value is not verified, because the question is whether this name denotes this entity and one of the
        matching names is a guess.
      - **⭐⭐⭐ The schema's rule and the nearest domain predicate were DIFFERENT SETS, one line apart from being
        treated as one.** `0009` permits a `confirmed` alias only from `user_statement`/`user_correction`, while
        `MemorySourceKind::permitted_trust()` returns `Authoritative` for those **and** `provider_record`. Trust
        asks "may this content instruct"; identity asks "who established this". Added
        `MemorySourceKind::is_user_stated()`, and **falsified the mistake**: substituting the trust predicate
        into the guard fails the route test with `503` where `422` is required — the caller's error surfacing as
        "the database is unavailable", sending an operator to debug the wrong thing.
      - **Five rules falsified**, each in the binary a developer runs: the verification conjunction (→
        disjunction); the listing's `status = 'active'` filter (→ `<> 'deleted'`, which re-listed a merged
        entity); the alias-source guard substituted with the trust predicate; the same guard disabled entirely
        (**the complement direction** — a guard that never fires must also fail a test); and
        `is_verified_alias`'s probabilistic arm (→ `true`, caught by two tests).
      - **⭐⭐ A mutation experiment corrupted three unrelated queries, and only a row count caught it.** The
        restore step used a **global** string replace, so `status <> 'deleted'` → `status = 'active'` was applied
        to `read_workspace_memories`, `read_entity_memories`, and `read_subject_relations` as well — silently
        narrowing each. `a_correction_supersedes_the_old_claim` failed because it **counted rows**; every other
        memory test passed, because they assert on `is_current_truth` and a narrowed filter drops exactly the
        rows those assertions ignore. The three queries were restored and an assertion was added beside the read:
        *the superseded claim must remain readable — an audit trail nobody can read is not one*. Re-applying the
        exact mutation then failed that assertion, so the detector is proven rather than assumed.
        **Generalise: a mutation restore must be scoped to its occurrence, and a read's filter needs a test that
        counts what it excludes.**
      - **⭐ `parse_summary_limit` became `parse_limit(raw, default)`** because there are now three listing bounds
        (200, 128, 50); three parsers would be three places for a bound to be wrong.
      - **Verified live** through the CLI against a real daemon (`database_schema=12`): `entity create "Ada
        Lovelace" --kind person --confidence confirmed` → an identifier; **`memory remember "Prefers morning
        meetings" --type preference --source user_statement --entity <id>` → `status active`**, which is the
        recorded gap closed; `entity alias … --verification confirmed --source user_statement` → attached;
        `entity lookup --alias-kind email` → `verified … matched alias`; `entity merge <other> --into <id>` →
        the winner, after which `entity list` no longer offers the loser; and
        `--verification confirmed --source provider_record` → `422` naming the remedy, exit `4` (`Denied`).
      - **Limits, recorded rather than glossed:** a label lookup is **exact** (case-insensitive, whole-label) and
        not a text search, because a fuzzy match would turn a guess into an identity — a caller wanting partial
        names has no surface yet; there is **no entity deletion**, so `merge` is the only way to retire one and an
        entity created in error stays listed until merged; `entity_relations` is still unreachable
        (`record_relation`, `read_subject_relations`), because a relation is a claim *between* entities and needs
        a surface of its own; and the CLI's argument parsing has one unit test per helper rather than an
        end-to-end one per verb.

## P5: Connectors

> **Condensed 2026-10-10.** The per-slice engineering notes that filled this section (about 3,060 lines, 45 "continued" entries under `P5-005`) are kept **word for word** in [docs/history/todo-p5-connectors-notes.md](docs/history/todo-p5-connectors-notes.md). Nothing was deleted: this section keeps the status and what is open.

**What exists.** `crates/jarvis-connectors` (64 source files, about 1.7 MB, tested offline against recorded wire fixtures) is a provider-neutral connector framework plus a Google implementation of it:

- **Contracts** (`P5-001`, `ADR-0054`): the connector manifest, account, auth flow, health, sync cursor, webhook, rate-limit, scope and diagnostics types. The manifest checks its own claims against each other (an outward connector cannot declare itself public, a secret is a field name and never a value, a research record is required).
- **OAuth 2.0 Authorization Code + PKCE** (`P5-002`, `ADR-0055`): one-use authorization transactions, state and issuer checks, loopback callback, refresh rotation, revocation, secret references. Research record `docs/research/integrations/oauth2-pkce-native-apps.md`.
- **Readiness checklist and scaffold** (`P5-003`, `ADR-0056`): a program rather than a document; `jarvis connector new|check|items`.
- **Google research** (`P5-004`): `docs/research/integrations/google.md` (live official sources, access dates, no live call).
- **Google framework** (`P5-005`, `ADR-0057` to `ADR-0118`, modules `google::{client, request, operations, definitions, scopes, token, credential, revocation, watch, channel, pubsub, routing, teardown, recovery, connection}`): request and error classification for Gmail and Calendar, scope categories, rate-limit units, Gmail `users.watch` and Calendar `events.watch` leases, push-delivery parsing and authentication, channel renewal and teardown, first-sync branching, and health with staleness. These are mostly **pure decisions with fixtures**; the notes list for each what it does not yet do.

**What the daemon actually runs.** Only the Google connector's declaration, auth endpoints and form encoding are used (`apps/jarvisd/src/google_account.rs`). The Google features the owner uses today (sign in from Settings, `jarvis.gmail.search|read|send|draft|save_attachment`, `jarvis.calendar.events|create`) were built **directly in the daemon** during `P9-048` to `P9-050`, `P9-071`, `P9-074` and `P9-081`, with a simple client, and were confirmed live by the owner. **The push and watch stack (`watch`, `channel`, `pubsub`, `routing`, `teardown`, `recovery`, `connection`) has no caller in the daemon: it is not live, and nothing in the product depends on it.**

- [x] `P5-001` Connector manifest, account, auth flow, health, sync cursor, webhook, rate-limit, scope and diagnostics contracts. `ADR-0054`.
- [x] `P5-002` Shared OAuth 2.0 Authorization Code + PKCE flow, state/nonce validation, loopback callback, refresh rotation, revocation, secret references. `ADR-0055`.
- [x] `P5-003` Connector quality checklist and scaffold generator (`jarvis connector`). `ADR-0056`.
- [x] `P5-004` Research Google identity, Gmail, Calendar, push notifications, quotas and restricted scopes.
- [x] `P5-005` Google connection setup and Gmail/Calendar read tools. **Met by the daemon's direct implementation** (`P9-048`, `P9-049`, `P9-050`, confirmed live), not by the framework above. The framework half (contracts, fixtures, classification) is built and tested; the claim "recorded wire fixtures" is true for the crate and **not** for the daemon's own client, which is tested against a local stand-in server.
- [ ] `P5-005b` **Decide the fate of the unused push and watch framework**: either compose it (a sync loop, a `watch` renewal task, a push endpoint, `jarvis.gmail.*` reading from it) because event-driven mail is wanted (`P6`), or retire it so the product does not carry about 1 MB of code nothing runs. Needs a measured requirement first (polling the mailbox from a schedule is what works today). Until decided, do not extend it.
- [ ] `P5-006` Research Microsoft identity platform and Microsoft Graph mail/calendar, subscriptions, delta queries and limits; record findings.
- [ ] `P5-007` Implement Microsoft connection setup and Outlook/Calendar read tools with recorded wire fixtures.
- [ ] `P5-008` Research and implement GitHub authentication and read tools.
- [ ] `P5-009` Draft/write operations behind policy and approval with provider idempotency where available. (Done for Gmail send/draft and Calendar create in the daemon, `P9-050`, `P9-081`; open for Microsoft and GitHub.)
- [ ] `P5-010` Reauth, token expiry, revoked scope, pagination, throttling, webhook replay and redacted diagnostics tests. (Gmail/Calendar: sign-in expiry and revocation are handled by the daemon's account module; the rest is open.)

## P6: Events And Workflows

- [x] `P6-011` **A first useful slice of proactivity: scheduled tasks** (added; does not close the Phase 6 gate).
  `jarvis schedule add "<objective>" --every 6h | --at <UTC instant>`, `list|pause|resume|remove`, and `jarvis runs` to
  read what a task said. A schedule is a request to start an **ordinary** run, so it has no authority an interactive run
  lacks: an unattended task that wants a held tool parks for a person. At most once (guarded claim before start), no
  backlog replay (next fire from now), no pile-up (a fire is skipped and counted while the previous run is still going),
  one session per recurring task. [ADR-0132](docs/adr/0132-a-schedule-is-a-request-to-start-an-ordinary-run.md); migration
  `0014`, schema version 14; `jarvis-core::schedule`, `jarvis-storage::schedule_repository`, `apps/jarvisd/src/schedule_service.rs`.
  - **Live:** two tasks every minute against a real model; the arithmetic task ran each time, the fetch task parked once,
    was skipped once, and left exactly one approval pending.
  - Tests: 6 domain, 10 storage (including claim-by-exactly-one-pass, backlog skip, restart), 8 daemon (real routes + real
    scheduler pass), 6 CLI.
  - **Limits (each is its own `P6` item, none is approximated):** no wall-clock cadence ("daily at 08:00") until time
    zones and DST are done properly (`P6-003`); no push notification when a task needs you (`P6-007`); no event-triggered
    runs (`P6-001`/`P6-002`); a fire lost to a crash between claim and start is not retried (at most once, by design);
    SQLite only.

- [ ] `P6-001` Define event envelope, source identity, schema version, causation/correlation IDs, dedupe key, visibility, and sensitivity.
- [ ] `P6-002` Implement transactional outbox/inbox, leasing, retry schedule, dead letters, and replay tooling.
- [ ] `P6-003` Implement timezone-aware schedules and deterministic next-run calculation across daylight-saving transitions.
- [ ] `P6-004` Define workflow, run, step, attempt, wait, approval, cancellation, and compensation records.
- [ ] `P6-005` Implement sequential and parallel steps, conditions, retries, timeouts, delays, event waits, and cancellation.
- [ ] `P6-006` Require idempotency or an explicit non-retryable classification for every effectful workflow step.
- [ ] `P6-007` Implement notifications and proactive suggestion budgets, quiet hours, dedupe, and user controls.
      > Status 2026-10-06: a pull view exists (`jarvis watch`, the console's waiting panel, the page title count); no push notification, quiet hours or budgets yet.
- [ ] `P6-008` Add crash-at-every-transition and duplicate-event tests; pass the Phase 6 gate.
- [ ] `P6-009` Define the skill-candidate and memory-nudge events in the envelope **before `P6-001` freezes
      it**: a complex task that completed, a repeated procedure worth proposing, and a nudge to persist
      knowledge. Each is an ordinary event with causation, correlation, and a dedupe key, and **none carries
      authority** — a candidate is a proposal (`ADR-0117` §4), decided by a person and not by the event.
- [ ] `P6-010` Implement delegation as a durable workflow step: a step whose effect is starting a child run,
      with the parent/child link recorded, the child's tool calls re-entering the same policy gateway
      (`P7-003`), and parallel workstreams tested for single-effect on duplicate delivery and for
      cancellation. The effect class "starts a run" is named explicitly rather than folded into an existing
      one.
      > Status 2026-10-06: the starts-a-child-run effect exists as `jarvis.agent.delegate` (`P3-034`, ADR-0134) but not yet as a durable workflow step; a parent/child link is recorded only by the objective marker, and a crash mid-delegation is not resumed.

## P7: External Runtimes

- [ ] `P7-001` Define runtime protocol v1: handshake, capabilities, start, event stream, tool request, input request, resume, cancel, settle, health, and errors.
- [ ] `P7-002` Implement process supervisor with resource limits, environment allowlist, crash backoff, health, log redaction, and orphan cleanup.
- [ ] `P7-003` Ensure external runtime tool requests re-enter the JARVIS policy gateway; disable unmediated native tools by default.
- [ ] `P7-004` Research and implement the current OpenClaw runtime or ACP integration.
- [ ] `P7-005` Research and implement an OpenAI Agents worker adapter.
- [ ] `P7-006` Implement a generic ACP adapter and conformance fixture.
- [ ] `P7-007` Add LangGraph only for a named use case that benefits from its checkpoint/interrupt model.
- [ ] `P7-008` Test crash isolation, cancellation, protocol skew, unavailable runtime fallback, and canonical audit ownership.
- [ ] `P7-009` Add delegate/subagent semantics to runtime protocol v1 **before it is frozen**: a start request
      that names a parent run, the child run's lifecycle and settlement, cancellation that propagates, and a
      bounded depth. Whether `agent_runs` gains a `parent_run_id` is decided here and recorded before the
      protocol ships, because the run table already exists and a child run's parentage cannot be added
      cheaply afterwards.
- [ ] `P7-010` Add a subagent delegation tool: an ordinary tool whose effect is "starts a run", classified
      with its own risk level and approval policy, so a delegated run is authorized like any other capability
      and a model cannot spawn work the actor was not granted.
      > Status 2026-10-06: delivered in a first form by `P3-034` (ADR-0134): `jarvis.agent.delegate`/`result`, depth limit one, at most four active, sub-agent answers fenced. Not yet classified with its own effect class, and not routed to external runtimes.
- [ ] `P7-011` Test delegation: depth limiting, crash isolation between parent and child, cancellation
      propagation, single-effect on duplicate delivery, and canonical audit ownership of a child run's steps.

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
      > Status 2026-10-06: partly delivered ahead of order by `P3-036`: browser-native push-to-talk, wake word, spoken answers and interruption in the console. Still open: VAD, local (non-cloud) STT/TTS provider ports, echo handling, and a non-browser client.
- [ ] `P8-011` Pass inbound, outbound, voicemail, no-answer, interruption, latency, replay, privacy, and provider-outage tests.
- [ ] `P8-012` Implement the turn-detection and interruption port: canonical turn events with confidence, backchannel suppression, false-interruption resume, and a silence setting used only as a maximum cap.
- [ ] `P8-013` Implement a `flux`-class turn-detecting STT adapter and assert that a pause-heavy request is not cut off at the silence threshold.
- [ ] `P8-014` Report turn-detection latency separately from perceived response latency, and record discarded speculative work.
- [ ] `P8-015` Implement the Speech Engine brain WebSocket transport (shared-secret upgrade, per-call signed URL, `ulaw_8000`) behind the same application command as the SSE path.
- [ ] `P8-016` Place a boundary and placement ADR for any external agent framework used in the voice path (runtime worker under the supervisor versus a `jarvis-voice` adapter), before implementing it.
- [ ] `P8-017` Decide and record realtime session scope before any realtime transport work: whether JARVIS adopts a room as a session concept, which participants and media types a session may carry, whether data/RPC channels are permitted as transport-only, and the E2EE-versus-inspection policy. Record explicit exclusions where the answer is "no".

## P9: Desktop, Installation, And Releases

- [ ] `P9-001` Scaffold Tauri desktop as a daemon client with generated API bindings and a narrowly scoped command allowlist.
      > Status 2026-10-06: an interim browser console ships as the daemon's main page (`P3-036`, ADR-0135): it can talk, watch, answer approvals and stop. The Tauri client remains the planned native client.
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
- [ ] `P9-010` Add messaging surfaces as authenticated gateway clients, one slice per platform, each binding an
      inbound sender to an actor, user, and workspace **without trusting caller-supplied identity**
      (`FR-ID-001`, `ADR-0038`: a network request is never a local caller). A platform that is not configured
      is an absent capability rather than a degraded one, so a delivery for an unconfigured surface is refused
      and named.
- [ ] `P9-011` Add an answer-quality evaluation harness: labeled prompts with expected properties, run against
      a configured provider, asserting answer quality and regression over time. Distinct from the acceptance
      gates, which assert correctness of behavior rather than the quality of an answer.

### P9-012 to P9-019: Onboarding, settings and distribution (spec: `docs/product/onboarding.md`)

- [x] Review of the first-run path, the key and settings story, the lifecycle commands and the release path (this entry).
- [x] `jarvis` with no arguments (or `jarvis launch`, or flags alone) sets up on the first run when a terminal is available, then
  starts the daemon and opens the console; without a terminal it explains itself. `[profile.release]` added (thin LTO, one
  codegen unit, stripped). `docs/user/quick-start.md` written; README and the developer guide link to it.
- [x] `P9-012` `POST /api/v1/shutdown` (authenticated, graceful, wakes the same shutdown path a signal does) and `jarvis stop` /
  `jarvis restart` that wait for the port to free. Also fixed: on Windows `jarvis start` made the daemon inherit the pipe a script
  captures the output through, so `jarvis start | anything` never returned; the daemon is now started through `Start-Process`
  with its process id tracked, and a daemon that dies at startup is reported in about a second. Live: stop 0.75 s, restart from
  stopped and while running through a capturing pipe, a bad configuration reported in 1 s. Tests: 401 without the credential,
  202 with it, the waiter wakes, stopping what is not running is not an error.
- [ ] `P9-013` *(Partly met in the console by `P9-085`: provider presets, the provider's own model list, a real test call before saving. Still open: the terminal `jarvis init`, a non-echoing key prompt, an optional voice step.)* Guided brain and key setup: provider presets, a non-echoing key prompt (dependency decided in an ADR), key to a private
  file, a real test call before saving, optional voice step; the same flow on first launch and in `jarvis init`.
- [x] `P9-014` `jarvis config show|get|set|unset` and `jarvis keys status|set|remove|test`: one setting at a time, secrets never
  printed or taken as arguments (environment variable, file or standard input), every change validated by the daemon's own
  parser and refused without touching the file, `--json` on the reads. No new third-party dependency (the workspace's `toml`).
  Tests: one change keeps the rest (folders, key file), invalid changes leave the file byte-identical, half a pair is refused,
  a key lands in a private file and never in the configuration, malformed keys are refused without echoing them. Live: set a
  model, kept the folder, refused a voice id without a key, `keys test model` against a real Ollama, `keys set voice --file`.
  Limits: the file is rewritten from its parsed form, so hand-written comments are lost; the paste prompt echoes (hidden
  input needs a dependency decision, `P9-013`); `keys test voice` needs a running daemon and a real key.
- [x] `P9-015` Settings screen in the console (`ADR-0139`): keys (password fields, write-only, status only), every setting with the
  daemon's own refusal reason, **Restart to apply** (`POST /api/v1/restart`). The settings logic moved into
  `jarvis_storage::settings` and the CLI is now a thin caller, so there is one validation path. Tests: storage (one change keeps
  the rest, invalid changes leave the file identical, keys never in the config or any listing), routes (401 on every one, key
  write-only and never echoed, invalid vs valid). Live: set a voice key through the password field (cleared after sending), saw
  "a port is a number from 1 to 65535" for a bad port, saved a voice id, pressed Restart: new process, page reconnected by itself,
  `GET /api/v1/speech` reported the voice enabled. Limits: no "test key" button yet (use `jarvis keys test`); the tool ask/trust
  list is edited as text (`policy.trust`), not per-tool toggles; the file loses hand-written comments on the first change.
- [x] `P9-015b` Settings in tabs (Brain, Voice, Folders & code, Permissions, Advanced). Every setting now carries its tab, its effective default and a
  plain "what unset means" (the review found no setting is "not built yet": each unset one is opt-in by design or needs a key,
  Docker or a folder; the screen simply did not say so and showed an unwritten default as "(not set)"). Related settings apply
  together (`PUT /api/v1/settings` is all-or-nothing, so the code image and its command are one save), and a Permissions tab sets
  one posture per tool (default / ask / trusted / off) over `PUT /api/v1/settings/tools/{tool}`, checked against the real tool
  registry. Tests: all-or-nothing batch, one posture per tool with the file left tidy, every setting has a tab and an
  explanation, 401 on the new routes. Live: tabs rendered, a tool made trusted, the sandbox pair saved in one click, a half
  pair refused. Reference doc: `docs/user/settings.md`.
- [ ] `P9-020` An editor for `mcp-servers.toml` (add, remove and test an MCP server) in the Settings screen and as `jarvis mcp`; today it is
  hand-edited.
- [ ] `P9-021` "Test" buttons beside each key in the console (model and voice), reusing `jarvis keys test`. *(Model and fallback: done by `P9-085`, a Test beside each model. Open: voice and search keys.)*
- [x] `P9-016` `jarvis doctor --live` (the existing offline `doctor` and its acceptance output are unchanged): asks the model server to list its models
  with the saved key (rejected key, unreachable server, model not listed), checks the granted folders exist, Docker and the pulled
  image when the code tool is on, whether the daemon is running and has the voice a saved key implies; every non-OK finding prints
  the command that fixes it, `--json` gives the same, and a failure exits non-zero. Also: `jarvis config set code_sandbox IMAGE
  COMMAND...` / `unset code_sandbox` set or remove the pair together (one at a time the daemon's parser refuses half a pair), and
  `config get` prints a command as `node -e`. Live: found Ollama unreachable (it was updating), a deleted folder, and Docker not
  running, each with its fix. Tests: both model-list shapes read exactly, every failing status carries a fix. Not done: a hidden key
  prompt and provider presets (`P9-013`).
- [x] `P9-017a` Release archives: `scripts/package-release.sh` builds `jarvis-<version>-<platform>.zip|tar.gz` (both programs, a README, the
  user docs, `THIRD_PARTY.md`) with a SHA-256 file, and `.github/workflows/release.yml` builds and smoke-tests it on Linux x86_64 and
  aarch64, macOS Apple silicon and Intel (cross-built) and Windows, then uploads workflow artifacts, and on a `v*` tag also attaches the
  archives to a **draft** release (visible only to people with write access). No published release is
  created: no licence has been chosen (README, "License Status"), which blocks distribution. Live: the Windows archive was built here,
  its checksum verified, and the unpacked `jarvis` and `jarvisd` run from an empty folder (about 35 MB). The workflow YAML parses;
  it has not run on GitHub yet. `THIRD_PARTY.md` now records the one bundled asset (the face mesh, Apache-2.0).
- [ ] `P9-017b` Publishing: choose a licence, generate the dependency-licence list for the archive, sign and notarize (macOS, Windows), publish
  a release, and add the one-line installers (`P9-004`). Also: `jarvis` should find `jarvisd` on `PATH` when it is not beside it.
- [x] `P9-018` `jarvis service install|uninstall` install and remove the per-user service: a systemd user unit (plus `loginctl
  enable-linger` for servers), a launchd agent, a Windows Run entry; the command sequences are pure data (`ServicePlan::install_steps`)
  and refuse to touch a definition JARVIS did not write. Also fixed: the launchd plist put its comment before the XML declaration
  (invalid, `launchctl` would reject it), and systemd paths with spaces are quoted. Live on Windows (install, registry entry,
  uninstall, nothing left behind); **Linux and macOS paths are unit-tested as data and type-check, but were not run on a real
  machine** (`P9-022`).
- [x] `P9-018b` Server readiness: a Unix daemon is started in its own process group so closing an SSH session does not stop it; `jarvis
  hud --print-url` prints the credentialed address for an SSH tunnel, and a machine with no display is told how to tunnel in instead
  of failing; `docs/user/platforms.md` states what is verified per platform. CI was red on all three platforms: three CLI init
  tests used `C:/…` paths (relative on Unix, so the daemon's own parser refused them), and one Windows gateway test had a 5 s ceiling
  on a background task on a slow runner; both fixed.
- [ ] `P9-022` First real run on a clean Linux VPS and a Mac (and an aarch64 Linux): install, init, start, tunnel to the console, run a
  conversation, `service install`, reboot; record what breaks. Prerequisite for calling either platform supported.
  CI-found so far, fixed: a Unix socket path over 103 bytes is now refused by name (`EndpointError::SocketPathTooLong`); sandbox tests no
  longer assume a delegated cgroup; the kill test no longer reads `/proc` (absent on macOS); tests no longer use `C:/` paths on Unix; the phase 3
  gate no longer looks for its freshness witnesses with backslash paths (it reported a built `jarvisd` as missing on Linux and macOS); the
  portable-mode test uses a short directory so its socket fits.
- [x] `P9-023` Remembering works in conversation (ADR-0140). Found in a real chat: "remember to reply with short sentences" failed
  because the model had to supply an entity identifier it cannot know. A claim with no named subject is now about the profile's owner
  (one confirmed `You` entity, created on first use); a named entity must still exist. A model's proposal stays a `Proposed`
  `model_inference` and is never retrieved, so the console shows proposals in "Waiting for you" as "remember?" cards: Keep files the
  text as the person's own statement (which retrieval reads), Dismiss forgets it. Live: asked to remember, the card appeared, Keep
  made it `active user_statement`, and a later answer in a new run followed it. Tests: two claims share one owner; an invented id is
  still refused with a 422; a proposal with no subject is still `Proposed`; an explicit empty `entity_ids` is still refused.
  Feedback: an answer you give now lands in the conversation itself ("Approved jarvis.web.fetch. Carrying on.", "Remembered: ...",
  "Denied ...", or a failure), and the "waiting for your approval" chip turns to "approved" or "denied"; before, only a hint under the
  box changed. Checked live with a held `jarvis.web.fetch` and with Keep.
  Not done: keeping a proposal by voice ("Jarvis, keep that") or in one terminal step.
- [x] `P9-025` The console is a face with a conversation (ADR-0141). The face fills the page and is no longer steered by the pointer: it
  has a mind of its own (expressions that change by themselves, saccades with the head following, irregular and double blinks, head
  wander, breathing, sighs, dozing off), reacts to listening, working, speaking (mood of the answer, beats of the voice), waiting and
  offline, and to events (nod and smile on approve, shake on deny or stop, concern on failure). The head rig gained eye lids, a smile
  and frown, and independent inner/outer brows. The chat is a panel on the right; the dashboard moved to an Ops page (`O`); approvals
  and "remember?" cards sit in a tray at the bottom left of the face page. Checked live in the browser (idle, amused, concerned, the
  approval tray with Approve and Deny). New chat (button, `N`, or "new chat" aloud) starts a conversation with no old context; History
  reopens earlier ones with their daemon session; conversations persist in the browser across reloads. Live: a codeword told in one chat
  was unknown in a new chat and remembered when the first was reopened. Not done: conversations are not stored on the daemon, so they
  do not follow you to another browser; a conversation cannot be renamed. Not done: the face has no ears for the room (it does not turn toward a speaker), and the
  expressions are tuned by eye on one mesh, not tested.
- [x] `P9-026` `jarvis path install|uninstall|status`: `jarvis hud` said "command not found" because nothing puts the folder on the PATH.
  Windows: adds the folder to the user `Path` through PowerShell (the directory is quoted so it cannot leave the string; only the
  user value is read or written; idempotent, tested). Linux and macOS: links both programs into `~/.local/bin` and says if that is not
  on the PATH, never replacing a file that is not its own link. `jarvis start` prints a tip when the folder is not on the PATH.
  Live on Windows: installed, second run reported "already", the user PATH holds the folder once. Not run on Linux or macOS (type-checked
  for both). Not done: installers (`P9-017b`) should do this for you.
- [x] `P9-027` No small limits, and no silent hangs (ADR-0142). A "create a Next.js app" request failed at its sixteenth tool call with "the
  run failed". Raised: model calls 8 to 400 and tool calls 16 to 1,200 as ceilings; a repeat guard replaces them as the loop stop; file
  write 4,000 to 60,000 characters (and the canonical-intent cap 8,192 to 1,048,576, without which no call over 8 KB worked; an edit
  stays at 2,000 and 4,000 because a held call must fit the 8,192-byte approval), context window 8,192 to 64,000 tokens, history 12 to 40 turns, model
  request timeout 2 to 15 minutes, request body 16 KB to 256 KB. Visibility: reasoning and tool-call writing are counted
  (`StreamEvent::Progress`, never kept) and shown as "thinking (~11k tokens)", the progress line shows step, file and time, failures
  say why and offer Continue, the model is asked to narrate stages (spoken when the voice is on), and a minute of quiet is spoken
  about. A model that reasons over 40,000 characters without answering is cut off and re-asked with `reasoning_effort: low`, sticky for
  the run. Live: a multi-page site ran past 50 tool calls with the limit firing and recovering. Not done: a setting for
  `reasoning_effort` (it is only sent after an overrun); a per-run "allow edits" (each edit asks, so a big coding task is many
  clicks; Settings, Permissions, can set Edit to run without asking); messages over 4,096 characters (a database check, needs a
  migration).
- [x] `P9-028` A stray `JARVIS_` environment variable no longer stops JARVIS starting. Found live: a user-level `JARVIS_MODEL_KEY` and
  `JARVIS_VOICE_KEY` (set for a different program) made every `jarvis` command fail with "no usable configuration (unknown configuration
  environment variable: JARVIS_MODEL_KEY)", the daemon stay down and the console read "offline". Unknown `JARVIS_*` variables are now
  ignored (the prefix is not ours alone) and named, never with values, by `jarvis doctor --live` and the daemon log; a variable JARVIS
  does define with a bad value is still an error. Tests: an unknown variable is ignored, only names are reported, a bad value for a known
  one still fails. **Second half, found by the phase 1 gate on this machine two days later:** a fresh install with *no config file* still
  took the strict path (`load_with_environment` applied the environment directly), so the stray variable still stopped a first run; fixed
  with a test for the missing-file case. Also learned: the gates use a prebuilt `jarvisd`, so a gate run before `cargo build --workspace`
  tests the old daemon.
- [x] `P9-029` CI after the limits push (run 131): Windows and Ubuntu green. Release workflow (run 7) built all five archives but every smoke test
  failed: `ls dist/*.zip` exits 2 when nothing matches and `pipefail` turned that into a failed step (now `find`); `package-release.sh` used
  `Compress-Archive` without 7z, which writes backslash paths (now Windows' bsdtar); verified locally by packaging, unzipping and running both
  programs. macOS failed the Phase 3 gate with no readable reason (job logs need a login), so `scripts/ci-run-gate.sh` now copies the panic
  message into `::error::` annotations for the three process-level gates. Still open: the macOS Phase 3 failure itself, until the next run
  shows its message.
- [x] `P9-030` A request with a line break no longer breaks a run, and a run that cannot continue is no longer left open. Found live while
  asking JARVIS to fix a build error pasted into a request: the newline made the context's one-line source reference invalid, the run failed
  before any model call, and nothing settled it, so it sat in `context_building` for good while the console said "working". The reference
  now collapses whitespace (the objective itself is untouched), and `fail_if_unfinished` settles any run the executor gives up on as
  `executor_error` with the reason, for runs and sub-agents. Tests: a multi-line objective is answered and the model receives it intact; an
  abandoned run is failed, a finished one is not touched. **Live end to end:** the same limits work as intended: "create a Next.js app" now
  completes in about 2.5 minutes (it failed at 16 calls before), JARVIS wrote 12 files in Dutch, the site failed `next build`
  (`styled-jsx` in a server component), a pasted error got three approved edits, and the rebuilt site builds (8 static pages).
  **What this shows is missing:** JARVIS cannot run a build or a test, so it cannot catch its own mistake. That is the next thing to build
  (`P9-024`: a command-running tool, which needs a decision on where it may run, since `jarvis.code.run` needs Docker).
- [x] `P9-031` JARVIS can run a command in a granted folder (ADR-0143): `jarvis.command.run` (`npm run build`, `cargo test`), no shell, bare
  program name on the PATH, directory confined to a granted root, allowlisted environment, whole-process-tree kill on timeout or drop, head and
  tail of the output fenced as data, asks every time unless trusted. It is host execution, not a sandbox, and the ADR says so. 15 tests run real
  commands (success, failure, timeout kill, escape attempts, secrets not inherited).
- [x] `P9-032` A run that waits for an approval remembers its turn (ADR-0143). Found live with the command tool: after each approval the run
  resumed with no memory of its steps, approved `npm run build` about fourteen times and ended empty; and an identical approved call was refused as
  a repeat. Migration 0015 makes the repeat guard apply only to pending/denied approvals; migration 0016 stores the run's own turns while it
  waits and puts them back on resume, with "not run" results for calls after the held one. Tests: a three-call turn resumes with a result for
  every call, trimming keeps whole turns, a storage round trip. **Live:** with an injected type error the run built, fixed and rebuilt, and answered.
- [x] `P9-033` One executable (ADR-0144). `jarvis daemon` runs the daemon (`jarvisd` is now a library plus a thin wrapper binary); `jarvis start`
  and the login service launch the `jarvis` file itself, and release archives, `jarvis path install` and the smoke test carry just `jarvis`.
  Tests: every service definition launches `<exe> daemon`; the PATH links are one file unless an old `jarvisd` is beside it. Live: see below.
- [x] `P9-034` The daemon is ready only when it listens. CI on macOS failed intermittently (phase 3 in run 132, phase 2 in run 133) with "error sending
  request" to a freshly started daemon: the gates wait for the lifecycle row `ready`, and the daemon wrote it (and set the health state, and
  logged "daemon ready") **before** it bound the HTTP port, so a client that trusted it could connect to nothing. `declare_ready` now runs after every
  listener is bound. This is a real fault, not a test one: `jarvis status` and a service manager read the same state. All 56 test binaries pass on
  Windows; macOS is confirmed only by the next CI run.
- [x] `P9-035` JARVIS knows the date and time. A model has no clock, so "what day is it" or "remind me tomorrow at nine" was a guess. Every run's
  system message now ends with the current weekday, date and local time with its UTC offset, plus the UTC instant (`clock.rs`). The offset is read
  once at startup, before the async runtime has threads (the `time` crate only does it soundly then), so a daemon that stays up across a daylight-saving
  change keeps the old offset until restarted; the UTC time is always right. Tests: the date follows the offset across midnight, negative and unknown
  offsets are stated honestly, a first turn carries the clock. **Live:** "what day and time is it" answered correctly (Friday 9 October 2026, 18:04, UTC+02:00).
- [x] `P9-036` A tool's permission changes at once, and the approval card says "Always allow" (ADR-0145). Saving a tool's posture used to need a restart,
  so "stop asking me for `npm run build`" meant leaving the conversation. The pipeline now holds the policy as a replaceable snapshot, the settings
  handler rebuilds it from the saved file with the daemon's own function and swaps it (`applied: true`); a file it cannot compose leaves the running
  policy untouched. Test: on one running pipeline a call asks, then (trusted) runs, then (off) is refused, each on the very next call. **Live** through
  the API: trusted applied at once, off refused the next call, default restored.
- [x] `P9-037` Scheduling from conversation (ADR-0145): `jarvis.schedule.add|list|remove` over the existing scheduler and its validation. Adding and removing
  ask (a schedule persists beyond the chat); listing is free; objectives read back are fenced. Tests on a real database: add/list/remove, and six bad inputs
  (both cadences, neither, past time, unparseable time, too-short interval, blank objective) each refused and creating nothing. **Live:** "remind me in
  2 hours" asked, was approved, scheduled for the right UTC time (from the clock, `P9-035`), and was listed back.
- [x] `P9-038` Scheduled work reports back in the console. A reminder or recurring check was only readable afterwards in `jarvis runs`. The console now watches
  the schedules it already polls: when one fires and that run finishes, the answer is put in the conversation ("Scheduled: ..." then the answer) and, with
  spoken answers on, said aloud; anything already fired when the page opened is not announced again. **Live** (browser): a one-off task scheduled 75 seconds
  ahead appeared in the open console when it ran. (The first attempt polled a wrong URL and showed nothing; the page's own error log showed the 405, which is
  why this was verified in a real browser.) Not done: a notification when no console is open (needs the desktop shell, `P9-019`).
- [x] `P9-039` A coding task now runs end to end, and its narration reads as paragraphs. Dogfood: "build a dependency-free Node todo CLI with `node:test` tests, run
  them and fix failures" completed in about two minutes with 10 passing tests and no help (the command tool was trusted, `P9-031`, `P9-036`). It showed one
  defect: the narrated stages ran together ("...run the test suite.App and tests are written") because each model call's text was streamed with nothing between
  them. A call after one that already spoke now starts its streamed text with a paragraph break (the settled answer is unchanged, and a resumed run counts as
  having spoken). Test: the stream of a tool turn plus an answer is `first.\n\nsecond`. **Live:** the same kind of task now prints each stage on its own line.
- [x] `P9-040` A default reasoning effort (ADR-0146): `daemon.executor_reasoning_effort` none/low/medium/high, applied in the model adapter when a request does not choose
  its own (the loop's drop to `low` after an overrun still wins); refused without a live model or with an unknown word. Tests: the four words and both refusals;
  the adapter sends the default, and a request's own choice wins. **Live:** `jarvis config set` accepted `low`, refused `maximum`; a question answered in 1.5 s with it set.
- [x] `P9-041` Desktop notifications for scheduled results (ADR-0146): on unless `notifications = off`; shown only when no console polled in the last 15 s; text passed
  apart from the script on every platform (tested with hostile text), Windows script single-quote-only. **Live:** a scheduled task with no console open logged and
  showed a notification; the exact toast script was run by hand and shown. macOS/Linux: command construction only.
- [x] `P9-042` Web search (ADR-0146), **verified live with the owner's key (2026-10-09)**: `jarvis.web.search` over Ollama's documented endpoint, opt-in by `jarvis keys set search`;
  research recorded in `docs/research/integrations/ollama-web-search.md` (the local Ollama server does not serve the endpoint, 404). Tests: documented request and fenced results,
  401/429/500/unreachable, malformed reply, long results, bad arguments, key never in `Debug`, composition skips a missing or malformed key. **Live:** with an invalid key the real
  service refused it and the assistant said so and fell back to a page fetch. **Positive path (owner's key):** `jarvis keys test search` accepted; a run for the current Node.js LTS used the tool and answered; the real reply is `results[{title,url,content}]`, with very long `content` (40 KB seen) that the adapter clips.
- [x] `P9-043` `jarvis install` / `jarvis uninstall` (ADR-0147): one command puts the single executable in a per-user folder, on the PATH, optionally starting at login; atomic replace;
  relative `--dir` refused; the profile is never touched. Tests: the folder choice, replace-and-no-leftovers (and executable bit on Unix), a missing source, a relative dir. **Live:**
  install into a scratch folder, replace, "already installed", uninstall. `--service` and macOS/Linux are not run here.
- [x] `P9-044` `jarvis.memory.search` (ADR-0147): the model looks up what is remembered, by words that must all appear; same eligibility gate and fencing as a context; reports how much
  it looked at. Tests on a real database (match rules, ranking, fencing and introduction, the count when nothing matches, argument limits). **Live:** after a fact was stored, "look up my cat" found it.
- [x] `P9-045` The search key can be set and removed in the console (Settings, Brain), not only with `jarvis keys`. **Live** in a browser: the row shows, with its help text.
- [x] `P9-046` `jarvis.files.search` (ADR-0148): find lines containing some text across a granted folder (literal, case-insensitive, optional file-name glob), through the same confinement as read and list,
  bounded in files/bytes/time/output with `truncated` set when it stops. Tests: nested results in stable order, glob and sub-folder, skipped folders/binary/large files and the match limit,
  **a link out of the root is not followed** (junction on Windows), bad arguments, glob rules. **Live:** found the right files and lines in a project with a decoy in `node_modules`.
- [x] `P9-047` Search then fetch works end to end with the owner's key (live): a research question used `jarvis.web.search` then `jarvis.web.fetch` and summarised the real latest Rust release with its source.- [x] `P9-048` Sign in with Google from Settings (ADR-0149): `GoogleAccount` (PKCE via the connector crate, code exchange, refresh, revoke, private token file), routes `/api/v1/google*` and the one public
  callback, and a Settings, Google tab (client id, client secret, Sign in, Disconnect, status). Tests: the authorization request (S256, state, redirect, exactly the two read scopes), the whole sign-in
  against a local Google stand-in (the verifier proves the challenge, the secret is sent, only the refresh token is stored), **forged, repeated, cancelled, wrong-path and expired answers store nothing**, a partial
  grant is refused and revoked, a revoked sign-in is removed, refresh, disconnect; at the gateway the callback is public but inert while the rest is not. **Live:** the tab renders; Google parsed the real authorization URL and
  refused only the fake client id. **Not verified:** a real sign-in (needs your own OAuth client).
- [x] `P9-049` `jarvis.gmail.search`, `jarvis.gmail.read`, `jarvis.calendar.events` (ADR-0149): read-only, held for the owner by default, fenced as untrusted, fixed-word failures (401/403/429/5xx, not signed in). Tests with a Google stand-in:
  fenced search results with an injection-shaped snippet, base64url plain-text decoding from a multipart message, calendar ordering and bad windows, failures never echo the provider. `docs/user/google.md` is the setup guide.
- [x] `P9-050` Sending mail and creating events (ADR-0150), opt-in: `daemon.google_actions` adds `gmail.send` + `calendar.events` to the sign-in; `jarvis.gmail.send` (external communication, risk 3, always asks, one validated recipient,
  no header injection) and `jarvis.calendar.create` (asks, no invitees). Tests with a Google stand-in: the exact MIME message, eight injection/recipient shapes refused with nothing sent, no send without the granted scope, event times to UTC
  and no attendees, the four-scope request and refusal of a partial grant, base64 both alphabets. **Not verified live:** a real send (turn actions on, sign in again; test to your own address only).
- [x] `P9-051` A stop or restart no longer silently interrupts working tasks (ADR-0150). Found when a restart killed five of the owner's sub-agent runs: `/shutdown` and `/restart` answer 409 with the count unless `?force=true`;
  `jarvis stop|restart` refuses and leaves the daemon running, `--wait` waits for the tasks, `--force` interrupts; the console offers "Restart anyway". Tests: counted, refused, forced, parked runs not counted, `force=true` parsing.
  Not run against the live daemon (it had work in flight).
- [x] `P9-052` Projects (ADR-0151): the owner writes a goal, standing guidance, a working folder and a status once, and every run of the project is told it, with a journal the model keeps. Storage
  (`0017_projects.sql`, `project_repository`: case-insensitive unique names, relative-folder validation, notes, session/schedule links, cascade delete), routes (`/api/v1/projects`, `project_id` on a
  run start and a schedule), the scheduler (a paused or done project's task is skipped and counted), `ProjectContext` in the executor (brief as a policy system message, journal fenced as data,
  sub-agents get the brief only). Tests: storage CRUD/validation/links, routes (auth, bad input, a refused start leaves no run behind, no authority fields), scheduler pause, an injection-shaped note staying out
  of the policy message.
- [x] `P9-053` `jarvis.project.note` (ADR-0151): the model's journal tool. No project argument: the project comes from the call, its run and the run's conversation, so a model cannot write elsewhere; cannot claim the
  `owner` kind; no approval (one line in the owner's own journal). A task scheduled from inside a project, and a sub-agent started from inside one, inherit it. Tests: written to the calling conversation's
  project, refused outside a project and for an unknown call, owner kind/empty/over-long refused, schedule inheritance.
- [x] `P9-054` Projects in the CLI and console: `jarvis project add|list|show|set|pause|resume|done|note|remove`, `ask|chat --project`, `schedule add --project`; Ops-page Projects panel, project window with
  journal, and a project selector beside the conversation title. `docs/user/projects.md`. Verified live on a scratch profile (debug build, local Ollama `glm-5.3:cloud`): the console panel, project window (edit, status, journal entry) and selector, then a console message in the project.
- [x] `P9-055` Use projects on the owner's real prospecting work. *(Done 2026-10-10: the owner's daemon was migrated, the "Jarvis B2B Netherlands Sales" project exists with its brief, journal and schedules, and its runs are in use daily.)* Original note: Done on a scratch profile with a live model: the brief reached the model (it answered in Dutch, named the goal and folder), the model wrote
  a journal entry through `jarvis.project.note` without an approval, a second conversation continued from that entry, and a paused project's schedule was shown as paused. Still to do: restart the owner's daemon on this
  build when no run is in flight (their database migrates 16 to 17 with a backup), create the project from their existing sales brief, and attach the six-hourly schedule.
- [x] `P9-056` Live view of what JARVIS is doing (ADR-0152): tool requests name their target (page, query, task; no URL query or credentials), a finished call reports `ok` and the links it touched (safe `http(s)` only, at most
  eight), `jarvis_web::links_from_results`; `mission.js` (feed, sources, satellites/beams/effects per kind of work, the face docking right); the console follows scheduled and sub-agent runs too; finished answers list their sources;
  links in answers are clickable under the same rule. Tests: target/secret stripping, hostile links refused, result classification, executor emits both events, parser, page security tests. Verified live (scratch profile, real model:
  multi-search research, a sub-agent run started from the CLI and watched from the console).
- [x] `P9-057` Console UX pass: Ops cards show the live action, scheduled tasks can be paused/resumed/removed and show their project, recent answers expand, tools grouped by family (Ops and Permissions), plain setting labels,
  scrolling sheets with a fixed footer, dark scrollbars and focus outlines, a tidier chat header, starting points in an empty chat. Fixed a real failure found on the way: search replies over 512 KiB (ordinary) were refused; bound now 8 MiB.
- [x] `P9-058` An existing schedule can be filed under a project (or taken out): `POST /api/v1/schedules/{id}/project`, `jarvis schedule project ID NAME|--none`, a selector on the Ops card; `set_project_link`. The Ops page
  puts finished one-off tasks behind a toggle. The finished event is also emitted when a call that waited for an approval completes (the resume path). Tests: re-filing in storage and through the route.
- [x] `P9-059` JARVIS can manage projects, memory and schedules itself (ADR-0153). Found in a real conversation: asked to "create this as a project" it wrote a README because the journal tool refused outside a project.
  New tools: `jarvis.project.list|use|create|update|assign_schedule` (create/update/assign ask), `jarvis.memory.correct|forget` (ask; the call quotes the claim and the tool refuses when the quotation is not in the stored claim),
  `jarvis.schedule.pause` (no ask) and `resume` (asks). A run in no project is given an index of the existing ones; `jarvis.memory.search` prints `memory_id`. Tests for each, including the false-quotation and no-move cases.
- [x] `P9-060` Main page: the face alone (ADR-0154). Rings, ticks, name ring, brackets, canvas text and the wire skull and neck removed; the face is about half its former size with a faint glow, drifting motes and a thin voice line.
  The live view is a persistent constellation (nodes on orbits that settle outward instead of leaving after five seconds, moons for links, hover names a node, pressing opens its step), the feed keeps the whole run (40 steps, older ones
  compact), the panel folds to one line after 90 seconds. Chat suggestions no longer overflow. Verified live on a scratch profile with a real research run.
- [x] `P9-061` Security review of the projects, tools, links and observers surfaces (this session), and fix what it finds, each with a fail-closed test.
  Done (ADR-0155). The review found one HIGH: the approval card clipped arguments, so a long harmless `goal` could hide a hostile `guidance`. `NEVER_TRUSTED_TOOLS` (project create/update/assign_schedule, memory correct/forget) ignore `trust` and the settings route refuses it with 422; the card shows every field whole with no "Always allow"; a spoken yes does not answer them; `watch` says when it cut something. Tests in jarvis-tools and the gateway.
- [x] `P9-062` Resume interrupted runs: after a restart, a run that was working gets one continuation in its session (never a continuation of a continuation, never a cancelled run); `daemon.resume_interrupted` can turn it off.
  Done (ADR-0155). `continuable_interrupted_runs` picks runs that recovery just settled, started within 2 hours, newest in their conversation, not a continuation or sub-agent; `resume.rs` starts one ordinary run in the same session with a check-first notice (at most three). Setting `daemon.resume_interrupted` (on). Tested end to end including "never continued again".
- [x] `P9-063` The project journal keeps important old entries: the context shows the recent notes plus older decision, result and blocker entries, so a long project does not forget what it decided.
  Done (ADR-0155). `earlier_important_notes`; the brief shows up to 30 older decision/result/blocker/owner notes (1300 chars) before the recent ones.
- [x] `P9-064` Per-project daily run limit (`daily_run_limit`): the scheduler skips fires over it; API, CLI and console field; runs today shown.
  Done (ADR-0155). Migration 0018; scheduler skips over-cap fires (rolling 24 hours); API, CLI `--daily-limit`, console "Daily runs" field and `runs_today`; the model cannot set it. Tested including raising the cap.
- [x] `P9-065` Digest and push: `GET /api/v1/digest`, `jarvis digest`, an Ops card (runs, outcomes, tokens, answers); opt-in ntfy push for approvals and scheduled results (research record first).
  Done (ADR-0155). `GET /api/v1/digest[/hours]`, `jarvis digest`, Ops "Last 24 hours" card; ntfy push (`daemon.push_topic`/`push_server`, content-free, off by default, desktop plus push only when no console is open, new approvals are announced), `jarvis push test`, research record `docs/research/integrations/ntfy.md`. Live push to a real topic NOT run (it would publish to a third party): the request shape is tested against a local listener. Access tokens not built.
- [x] `P9-066` Contacts store: `jarvis.contacts.save|search|stats` tools, `jarvis contacts list|export`, so prospect data is not only a CSV file.
  Done (ADR-0155). Migration 0019; `jarvis.contacts.save|search|stats` (no approval, fenced read-back, a model cannot lift do_not_contact), `/api/v1/contacts`, `jarvis contacts list|stats|add|remove|export` (CSV with formula defusing). Not built: enforcement of do_not_contact inside the Gmail send tool, a console window for contacts.
- [x] `P9-067` Memory search falls back to ranking by matched words when no claim contains every word.
  Done (ADR-0155). `jarvis.memory.search` ranks by matched words and says "partial" when no claim has every word.
- [x] `P9-068` Console: follow runs waiting for an approval, show tokens and runs today, CI syntax check of the console scripts.
  Done (ADR-0155). Parked runs are followed as a held step (working first, three in all); digest card shows tokens; project sheet shows runs used; CI parses hud.js, mission.js and head.js with `node --check`.
- [x] `P9-070` A run waits out a provider rate limit (ADR-0156): when a model call is refused with a rate limit before any stream opened, it waits 20/45/90/180 s (cancellable, visible as activity) and asks again, then fails as before. Tested: clears after two, and never clears.
- [x] `P9-071` `jarvis.gmail.send` refuses an address marked `do_not_contact` in the contact list, and fails closed when the list cannot be read (ADR-0156). Tested including case-insensitivity and the closed-database case.
- [x] `P9-072` Console: an Ops "Contacts" card (counts by status, the first 40, and a status selector per row, which is how the owner lifts a do-not-contact).
- [x] `P9-073` Concurrent run starts no longer fail (ADR-0156): `start_run`, the memory purge and the summary write begin `BEGIN IMMEDIATE` (they read before they write, and a deferred transaction lost the race with `SQLITE_BUSY_SNAPSHOT`); the reason behind a refused run request is logged. Regression test: 24 simultaneous starts, which fails without the change. Found because the phase 2 gate failed once the restart continuation ran beside a new request.
- [x] `P9-074` Gmail can carry files, answer in its thread and save what arrives (ADR-0157): `jarvis.gmail.send` takes `attachments` (granted folders only, 5 files, 5 MB each, 10 MB total, secrets refused) and `reply_to_message_id` (thread headers sanitised, `threadId` sent); `jarvis.gmail.read` lists attachments; new `jarvis.gmail.save_attachment` (asks, never replaces, refuses programs). Research record: google.md Finding 25 (Google's size limit for send not found: unresolved). Verified live by the owner (2026-10-10): attachments sent and received fine.
- [x] `P9-075` JARVIS reads PDF, Word, spreadsheet and slide files (ADR-0158): new crate `jarvis-documents` (`lopdf`, `zip`, `roxmltree`; bounded, panic-safe, no entity expansion) and the tool `jarvis.files.read_document` (granted folders, read-only, 3,500-character slices with `next_offset`, fenced). Research record `document-reading-crates.md`. Tests build their documents in memory; verified live with a hand-made .docx through a real run (it found the codeword and the price); the owner then reported it works on real Office, LibreOffice and PDF files (2026-10-10).
- [x] `P9-076` `jarvis.files.move` (ADR-0159): moves or renames a file or folder within one granted root, never replaces anything (hard-link then remove for files, so a name taken a moment later still refuses), never crosses roots, refuses moving onto or into itself; risk low, runs without asking; not served over the inbound MCP endpoint. Tests include escapes in both directions.
- [x] `P9-077` Model fallback (ADR-0159): `daemon.executor_fallback_model_name` asks a second model at the same provider when a call is rate limited or overloaded before any stream opened (`FallbackGateway`, wrapped around the live adapter; one extra try, that call only). Other refusals are never worked around. Tested with a recording model; a live rate limit has not been provoked.
- [x] `P9-078` Web pages are read in parts, and PDFs on the web are read (ADR-0160): `jarvis.web.fetch` takes `offset` and answers `total_chars`/`next_offset`; `application/pdf` is read through `jarvis-documents` (5 MB, 15 s, panic-safe, same egress guard). Tests: parts join back into the whole, a PDF is read and fenced, a broken and an oversized PDF fail in words.
- [x] `P9-079` Word notes (ADR-0160): `.docx` reading includes headers and footers (each distinct text once), footnotes and endnotes with numbers, without the separators.
- [x] `P9-080` A stop no longer waits for an open stream (ADR-0160): the daemon's graceful shutdown waited without limit for open connections, and a console watching a run keeps a stream open, so `jarvis stop` left a daemon alive and unreachable until killed. The wait is now 8 seconds, then the connections are closed; tested with a real endless stream.
- [x] `P9-081` Gmail drafts (ADR-0161): `jarvis.gmail.draft` saves the message a send would carry (attachments, reply threading, do-not-contact check) in the owner's Drafts through `drafts.create`; needs the new `gmail.compose` scope (restricted), asked for with the other action scopes, so an existing sign-in must be repeated; runs without asking (writes only to the owner's mailbox). Research record: google.md Finding 26. Live draft and the consent flow for the restricted scope NOT run.
- [x] `P9-082` Text out of pictures (ADR-0162): `jarvis.files.read_document` reads PNG/JPEG/BMP and JPEG-scanned PDFs (`jarvis-documents::pdf_scan`) by running the owner's `tesseract` (stdin to stdout, absolute-path lookup, cleared env, time/size/pixel limits, optional validated `language`, `read_by: "ocr"`, last result cached for paging). Research record: tesseract.md. Tested with a stand-in program only; NOT run against a real tesseract (none installed); non-JPEG PDF images and TIFF/GIF unsupported.
- [x] `P9-089` What each model call costs in tool definitions is now visible: `offer_tokens` per tool on `GET /api/v1/tools` and in `jarvis tools list` (name + description + argument schema, characters / 4), with the total on top. Measured on the live daemon: 37 tools, about 7,200 estimated tokens sent on every model call (a one-word run measured 6,643 input tokens in all), heaviest `gmail.draft` 377, `gmail.send` 345, `contacts.save` 327, `files.read_document` 327, `command.run` 317. Not built: offering fewer tools per run, shorter descriptions (needs a tool-use quality check first), OpenAI-style prompt-cache accounting (`cached_input_tokens` is recorded but not shown).
- [x] `P9-088` A finished run's answer is found however long its event stream is: `last_answer` read only the first 1,000 events and looked backwards, so a run that streamed more fragments than that lost its answer (to the parent that delegated it via `jarvis.agent.result`, to the owner's notification, and to the transcript). New `latest_run_event_payload` asks the database for the latest `output_completed` directly; regression test with 1,300 events. The sub-agent "EMPTY" reports in the Sales journal were investigated: the four runs I could still see (567 to 665 events) did return 2 to 3 KB answers after 5 to 9 minutes, so that cause is not proven for those; this bug is a real way for the symptom to happen on longer runs.
- [x] `P9-087` Token usage is now recorded (found while building P9-086, ADR-0165): the OpenAI-compatible stream adapter stopped at the finish reason and never read the usage chunk that follows it, so no run ever recorded a `usage_updated` event; the finish is now held for the usage chunk, `[DONE]`, connection close, or 2 s. Tests: usage after finish, after a tool-call turn, no usage with `[DONE]` or close, and a server that never closes. Runs from before have no usage.
- [x] `P9-086` Token cap per project (ADR-0165): migration 0020 `projects.daily_token_limit`; `project_tokens_since` sums the `usage_updated` events of the project's runs over a rolling 24 hours; `tokens_today` and `daily_token_limit` on the project API, `jarvis project add|set --token-limit`, **Daily tokens** in the project window; the scheduler holds a fire at or over the cap (fails closed if usage cannot be read); the project tool cannot change it. Not built: stopping a run midway, money or prices, a workspace-wide cap, caps outside projects.
- [x] `P9-085` Brain settings (ADR-0164): provider presets (Ollama local/cloud, OpenAI, Gemini, Anthropic, other) + live model list + Test + Save on one card each for the main and fallback model; `OpenAiCompatibleProvider::list_models`; `POST /api/v1/settings/models` (daemon-side, stored key only to the address it was stored for, typed key otherwise); Save refuses a changed address without that provider's key; web-search key moved to its own tab; Google tab intro corrected. Research record: model-providers.md. Verified live against local Ollama (5 models listed, test passed in 26.7 s; other-address request refused). NOT run against OpenAI, Gemini, Anthropic or Ollama Cloud (no keys); no tool-call probe; saving still needs a restart.
- [x] `P9-084` hud.js split, first part: the settings dialog (five tabs, permissions, Google sign-in; about 370 lines) moved to `hud/settings.js`, behind `JarvisSettings(context)` with the seven helpers it shares; `explain` and `putJson` moved up beside `api`. hud.js 2578 to 2222 lines. Checked in a real browser against the live daemon (all tabs, Ops page and project form render, no page errors). The Ops region (about 1,000 lines) was measured and is NOT split: it shares 30 names and reassigns 6 variables with the rest, so it needs an explicit state object first.
- [x] `P9-083` Fallback at a different provider (ADR-0163): `executor_fallback_base_url` + `executor_fallback_api_key_ref` (+ `jarvis keys set|test|remove fallback`, Settings "Fallback key"); `FallbackGateway::at_another_provider` asks the fallback model there on rate limit, overload, transient failure or spent credit, never on auth/model/context/content/invalid-request refusals; unreadable key degrades to no fallback. Tested with scripted models; NOT run against a real second provider.
- [ ] `P9-069` Not started, recorded: a browser tool for JavaScript pages, an embedding memory index (P4-005), a second search provider, signed installer (P9-019), splitting the rest of hud.js (the Ops region needs a shared state object first) and a browser test in CI, replay of past constellations, ntfy access tokens.
- [ ] `P9-024` Give the model more to do. **Status 2026-10-10: largely delivered.** The model now has 37 tools: files (read, list, search, write, edit, move, read_document with OCR), web search and fetch, shell command, memory (search, propose, correct, forget), schedules, projects, contacts, Gmail and Calendar, and sub-agents. Not yet: a browser for JavaScript pages (`P9-069`), a notes tool beyond the project journal, and coding-agent runtimes (`P7`). The original text follows. Today Docker-isolated snippets run code too. A fresh install offers four tools (web fetch, delegate, delegate result, propose memory);
  files need a granted folder (`P9-015b`), `run_code` needs Docker, and nothing else is model-facing yet. Next, in value order: a
  model-facing memory search (so "what do you remember about me" is answered from the store, not from what happened to be in
  context), a clock/date tool, scheduling a task from conversation (the scheduler exists, `jarvis schedule`, but only as a command),
  a notes/todo tool, then the connectors (mail, calendar: `P5`, needing your OAuth credentials) and coding-agent runtimes (`P7`).
  Also show in the console, next to "Can do", what turns each missing tool on.
- [ ] `P9-019` Tauri shell with a tray icon over the same console (see `P9-001`).

## P10: Server And Multi-Device

- [ ] `P10-001` Define server deployment profile, TLS termination, trusted proxy rules, backup/restore, and operator runbooks.
- [ ] `P10-002` Complete PostgreSQL parity and concurrency tests for every canonical repository.
- [ ] `P10-003` Implement device enrollment, pairing proof, scoped credentials, rotation, revocation, and lost-device response.
- [ ] `P10-004` Implement remote access with explicit enablement, secure defaults, rate limits, and exposure audit.
- [ ] `P10-005` Add team membership and roles only after workspace isolation is proven.
- [ ] `P10-006` Establish service-level objectives, load profiles, recovery objectives, restore drills, and capacity evidence.
- [ ] `P10-007` Evaluate optional infrastructure only with measured bottlenecks and an ADR per adoption.
