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

- [x] `P5-001` Define connector manifest, account, auth flow, health, sync cursor, webhook, rate-limit, scope, and diagnostics contracts.
      **New crate `crates/jarvis-connectors`** (8 modules, **80 tests**), an adapter depending only on
      `jarvis-core` + `jarvis-tools`. `ADR-0054`.
      The manifest is a *security* document: `docs/architecture/tools-and-connectors.md`'s "parseable without
      loading provider code" is a **trust** requirement, not a packaging one, because the manifest is the only
      artifact an operator reads before granting access and the provider code it describes is what has *not*
      run. So the module's job is not to list fields but to **check its own claims against each other**:
      - **Effects are per operation and the classification is cross-checked.** A connector whose operations
        reach outward may not declare `Public` or `Internal` — those are exactly the levels
        `Classification::may_reach_a_remote_model` permits, so the combination mislabels content leaving the
        machine. The rule is a **floor, not an equality**: a mail connector legitimately declares
        `Confidential` because what it *handles* is confidential, and requiring equality would push every
        outward connector to the top of the ladder, making the classification meaningless. Tested in all three
        directions (two low levels refused, three high accepted, a read-only connector allowed `Internal`).
      - **A secret is a *field name*, never a value.** No `String` in the crate holds token material; that
        absence is the mechanical form of `security.md`. Names are environment-variable-shaped because they
        become config keys, and a name that could not be one would fail at deployment.
      - **Documentation links must discharge the research requirement.** `LinkKind`'s variants *are*
        `external-research.md`'s ordered source list (`LlmsTxt`, `Documentation`, `Specification`, `Sdk`, …), so
        a manifest with only a homepage is **incomplete rather than terse** — `satisfies_research_requirement()`
        must be true for at least one link. Plus a `ResearchRecord` path that is refused if absolute or holding
        `..`/`:`.
      - **The risk floor is checked here**, the first place an operation's risk is stated. Deliberately *not*
        checked: `P3-001`'s blind-retry rule — a non-idempotent outward operation is legitimate and common, and
        **this type carries no retry policy to disagree with**. That pairing becomes expressible in `P5-009`;
        the boundary is written down so a later reader does not "fix" it by refusing every write connector.
      - **A cursor's *kind* names what may be concluded from it.** `Start` is a variant with **no token**, not
        "no cursor yet", and a `Start` carrying a token is refused because it makes *never synced* and *synced
        to here* indistinguishable — the one distinction the type exists to keep. Bound to the account **and
        connector version** (`applies_to`), since a 1.0.0 cursor applied after a 2.0.0 upgrade is a silent
        misread.
      - **Health carries its probe, and staleness takes a *supplied* instant.** A state without its probe is an
        assertion with no evidence: "connected" by an identity call and by a subscription are different claims.
        `unix_nanos` comparison, not RFC 3339 text (`P3-004`'s trap: a zero fraction makes the text not sort).
      - **`Unknown` never permits a call, and `RetryClass::Unknown` never retries whatever the idempotency
        says.** Two rules, one shape: *the unanswered question refuses*. `Transient`/`Throttled`/`ProviderFault`
        delegate to the provider's idempotency claim; `Unknown` returns false in **every** column because the
        question is unanswered rather than because the provider said no.
      - **A webhook body is raw bytes, and an ambiguous security header is refused rather than picked.**
        `single_header` returns `None` for zero **and** for more than one: a repeated security header is either
        an accident or an attack, and taking the first turns *two signatures disagree* into *one was valid*.
        Only `Replayed` is acknowledged to the provider, so a genuine retry stops and a forgery gets no signal.
      - **The diagnostics field set is closed so `is_loggable()` is true for all twenty.** The constancy is the
        point — a field needing an exception would mean a value in the type was not safe to log.
        `may_reach_a_model()` is false only for `ProviderRequestId`, and **not because it is a secret** but
        because `security.md` minimizes what reaches a model rather than deciding per call site. A cursor's
        token is never a field, only its kind and observation time.
      - **PKCE against RFC 7636's own numbers**, with the S256 challenge asserted on **Appendix B's published
        vector** rather than a second implementation of the same mistake. `is_loopback_redirect` matches whole
        hosts over `http://` only, since a substring check accepts `127.0.0.1.evil.example`.
      **Two defects this slice found in itself, both from writing the tests, both an implementation disagreeing
      with its own doc comment:** `VerifiedAccount::new` checked `is_empty()` rather than `trim().is_empty()`, so
      `"  "` — an identifier whose whole purpose is to be usable — was accepted; and `ConnectorHealth::is_fresh_at`
      rejected only `checked_sub`'s overflow, so **a future observation was reported as fresh**, letting a record
      whose clock moved backwards look current.
      **All 12 guards falsified in an A-B-A design** (guard intact *passes* → neutered *fails* → restored
      *passes*). A′ is load-bearing: restoring with `Copy-Item` sets the mtime **backwards**, older than the
      mutant build, so cargo can run the stale mutant again — which happened here and produced a false
      "SURVIVED" for a guard whose test does catch it. The first harness also produced two false verdicts
      because the *mutant* was wrong, not the guard (`elapsed < -1000000` is still true one second in the
      future; one probe mutated `needs_full_resync_when_lost` while the test exercised `requires_full_resync`).
      **Limits:** **nothing consumes this crate** — no `Connector` trait, no HTTP, no RNG (so no code path
      *generates* a verifier), no loopback listener, no `SecretStore`, no persistence, no daemon wiring
      (`P5-002`+). No signature is verified (`P5-010`), no pagination is followed, no rate limit is enforced,
      and the manifest checks **self-consistency, not truth**: a connector that lies consistently is caught by
      review and the recorded research, not by a type. The ceilings are chosen, not measured.
- [x] `P5-002` Implement shared OAuth 2.0 Authorization Code plus PKCE flow, state/nonce validation, loopback callback, refresh rotation, revocation, and secret references.
      **New modules `authorization` + `token`** (36 tests, so **116 in `jarvis-connectors`**). `ADR-0055`.
      Research record `docs/research/integrations/oauth2-pkce-native-apps.md` written **before** the code, from
      the live RFC texts (RFC 7636, RFC 8252 BCP 212, RFC 9700 BCP 240, with RFC 6749 + RFC 9207 as
      referenced). `getrandom` added — already in the workspace and the lock file, so **no new package**.
      - **The transaction is consumable once, by construction.** `consume(self, ..)` takes `self` by value, so
        "answer the same setup twice" has no expression — RFC 9700 §4.2.4's "SHOULD be invalidated after its
        first use" as a property of the type rather than a caller's discipline. It carries the verifier, method,
        state, nonce, **the redirect URI**, the optional issuer and the instant in ONE value, because RFC 8252
        §8.10 requires exactly that grouping ("MUST store the redirect URI […] along with 'state'").
      - **A loopback redirect is a two-variant type, so nothing else is representable.** `LoopbackHost` is
        `127.0.0.1` or `[::1]`, so `http://evil.example/cb` and **`http://localhost/cb`** cannot be constructed
        rather than being checked. `localhost` is excluded deliberately: RFC 8252 §8.3 says NOT RECOMMENDED
        because it "avoids inadvertently listening on network interfaces other than the loopback interface" and
        is "less susceptible to […] misconfigured host name resolution" — a name whose meaning depends on a
        resolver cannot ground "only reachable from this machine".
      - **The port varies and nothing else does, in the direction the RFCs specify.** `matches_except_port`
        (RFC 9700 §2.1's "exact string matching except for port numbers in localhost redirection URIs"), plus
        `matches_exactly` for the portless *registration* versus the ported *listening* URI. Tested both ways
        (a different port accepted, a different **path** refused), since a one-sided test passes on a check that
        refuses everything.
      - **`state` is always sent even though PKCE can substitute.** RFC 9700 §2.1 permits PKCE-as-CSRF only
        after confirming PKCE support and §4.7.1 makes that confirmation a **MUST** — so rather than make safety
        depend on a discovery step, the state is unconditional and `parameters()` has no branch without it.
        `StateMissing` and `StateMismatch` are distinct refusals; the value comparison is constant-time.
      - **`nonce` is carried and handed back, unvalidated.** RFC 9700 §4.5.3.2 makes it meaningful only with an
        ID token, and verifying one needs OIDC (signature/`iss`/`aud`/`at_hash`). A `validate_nonce` that could
        not validate anything would be the declared-but-unconstructed pattern this repo has removed four times.
      - **The mix-up defence has three states and only one is a refusal.** `NotNeeded` / `IssuerConfirmed` /
        `NotSatisfied` / `IssuerMismatch`. RFC 9207's `iss` is **optional**, so its absence is not evidence of an
        attack — collapsing `NotSatisfied` into a refusal breaks every conforming server that omits it, and
        collapsing it into `IssuerConfirmed` reports a defence that never ran. Only a mismatch aborts.
      - **The listener is a trait whose capabilities are reported, security separated from compatibility.**
        RFC 8252 §8.3 + B.3 (`SO_EXCLUSIVEADDRUSE`) + B.5 ("SHOULD NOT set `SO_REUSEPORT`/`SO_REUSEADDR`") are
        one rule for two OSes: **no second binder on the port**. Both-stacks (§7.3) is a *compatibility* concern,
        so `UnmetListenerRequirement::is_security` is false for it — conflating a single-stack listener with an
        off-machine-reachable one makes a real gap dismissible as cosmetic.
      - **`TokenSet` has no access-token field, and that absence is the design.** Extends `P5-001`'s absence to
        the exchange: the lifetime, the granted scopes, the scope change and a `SecretRef` **locating** the
        refresh material, plus `has_refresh_token: bool` rather than the text. So no token material can reach a
        model context, log, diagnostic, or column even by mistake. A test asserts the `Debug` rendering contains
        neither `access_token` nor `refresh_token`, which is what catches a field added later.
      - **Retry-safety is a variant, because the two cases are indistinguishable and opposite.** `NeverSent`
        (safe) vs `SentAnswerUnknown` (**not** safe): RFC 9700 §4.2.4 makes a retry of a lost-answer request
        able to get `invalid_grant` **and destroy a working grant** the first attempt issued. Same shape as
        `P3-001`'s `RefusedBeforeReaching`/`AmbiguousAfterReaching`, one protocol over.
      - **Rotation is detected, because noticing is what makes replay visible.** RFC 9700 §4.14.2's detection
        property needs the client to see which shape came back; a client that kept its old token discards the
        defence. `Rotated` is reported **even when the caller supplied no reference**, since a caller's storage
        defect must not hide the half of the exchange that makes replay detectable.
      - **The transient check outranks the error code — and that ordering is the falsification run's finding.**
        A 503 carrying `invalid_grant` is a real shape (a proxy picks the code), and checking the grant first
        would send the user to a consent screen **during a provider outage**. The first mutation of this
        SURVIVED, and diagnosing why found the **fixture** was weak: it used `server_error`, for which both
        orders agree. A fixture with a transient `invalid_grant` is the only shape where they differ, and the
        guard then falsified.
      - **`invalid_grant` needs the user; a client misconfiguration does not.** RFC 6749 §5.2's `invalid_grant`
        covers "invalid, expired, revoked, does not match the redirection URI […] or issued to another client",
        all needing a new authorization. `invalid_client`/`unauthorized_client` mean the *client* is broken, so
        a consent screen lands on the same failure — they map to `Transient`, whose `needs_user` is false.
      - **A revocation's failure is not the same claim as its success.** RFC 7009 §2.2 makes "already invalid" a
        **success** (200 "if the token has been revoked successfully **or** if the client submitted an invalid
        token"), so `AlreadyInvalid::is_withdrawn` is true while `Unsupported`/`Refused`/`Unreachable` are not.
        Local material is discarded on every outcome, and an always-`true` predicate written here first was
        **removed** — a predicate with one answer is not a predicate.
      - **The generated verifier is RFC 7636 §7.1's own recipe** (32 octets → 43 unpadded base64url chars, the
        RFC's floor) and goes **through `new()`** anyway, because the generator and the validator are two
        implementations of one rule. `plain` stays representable (a server MAY ignore PKCE entirely, §5) but is
        not reachable by accident: `code_challenge_method` is always sent, removing the `plain` default's effect.
      **TWO DEFECTS THIS SLICE FOUND IN ITSELF, both a claim disagreeing with the code.** A doc comment said a
      pathless redirect URI would be refused; the code normalises it to `/`, which is what **RFC 3986 §6.2.3**
      says it is — the test written from the claim is what surfaced it. And a scope-loss fixture asserted a loss
      from a grant that had lost nothing (`previous=[mail.read, calendar.read]`, `current=[mail.read,
      calendar.read, new.scope]` is a pure *gain*), so the precedence rule was never exercised; the fixture now
      loses one scope **and** gains another.
      **All 25 guards falsified, A-B-A** (intact passes → neutered fails → restored passes). Two did not prove
      first time: one mutant was invalid (it left its `match` unbalanced — the second attempt kept the arm and
      made it unreachable), and one SURVIVED, which is what produced the ordering finding above. The harness
      restores with `WriteAllText` + a forward mtime bump, because `Copy-Item` restores the **older** timestamp
      and cargo then re-runs the stale mutant — the trap `P5-001` recorded.
      **Limits:** **nothing consumes this crate** — no `Connector` trait, HTTP client, socket, `SecretStore`,
      persistence, or daemon wiring. **No live verification**, deliberately: a live test needs a vendor,
      registered credentials and a browser, i.e. the per-connector smoke test (`P5-005`+). `nonce` carried but
      **not validated** (OIDC not implemented, so injection defence rests on PKCE alone). The mix-up defence
      depends on a vendor sending `iss`. `Expired` vs `Revoked` is the caller's classification (no code
      distinguishes them). **Sender-constraining is not implemented**, so a provider that does **not** rotate is
      a configuration this client cannot make RFC 9700 §2.2.2-compliant on its own — the strongest reason DPoP
      is on the roadmap. No automatic token-request retry, by design. No `localhost`, and no flag to enable it.
      `AuthError::RandomUnavailable`'s branch is **untestable here** (the platform source does not fail on
      demand), so it is verified by reading.
- [x] `P5-003` Create connector quality checklist and scaffold generator modeled on manifest-driven integration projects.
      **New modules `readiness` + `scaffold`** in `jarvis-connectors` (24 tests, so **140 in the crate**), and a
      **`jarvis connector` verb group** in `jarvis-cli` (`new`, `check`, `items`). `ADR-0056`.
      **49 suites, 1393 tests, 0 failed, 0 ignored** (was 49/1368). The lock delta is one line — `jarvis-cli`
      depending on a crate already in the workspace — so **no package joined the tree**.
      - **The checklist is a program, not a Markdown file, and the reason is `AGENTS.md`'s hardest-won rule:** a
        recorded claim is read downstream as *verified evidence* with nothing distinguishing "checked" from
        "assumed". A prose gate is read once and then trusted. `docs/architecture/tools-and-connectors.md`'s
        "Connector Completion Gate" is the checklist; this evaluates it.
      - **Every item is one of four evidence kinds, and the kind is reported.** 3 `Derived` (inside the manifest,
        so it cannot be asserted falsely), 3 `DeclaredArtifact` (a path, shape-checked), 5 `DeclaredCount` (a
        number), 1 `Conditional` (refusable, with a reason). A gate that printed a uniform "ok" would hide that
        **nine of twelve rest on the author's word and three on the manifest** — so the counts are asserted in a
        test and `jarvis connector items` prints the kind beside each item.
      - **⭐ THE SLICE'S CENTRAL DEFECT: the two webhook items were classified as `Derived` from the manifest.**
        That conflates **applicability** with **evidence**: *does this connector need webhook tests* is a
        manifest fact, but *does it have them* is not, and a manifest declaring `hmac_sha256` proves nothing was
        tested. The gate would have reported a push connector **complete** with no signature or replay test —
        exactly the condition `WebhookRejection` exists to make visible. The test asserting a push connector
        without those suites still reports them as gaps is what caught it (`got []` for the two expected).
        `applies_to` and `evidence` are now separate questions with separate answers.
      - **⭐ A SECOND DEFECT ONLY RUNNING THE COMMAND COULD FIND: the sidecar path.** The first version used
        `Path::with_extension("readiness.json")`, which replaces only the **last** extension, so
        `vendor.manifest.json` gave `vendor.manifest.readiness.json` while `connector new` writes
        `vendor.readiness.json`. The documented workflow produced a connector whose every declared item silently
        became a gap. **No unit test could have caught it** — both sides were "correct" against the same wrong
        assumption and disagreed only when the two verbs ran in sequence. Now one `sidecar_path` function with
        a regression test, so the writer and the reader share a rule.
      - **⭐ A THIRD: an unreachable refusal inside `EvidencePath`.** A separate `chars().nth(1) == Some(':')`
        drive-prefix branch, commented "a colon is legal later in a path on unix", **could never fire** because
        the next check refuses a colon *anywhere*. An unreachable refusal reads as protection while enforcing
        nothing — the defect `P5-001` recorded for an unreachable bound, found here **by mutation** rather than
        by reading. Removed.
      - **Applicability is derived and a wrong attestation is REFUSED, not dropped.** `applies_to(&WebhookSupport)`
        omits a non-applying item from the review *and* refuses an attestation naming it
        (`NotApplicable`), because a silent drop hides that the author is working from a template rather than
        from their own connector.
      - **A `ReadinessReview` exists only when the items are satisfied**, so "is this complete" is answered by
        whether the value exists (`ADR-0037`'s and `ADR-0055`'s move). `gaps` is the complement and names
        **every** outstanding item at once rather than refusing at the first.
      - **Each declared kind has a shape, and the wrong shape is an error.** Artifact ⇒ path, no count; suite ⇒
        count ≥1, no path (`None` and `Some(0)` are different claims — "not reported" vs "there are none");
        every item ⇒ a purpose; `covers_failure` ⇒ meaningful for **one** item and refused elsewhere, because a
        caller that set it believed it said something; onboarding **must** report failure coverage, since the
        document says "successful **and** failed".
      - **`EvidencePath` checks shape, not existence, and its doc says so.** It refuses empty/whitespace-padded,
        absolute, home-relative, `..`, a colon, an unknown extension, and >512 chars — and **cannot** prove a
        file exists, because this crate has no filesystem. The honest claim is "a path that *could* name a
        repository artifact", and that limitation is in the type's own docs.
      - **The conditionally-refused item carries its reason in the type.** `LiveSmokeTest` is
        `Present { path, gate }` or `Absent { reason }` and **both** constructors refuse a blank string, so the
        rule lives where the value is built. `ReadinessAssessment` carries the **strength explicitly** rather
        than letting a reader infer it from `declared_by.is_some()` — the conditional item's `declared_by` is
        `None`, so the first version made a stated *reason* indistinguishable from a manifest fact.
      - **The scaffold is generated from the manifest's own constructor**, so it **cannot be stale** — a
        committed template drifts the moment `ConnectorManifest` gains a field, and the failure surfaces far
        from the template. It refuses to invent three things: operations (empty, because `ConnectorManifest::new`
        refuses an empty list, so the skeleton is **deliberately invalid**), auth methods/secret fields/links
        (vendor facts), and the research date (**`--research-date` is required**, because defaulting it would put
        a real date in `last_verified` on a record whose every section is blank — a stub that looks verified).
      - **The skeleton is provably not a manifest**: it carries `_comment` keys and
        `deny_unknown_fields` refuses it. The test asserts the refusal, every field name, the emptiness of the
        four collections, the classification's starting level (`confidential`, since `ADR-0054` refuses below it
        for an outward operation), and that **every non-field key starts with `_comment` and is non-trivial**.
      - **The skeleton is built with `serde_json::json!`, and the first version was a `format!` template that
        produced invalid JSON** — its comments quote JSON examples, so their quotes needed escaping and were not.
        A hand-written template that must quote a document inside itself is a defect waiting for an edit.
      - **The two verbs are local, and that is a property of the subject**: a manifest and a research record are
        repository artifacts reviewed in a pull request, so putting them behind the daemon would mean starting a
        service to write a file the service must read back. `check` exits `DoctorWarnings` (6) rather than a
        failure code, because an unfinished connector is normal work in progress.
      - The attestations live in a **sidecar** (`<connector>.readiness.json`), not in the manifest, because
        `deny_unknown_fields` would make a readiness section part of the document an operator reads to decide
        whether to grant access — and evidence about tests is not a fact about authority.
      **All 21 guards falsified A-B-A.** Three findings beyond the defects above: **two tests were insensitive to
      their mutation and the fix was the assertion, not the mutant** (the skeleton test accepted *any*
      unknown-field refusal, so renaming a `_comment` key to `note` still failed to deserialize and the test
      passed); **two probes were written from a guess** rather than read from the file (a four-line formatted
      refusal; the JSON `"_comment"` string); and **a harness bug produced a uniform false negative** —
      `cargo test -p a -p b <filter>` treats `<filter>` as another `-p` pattern, so the first run reported
      "vacuous" for all 21 cases because **no test ran**. A harness that runs nothing reports success-shaped
      output, which is precisely what the A-B-A design exposed.
      **A `--force` flag the usage string promised but the program did not have was found while writing the ADR
      and fixed rather than recorded** — a one-line correction is cheaper than a documented inconsistency.
      **Limits:** **nothing consumes the review** — no connector exists, no daemon reads a sidecar, and no CI job
      runs `connector check`. `EvidencePath` **cannot prove a file exists**, so a connector can pass while
      naming three artifacts that do not. **Nine of twelve items rest on the author's word**, and the counts are
      **not compared against anything** — nothing runs a suite and checks the number. The scaffold writes two
      documents and nothing else (no crate, no test scaffolding, no registry entry). `check` reads one manifest
      at a time and has no notion of a connector repository.
- [x] `P5-004` Research Google identity, Gmail, Calendar, push notifications, quotas, and restricted scopes; record findings.
  - `docs/research/integrations/google.md`, fifteen live official sources with access dates, indexed in
    `docs/research/integrations/README.md`. **No Google API was called, no credential was used, and no Cloud
    project was created**; the record says so and claims no live verification.
  - **`llms.txt` is `not found`**, not assumed absent: `https://developers.google.com/llms.txt` and
    `https://developers.google.com/gmail/api/llms.txt` both return **HTTP 404**. Recorded with the substitution
    (the official guide and reference pages), because `external-research.md` requires a missing index to be
    recorded as missing rather than silently worked around.
  - **The research found a defect in shipped code, and the defect is the substantive outcome of this slice.**
    Writing down Google's two push mechanisms — an OIDC bearer JWT for Pub/Sub, an echoed channel token for
    Calendar, with a **zero-length body** so there is nothing to MAC — required explaining why neither fits
    `WebhookSupport::Push`. Reading the type to explain that showed that `SignatureAlgorithm::None`'s doc claimed
    the value "is refused by `WebhookBinding::new`", that `WebhookBinding::new` validates the *binding* and never
    sees the algorithm, and that `SignatureScheme::authenticates()` was reachable **only from tests**.
    `ConnectorManifest::new` never validated the webhook at all, so a manifest could declare `push` with an
    authenticator of `none` and be accepted — an endpoint applying unauthenticated writes, declared in a
    document a reviewer would read as having provided a control. **Fixed**, not merely recorded:
    `validate_webhook` + `ConnectorError::Webhook`, with a refusal message that names the honest alternative
    (`polling`). Two tests: the refusal, and an **over-refusal guard** proving all three real algorithms and
    `polling` are still accepted — because a rule that refused every push declaration would satisfy the first
    test while making the capability unusable.
  - **Guard falsified with the three-run A-B-A design**, not asserted. A (intact) `1 passed`; B (branch
    replaced with `false`) the refusal test **panicked at `manifest_tests.rs:1069`**; A′ (restored,
    `mutant=0 guard=1` verified by regex count) `2 passed`. The first attempt used the filter `verify_nothing`
    and reported `0 passed; 142 filtered out` — **a vacuous run that reads as a pass**, the trap `P5-003`
    recorded; the correct filter is `verifies`. Recorded because the near-miss is the reusable part.
  - **A second false doc claim was corrected in the same type**: `is_keyed_mac`'s comment said "a keyed MAC is
    the only mechanism that authenticates the **bytes**", which `Ed25519` contradicts — it authenticates the
    bytes with a public key, so the property the method actually answers is "does verifying require a secret".
    The `Ed25519` and `None` variant docs were rewritten to match. Both were claims that read as protections.
  - **Two findings reach past this integration.** (1) **Stale cursors arrive as ordinary status codes on a
    read**: Gmail returns **HTTP 404** for a `startHistoryId` outside the retained window, which is the same
    status as an absent account, while the required remedy is a full resync; Calendar returns **410 Gone**. So
    `SyncCursorKind::can_be_detected_as_stale()` is answered "only from the read's response, not the cursor's
    shape", and a Gmail connector must classify a 404 on `history.list` specifically. (2) **A 200 from a Gmail
    send does not mean the mail was sent** — the page says so verbatim, the quota is shared with the user's web
    client and IMAP, and 429s can lag "several minutes" — so `P5-009`'s send must be non-idempotent, never
    auto-retried, and reported as *submitted* rather than *delivered*.
  - **A third finding is an alternative to `P5-005` itself:** Google publishes a first-party **Gmail MCP
    server** (`https://gmailmcp.googleapis.com/mcp/v1`) in **Developer Preview**, which `jarvis-mcp-transport`'s
    existing `StreamableHttpClientTransport` could reach. Recorded as a **second surface, not a substitute** —
    ten tools, no `watch`, no `history.list`, no send, no sync, and its tools would arrive as an unclassified
    third-party source held for approval.
  - **Limits:** the record **cannot state a Calendar scope's category** — the Gmail page gives categories per
    scope, the Calendar page lists scopes without them, and the consent page gives the category *table* but not
    the mapping, so it is an Unresolved Question rather than a guess. **Whether a self-hosted single-user JARVIS
    qualifies for Google's internal-app exemption is unverified** and is the difference between days and months
    of lead time before a Gmail connector is usable by its own author; **`quotaUser`/`userIp` behaviour was not
    checked**, and it decides whether a multi-mailbox connector pays the 6,000-units/minute ceiling once or per
    mailbox. Gmail read scopes are **all restricted** — including `gmail.metadata`, the least-privileged way to
    read a mailbox — meaning a security assessment if the data is stored or transmitted. The quota model changed
    **2026-05-01** and charges are announced as pending later in 2026, so quota must be **configuration, not a
    constant**. **Six unresolved questions** are listed with impact and blocked capability; **no test for this
    research exists** and the record's Verification Plan names them as `P5-005`'s work.
- [ ] `P5-005` Implement Google connection setup and Gmail/Calendar read tools with recorded wire fixtures.
  - **PARTIALLY DONE, so the box stays unchecked.** Done: the connector's declared contract and its **auth
    flow** — `crates/jarvis-connectors/src/google.rs` (`GoogleConnector::manifest()`, `auth_flow()`, the three
    endpoints, the registered redirect) plus `google_tests.rs`. NOT done: the provider client, **any**
    operation implementation, and **no wire fixture** — which is the other half of the task.
    `CompatibilityVerdict::Unverified` says so in the manifest itself, and `is_installable()` is false.
  - **The declared contract** (`ADR-0057`, previous commit): three read operations at risk 0, `oauth_pkce`, an
    empty secret list, `Confidential` classification, `Polling { interval: Unknown }`, and the documented Gmail
    quota as a `PerClient` limit. The one link per `LinkKind` constraint is real for a multi-API connector, so
    Gmail's contract is the link of record and Calendar's pages live in the research record.
  - **This round settled the auth flow, which the previous round deliberately refused to guess.**
    `AuthFlow::new` requires an `https://` authorization endpoint and a redirect URI, and `P5-004`'s record did
    not establish either — so the previous commit exposed a bare endpoint constant and **constructed no flow**.
    Two authoritative sources fixed that: **`https://accounts.google.com/.well-known/openid-configuration`**,
    which is the one source here that is **machine-readable** (the server's own published configuration rather
    than a page's example), and the OAuth 2.0 for native apps guide.
  - **Verified endpoint facts**, now in the record with the truncation of
    `token_endpoint_auth_methods_supported` **recorded rather than glossed**: `authorization_endpoint`
    `https://accounts.google.com/o/oauth2/v2/auth`, `token_endpoint` `https://oauth2.googleapis.com/token`
    (**a different host from the consent screen**, which a reader might "fix"), `revocation_endpoint`
    `https://oauth2.googleapis.com/revoke`, `userinfo_endpoint`, `jwks_uri`, `code_challenge_methods_supported`
    = `plain`+`S256`, and **`authorization_response_iss_parameter_supported: true`** — which is what gives
    `AuthorizationTransaction::with_issuer` something to check instead of a permanent `NotSatisfied`.
  - **The loopback method is the only non-embedded option**: the page calls it the recommended desktop
    mechanism and states that **custom URI schemes are no longer supported** "due to the risk of app
    impersonation", with the OOB copy/paste method deprecated. Google's own advice also discourages `localhost`
    because of client firewalls — a second, independent reason for the refusal `P5-002` already makes.
  - **Three more facts that change how the connector must behave**, all recorded:
    **`client_secret` is `Optional`** on both the code exchange and the refresh ("not applicable to requests
    from clients registered as Android, iOS, or Chrome applications"), which is what the manifest's **empty
    `secret_fields`** rests on; **refresh-token issuance is limited** so "older refresh tokens will stop
    working" — a deployment that re-authorizes repeatedly can invalidate the token it was relying on; and
    **revocation removes the project's grants**, not one account's, taking "some time" to take effect, so a
    disconnect must not assume its effect is scoped to the account it named.
  - **`id_token` is expected on the exchange** because the manifest requests `openid`, and `P5-002` records that
    `nonce` is carried but **not validated**. The discovery document supplies `jwks_uri`, so the verification is
    now unblocked but still unbuilt; recorded as a limit rather than a capability.
  - **DPoP is recorded as a decision, not a flag.** Google supports it, recommends it, and (for a code exchange)
    requires `jti = BASE64URL(SHA256(AUTHORIZATION_CODE))` with a cacheable `DPoP-Nonce`. It binds the **refresh
    token** to a private key Google advises storing so it "cannot be copied off-device, for example by using
    TPMs, Secure Enclaves, or other hardware-backed keystores" — which is exactly the sender-constraining
    `P5-002` records as **not implemented** and whose own ADR calls the strongest argument for the work.
    Bypassing it is safe because it is optional; adopting it is a slice.
  - **One fact is still not established, and it is a question rather than a guess.** Google's page shows the
    *exchange request* using a ported URI (`redirect_uri=http://127.0.0.1:9004`) and requires `redirect_uri` to
    match an authorized URI **exactly**, but does **not** state which string the console accepts as the
    **registered** value for a Desktop-app client. Recorded as Unresolved Question 7. The connector uses the
    **portless** form because that is what `P5-002`'s own rules produce and `matches_except_port` compares, and
    `the_registered_redirect_is_the_portless_loopback_form` asserts the comparison works **while explicitly not
    claiming** the console accepts it.
  - **Falsified, one guard A-B-A**: the redirect comparison joining a registration to a listener
    (`self.host == other.host && self.path == other.path`), mutated by dropping the path half. Intact `PASS`,
    mutant `FAIL`, restored `PASS`, restore byte-identical.
  - **Limits:** no client, no operation implementation, **no wire fixture**; the registered redirect form is
    unconfirmed against Google's console (Unresolved Question 7); `nonce` is not validated, so the ID token is
    received but unverified; nothing is callable, because no `ToolDefinition` is derived from the operation ids;
    and **no token has been exchanged**, so `token_endpoint` and `revocation_endpoint` are transcribed and
    asserted but never used.
  - **This round added the provider's decisions and the documented directory shape.**
    `crates/jarvis-connectors/src/google/` is now `mod.rs` (the contract), `client.rs` (the provider's
    decisions), `tests.rs` and `client_tests.rs` (16 new tests, so **181 in the crate**), which is
    `repository-layout.md`'s integration shape. **`ADR-0058`.**
  - **The decisions are pure functions of a response, because this crate has no HTTP stack** — and that is the
    point rather than an accident. `classify` maps a status and reason to a `RetryDecision`; `next_page` bounds
    a page token; `advance_gmail_history` and `advance_calendar_sync` map a response to a successor or to a
    resync. Nothing here can send a request, so every branch is testable without one, and the transport binding
    stays a named step instead of becoming the shape the rules are written against. `jarvis-models`' injected
    `Transport` is the precedent, and the Google transport binding is now a bounded piece of work.
  - **The classification table is a function of Google's own error vocabulary, not of its status codes.**
    `GmailErrorReason` is the closed set the error page names, and a **403 is classified by its `reason`**
    because four documented reasons share that status with three different remedies. The case that justifies the
    table is **`domainPolicy`** — "the domain administrators have disabled Gmail apps" — which arrives as a
    `403` exactly like the two throttling reasons and whose remedy is a conversation with an administrator, not
    a retry. A status-only classifier retries an administrator's decision forever.
  - **An unknown reason is representable and classified conservatively.** `GmailErrorReason::Unrecognised`
    exists because Google adds reasons and refusing to parse one would turn "a new error code" into "a
    connector that cannot read its own errors"; its classification is the **status's**, so it can never loosen a
    decision. For a 403 that means `Permanent`/`DoNotRetry`, argued in the code: a 403 is a refusal with **no
    effect**, so `Unknown`/`Reconcile` would send a caller to establish whether an effect happened when the
    status already says it did not. A new *throttling* reason appearing as a 403 is the cost, and it fails in
    the direction that cannot cause a second effect.
  - **`GmailErrorBody` has no field for the error message**, so the classification cannot be derived from message
    text even by accident — the structural form of `P3-008c`'s "do not derive a safety flag from message text".
    `jarvis-models`' `McpToolListing` uses the same technique for a server's annotations.
  - **Two guards that encode provider-specific judgement, both falsified A-B-A with compiling mutants:**
    the 403-permanent arm (mutated to throttled: intact `PASS`, mutant `FAIL`, restored `PASS`), and
    `gmail_history_status_is_pruned` (mutated `status == 404` → `status < 404`: same shape). A **third** guard,
    the backwards-`historyId` refusal (`next_id < previous_id` → `false`), was also falsified. **The first
    attempt at two of the three was worthless and the record says why**: one mutant did not compile (so the
    verdict was `VACUOUS`, not a weak guard) and one multi-line probe matched nothing because PowerShell
    here-strings are CRLF while these files are LF — both traps this workspace has recorded before, hit again.
  - **A monotonic cursor may not move backwards, and an opaque one may not be compared at all.**
    `advance_gmail_history` refuses a smaller `historyId` because `historyId` increases, so a smaller value is a
    stale or foreign response and storing it would re-walk processed history — a repeat, for a connector that
    acts on changes. `advance_calendar_sync` performs **no** ordering check, because `nextSyncToken` is opaque
    and comparing two would invent a property the provider never offered.
  - **Google's numbers are constants with tests around their relationship**, not prose:
    `GMAIL_BATCH_LIMIT` (50) and `GMAIL_MAX_RESULTS_CAP` (500) with an assertion that the first is below the
    second, because batching is what makes a full sync affordable *and* is itself a rate-limit trigger —
    confusing the two would ask for 500 sub-requests at once.
  - **Limits:** no request has been sent and **no response has ever been parsed**, so the tests prove the code
    implements the *record* and nothing about the record matching Google; there is no transport, no token
    source, and no operation, so nothing builds an authorization request or calls `users.messages.list`;
    `GOOGLE_MAX_BACKOFF_SECONDS` has **no caller and is carried by no decision**, because
    `RetryGuidance::BackoffSeconds` states a starting delay and a ceiling would need a field the shared type does
    not have; and `SyncAdvance::Refused` is **never constructed**, which by `P5-001`'s standard is a variant that
    reads as a live condition.
  - **This round derived the model-facing tool definitions from the manifest.**
    `crates/jarvis-connectors/src/google/definitions.rs` + `definitions_tests.rs` (14 new tests, so **195 in the
    crate**). **`ADR-0059`.** `definitions()` reads the manifest and produces one `ToolDefinition` per declared
    operation, so every effect, risk, scope and idempotency value comes from the operation it belongs to and a
    definition **cannot disagree with the manifest** — the rule `P3-006d` established for a native adapter,
    extended to a connector. Only what the manifest has no field for is stated here: the schemas, the title,
    the timeout and the retry policy.
  - **The identifier prefix is load-bearing.** `google.<operation id>`, because `ToolSource::from_namespace`
    classifies by the **leading segment** — so the prefix is what makes these `Connector` tools rather than
    `Native` ones, and a connector tool that could declare itself `Native` would be claiming JARVIS wrote the
    adapter for a third party's API. A test asserts the classification, which is also what stops a future edit
    from renaming the namespace.
  - **The two idempotency vocabularies are mapped explicitly and totally.** The distinction that matters is
    between the two *safe* variants: `Declared` means the provider makes a repeat a no-op, so **no JARVIS key
    is needed**, while `ProviderKey` means the caller must supply one. Swapping them would either demand a key
    the provider ignores or omit one it requires. `Unknown` and `NotIdempotent` both collapse to
    `Unsupported`, which refuses repeats — safe to collapse, because neither claims a repeat is safe.
  - **⭐ A retry gate that the existing check does not cover.** `RetryPolicy::blind` refuses a retry only for a
    **mutating** effect, so a read-only operation passes it whatever its idempotency says — meaning the check
    alone would let this module retry everything. `retry_declaration` therefore also consults
    `ProviderIdempotency::permits_automatic_retry`, because **a blind retry is only free when repeating the
    call is free**. An operation whose provider behaviour is `Unknown` gets no automatic retry however
    harmless its effect looks.
  - **⭐ A check and a test were both written and then removed as UNREACHABLE, and the removal is checked.**
    `definitions.rs` had a risk-ceiling refusal, and its test built an over-ceiling operation directly. But
    `ValidatedOperation` has **private fields** and is built only by `ConnectorManifest::new`, which already
    refuses a risk above the platform ceiling — so the refusal could never fire. An unreachable refusal reads
    as protection while enforcing nothing, the defect `P5-001` and `P5-003` each recorded from a different
    direction. Its place is taken by a test asserting the **reachable** path still enforces the ceiling, so
    the deletion is verified rather than asserted.
  - **⭐ A test's own premise was wrong and the failure surfaced it.** `the_schemas_are_the_2020_12_dialect…`
    asserted that *every* input schema has a `required` keyword. It failed on `gmail_messages_list` — whose
    arguments are all legitimately optional, because calling it with nothing means "the newest messages",
    which the schema's own `query` description says. **The claim was too broad and the schema was right.** It
    now asserts per operation: the two resource-addressing reads must name their message or calendar, and the
    list must require **nothing**.
  - **The schemas are deliberately narrower than the APIs.** `format=raw` is absent, because it returns the
    unparsed MIME message and nothing in this connector parses it; `max_results` is capped at Gmail's
    documented 500; `calendar_events_read` takes an opaque `sync_token` whose description records that a 410
    requires a full resync. `additionalProperties: false` throughout, so a model inventing a field gets a
    refusal rather than a silent ignore — asserted by validating an accepted and a refused instance.
  - **Input and output classifications differ on purpose.** A message id is `Internal`; the message is
    `Confidential`. A single field would have to be the maximum, which would over-restrict the input and hide
    what the tool consumes, and `ToolSensitivity::ceiling()` is what a placement decision reads.
  - **Three guards falsified A-B-A with compiling mutants:** the idempotency mapping's `Declared` direction,
    the retry gate (`permits_automatic_retry()` → `true`), and the title-table fallback (`_ => None` → an
    invented title). **One of the three first attempts hit the CRLF trap again** — a multi-line here-string
    probe matched nothing against these LF sources — and was retried with an LF-joined anchor. The trap is the
    third occurrence in this workspace; it is recorded again because the cost is a whole wasted run.
  - **The timeout and backoff are recorded as an unfixed problem rather than smoothed over.**
    `TOOL_TIMEOUT_SECONDS` is 30 (a JARVIS choice; Google documents no per-request deadline) and
    `TOOL_BACKOFF_CEILING_SECONDS` is 32 (Google's lower published figure), so **two attempts at 32 s cannot
    both complete inside a 30 s deadline — only the first retry is reachable.** The test states that
    consequence instead of asserting a comfortable inequality, and the fix (a longer deadline or a smaller
    ceiling) needs a measured provider latency, which needs a live call.
  - **Limits:** the definitions are **registered nowhere**, so no model can discover or call them; **no
    executor** implements `ToolExecutor` for this connector; the schemas are this connector's construction and
    have never been validated against a real response; `output_schema` describes a normalised JARVIS shape
    rather than Gmail's message resource, so it cannot be checked against a provider document; and
    `GoogleToolError::NoSchema` **cannot be reached from a test** because the operation it would be built from
    cannot be constructed — verified by construction rather than by exercise, which is stated rather than
    implied.
  - **This round built the requests, which is the other half of the client that needs no socket.**
    `crates/jarvis-connectors/src/google/request.rs` + `request_tests.rs` (18 new tests, so **213 in the crate**).
    **`ADR-0060`.** A request is a **value** here — method, URL, ordered parameters — and nothing sends it, the
    same split `client.rs` makes. Three request builders (`gmail_messages_list`, `gmail_messages_get`,
    `calendar_events_list`) and three response parsers.
  - **⭐ THE ENCODING IS A SECURITY CONTROL, NOT TIDINESS.** A Gmail query is model-chosen text, and each
    hazardous character changes the **request** rather than the query: `&` starts a new parameter, so
    `is:unread&maxResults=999` **replaces the connector's own bound with the model's**; `=` reads as an
    assignment; `#` ends the query; `%` starts an escape of the value's choosing; `?` starts a second query
    string. Every value goes through `percent_encode` (RFC 3986 unreserved set, **uppercase** hex per §6.2.2.1
    normalisation), and the falsification mutated the unreserved set to admit `& = # %` and the injection test
    failed. **A space is `%20` and a literal `+` is `%2B`**, because the form-urlencoded convention is
    ambiguous in both directions and `%20` is correct in every query position.
  - **⭐ `HttpRequest` HAS NO FIELD A CREDENTIAL COULD GO IN — the absence is the control.** Google's own page
    offers `?access_token=` and adds that "query strings tend to be visible in server logs", so the parameter
    is a supported way to do the one thing that leaks a token into every artifact a debugging session
    produces. The type is a method, a URL, ordered parameters and an `Accept` value: no header map, no token,
    and **no body** (all three operations are `GET`s, so a body field would be a shape nothing uses). The
    header's *name* and scheme are constants so a transport knows where a credential belongs; the value is not
    in this crate. Asserted on every request by searching each rendered URL for `access_token`/`token=`/`key=`.
  - **`Display` reports parameter NAMES and never a value**, because a rendering reaches a log line and a
    value is model-chosen text. That is why `url()` (path only) and `url_with_query()` are split: a diagnostic
    can render the shape while the transport builds the target.
  - **`status` is checked BEFORE the body is parsed, in every parser.** The failure mode is specific: an error
    document parsed as a page reports "no results", and a caller cannot then tell a successful empty mailbox
    from a refused request. The refusal names `client::classify` as where the outcome belongs. Falsified by
    making the status check `if false`.
  - **`nextSyncToken` and `nextPageToken` are separate fields and are not interchangeable.** Google's sync
    guide says the sync token "is present only on the very last page" while the page token continues the
    current walk, so a caller storing the page token as a cursor would store something that expires with the
    walk.
  - **`format=raw` is not representable.** `MessageFormat` has no `Raw` variant, because that format returns
    the unparsed MIME message including attachments and nothing in this connector parses it — a type that
    cannot express the value is stronger than a check that refuses it.
  - **`maxResults` is bounded per API** (500 Gmail, 2 500 Calendar), because one shared bound would be wrong
    for one of them; zero is refused rather than read as "unlimited"; and the value **at** each cap is
    accepted so neither bound is unreachable. Falsified by dropping the zero check.
  - **⭐ MY OWN TEST'S PREMISE WAS WRONG AND THE FAILURE SURFACED IT.** The injection test asserted that a
    hazardous character "must not survive encoding" — which is **false for `%`**, whose escape is `%25` and
    therefore *contains* `%` as the escape marker. The property happened to hold for `&`, `=`, `#` and `?`,
    which is why it looked right. Replaced with the **exact** expected encoding per character plus the real
    property (`%` only as the marker). **A property that holds for every case you tried is not yet a rule.**
  - **Three guards falsified A-B-A with compiling mutants:** the encoding's unreserved set, the status-first
    check, and the zero-`maxResults` refusal. **Two probes needed a second attempt** — both were multi-line and
    hit the CRLF here-string trap (4th and 5th occurrences in this workspace), retried with **LF-joined
    anchors**.
  - **Limits:** **no request has been sent and nothing performs one**, so `HttpRequest` has **no production
    caller**; the response parsers have never seen a real response, so they prove the code reads the *record's*
    shape; `parse_id_page` accepts a body with no `messages` array as an empty page — correct for
    `{"resultSizeEstimate": 0}` and indistinguishable from a *successful* wrong-shaped document, recorded
    rather than fixed because the fix would be a stricter schema nobody has observed; **no retry, pacing or
    budget accounting happens here**, so nothing connects a request to a `classify` decision; and only `Accept`
    is stated, so a transport still has to supply `Authorization` and whatever else a live call needs — a
    coverage gap rather than a decision.
  - **This round built the credential boundary.** `crates/jarvis-connectors/src/google/credential.rs` +
    `credential_tests.rs` (10 new tests, so **223 in the crate**). **`ADR-0061`.**
  - **⭐ THE FINDING: ADR-0060 CLOSED ONE ROUTE AND NOT THE ONES THAT ACTUALLY LEAK.** Giving `HttpRequest`
    no credential field makes a token unrepresentable *in a URL*, but a token reaches an artifact through an
    **impl**, not a data structure: a derived `Debug` (one `{:?}` in a log renders whatever the struct holds),
    a `Serialize` into a durable row or a wire DTO, a `Display` in a transport's error wrapper, or a plain
    `String` argument that every holder can print. So `AccessToken` implements **neither `Serialize` nor a
    derived `Debug`** — the `Debug` is hand-written to `[REDACTED]` plus the character count, which is not the
    value and is what distinguishes two credentials in a diagnostic.
  - **The absence of `Serialize` is ASSERTED, not described, and the assertion was falsified.** A
    `compile_fail` doctest serializes the token, so a later `#[derive(Serialize)]` breaks the build. **A
    `compile_fail` test can pass for the wrong reason** — a bad import path also fails to compile — so I replaced
    the serialization line with a call that *should* compile and the doctest then **failed**, which is what
    proves it fails for the serialization. Second such assertion in the workspace, after `SecretRef`.
  - **The bytes are reachable through one accessor and its name is the warning.** `with_exposed(|token| ..)`
    takes a **closure** rather than returning a `&str`, so the borrow cannot outlive the call and a caller
    cannot move the material somewhere a later `Debug` could reach; and the closure gets only the text, not the
    token, so reaching it requires writing the word `exposed`.
  - **The header is rendered by the token, not by the transport** — `authorization_header_value()` is the
    single place the bytes and the scheme meet. A transport building `"Bearer " + value` itself is where a
    missing space or a **doubled** scheme comes from, and the doubled scheme is a real shape because a pasted
    value may already carry it.
  - **A pasted `Authorization` header reports removing the scheme, not the whitespace.** `Bearer <token>`
    contains both, so the **scheme check runs first** and the message says what to do; a whitespace-first check
    would report “contains whitespace” — true, useless, and it sends a reader hunting an invisible character.
    `P5-004` records the same ordering decision for an API key, and the variants are separate because the
    remedies differ. A 20-character floor then makes a client-id paste mistake surface at construction rather
    than at the provider, where a generic auth error points at the credential's *validity* instead of its
    *shape*.
  - **The type may name which credential it came from without naming what it is.** `origin: Option<SecretRef>`
    is the one field a diagnostic prints — “the stored refresh exchange for this account failed” — and a
    `SecretRef`'s own redaction is **asserted** rather than assumed. Every refusal's message is asserted not to
    render the value it refused, and for **every variant at once**, because a new variant is the case that
    would forget.
  - **⭐ MY OWN MEASUREMENT WAS WRONG AND THE ARITHMETIC CAUGHT IT.** A workspace run appeared to report
    “30 suites, 1 failed, 1 compile error”, which sent me looking for a failure that did not exist. The cause:
    **`Select-String` is CASE-INSENSITIVE by default**, so a pattern of `FAILED` matched the `0 failed` in every
    one of the 48 `test result` lines — inflating the “failed” count and truncating the file I was reading into
    a stale fragment. The real totals are **48 suites / 1477 passed / 0 failed**. **Use `-CaseSensitive` for a
    verdict search**, and prefer counting `test result:` lines and parsing `N passed` / `N failed` from each.
  - **Limits:** **nothing holds an `AccessToken` in production code** — no transport, no token source, no
    exchange, so the type has no caller outside its tests; **the material is not zeroized on drop** (`zeroize`
    is not a dependency and a partial answer about which types zeroize would imply coverage it lacks);
    `authorization_header_value()` **allocates a second buffer holding the credential**, which is a small
    widening recorded rather than hidden; `Debug` on a `Vec<u8>` derived from the header would still print it,
    because this type controls its own rendering and not anything derived from it; the length floor is a
    heuristic and proves nothing about validity; and **nothing checks the token is for Google**, because
    provenance is not a property a string carries.
  - **This round built the transport port and the read operations — the connective tissue that makes every
    earlier slice reachable.** `crates/jarvis-connectors/src/google/transport.rs` + `transport_tests.rs` and
    `operations.rs` + `operations_tests.rs` (28 new tests, so **251 in the crate**). **`ADR-0062`.**
  - **⭐ I SHIPPED A SYNCHRONOUS PORT AND `ToolExecutor::execute` IS `async` — so the port could never back an
    adapter.** Written, formatted, tested, gated, and committed as `416fd94` before the flaw was noticed: a
    synchronous `send` cannot be called from an `async fn` without blocking a runtime worker for the whole round
    trip, which is the failure `P2-007` records for a blocking read inside a stream. The reasoning that produced
    it — "defer the async binding to whoever owns a runtime" — is internally consistent and still produced an
    **unreachable** port: the one interface it exists to satisfy is async, so the deferral meant "never". Fixed
    by making the port `#[async_trait]` **and** by implementing `ToolExecutor` for the adapter, which is what
    turns "the port exists" into "the port can be used". Recorded because a plausible deferral is the easiest
    way to build a component nothing can consume.
  - **⭐ THE FINDING: "THE REQUEST FAILED" IS NOT ONE CONDITION, AND THE TWO DIRECTIONS ARE NOT SYMMETRIC.**
    `TransportFailure` is `Connect | Send | Body | Timeout | Refused`, and the variants encode **when** the
    failure happened rather than what it was called. `Send` exists separately from `Connect` because "the
    request was written and then the connection broke" is not "the request could not be sent"; conflating them
    is exactly how an ambiguous failure becomes a certain one. `may_have_reached_the_provider()` is `false` for
    `Connect`/`Refused` and `true` for `Send`/`Timeout`/`Body`, and it is deliberately **not** named
    `is_certain_nothing_happened` — that reading invites a `true` default in a `match` fallback, and a new
    variant would then silently become retryable. It is a **wildcard-free `match`**, so a new variant is a
    compile error rather than a default.
  - **The subtle case is `Body`: the provider DID answer.** A response whose body could not be read means the
    request was certainly received and nothing is known about what it did — so grouping it with `Connect` would
    be the precise mistake the ambiguous variant exists to prevent. Its consequence is asserted, not just its
    class: `AmbiguousAfterReaching`, which **refuses an automatic retry** — because for a non-idempotent effect
    a retry is a second effect.
  - **A refusal is a RESULT, not an error, and the reason is the provider's machine-readable code.** A `403` was
    received and refused, so it becomes a `ToolCallResult` with a `Failed` outcome. Collapsing a non-2xx into a
    transport error would lose the status and the `errors[].reason` code, which is everything the next decision
    needs — and the prose is never carried, because `P3-008c` forbids deriving a decision from message text. A
    body that cannot be parsed still yields a reason naming the **status**, which is a fact even when the body
    is not: the difference between "we know little" and "we know nothing".
  - **⭐ A 200 WHOSE BODY IS UNREADABLE IS `Unknown` — AND A RESULT, NOT AN ERROR.** The status proves the
    request was answered; the body says nothing about what it produced. Reporting `Confirmed` would claim an
    effect from a status code and reporting `Failed` would claim nothing happened, so `Unknown` is the only
    honest reading. It is reached through a returned result rather than an error **because the provider did
    answer**, and `Unknown` refuses an automatic retry — the consequence that matters.
  - **The port keeps the credential boundary `ADR-0061` built.** `send` takes `request` and `token` as
    **separate parameters** rather than one authenticated-request type, because `HttpRequest` may be rendered
    (its `Display` prints the path and parameter *names*) and `AccessToken` may not; merging them would make a
    single `{:?}` leak the token. `HttpMethod` is a **closed enum with one variant** — `POST`/`PATCH`/`PUT`/
    `DELETE` are absent rather than present-and-unused, because a variant nothing constructs is a method a
    reader assumes is reachable — so `P5-009`'s write grows the enum and turns every `match` into a compile
    error, which is what makes a new method a deliberate edit.
  - **The adapter is a `ToolExecutor`, and its test drives the real trait with a request the REAL policy engine
    authorized.** The receipt's digest is recomputed from the arguments the request carries and the decision
    comes from an actual `evaluate` over the tool's own derived definition, because `AuthorizationReceipt::new`
    and `ToolExecutionRequest::new` verify everything — a fixture cannot stand in an invented digest or a
    fabricated decision. The actor holds exactly `mail.read` + `calendar.read`, the scopes the manifest
    declares, so the decision is allowed by the connector's own contract rather than by a wildcard.
  - **The deadline is checked before anything is sent, and refused rather than failed.** `RefusedBeforeReaching`
    is honest — nothing was sent, so nothing happened — and `AmbiguousAfterReaching` would send a reader
    investigating an effect that never existed. `jarvis-tools`' filesystem adapter makes the same choice before
    its first read. The pure `run` method takes `now` as a parameter while the trait `execute` reads the clock,
    so the deterministic layer can assert an exact `reported_at` and the logic is implemented once.
  - **The operation layer renders the tool's DECLARED output, not the provider's resource.** `read_output`
    emits `message_ids`/`event_ids` plus the declared schema's fields, because a parser returning Gmail's
    `Message` would make `ADR-0059`'s schema a fiction. `next_sync_token` is rendered **separately** from
    `next_page_token`: the sync token positions a **future** incremental sync while the page token continues the
    current walk, so merging them would store a cursor that expires with the walk.
  - **An argument that is supplied but wrong is REFUSED rather than dropped** — the defect `P5-004` records from
    the other direction, where a value that is present and ignored is indistinguishable from one that was
    honoured. A `null` is treated as *absent* (that is what a serializer emits for an unset optional field) and
    accepted; a wrong-typed or out-of-range value is refused. An unknown tool is `NotImplemented` rather than a
    default, because a fallback would make a mistyped name silently read a mailbox.
  - **⭐ FOUR GUARDS FALSIFIED WITH COMPILING MUTANTS.** Making `Timeout` certain → **4 tests detected**;
    making `Body` certain → **3 detected**; ignoring the status in the response classifier → **2 detected**;
    deleting the deadline check → **1 detected**; turning the deadline refusal into `AmbiguousAfterReaching` →
    **1 detected**. Each mutant **compiled**, so `FAIL` is a real detection rather than a build failure — the
    `VACUOUS`/`FAIL` distinction kept, and verified with a `git diff --stat` afterwards because an earlier
    aborted run had left one mutant in place.
  - **One new dependency edge: `async-trait`** (already a workspace dependency, so nothing new is fetched), for
    an object-safe `async` trait method. Still **no `reqwest` and no socket** in this crate.
  - **Limits:** **there is no transport implementation** — the port has a test double and nothing else, so **no
    request has been sent to Google and no response has been parsed from Google**, and every fixture is
    constructed from the research record, which means these tests prove the layer implements the *record*, not
    that the record matches the provider; **the port's own requirements (no redirect, no retry, no proxy from
    the environment) are unenforced** because the only implementations are test doubles — they become testable
    only when a real transport is written; **no credential is minted, refreshed, or expiry-checked** (`run`
    takes an `AccessToken` it did not obtain, so a stale token becomes a provider refusal); **the deadline is
    read from the system clock**, so the trait boundary can only be tested with a *lapsed* deadline; **no
    deadline is passed to the transport**, so "an implementation must bound its own wait and report `Timeout`"
    is a requirement on an implementation that does not exist; **`Retry-After` is carried and never
    interpreted**, so a transport could report it and a caller could ignore it with no test failing;
    **`evidence_from` has no caller in this crate** (provider evidence locates an *effect* and a read produces
    none, so it exists for `P5-009`); ~~the output rendering is not validated back through the schemas it claims
    to match~~ (**closed this round** — see below); and **nothing constructs the adapter in a binary** — no
    registry, executor, or route offers
    `google.gmail_messages_list`, so the adapter is reachable from a test and not from a run.
  - **This round added the wire fixtures — and the interesting part is what they are NOT.** Six files under
    `crates/jarvis-connectors/tests/fixtures/google/` plus `tests/google_fixtures.rs` (9 new tests, so **262 in
    the crate**). **`ADR-0063`.**
  - **⭐⭐ THE FINDING: A HAND-BUILT RESPONSE IS EASY TO MAKE INTERNALLY CONSISTENT AND IMPOSSIBLE TO MAKE
    REALISTIC IN ITS CONSTRAINTS — AND MY OWN EARLIER TEST PROVED IT.** I had written a test asserting a Calendar
    page carrying **both** `nextPageToken` and `nextSyncToken`. It passed. **Google cannot produce that body.**
    The reference page documents the two as **mutually exclusive**: `nextPageToken` is "Omitted if no further
    results are available, in which case nextSyncToken is provided", and `nextSyncToken` is "Omitted if further
    results are available, in which case nextPageToken is provided". The constraint lives in the **field
    description**, not in the sample JSON — so reading the *example* tells you the shape while reading the
    *prose* tells you what may co-occur. The test now asserts the two states **apart** (a mid-walk page with a
    page token and no sync token; a last page with a sync token and no page token) and the fixtures are that pair.
  - **⭐ "WE HAVE FIXTURES" MUST NOT BE READABLE AS "WE HAVE A RECORDING".** The acceptance text says "with
    recorded wire fixtures", and a directory of clean JSON looks identical whether it was copied from a live
    response or assembled from documentation. Writing shapes and calling them recordings is the failure mode the
    ADR exists to prevent, and it is the same shape as `ADR-0056`'s scaffold inventing a completed checklist. So
    each fixture carries `_not_a_capture: true` and `_shape_documented_at: <url>` **in the data**, the suite
    **asserts the marker** (a file that dropped it fails rather than passing as an apparent capture), and the
    research record repeats it. A fixture that was never captured proves the **reader**; only a capture proves
    the **record**.
  - **A fixture states its own point, because a reader cannot recover it from the bytes.**
    `_the_point_of_this_fixture` records *why* a payload exists — that the Calendar pair exists because the
    tokens are exclusive, and that the two 403 fixtures are a **pair whose only difference is the reason code**.
    Without that, a maintainer "simplifying" the pair into one file deletes the only test that distinguishes a
    Workspace administrator's decision from a throttling limit.
  - **The 403 pair makes the reason-vs-status split falsifiable, and both files are a 403.** One says
    `domainPolicy` and classifies as permanent; the other says `rateLimitExceeded` and classifies as retryable.
    The load-bearing assertion is that the two decisions **differ** — a classifier switching on the status alone
    would give them the same answer. The `domainPolicy` fixture's `message` deliberately invites a retry ("This
    looks like a transient error…") while its `reason` code forbids one, so a classifier reading prose instead
    of the code classifies it **wrongly**.
  - **The error fixture also proves the prose never reaches a decision.** `GmailErrorBody` has no field for
    `message`, so the fixture's text parses and then becomes **unreachable** — asserted by checking the derived
    `Debug` does not contain it. That is `P3-008c` as a property of the type rather than a convention.
  - **⭐ TWO MORE GUARDS FALSIFIED WITH COMPILING MUTANTS.** Flipping `_not_a_capture` to `false` → **3 tests
    detected**; deleting `nextPageToken` from the mid-walk page → **3 tests detected**. The second is the
    important one: it shows the mutual-exclusion property is genuinely load-bearing rather than incidentally
    true. Tree verified clean with `git diff --stat` afterwards.
  - **⭐ ONE RECORDED LIMIT WAS CLOSED RATHER THAN REPEATED.** Three different rounds had listed "the output
    rendering is not validated back through the schemas it claims to match" as a limit. A limit that is recorded
    three times and never closed is a **todo wearing a limit's clothes**, so it was fixed:
    `every_rendered_output_satisfies_the_schema_the_definition_declares` renders a page through `read_output`
    for **all three** operations and validates the result against `ToolDefinition::output_schema()`, which is
    **derived from the manifest and not restated**. The empty-page case is included, because an omitted
    `message_ids` and an empty `message_ids` are different documents and only the second satisfies the schema's
    `required`. Falsified with two compiling mutants — renaming a rendered field and omitting a required array —
    each detected by the new test **and** by the existing shape test.
  - **The research record's Verification Plan is now auditable.** It had eleven items and said "none exist yet";
    each is now marked **WRITTEN** or **Not written**, with the one written item named and the six fixtures
    listed as hand-built. A plan where every line is unmarked reads as done; a plan where every line is marked
    can be checked. The live smoke test remains **not written**.
  - **NEW LIMITS:** **no fixture is a capture and no live call has been made** — no credential exists, no Cloud
    project was created, no Google API was contacted; **a hand-built fixture cannot reveal a constraint nobody
    documented**, so a rule Google enforces but does not write down would still be invisible; **the fixtures do
    not cover every declared operation** — `history.list` (the operation Finding 2 most depends on), the batch
    endpoint, and the `format=metadata` envelope have **no** fixture at all; **the sweep checks provenance and
    not shape**, so a fixture is not validated against the schema it feeds (the *rendering* now is, which is a
    different join); and **`_shape_documented_at` is a URL nothing re-checks**, so a fixture can drift from the
    page it cites with no assertion failing.
  - **This round built the token exchange — the "connection setup" half, which was the least built part.**
    `crates/jarvis-connectors/src/google/token.rs` + `token_tests.rs` (25 new tests, so **278 in the crate**).
    Until now a connector could *start* a flow and *classify* a refresh and could not complete a first
    authorization. **`ADR-0064`.**
  - **⭐ THE FINDING: OAUTH ANSWERS FROM THE BODY AND NOT FROM THE STATUS — THE OPPOSITE OF THE REST OF THIS
    CRATE.** Everywhere else here a refusal is read from the status: `client::classify` switches on the code and
    `TransportResponse::is_success` is `status == 200`. RFC 6749 §5.1 puts the token parameters in a *successful*
    body, and §5.2 makes a failure an `error` parameter with those parameters **omitted** — so the presence of
    `error` is what makes an answer a refusal, and a `400` **without** one is a proxy's page or a misrouted
    request. A status-first reading would report "the server refused" and attribute a decision to a server that
    never made one. The status keeps exactly one bit: `5xx` marks the refusal transient, because RFC 6749 §5.2's
    codes describe the *request* and cannot say whether the server is unwell — that is the transport's
    observation.
  - **⭐⭐ I WROTE THE EXACT BUG `ADR-0061` EXISTS TO PREVENT, IN THE FIRST DRAFT OF THIS MODULE.**
    `parse_answer` read `access_token` into an owned `String` and then dropped it, with a comment saying so. It
    satisfies the letter of the rule and breaks its purpose: an owned copy existed, **with a lifetime**, in a
    function whose other outputs are `Debug`-printed. The rule is about **copies, not lifetimes**, so the draft
    was replaced — `parse_answer` now asks only whether a non-empty token is *present* (`.is_some_and`, producing
    a `bool`) and never binds the bytes to a named value, and `Granted` has **no field for them**: a field would
    be a second, unredacted copy of the credential in a value the rest of the crate prints. A test renders a
    granted answer and asserts **neither** the access token **nor** the refresh token appears.
  - **A refusal is a VARIANT, not an error, and the three outcomes stay apart.** An **unreadable** `400` (HTML,
    or JSON with neither a token nor an error) is `TokenRequestError::Body` — not `Refused`, which would invent a
    provider decision, and not a transport failure, which would claim unreadability when the provider answered.
    A grant that *completed* but is unusable (`token_type` not `Bearer`, or a lifetime past the bound) is
    `UnusableGrant` rather than `Refused`, because saying the provider declined would be false. And
    `TokenEndpointAnswer`'s two accessors are **disjoint** — exactly one of `response()` and `failure()` is
    `Some` — which is asserted, because it is the property the whole split rests on.
  - **The transient check precedes the error code, and both halves are asserted.** A provider behind a proxy can
    answer a `503` through the protocol's own channel carrying `invalid_grant` — a real shape — so reading the
    code first would send a user to a consent screen **during an outage**, which finds the same failure and looks
    like a broken connector. The test asserts the ordering **and** the control: the same code without the outage
    **is** the user's problem, so the rule is not simply "ignore the code".
  - **The rotation rule is about ARRIVAL, never about storage.** `invalid_grant` covers "invalid, expired,
    revoked, does not match the redirection URI, or was issued to another client" and the protocol does not say
    which — so `vendor_says_revoked` is a **parameter**, and a refresh is `Rotated` by whether new material
    arrived rather than by whether the caller stored it. A caller that failed to store one has a defect of its
    own, and reporting `Refreshed` would hide the half of the exchange that makes replay detectable.
  - **The request is a parameter LIST, not an `HttpRequest`, and there is no `client_secret` field anywhere.**
    `HttpRequest` is a `GET` with no body and no credential field *by design*, so forcing a credential-bearing
    form `POST` into it would undo `ADR-0060`. The list has no `Display` and no `Serialize`. The parameter set is
    asserted as an exact **set**, so an added parameter is a failing test — and the manifest's **empty
    `secret_fields`** and this request now cannot disagree, because there is no field to put a secret in.
  - **`Secret` refuses the paste mistake at construction, ordered by actionability.** Empty, oversized,
    whitespace-containing, and control-containing are four separate reasons; the specific case is a **trailing
    newline from a paste**, which reaches the provider as a different string and surfaces as a generic auth
    failure that sends a reader to debug the credential's *validity* instead of its *shape*. A PKCE verifier is
    separately checked against RFC 7636's 43–128 bounds **and** its unreserved alphabet, because a verifier
    outside it produces a challenge mismatch that looks like a PKCE bug.
  - **⭐ FOUR GUARDS FALSIFIED WITH COMPILING MUTANTS.** Ignoring the `error` parameter → **8 tests detected**;
    letting the outage lose precedence to the error code → **2**; never reporting a rotation → **2**; dropping
    the credential-shape whitespace check → **2**. Tree verified clean with `git diff --stat` afterwards.
  - **NEW LIMITS:** **no request is sent and no token has ever been obtained** — there is no transport for a
    form `POST`, and the fixtures do **not** yet include a token response; **the access token deliberately never
    reaches a value, so a caller must read it from the response body itself**, which means those bytes exist
    outside this module's boundary where it cannot enforce anything about them; **no ID-token verification**, so
    the `nonce` `P5-002` carries is still unvalidated; **no DPoP**, which needs a non-exportable key and is a
    decision rather than a header; **`redirect_uri` is not validated here** (the flow owns the registered value,
    so a caller could build a request the flow would refuse); **the parameter list is never tested as a body**,
    because nothing joins it — the encoding is asserted per value and the joining is not; and the **`client_id`
    is only checked for non-emptiness**, so a pasted URL or a project number is accepted here and fails at the
    provider.
  - **This round built revocation — the teardown half of connection setup — and found a contract contradiction.**
    `crates/jarvis-connectors/src/google/revocation.rs` + `revocation_tests.rs` (11 new tests, so **289 in the
    crate**). **`ADR-0065`.**
  - **⭐⭐ THE FINDING: `RevocationKind::requires_reauth_afterwards()` IS WRONG FOR GOOGLE, AND THE SHARED TYPE IS
    RIGHT ABOUT THE PROTOCOL.** `P5-002` reads RFC 7009 correctly: `token_type_hint` **selects which token** is
    revoked, so revoking the access token leaves the grant intact and `AccessToken.requires_reauth_afterwards()`
    returns `false`. Google's own page (last updated **2026-09-14**) says the opposite twice: revoking an access
    token **also revokes the paired refresh token**, and revocation "removes **all** OAuth 2.0 scopes previously
    granted to a **project**, invalidating any issued access or refresh tokens for **all clients registered under
    that project**". So on Google **no** revocation kind leaves the account usable, and a caller that trusted the
    shared `false` would tell a user their connection was disconnected-but-authorised while the account in fact
    needs a new consent. The shared type is **not changed** — it is provider-agnostic and correct — and the
    provider's answer lives in `effect_of`, with a test asserting **both sides and their divergence**, so a change
    to either fails rather than silently re-opening the gap.
  - **⭐ THE LINT IMPROVED THE DESIGN, AND IT WAS A REAL SIGNAL RATHER THAN NOISE.** `clippy::struct_excessive_bools`
    fired on a five-bool `RevokedEffect`, and it was right: `refresh_invalidated: false` could not distinguish
    *the refresh token survived* from *the account never had one* — two facts with opposite consequences, one of
    which would tell a user their refresh token was revoked when it never existed. So `MaterialState` has three
    variants (`Invalidated`, `Absent`, `Untouched`), where `Untouched` is never produced for this provider and
    exists so "we do not know" is representable rather than rounded. Likewise `EffectTiming { Immediate,
    MayTakeTime }` rather than a `bool`: "it might take some time" is the whole content of the distinction.
  - **`requires_reauth` is DERIVED from the grant's removal, not stored beside it.** With the scopes gone there is
    no consent to reuse and no refresh token to mint from, so needing a person follows from `scopes_removed`. A
    separate stored field would be a second value that must agree with the first — the defect class this repo
    keeps recording — and a mutant that hard-codes it is detected by 3 tests.
  - **A `200` means ACCEPTED, not IN FORCE.** "Following a successful revocation response, it might take some
    time before the revocation has full effect." A caller must not treat it as proof that a concurrent call will
    now fail, and **a test that asserted it would be flaky against the provider itself** — which is why the
    timing is a state a caller reads rather than a comment.
  - **An unreachable provider is never reported as revoked.** `parse_revocation_answer` takes `reached`
    **separately from the status**, because a transport cannot say "the provider refused" and "nothing answered"
    with one value. `Unreachable.is_withdrawn()` is `false`, so a `200` that was never received cannot read as
    success — the overclaim direction worth designing against. Likewise a **gateway's `502`** classifies as a
    refusal (the provider *was* reached, through something) and `status_is_documented` lets a caller tell the
    documented `200`/`400` pair from a layer in front of the endpoint.
  - **A `200` cannot report "already invalid", and the gap is stated rather than papered over.** RFC 7009 §2.2
    makes `200` cover both success and "the client submitted an invalid token", so the status carries **no signal
    about whether the token was live**. `AlreadyInvalid` stays reachable out of band but is **not derived from a
    response**, because inventing it from the status would read a fact out of a response that does not carry it.
  - **⭐ FOUR GUARDS FALSIFIED WITH COMPILING MUTANTS.** Hard-coding `requires_reauth` to `false` → **3 tests
    detected**; collapsing `Absent` into `Invalidated` → **2**; claiming an immediate effect → **2**; letting an
    unreachable provider read as `Revoked` → **2**. One first attempt was `VACUOUS` (a move error, not a test
    failure) and was replaced with a compiling mutant — the `VACUOUS`/`FAIL` distinction kept, tree verified
    clean with `git status` afterwards.
  - **NEW LIMITS:** **no request is sent and nothing has been revoked** — there is no transport for a form `POST`,
    so no `200` has been seen and the behaviour is derived from documentation and the RFC; **the divergence is
    documented, not measured** — confirming that Google revokes the paired refresh token would need a live grant
    and deliberately destroying it, so a provider whose *behaviour* differed from its *documentation* would not be
    caught; **`has_refresh_token` is the caller's claim about its own store**, and answering `false` incorrectly
    narrows the reported effect; **`RevocationKind::Grant` cannot be performed in one call to a strictly
    conforming server** (RFC 7009 revokes one token at a time), so relying on Google's single-call behaviour is a
    recorded provider property rather than an encoded one; **nothing handles the multi-account consequence** — the
    module reports the blast radius is the project, and no type holds the set of *other* accounts whose grants
    were withdrawn; **`takes_effect_later` has no duration**, so nothing can schedule a verification or re-check a
    revocation; and **revocation is not wired to anything** — no route, command, or teardown path calls this
    module, so a disconnect is still not something a deployment can perform.
  - **This round fixed the sync advance path, and found that a documented remedy was UNREACHABLE.** `SyncSignal`
    added to `client.rs`, `advance_gmail_history`/`advance_calendar_sync` rewritten to take it, and a false
    predicate replaced (2 new lib tests + 1 new fixture test, so **291 in the crate**). **`ADR-0066`.**
  - **⭐⭐ THE FINDING: `SyncAdvance::HistoryPruned` AND `TokenInvalidated` COULD NOT BE PRODUCED BY ANY INPUT.**
    `SyncAdvance` has a `HistoryPruned` variant whose own doc calls it "the finding this module was written
    around" — and `advance_gmail_history` took only `next_history_id`, never a status or any failure signal, so
    **no caller could reach the resync path at all**. Calendar's `TokenInvalidated` was unreachable the same way.
    This is the "refusal that can never fire" pattern `P5-001`/`P5-003` each recorded, in its **inverse** form: a
    *remedy* that can never be produced. The fix is `SyncSignal { Advanced { history_id }, CursorUnusable,
    Refused(RetryDecision) }`, which also makes the third case explicit so a refusal that is neither an advance
    nor a dead cursor is carried with its classification rather than swallowed.
  - **The signal is a parameter because the INFERENCE DEPENDS ON THE METHOD, and only the caller knows it.** A
    `404` on `users.history.list` may mean pruned history; a `404` on `users.messages.get` means the message does
    not exist, and resyncing on that would discard a whole sync over one missing message. Requiring the signal
    turns a caller's inference into an explicit act instead of a comparison hidden inside a function that never
    saw the request.
  - **⭐⭐ A PREDICATE ASSERTED A DISTINCTION THE PROVIDER DOES NOT PUBLISH — AND RESEARCHING IT PROPERLY MADE
    THAT WORSE, NOT BETTER.** `gmail_history_status_is_pruned` returned `status == 404`, claiming that a 404 on
    `history.list` IS pruned history while Finding 2 says the two causes are indistinguishable. The error guide
    (`handle-errors`, updated **2026-09-15**) settles it the wrong way for the predicate: its status summary
    lists `404 - Not Found` and it then has **NO 404 subsection at all** (its sections are 400, 401, 403, 429,
    5xx), so Google publishes **no `reason` code** for a 404. The ambiguity is **irreducible from the response**,
    so no predicate can resolve it and I removed the one that pretended to. It is replaced by
    `gmail_history_status_cannot_prove_usable`, which claims only what is readable.
  - **⭐ WHY RESYNCING ON AN AMBIGUOUS 404 IS SAFE: THE WRONG READING IS SELF-CORRECTING.** For **both**
    documented causes the first correct action is a full sync — if the history was pruned that is the documented
    remedy, and if the account is gone the full sync's own first call fails and surfaces *that*. So the wrong
    reading costs **one extra `messages.list` call**, while the alternative (resuming from a rejected position)
    yields a store that reports itself in sync while missing everything. Cost asymmetry decides the direction,
    the same reasoning `ADR-0062` uses for an ambiguous transport failure. And a dead cursor now carries
    **no cursor forward**, so a caller cannot resume from the position the provider just rejected.
  - **`429` and `5xx` are explicitly NOT dead cursors.** The predicate is true for `404` alone, asserted false for
    the retryable family — because a resync on a transient failure discards a working store, which is the
    opposite mistake and a far more expensive one.
  - **⭐ THREE GUARDS FALSIFIED WITH COMPILING MUTANTS.** Carrying a rejected cursor forward → **2 tests
    detected**; making the predicate unfailable → **2**; reporting a refusal as an advance → **2**. The
    multi-line anchors needed LF-joined strings built in the script, because here-strings are CRLF — the trap
    this session records, hit again.
  - **⭐ A VERIFICATION-PLAN ITEM WAS FALSIFIED AND THE RECORD SAYS SO.** The plan's "404-is-staleness test"
    required a fixture showing two causes produce opposite outcomes; that comparison needs information the
    response does not carry, so the test **cannot be written as stated**. The item is marked **WRITTEN** and
    **corrected**: the new fixture `gmail_history_404_no_reason.json` asserts the **absence of an `errors` array
    and therefore of a `reason`**, and the research record explains that the discrimination it asked for is
    impossible. A plan item being disproved is a result rather than a failure — `ADR-0063` records the same
    outcome for a different assumption.
  - **NEW LIMITS:** **no request is sent** — the signal is produced by a caller that does not exist, since there
    is no `history.list` transport, no sync loop, and no resync orchestration; **nothing can PERFORM the full
    sync**, so a caller reaching `HistoryPruned` has a verdict and no remedy; **the 404 reading is still an
    inference, now named as one** — the doc argues it is safe because self-correcting, not that the response
    distinguishes the cases, so a provider that changed its 404 behaviour would not be caught; **Calendar's
    `400`-is-a-query-error path still has no fixture**, so nothing pins which reason code accompanies it; and the
    signal does **not** protect against calling the wrong `advance_*` function — a mismatched cursor is caught by
    `SyncCursorKind`, not by the signal.
  - **This round added the `history.list` read — the producer the previous round's signal was missing.**
    `request::{gmail_history_list, parse_history_page, HistoryPage}`, `client::gmail_history_signal`, the
    `gmail_history_list` manifest operation and tool definition, and two fixtures
    (`gmail_history_list.json`, `gmail_history_list_last_page.json`). **4 new lib tests + 1 new fixture test,
    so 295 in the crate and 11 in the fixtures suite.** **`ADR-0067`.** Both new guards were falsified A-B-A
    with compiling mutants and restored byte-identical.
  - **⭐⭐ THE FINDING: `ADR-0066` FIXED THE VARIANT FORM OF THE DEFECT AND REPRODUCED IT AS A PARAMETER FORM.**
    That round widened `advance_gmail_history` to take a `SyncSignal` so the resync remedy became producible —
    but **nothing could build a signal from a response**, because there was no `history.list` request in the
    crate. The only construction sites were test fixtures, so the whole enum was fixture-only. Widening a
    function's signature does not supply the value, and the check `P5-001`/`P5-003` record — *for every variant,
    grep the constructing sites, not the definition* — applies to a parameter as much as to an enum. Applying it
    showed `Advanced` and `Refused` had no producer either.
  - **The asymmetry that carries the weight is the predicate's ORDER.** `gmail_history_signal` checks
    `gmail_history_status_cannot_prove_usable` **first**, so the retryable family (`429`, `5xx`) can never fall
    into the dead-cursor arm. A resync on a transient failure discards a working store — the opposite mistake
    from the one the 404 heuristic tolerates, and the more expensive one. Asserted over `400/403/429/500/502/503/504`.
  - **⭐ TWO DEFECTS IN MY OWN FIRST DRAFT, both found by fetching the live method reference before writing the
    code — and both living in field *descriptions* rather than the example**, the `ADR-0063` lesson hit again.
    (1) I made `history_id` **required** in the output schema, assuming a success always states the new
    position; the reference says the id "can be stored … for a future request" when no `nextPageToken` is
    returned, which is about *when it is usable*, not *when it is present*. Requiring it would have made the
    connector's declaration stricter than the provider's. (2) I nearly treated `historyId` and `nextPageToken`
    as Calendar's mutually-exclusive pair; they are **not** — `historyId` is present on every success while
    `nextPageToken` appears only mid-walk, so a reader copying the Calendar pattern would take its durable
    cursor from the field that expires when the walk ends. The two fixtures are that pair.
  - **`historyTypes[]` is deliberately not offered.** The parameter filters the change kinds returned, so a sync
    using it would silently drop the kinds it excluded; the general `messages` field is populated on every
    change and is read instead. The operation also gets its **own** rate-limit entry, because `history.list`
    costs **2** quota units against a message read's 20 and reusing the read limit would over-state a sync.
  - **Falsified, two guards A-B-A with compiling mutants.** `gmail_history_signal`'s 404 arm → `if false`
    (detected by `a_history_status_becomes_the_signal_the_cursor_decision_consumes`); `parse_history_page`'s
    status check → `if false` (detected by `a_history_page_keeps_the_cursor_and_the_page_token_apart`). Both
    restored and verified byte-identical with the mutant string absent from the file.
  - **NEW LIMITS:** **no request is sent and no response has been parsed from Google** — the transport slice is
    still unbuilt, so these tests prove the layer implements the *record*; **the operation is registered
    nowhere**, so no model can reach it; **the signal producer has no caller** — a sync loop would be the
    consumer, and no sync loop exists; the fixtures are hand-built, so a shape Google sends but does not
    document is still invisible; and **`history_types` filtering and the `labelId` parameter are unimplemented**
    rather than absent-as-a-decision.
  - **This round built the transport implementation — the limit every previous slice of `P5-005` recorded.**
    `crates/jarvis-connectors/src/google/http.rs` + `http_tests.rs` (6 new tests, so **301 in the crate**),
    `reqwest` declared on the crate (resolving to the already-locked `=0.13.5`, so **no new package**), and a
    `#[cfg(test)]` `HttpRequest::rebase_to` seam so a transport test drives a **real product request** against a
    loopback server. **`ADR-0068`.** **50 suites / 1566 workspace tests, 0 failed, 0 ignored.** Both new guards
    falsified A-B-A with compiling mutants and restored byte-identical.
  - **⭐⭐ THE FINDING: `ADR-0062`'s PORT REQUIREMENTS WERE UNENFORCEABLE AND READ AS SATISFIED.** The port listed
    four things an implementation "must not do" — follow a redirect, retry, read a proxy from the environment,
    return an error for a non-2xx — and the only implementations were **test doubles**, which have no redirect
    policy, no proxy configuration and no retry to disable. So all four were true **by construction** and none
    was tested. The crate's own limits admitted this ("they become testable only when a real transport is
    written"), which is the honest form — but a requirement that *reads* as met is worse than one nobody wrote
    down. `ADR-0067`'s lesson (a parameter with no producer) applies to a **constraint with no implementation**.
  - **The ordering in the failure map is the substance, and it is not the obvious order.** `classify_error`
    checks `is_timeout()` **before** `is_connect()`, although `is_connect()` is the more specific answer where
    both apply. The reason is the *failure direction*: a timeout is **ambiguous** (the request may have been
    written), so choosing it where the certain variant might also fit can only make a caller *less* willing to
    retry — whereas the reverse mistake would let a non-idempotent effect repeat. A pure connect failure is not
    a timeout, so DNS and refused-connection failures still report `Connect` and the specific case is not lost.
    `Send` is the final arm, because an unrecognised failure is exactly where certainty is unwarranted.
  - **A branch was deliberately NOT written, and the reason is a recorded defect class.** There is no
    `error.is_redirect()` arm: with `Policy::none()` no redirect produces an error, so the arm could never fire
    — the unreachable refusal `P5-001` and `P5-003` each recorded. The redirect is instead classified by the
    caller from the `302` the transport now *returns*, which is also the control that proves nothing was
    followed: **the redirect target server is asserted to have received zero connections**.
  - **Four port requirements became controls with tests**, each against a hand-written HTTP/1.1 server (the
    `jarvis-mcp-transport` precedent — a framework would share assumptions with the client under test): no
    redirect followed; no retry (exactly one request per `send`); no environment proxy (`no_proxy()` explicit,
    because `reqwest`'s `system-proxy` default is ON); and a `403` arriving as a `TransportResponse` rather than
    an error. The timeout case is driven by a server that accepts and **never answers**, so the client's own
    deadline is what ends it rather than an error the test invented.
  - **The dependency was measured before it was declared.** `reqwest` resolves to the `=0.13.5` already in the
    lock file through `jarvis-cli`, `jarvis-models` and `jarvis-mcp-transport`, so `cargo tree` shows one new
    edge and `Cargo.lock` gained **one line** — no package joined the tree, and `cargo deny check` is
    advisories/bans/licenses/sources **all ok**. `default-features = false` with `rustls` keeps the
    bundled-TLS policy.
  - **Falsified, two guards A-B-A with compiling mutants.** The timeout-first ordering → `if false`, detected
    by `a_refused_connection_is_certain_and_a_timeout_is_not` (`left: Send, right: Timeout`); the redirect
    policy → `Policy::limited(10)`, detected by
    `a_redirect_is_not_followed_and_the_target_is_never_contacted` (`left: 200, right: 302`). Both restored and
    verified byte-identical.
  - **NEW LIMITS:** **no request has been sent to Google and no Google response has been parsed** — every
    response is written by the test file, so these tests prove the transport's own controls rather than the
    record; **the transport has no production caller**, because no composition root constructs the connector
    (the daemon owns that); **no body-size bound**, since `TransportFailure` has no "too large" variant and a
    streaming cap needs `P5-009`'s output policy; **no token refresh**, so a stale token becomes a provider
    refusal; **no `Retry-After` interpretation** (it is carried and never acted on); and the timeout/connect
    pair is a **JARVIS choice**, because the port requires a bound while Google publishes no deadline.
  - **⚠ AND THE DEPENDENCY COST A LOCAL CHECK, WHICH IS RECORDED RATHER THAN WORKED AROUND.** `reqwest`'s `rustls`
    backend pulls `aws-lc-sys`, so `cargo clippy -p jarvis-connectors --target x86_64-unknown-linux-gnu` now
    fails with `ToolNotFound` even with the zig linker shim — the crate joins `jarvis-models` in the set that
    **cannot be cross-linted locally**. Native CI still lints it on all three OSes, so this narrows local
    verification rather than the gate; a `#[cfg(unix)]`-only defect would now surface only on CI.
  - **This round tested the SEAM — the join that had no test in either half.** `http_tests.rs` gained 4 tests (so
    **305 in the crate**) driving `GoogleReadTool` against a real `ReqwestTransport` over a socket, plus two
    recorded `#[cfg(test)]` seams: `HttpRequest::rebase_to` and `GoogleReadTool::run_with_origin`. **`ADR-0069`.**
    **50 suites / 1570 workspace tests, 0 failed, 0 ignored.**
  - **⭐⭐ THE FINDING: EVERY PART HAD A GREEN TEST AND THE JOINT HAD NONE.** The adapter was tested against a
    **`Scripted`** transport; the transport was tested against a **hand-written request** it was handed. Neither
    had ever met the other, so a URL the adapter builds that the transport sends to the wrong place — or a
    header one sets and the other drops — was invisible to both suites. This is `P3-006a`'s shape (each slice
    self-consistent; the defect lives between two correct modules) and the third variant of the same class this
    phase has found: `ADR-0067` a variant with no producer, `ADR-0068` a constraint with no implementation,
    `ADR-0069` a **joint with no test**. **Rule: for every pair that must agree, ask which test drives them
    TOGETHER.**
  - **⭐ THE CROSS-OPERATION TEST IS THE ONE WITH THE TEETH, and the single-operation seam tests would not have
    caught a mis-route.** Those assert that the expected data came back — and an operation wrongly routed to
    another endpoint that still answered with a parseable body would satisfy them. So one test drives **all
    four** operations and compares a **path per operation**. Verified rather than asserted: mutating the history
    builder's path to `/users/me/messages` failed with
    `google.gmail_history_list must address /gmail/v1/users/me/history, got /gmail/v1/users/me/messages?startHistoryId=12345`.
  - **⭐ TWO ROUTING MUTANTS WERE `VACUOUS`, NOT `FAIL`, AND THAT IS ITSELF A FINDING.** Pointing
    `gmail_history_list` at the messages *builder* failed to compile twice (`?` has incompatible types; 3
    arguments vs 4), because changing a builder's identity changes its argument types. So a routing mistake
    **inside** one module is largely unrepresentable — which is exactly why the seam (where the routing is
    *selected* and the request is *sent*) is where the defect can live. The `VACUOUS`/`FAIL` distinction kept the
    reading honest rather than counting a compile error as a detection.
  - **Both seams are `#[cfg(test)]` on BOTH sides**: `apply_origin` has a `#[cfg(not(test))]` sibling, so a
    shipped build contains no reference to the test origin at all — a single branch would have forced the seam
    to exist in every build for a test's sake.
  - **`HttpRequest`/`GoogleReadTool` assertions on the wire**: method, the encoded query (`q=is%3Aunread`,
    `maxResults=10`), the `Accept` header, a `Bearer ` authorization header, and no credential in the URL. The
    request **target** includes the query, so paths are compared with `split('?').next()` — the same split
    `url()`/`url_with_query()` makes; my first assertion compared the whole target and was wrong, not the code.
  - **NEW LIMITS:** still **no request to Google** — every response is written by the test file, so this proves
    the seam and not the record; the transport still has **no production caller** (no composition root builds
    the connector); the live smoke test remains unwritten.
  - **This round built the callback reader — the listener half of the flow had no implementation.**
    `Callback::from_request_target` in `authorization.rs`, plus `FormParameters` and `form_decode`, and two new
    `AuthRefusal` variants (so **310 in the crate**, 5 new tests). **`ADR-0070`.** **50 suites / 1575 workspace
    tests, 0 failed, 0 ignored.** Two guards falsified A-B-A with compiling mutants and restored byte-identical.
  - **⭐⭐ THE FINDING: THE CALLBACK DECODING IS THE OPPOSITE OF THE REQUEST ENCODING, AND THE MISTAKE LOOKS
    LIKE AN ATTACK.** RFC 6749 §4.1.2 says the response parameters are added "using the
    `application/x-www-form-urlencoded` format, per Appendix B" — where a space is **`+`**. The request side
    (`ADR-0060`) encodes a space as `%20` **and a literal `+` as `%2B`**, deliberately, because RFC 3986 has no
    form semantics. So the same crate holds both rules and they disagree about one character. A `state` of
    `a b` arrives as `state=a+b`, and a decoder that kept the `+` would compare `a+b` against `a b` and refuse
    a **legitimate** response — surfacing as `StateMismatch`, i.e. as a possible forgery, while the cause is one
    character of decoding. **The worst shape a security refusal can take is a false one that reads as an
    attack.**
  - **⭐ A TEST FORCED A VARIANT SPLIT, and the failure was the finding.** `CallbackUnparsable` was one variant
    for both "the bytes could not be read" and "a parameter was repeated", and the codetable test failed on
    `indicates_forgery` (`left: true, right: false`) because one variant had to answer for both. They are now
    `CallbackMalformed { reason }` (a listener defect — a truncated escape, invalid UTF-8, or a target that
    names no loopback redirect) and `ParameterRepeated` (the shape an appended value takes). `indicates_forgery`
    is `true` only for the four mismatch/repeat variants. **`clippy::struct_excessive_bools`' lesson again: two
    values standing for more than two situations must be an enum.**
  - **A repeated parameter is REFUSED, not last-wins.** RFC 6749 §3.1 and §3.2 both require a parameter "MUST
    NOT be included more than once", and taking the last value is exactly how a `state` check is defeated —
    the server's own value comes first and an appended one second. A malformed escape or invalid UTF-8 is
    **refused rather than lossily decoded**, because a lossy decode turns a corrupted `state` into a *different*
    string that merely fails to match, hiding a transport fault behind a security refusal.
  - **`Callback`'s own doc claimed the decode lived elsewhere — and that division left it UNWRITTEN.** Every
    `Callback` in the tree was hand-assembled in a test, so no code here had ever read a request target. That is
    `ADR-0068`'s shape (a requirement with no implementation) and `ADR-0069`'s (a joint nobody drives) one layer
    further out: the *listener* had no reader. The `received_on` value is recovered **portless**, because an
    origin-form target carries no port — the `Host` header does — and `consume` still compares it against the
    transaction's ported registration with `matches_except_port`, so the port check is not lost, only moved to
    where both values exist.
  - **⚠ ONE OF MY OWN TEST PREMISES WAS WRONG AND THE FAILURE SURFACED IT.** I asserted a non-loopback target
    (`https://evil.example/cb?state=x`) would be refused; it is **accepted**, because the prefix check strips
    `http://` and the rest parses as a *path*. The claim was too strong; it was replaced by the accurate one
    (a target naming no loopback redirect is refused, asserted through the absolute-form case that genuinely
    fails) rather than by weakening the code.
  - **NEW LIMITS:** **no listener is bound**, so a callback has still never arrived over HTTP — what is closed
    is that the decode exists and is tested; the **form-POST transport for the token exchange** is still
    unbuilt, so `token.rs`'s exchange remains caller-driven; and nothing wires `from_request_target` into a
    server, because a loopback listener belongs to the composition root.
  - **This round moved the codec to one module and finished the rule — the encoder half was still missing.**
    New `crate::form` (`encode_component`, `decode_component`, `encode_body`, `CONTENT_TYPE`) with
    `form_tests.rs` (7 tests, so **318 in the crate**), and `authorization.rs`'s local `form_decode` deleted in
    favour of the shared decoder. **`ADR-0071`.** **50 suites / 1583 workspace tests, 0 failed, 0 ignored.**
    Two guards falsified A-B-A with compiling mutants and restored byte-identical.
  - **⭐⭐ THE FINDING: THE ENCODING RULE IS NARROWER THAN `ADR-0070` RECORDED — the alphabet is
    ALPHANUMERICS ONLY.** `ADR-0070` established that a space is `+` rather than `%20`, and stopped there.
    HTML 4.01 §17.13.4 (which RFC 6749 Appendix B points at) actually says: "Space characters are replaced by
    `+', and then reserved characters are escaped ... **Non-alphanumeric characters are replaced by `%HH`'"**.
    So `~ - . _` are escaped too (`%7E %2D %2E %5F`), where RFC 3986's unreserved set — and this crate's
    `percent_encode` — leave them alone. The **decoder** accepted them either way, so round one could not see
    it; the gap appears only when an encoder exists. Escaping *more* is interoperable in both directions, while
    escaping less produces a value the server reads differently, so the wider escape is the safe one.
  - **TWO IMPLEMENTATIONS OF THE SAME ONE-CHARACTER RULE WERE ABOUT TO EXIST.** The token endpoint's form
    `POST` body needs this encoding in the **opposite direction** from the callback, so writing the encoder
    inside `token.rs` would have put a second copy of the space rule where the callback already had one —
    `P3-006a`'s "two values that must agree, with nothing holding both", for a codec rather than a struct. One
    module now holds both directions, and a test asserts the form encoder and `percent_encode` produce
    **different** output for the same input, so a refactor that merged them fails.
  - **⚠ MY OWN NEW TEST PREMISE WAS WRONG AND THE FAILURE SURFACED IT, exactly as in `ADR-0070`.** I wrote
    `encode_body`'s expectation as `"grant_type=authorization_code&code=a+b"` — with the parameter **names
    unescaped**. The code produced `"grant%5Ftype=authorization%5Fcode"`, and the code was right: the rule is
    stated about "control names **and** values". A named test now covers a name holding `&` and `=`
    (`"a&b=c"` → `"a%26b%3Dc"`) and fails if only values are encoded. **The reasonable-looking expectation is
    the one to check.**
  - **⭐ THE MODULE IS EXPORTED BECAUSE THE ENCODER HAS NO CALLER YET, AND CLIPPY SAID SO.** `pub` items inside
    a private module are unreachable and therefore dead code — the defect `P5-001` recorded and this crate hit
    when `google.rs` was private. Marking the encoder `#[cfg(test)]` instead would have hidden a function the
    next slice is meant to use, so the module is `pub` with the reason in its own doc.
  - **Falsified, two guards A-B-A with compiling mutants.** The space rule → a literal space (detected by
    **three** tests, incl. `left: "code=a b", right: "code=a+b"`); the name escaping → the raw name
    (detected by **two**, incl. `left: "a&b=c=v"`). One attempt printed `NOCHANGE` because the formatting had
    been refactored — the string-changed probe caught that rather than reporting a false pass.
  - **NEW LIMITS:** the **encoder has no caller** — `encode_body` is exercised only by its tests, because the
    token endpoint's form `POST` transport is unbuilt; HTML 4.01's line-break normalisation is deliberately
    **not** performed (a codec that rewrote the bytes would make a caller's refusal unobservable, and no OAuth
    parameter admits CR or LF anyway); and the codec does not validate what it encodes, so a value that is
    invalid for OAuth (a line break, an over-long token) encodes faithfully and is refused — or not — by the
    caller that owns the rule.
  - **This round found and fixed a LATENT double-encoding defect in the token exchange, one round after the
    encoder that would have exposed it was written.** `ExchangeIdentity::parameters()` no longer pre-encodes;
    `token::{body, content_type}` and `ExchangeIdentity::endpoint()` are added, and the test that asserted the
    pre-encoded form is **reversed** (3 new tests, so **320 in the crate**). **`ADR-0072`.** **50 suites /
    1585 workspace tests, 0 failed, 0 ignored.** One guard falsified A-B-A with a compiling mutant.
  - **⭐⭐ THE FINDING: THE TOKEN EXCHANGE WAS ESCAPING ITS VALUES WHERE THEY WERE STORED, NOT WHERE THEY WERE
    RENDERED — a double-encoding trap with no caller.** `ExchangeIdentity::parameters()` ran
    `percent_encode` over `client_id` and `redirect_uri`, and a test asserted `redirect_uri ==
    "http%3A%2F%2F127.0.0.1%2F"`. `ADR-0071`'s `encode_body` escapes everything it is given, so the two together
    produce `http%253A%252F%252F127.0.0.1%252F` — and Google answers a double-encoded `redirect_uri` with
    **`redirect_uri_mismatch`**, which names **client registration** rather than the encoding. **A fault whose
    symptom is one layer away from its cause is the one that costs a day.**
  - **⭐ THE DEFECT WAS LATENT UNTIL THE ENCODER EXISTED, AND THAT IS THE ARGUMENT FOR BUILDING THE RENDERER
    BEFORE THE TRANSPORT.** Nothing built a body, so the wrong-encoding path had no caller and the crate was
    green for several rounds. The moment `encode_body` landed, the two layers disagreed. Same family as
    `ADR-0068` (a requirement with no implementation) and `ADR-0069` (a joint nobody drives) — here the two
    halves are the *producer* and the *renderer* of one value, and **only a pipeline test covers them.**
  - **The rule this makes explicit: a value is encoded where it is RENDERED, never where it is STORED.** A list
    of values with an encoding already applied is a *half-rendered request*: every consumer must know whether
    it has been rendered, and the one that guesses wrong produces this failure. The general assertion is now
    part of the tests — **a `%25` anywhere in a rendered form body means some value was escaped where it was
    stored** — and it is checked for both the identity parameters and the whole exchange.
  - Two accessors make the rendering layer reachable without a socket: `token::body(&[(String, String)])` and
    `token::content_type()`, plus `ExchangeIdentity::endpoint()`, which takes the host **from the manifest**
    rather than restating it. That preserves `P5-004`'s recorded oddity — the consent screen and the exchange
    are on **different hosts** (`accounts.google.com` vs `oauth2.googleapis.com`), so a reader "tidying" them
    into one constant would break every exchange — and a test asserts the literal.
  - **Falsified, one guard A-B-A with a compiling mutant.** Restoring the pre-encoding was detected by **two**
    tests, and the failure printed the whole doubled body (`redirect%5Furi=http%253A%252F%252F127%2E0%2E0%2E1%252F`)
    so the message names its own cause. Restored byte-identical.
  - **NEW LIMITS:** nothing sends the request — `HttpMethod` has one variant, so a form `POST` is not
    expressible by the read transport and the caller still drives the exchange; the endpoint, parameter lists,
    body and answer reader are all present and tested, so the transport slice is now a thin binding; and the
    `client_id` is still only checked for non-emptiness, so a pasted URL is accepted here and fails at the
    provider.
  - **This round built the exchange over a transport — and `TokenRequestOutcome` finally has a producer.**
    `request::FormRequest`, `GoogleTransport::send_form` with its `reqwest` arm, `token::exchange`, and
    `TransportFailure::reason` (a `const fn`, because `NeverSent` holds a static reason); a new
    `token_exchange_tests.rs` (6 tests, so **326 in the crate**). **`ADR-0073`.** **50 suites / 1591 workspace
    tests, 0 failed, 0 ignored.** Two guards falsified A-B-A with compiling mutants and restored byte-identical.
  - **⭐⭐ THE FINDING: `TokenRequestOutcome` WAS DEAD CODE WEARING A CONTRACT, AND TWO THINGS BLOCKED IT.** Its
    own doc called the `NeverSent`/`SentAnswerUnknown` distinction consequential (RFC 9700 §4.2.4: a retry of a
    lost-answer request may consume the code **and destroy a working grant the first attempt issued**), and
    **every construction site was in `token_tests.rs`** — the `P5-001` defect, and the third instance this
    phase of *a declaration nothing produces* (`ADR-0067` the variant form, `ADR-0068` the constraint form,
    this the **outcome** form). The second blocker was structural: the exchange is a form `POST` and
    `HttpMethod` deliberately has one variant, so the port could not express the method — the type had neither
    a producer nor the **ability** to have one.
  - **⭐ A SEPARATE `FormRequest` TYPE, because the two request shapes have OPPOSITE credential boundaries.**
    `HttpRequest` exists so it **cannot** hold a credential in its URL (`ADR-0060`); a token request's body
    **is** the credential-bearing text. A union would either give `HttpRequest` a field a credential goes in or
    force every caller to prove which kind it holds. `FormRequest` takes the **already-rendered** body, so it
    cannot encode and therefore cannot double-encode (`ADR-0072`), and its `Debug` is hand-written to
    `[REDACTED]` + a character count with **no `Display`** — `ADR-0061`'s rule, since a derived `Debug` would
    render the body through any `{:?}`. A test asserts the rendering and the count.
  - **⭐ `send_form` IS A SECOND PORT METHOD, NOT A THIRD `HttpMethod` VARIANT.** `HttpMethod`'s own doc says
    adding a write is `P5-009`'s decision; this is not that — it is the token endpoint's *framing*, which
    authenticates by its body's `client_id` and has **no bearer token**. Folding it into `send` would give the
    read path a body it does not have and the token path a credential parameter it does not use: one method
    with two disjoint modes. Both test doubles gained a `send_form` arm that **panics** when the wrong path
    calls it, so a mistake is a test failure rather than a silent success.
  - **⭐ `reqwest`'s `form` FEATURE IS DELIBERATELY NOT ENABLED.** `.form()` exists but is feature-gated, and
    enabling it would add the dependency's form encoder beside `crate::form` — two implementations of the one
    character `ADR-0071`/`ADR-0072` exist to keep single, disagreeing **invisibly** because both produce a
    plausible body. So the request is built with `.body(rendered)` and an explicit `Content-Type`, and a test
    asserts the recorded body contains no `%25`.
  - **The retry-safety cases are asserted AGAINST `may_have_reached_the_provider`, not against a hand-written
    list of which failures are ambiguous.** A new `TransportFailure` variant therefore cannot be classified by
    omission: the fixture fails first if the predicate and the expectation disagree. And an unreadable body is
    an **`Err`**, not `Refused` — folding it in would send a user to a consent screen when the fault is a body
    this client cannot parse, which is a different thing from the provider answering "no".
  - **Falsified, two guards A-B-A with compiling mutants.** Collapsing the ambiguity to `NeverSent` was caught
    with `Send may have been written, so a retry could repeat an effect` (`left: NeverSent` / `right:
    SentAnswerUnknown`); turning an unreadable body into a refusal was caught by the exchange test. Restored
    byte-identical.
  - **NEW LIMITS:** **the exchange has no caller** — nothing constructs a `FormRequest` outside a test, because
    no composition root builds the connector, so a live call needs credentials and a composition root rather
    than more code here; the `refresh` path is not yet driven through `exchange` (only the code exchange is);
    and the request carries no deadline of its own, so it relies on the client's configured timeout.
  - **This round gave the refresh path a producer — and found a doc claiming a delegation the code never made.**
    `token::refresh_with` (so `RefreshExchange` has a caller outside a test), the shared `send_and_parse` join
    plus an `ExchangeFailure` enum keeping transport-failure and unreadable-body apart, and `classify_refresh`
    reduced to a one-line delegation; the private `refresh_outcome` copy deleted. 4 new tests, so **330 in the
    crate**. **`ADR-0074`.** **50 suites / 1595 workspace tests, 0 failed, 0 ignored.** Two guards falsified
    A-B-A with compiling mutants.
  - **⭐⭐ THE FINDING, AND IT WAS FOUND BY A MUTATION SURVIVING: `classify_refresh`'s DOC NAMED THE SHARED
    CLASSIFIER AS THE AUTHORITY AND ITS BODY CALLED A PRIVATE COPY.** Its own text said "The classification
    itself is [`RefreshExchange::classify`]'s", while the code called `refresh_outcome(&self, ..)` — a local
    `match` restating **both** rules the shared module owns: the rotation test, *with its own copy of the
    "arrival, not storage" comment*, and the transient-first ordering, *with its own copy of the "a 503 can
    carry `invalid_grant`" comment*. Two implementations of an ordering whose whole content IS the ordering,
    and a doc naming the other as the authority. This is `P5-004`'s class: **a doc saying "the rule lives in
    X" is a claim about code, and X has to be read.**
  - **⭐⭐ A SURVIVING MUTATION IS NOT ALWAYS A WEAK TEST — it can be a ROUTING signal, and the two are told
    apart by checking REACHABILITY.** The mutant was `if failure.transient` → `if false` **in `token.rs`**, and
    it survived every test including the one written to assert that ordering. A weak test and a wrong path look
    identical from the result; the way to distinguish them is to ask whether the mutated code is reachable from
    the assertion — **grep its CALLERS, not its definition**. After the dedup the same mutant fails with
    `left: Expired, right: Transient`. So the test was sound and only blind while production took another path.
  - **⚠ A FALSIFICATION CAN REPORT A MISLEADING RESTORE, AND MINE DID.** One probe printed `RESTORED=True`
    while a **second** mutant was still in the file: the `.bak` had been captured after the first mutation was
    applied, so the "byte-identical" check compared against a corrupted baseline. It surfaced as three failing
    tests **after** the refactor. **Verify the mutant TEXT is absent (`Select-String`) rather than trusting a
    restore flag that compares to a snapshot you may have taken at the wrong moment.**
  - **The `#[cfg(test)]` import is now a compile-time witness.** `RefreshOutcome` is needed by this module only
    through the shared classifier, so its import is test-only; if a future edit reintroduces a local
    classification the import becomes unused and the build fails. Clippy caught the first version either way.
  - **⭐ THE TRANSPORT-FAILURE ASYMMETRY, asserted through the producer rather than the classifier.** A refresh's
    ambiguous transport failure is `Transient` — and that is **safe** because a refresh presents the *stored*
    token: if a rotation silently landed, the stored reference is invalid and the next attempt answers
    `invalid_grant` → `Expired`/`Revoked` → `needs_user`. So it **self-corrects over one extra call**, where
    retrying a lost *code* exchange could revoke tokens. The test drives both steps rather than arguing the
    point, and records that a rotation arriving unread is a limit of `RefreshOutcome`'s vocabulary.
  - **Falsified, two guards A-B-A with compiling mutants.** The rotation rule, mutated to read the caller's
    storage (`response.has_refresh_token && new_reference.is_some()`), caught with `left: Refreshed, right:
    Rotated`; the transient ordering, caught as above. The first attempt at each targeted the wrong FILE
    (`google/token.rs`, where the copy lived) — which is itself the evidence for the finding.
  - **NEW LIMITS:** **`refresh_with` has no caller outside its tests** — the composition root owns the secret
    store, so minting and storing a rotated reference is still unbuilt; a **rotation that arrives unread** is
    invisible until the next attempt (a `RefreshOutcome` vocabulary gap, not a defect in this function); and
    the refresh branch is reachable only through the producer, so `exchange`'s own refresh handling is
    untouched by these tests.
  - **This round wired the retry-classification table into the refusal path — it had no production caller.**
    `google::operations::refusal` now calls `client::classify` and appends the class and the stated delay to the
    bounded reason; 2 new tests (so **332 in the crate**). **`ADR-0075`.** **50 suites / 1597 workspace tests,
    0 failed, 0 ignored.** Two guards falsified A-B-A with compiling mutants.
  - **⭐⭐ THE FINDING: THE TABLE `ADR-0058` EXISTS FOR DECIDED NOTHING.** `client::classify`'s own doc states
    the case for its existence — "**a 403's meaning comes from its `reason`, never from the status** … a
    classifier switching on the status alone would retry an administrator's decision" — and **nothing in
    production called it**. `refusal` read the machine-readable reason code and formatted it, so `RetryClass`
    and `RetryGuidance` reached nothing, and **`TransportResponse::retry_after_seconds` was carried, validated,
    documented as "whether a stated delay is honoured is `client::classify`'s decision", and then dropped**. A
    `429` with `Retry-After: 37` produced a reason naming neither the class nor the delay. This is the fourth
    form of *a declaration nothing produces* this phase: the variant (`ADR-0067`), the constraint (`ADR-0068`),
    the outcome (`ADR-0073`) and now the **policy table**.
  - **⭐ THE DELAY IS APPENDED ONLY WHEN `guidance.delay_seconds()` IS `Some`, and the test proves it with a
    CONTROLLED case.** The permanent/authentication/reconcile arms carry no seconds, so a refusal that must not
    be retried cannot show a delay **even when the wire carried one** — a caller seeing "retry after 60s"
    beside `domainPolicy` would back off and retry an administrator's decision. The test sends a
    `403 domainPolicy` **with** `Retry-After: 60` and asserts the absence, which is the case a naive
    "append the header" implementation gets wrong.
  - **The class rides in the reason because `jarvis-tools`' `AdapterError` has no field for a connector's own
    retry vocabulary** — a real constraint rather than a preference, and the reason `ToolOutcomeRecord`'s
    bounded reason is the one channel that reaches a caller. Verified shape:
    `the provider refused the call: rateLimitExceeded (throttled); retry after 37s`.
  - **An unreadable body is still classified by its status**, so a `503` with HTML stays `provider_fault` and an
    unrecognised `418` says `unknown` **and carries no delay** (`Reconcile` offers none, because a retry is not
    the action). That extends "we know little versus we know nothing" to the retry decision.
  - **Falsified, two guards A-B-A with compiling mutants.** Dropping the stated delay (caught, and the failure
    printed the fallback floor `retry after 1s`, so the mutation was a genuine loss); removing the class from
    the reason (caught, printing `rateLimitExceeded ()`).
  - **NEW LIMITS:** **`RetryDecision::provider_request_id` is still unpopulated in production**, and that is a
    real gap rather than an oversight: `TransportResponse` carries no headers at all. (A previous version of
    this limit asserted that Google "returns it in a response **header**"; Gmail's `handle-errors` guide, read
    2026-09-15, names **no request-id header**, so that was an assumption stated as a finding — corrected in
    `ADR-0076`. Whether Google supplies one is **unverified**.) A `TransportResponse` change with its own
    falsifying test is what it needs, not a fix here; the reason is a
    **rendering** of the decision rather than the decision itself, because `ToolExecutor`'s contract has no
    place for a connector's vocabulary (ADR-0047's boundary); and `GOOGLE_RETRY_FLOOR_SECONDS` is a JARVIS
    choice where Google publishes a range, so a delay of that value is indistinguishable in the reason from a
    provider-stated one.
- [ ] `P5-005` **(continued — `Retry-After` is three situations, not two)**: `TransportResponse::retry_after` is
    `Option<RetryAfter>` (`Seconds(u32)` | `NotSeconds`) instead of `Option<u32>`, both `reqwest` sites use the
    new `google::transport::parse_retry_after`, and `client::classify`'s `429` arm keeps *absent*, *stated as
    seconds*, and *stated but unreadable* apart, rendering the third as
    `… (throttled); retry after 1s (the provider stated a delay this client could not read)`. `RetryGuidance`
    gained `BackoffAfterUnreadableDelay`. **1 new test (so 333 in the crate).** **`ADR-0076`.** Two guards
    falsified A-B-A with compiling mutants.
  - **⭐⭐ THE DEFECT: TWO VALUES STANDING FOR THREE SITUATIONS.** `RFC 9110` §10.2.3 defines
    `Retry-After = HTTP-date / delay-seconds`, and `Option<u32>` cannot tell *the header was absent* from
    *the header was present in the date form*. The transport parsed digits only, so a conforming `HTTP-date`
    became `None` — **the same value as no header at all** — and both fell to the floor. That is the direction
    that retries **too soon**, and §5.6.7 makes the date form one a recipient **MUST accept**. The old test had
    already recorded the shape of the gap in the words chosen for it — "an unreadable value is **absent**, not
    zero" — which was true of the old type and is exactly the conflation the new type removes. Fifth form of
    *a declaration whose values do not cover its situations* this phase, and the first **value** form.
  - **⭐ THE VARIANT IS NAMED FOR WHAT IS READABLE, NOT FOR THE CONCLUSION.** `HttpDate` was the obvious name
    and would be a **lie**: `delay-seconds = 1*DIGIT` has **no upper bound**, so
    `Retry-After: 99999999999999999999` is `delay-seconds` by grammar and does not fit a `u32` — a plain
    `parse::<u32>().ok()` returns `None` for it, which reads as "retry now". Both land in `NotSeconds`, because
    the only distinction a retry decision needs is *"can this client express the stated delay in seconds"*;
    naming it after the diagnosis would have hidden the case where the diagnosis is wrong.
  - **The wait is the floor for both non-numeric cases; the WORDS are what distinguish them.** A `RetryGuidance`
    variant rather than a different number, because honouring the date form needs a clock and a transport that
    read its own clock would make the retry decision the port's own doc says it must not. **Refusing** to retry
    was rejected (throttling is the most retryable class); a **silent** fallback was rejected because it presents
    a JARVIS floor as the provider's instruction.
  - **The correction that makes this slice's record honest.** `ADR-0075` asserted Google "returns the identifier
    in a response **header**". Re-fetching the source it was written from (Gmail `handle-errors`, via
    `developers.google.com`): the page names **no request-id header**, so that was an **assumption stated as a
    finding**. Corrected in `ADR-0075`, here, and in the code comment; **whether Google supplies one is now
    recorded as unverified** rather than as a premise. No reader was built — settling an unverified name first
    is the point.
  - **An empty value is absent.** Both grammars require content (`1*DIGIT`, `HTTP-date`), so a present-but-empty
    value carries nothing to interpret. The one case where "present" and "absent" legitimately coincide.
  - **NEW LIMITS:** the date form is **recognised but not converted to a delay** — the conversion needs a clock
    and a policy for a skewed or hostile one (RFC 9110 §8.8.1 is explicit that a validator is not a trust
    mechanism), so it is a separate decision with its own falsifying test. `provider_request_id` remains
    unpopulated **and unverified** as above. `parse_retry_after` accepts exactly `1*DIGIT` — not `+30`, not
    `30.5` — which matches the grammar but means a provider that sent a signed or fractional value lands in
    `NotSeconds` rather than being coerced.
- [ ] `P5-005` **(continued — a bound that is documented but not applied is not a bound)**: `RetryGuidance`
    gained `DeferSeconds(u32)` plus a `deferred_seconds()` accessor, and `RetryGuidance::for_stated_delay` is the
    **one** path from a provider's stated number to retry guidance, applying `MAX_RETRY_AFTER_SECONDS`. The dead
    `RateLimitError::RetryAfterTooLong` variant was removed. `classify`'s `429` arm uses the constructor, and the
    reason renders the over-ceiling case as `defer for 18000s (above the 3600s this caller will hold)`. **2 new
    tests (so 334 in the crate).** **`ADR-0077`.** Two guards falsified A-B-A with compiling mutants.
  - **⭐⭐ THE FINDING: `MAX_RETRY_AFTER_SECONDS` WAS A RULE NOTHING APPLIED.** Its doc states a *rule*, not a
    figure — "a longer value is **refused** rather than clamped: a clamp would silently retry **sooner** than the
    provider asked" — and a repository search found the identifier in exactly **three** places: its definition,
    a `pub use`, and a doc link. **No code read it.** The companion `RateLimitError::RetryAfterTooLong` made the
    same claim ("enforced by the constructor a caller would use") and had **no constructor**; its only
    construction in the tree was inside a test that asserted the message **formatting**, which passed for as long
    as the type existed and proved nothing. **A construction that exists only in a test is the signature: the
    type can hold the value, no code produces it.**
  - **⭐ `ADR-0076` made the gap load-bearing.** Routing the provider's stated delay into `RetryAfterSeconds`
    *without* the bound meant a `429` answering `Retry-After: 18000` produced `retry after 18000s` — a **five-hour
    wait presented as an ordinary retry delay**. Google documents exactly this: a daily-limit `429` "might result
    in these errors for multiple hours". So this is a new form of the phase's recurring defect: not a *type* that
    can hold a value nothing produces (`0067`/`0068`/`0073`/`0075`/`0076`), but a **constant whose stated rule
    nothing applied** — declaration and behaviour one comment apart, the comment the only thing that knew.
  - **`DeferSeconds` answers `None` from `delay_seconds()` — and that is the load-bearing choice.** That accessor
    answers "how long before the automatic retry", and this guidance **forbids** the automatic retry; a number
    there would make `delay_seconds().is_some()` mean "retryable" and reintroduce the hazard. The provider's
    number moves to a separate `deferred_seconds()`, and a test asserts the two accessors are **disjoint for
    every variant**.
  - **The refusal is a VARIANT, not an error.** A provider asking to wait five hours is a real response meaning
    *defer the work* — a state this vocabulary already names (`BudgetOutcome::Exhausted` is the budget-side twin).
    A `Result` every caller immediately converted into `DeferSeconds` would be ceremony, and a caller could `?` a
    plan to wait into a hard failure, so `for_stated_delay` is **total**.
  - **The dead error variant was removed with the reason recorded in place**, because keeping it would leave a
    second declaration of the same rule that nothing constructs — the defect this slice is about.
  - **NEW LIMITS:** no consumer reads `RetryDecision` outside `jarvis-connectors` yet, so nothing *schedules* the
    deferred work — the deferral reaches an operator's eyes through the bounded reason, and the pipeline half is
    the same run-record gap `ADR-0075` recorded for `provider_request_id`. The ceiling is a **platform constant**
    rather than a per-provider or policy value, so it cannot be tuned without a code change.
- [ ] `P5-005` **(continued — a scope's category is a review burden)**: `google::scopes` adds `ScopeCategory`,
  `VerificationBurden`, `AssessmentRequirement`, `ScopeAccounting` and the pure `account` function, plus
  `google::gmail_scope_categories()` as the dated table transcribed from the scopes page. **8 new tests (so 342
  in the crate).** **`ADR-0078`.** Two guards falsified A-B-A with compiling mutants. **This writes the research
  record's own "scope-category tests" item, which was open.**
  - **⭐⭐ THE FINDING: THE CATEGORY DECIDES A SECURITY ASSESSMENT, AND IT LIVED ONLY IN A DOC COMMENT.** The
    scopes page states that a restricted scope requires "restricted scope OAuth App Verification" **and** that
    "**if you store restricted scope data on servers (or transmit), then you must go through a security
    assessment**". `SCOPE_GMAIL_READONLY`'s comment said "**Restricted** rather than sensitive, which is the fact
    that governs what deploying this connector costs" — correct, and the **only** place the fact existed. Nothing
    could check it, and a new scope could be added without anyone deciding its category. Re-fetched the page: the
    recorded categories were **correct**, and are now a dated table.
  - **⭐ THE ACCOUNTING IS OVER THE REAL MANIFEST, NOT A FIXTURE LIST — and it reports a partial reading honestly.**
    The record's own item asked for scopes "all *accounted for* … so adding a scope forces a decision", which is
    **not** the same as "assert these three are restricted" (that passes when a fourth is added). Two of the four
    declared scopes — `openid` and `calendar.readonly` — have **no recorded category**, so the deployment's
    burden is `Unestablished` rather than "restricted" inferred from the two that are known. A partial maximum
    would present a partial reading as a complete one.
  - **⭐ AN UNLISTED SCOPE IS NOT A CHEAP SCOPE.** `Unknown`'s burden is `Unestablished`, **not** `BasicReview`.
    Under-reporting is the direction that harms: an operator told "basic review" who ships a restricted scope has
    skipped an assessment Google requires, and nothing would have said so. The assessment is therefore a
    **three-valued** `AssessmentRequirement`, because a `bool` would make `NotRequired` and `NotEstablished` the
    same value — the same conflation as `RateLimitEvidence`'s `Documented` vs `Observed`.
  - **The scope match is EXACT.** A scope string goes into an authorization request, so a prefix match would let
    a longer scope inherit a **cheaper** category from a scope that is its prefix — under-reporting again.
  - **A defect in this slice's own first draft, caught by its own test.** `AssessmentRequirement` originally had
    `is_required_regardless`, whose comment claimed "both answer `false`" while the code returned `true`; once
    fixed to match the comment, the predicate was **false for every value** — Google's rule is conditional for
    the only category that requires an assessment, so nothing is ever required *regardless*. Replaced by
    `may_require_an_assessment`, which fails closed and a caller can branch on. **Same class as `0067`–`0077`: a
    declaration no input can exercise** — found in the slice that was writing about that class.
  - **A date inconsistency corrected.** `ADR-0076` and two research-log rows were dated **2026-09-28** while the
    current date and every other record in the session are 2026-09-27. Corrected.
  - **NEW LIMITS:** the Calendar and `openid` scope categories are **not established** (the pages read do not
    state one), recorded as Unresolved Question 2 and surfaced by the accounting rather than assumed away. The
    **internal-app exemption** is deliberately **not** modelled — it is a property of the consent screen's
    audience setting, not of the scope set, so a field for it could only be set wrongly.
- [ ] `P5-005` **(continued — a rate limit without its unit)**: `RateLimit` gains a required `unit`
  (`RateLimitUnit::{Requests, CostUnits}`) and a `sustained_requests_per_second()` that answers `None` for a
  cost-unit limit; `ConnectorOperation` gains `quota_cost` (`QuotaCost::{Documented(u32), Unstated}`) with a
  `calls_per_window` conversion; the four operations declare their published costs. The two byte-identical
  `gmail_read_rate_limit`/`gmail_history_rate_limit` functions collapse to one `gmail_rate_limit()`. **3 new
  tests (so 345 in the crate).** **`ADR-0079`.** Two guards falsified A-B-A with compiling mutants. **This writes
  the research record's own "quota-cost test" item**, and it fixed two more defects found against the docs.
  - **⭐⭐ THE FINDING: A QUOTA-UNIT FIGURE WAS BEING READ AS A REQUEST RATE, 20× OVER.** `RateLimit`'s fields and
    `sustained_per_second()` both said **requests**; Google publishes 1,200,000 and 6,000 as **quota units**,
    defining them as "an abstract unit of measurement representing Gmail resource usage", with per-method costs
    from 1 to 100. So the read path reported **20,000/s** where the true `messages.get` rate is **1,000/s** — and
    the error is **silent**: the connector behaves correctly until the provider starts refusing calls. The
    divergence is per operation, not per connector: the same ceiling is 60,000 `messages.get`/min and 600,000
    `history.list`/min, a **10× spread**.
  - **⭐ THE DUPLICATE THAT NAMED A REAL DISTINCTION IN THE WRONG PLACE.** Two rate-limit functions were
    **byte-identical**, and the second's doc justified itself by "the two calls have different documented costs".
    The costs *do* differ (20 vs 2) — the reason was **true** — but a per-call cost is not a property of a rate
    limit, and `RateLimit` had no field for it. So two identical limits carried a distinction that belonged to
    the operation. Moving it there is what lets one ceiling be declared once.
  - **`sustained_requests_per_second()` answers `None` for a cost-unit limit.** A cost-unit limit has **no**
    request rate until an operation's cost is known, so the value a scheduler would most easily mistake is not
    offered at all. `sustained_per_second` survives with its doc corrected to say the result is **in the limit's
    own unit**.
  - **`QuotaCost::Unstated` is not `Documented(1)` — and it is the `Default`.** Reading an unstated cost as 1
    computes the whole allowance as a request rate and over-plans by the real cost. `calendar_events_read`
    **ships** `Unstated`, because this page publishes no Calendar cost; claiming 1 would invent a figure the
    provider never stated.
  - **Two further defects found by reading code against its own docs.** (a) `SCOPE_OPENID`'s doc said
    `users.getProfile` "is the operation declared below" — **no profile operation exists**; the doc now says the
    scope is requested and the operation is not yet declared. (b) `RateLimit::new`'s error message said "1 to
    1000000 **requests** per window" while the figure may be cost units; corrected to "per window".
  - **NEW LIMITS:** `RateLimit` has **no daily window**, so Google's 80,000,000-unit daily threshold (which
    **cannot be raised**) stays in the research record rather than in a declaration — a window kind is a
    separate decision with its own falsifying test. The cost-unit conversion is only tested as arithmetic; no
    run has planned against a real provider yet, so a conversion that still over-plans because the *per-user*
    ceiling binds is not observable here. And `sustained_requests_per_second()`, `QuotaCost::units()` and
    `QuotaCost::is_documented()` have **test-only consumers** — the same shape `ADR-0057` already records for
    `PollingInterval::is_documented`, since nothing schedules yet. Stated rather than left for a reader to
    discover: their callers appear when a scheduler consumes the conversion, not before.
- [ ] `P5-005` **(continued — a limit and a recommendation are two facts)**: `GMAIL_BATCH_LIMIT` (50, "the
  largest batch Gmail accepts") splits into `GMAIL_BATCH_HARD_LIMIT` (100) and `GMAIL_BATCH_RECOMMENDED` (50),
  with `BatchPlan`/`batch_plan` as the producer. **3 new tests (so 348 in the crate).** **`ADR-0080`.** Two
  guards falsified A-B-A with compiling mutants. **Writes the record's "full-sync budget test that asserts
  batching" item, and corrects its premise.**
  - **⭐⭐ THE FINDING: A FIGURE FROM A PAGE THE RECORD NEVER LISTED, FILED UNDER ONE THAT DOES NOT STATE IT.**
    The record's "Other limits" said "**Batch requests: no more than 50**", and its Verification Log attributed
    "the batch ceiling of 50" to the **quota page** — which states **no batch limit at all**. The **batch
    reference** (`guides/batch`) was **absent from the source table entirely**. Re-fetched: it says "You're
    limited to **100** calls in a single batch request" and separately "We recommend sending batches of no more
    than **50**". So the number was real, **half-right in value and half-right in meaning, attached to the wrong
    document** — the quietest form of the failure `external-research.md` warns about: not an invented number, but
    a real one from an unrecorded source. A reader checking the record against the quota page would not find it.
  - **⭐ ONE CONSTANT COLLAPSED A REFUSAL INTO A SLOWDOWN.** "Two documented facts, in tension" was the old doc's
    own words, and it resolved the tension by picking one figure and calling it a ceiling. But exceeding **100**
    *fails the request* while exceeding **50** *degrades throughput* — different failures, different remedies —
    so a caller holding one number cannot know whether a size between them is a bug or a trade-off. The names now
    carry it: `HARD_LIMIT` is refused above, `RECOMMENDED` is **allowed and reported**.
  - **`batch_plan` rounds the request count UP, and that is the arithmetic a naive planner gets wrong.** 101
    calls at 50 per batch is **three** requests, not two; floor division **drops the remainder's request** and
    therefore skips part of a sync *while reporting success*. `final_batch_size` is carried so the partial batch
    is a value rather than a recomputation.
  - **A size above the recommendation is permitted, not refused.** Refusing it would be stricter than Google and
    would hide the 50–100 range the API accepts. `is_within_recommendation()`'s `false` means "invites
    throttling", and it is asserted in both directions so it is not false for everything — the defect `ADR-0078`
    found in its own first draft.
  - **The batch page states a fact that changes how a sync is sized: batching saves connections and NO quota.**
    "A set of *n* requests batched together counts toward your usage limit as *n* requests, not as one request."
    Worth recording because "batch the sync to make it affordable" is the natural misreading, and the record's
    own `5 + 20N` estimate is **unchanged** by batching.
  - **`ADR-0058` was annotated**, because it introduced the wrong constant and credited it with being a
    provider fact rather than a correction.
  - **NEW LIMITS:** `batch_plan` is arithmetic and a type, not a scheduler — nothing yet *paces* batches, so
    `is_within_recommendation()` has a **test-only consumer**, the same pipeline-side gap `ADR-0075` records for
    `provider_request_id`. No delay is asserted for a recommended-size batch because the record states **no
    figure** for it, only that throttling is *likely* — a risk rather than a number, and inventing one would be
    the defect this slice is about.
- [ ] `P5-005` **(continued — Calendar's 410 needs its reason)**: `CalendarGoneReason`
  (`FullSyncRequired`/`ResourceAlreadyDeleted`/`Unrecognised`) plus `client::calendar_signal` as the producer;
  three Calendar error fixtures, including a **same-status 410 pair with opposite remedies**. **4 new tests (so
  348 in the crate; the fixture suite went 11 → 15).** **`ADR-0081`.** Two guards falsified A-B-A with compiling
  mutants. **Writes the record's "410-is-staleness test" item and corrects its premise.**
  - **⭐⭐ THE FINDING: "CALENDAR'S 410 HAS NO SUCH AMBIGUITY" WAS TRUE OF THE STATUS AND FALSE OF THE DECISION.**
    The record asserted it while contrasting Calendar with Gmail's unclassifiable 404. The Calendar **errors**
    page — a page the record's source table did not list for this fact — publishes **three** bodies for
    `410 Gone` and only two resync: `fullSyncRequired` ("wipe the store and re-sync"), `updatedMinTooLongAgo`
    (same), and **`deleted`**, whose suggested action is **"no further action is necessary"**. So a connector
    reading the status alone **wipes a whole sync store when a user deletes one event** — and `advance_calendar_sync`
    maps `CursorUnusable` to a full wipe with **no cursor**, so the wrong branch is the destructive one.
  - **⭐ THE `deleted` CASE IS A REFUSAL, NOT A SUCCESS.** A delete of an already-deleted event did not do what
    was asked, even though nothing needs repairing — so it is carried as `SyncSignal::Refused` rather than
    pretending the call succeeded. Same rule as `P3-005` for an outcome the adapter could not establish.
  - **⭐ AN UNREADABLE 410 STILL RESYNCS, AND THAT IS THE OPPOSITE OF THE CRATE'S USUAL RULE — deliberately.**
    `RetryClass::Unknown` refuses because a retry could send a **second effect**; here the thing at risk is a
    **store's liveness**. Resyncing needlessly costs a slower next sync; *not* resyncing a genuinely dead token
    costs a store that never syncs again and never says so. The direction is argued on the variant so a reader
    meets the reasoning rather than inferring a contradiction.
  - **The status-only predicate SURVIVES with its competence narrowed.** It cannot be deleted (an unparseable
    body still needs a recovery path) and cannot be made strict (that regresses the unreadable case), so two
    functions answer the same question for callers holding different information — normally a defect, and here
    the difference is exactly which facts the caller has. Both are asserted, including that they disagree on
    `deleted`.
  - **The fixture pair is what makes the claim falsifiable.** Two files with the **same status** and opposite
    remedies: a connector reading the status gives them the same answer, and the test asserts they differ. The
    three Calendar fixtures also completed the record's fixture table (corrected **nine → twelve**).
  - **NEW LIMITS:** `calendar_signal` is a producer with **no production caller** — there is no `events.list`
    request to obtain a status and body from — so the tests are what currently hold the distinction in place;
    the same pipeline-side gap `ADR-0075` records for `provider_request_id`. The record still lists **no source
    for `updatedMinTooLongAgo` beyond the errors page**, and the two sync-token causes share one variant because
    they share a remedy, so a diagnostic cannot distinguish them from the type alone.
- [ ] `P5-005` **(continued — one classifier for two APIs)**: `classify` takes the API, and the error vocabulary
  is renamed for what it is. **4 new tests (so 352 in the crate).** **`ADR-0082`.** Two guards falsified A-B-A
  with compiling mutants. **Adds Finding 6 to the record.**
  - **⭐⭐ THE FINDING: THE TWO ERROR PAGES PUBLISH DIFFERENT STATUS SETS, AND ONE CLASSIFIER ANSWERED FOR THE
    LESS INFORMATIVE ONE.** `GmailErrorReason`/`GmailError`* were named for Gmail but parsed **every** error body
    — Calendar's included — and `classify` had no API parameter. Gmail's error guide documents **no `410`
    subsection at all**; Calendar's documents `410 Gone` in detail. So a Calendar `410` — the dead sync token
    `ADR-0081` had just built a reason vocabulary around — fell to the catch-all and reached a caller as
    *"the provider answered 410 (unknown)"*, i.e. `Reconcile`, *establish what happened before doing anything
    else*, when the provider had already stated the cause and the remedy.
  - **The API is an input, not a second table.** Two tables would duplicate every arm the pages **do** agree on
    (`401`; the `5xx` family; `403`'s throttling reasons, which the Calendar page itself calls "functionally
    similar" across `403` and `429`). A duplicated table drifts, which is the defect this crate has found
    repeatedly, so `classify(api, status, …)` carries the API beside the status exactly as the status sits
    beside the reason.
  - **A Calendar `410` is `Permanent`/`DoNotRetry` and that does not contradict the resync remedy.** A resync is
    not a retry of *this* request — the same token cannot succeed — so the refusal is right, and the comment
    spells the distinction out because `DoNotRetry` read as "give up on the sync" is the mistake it prevents.
  - **The second divergence is recorded, not silently resolved.** Calendar's page suggests "use exponential
    backoff" for a `404`; Gmail's states no action. The crate keeps `DoNotRetry` for both — Calendar's own two
    `404` causes are a resource that never existed and a calendar the user cannot access, neither repaired by
    resending — and
    **asserts the divergence in a test that names which document wins** (`ADR-0081`'s technique applied to a
    documented disagreement). Logged as Unresolved Question 8, because the page's sentence and its own causes
    disagree and only a live call can settle it.
  - **The API is derived, not passed beside the operation name.** `api_of` reads the `gmail_`/`calendar_` prefix
    the manifest already uses, so `calendar_events_read` + `GoogleApi::Gmail` — the pairing that would
    reintroduce the defect — is unrepresentable rather than merely discouraged. The rename is part of the fix:
    a type named `GmailErrorReason` that Calendar responses are parsed into is a false statement about scope.
  - **NEW LIMITS:** the refusal message for an unparsed Calendar `410` renders `"the provider answered 410
    (permanent)"` and does **not** name the dead sync token — the class is right and the explanation is thin,
    because the words come from `reason` and a `410` need not carry one. `api_of` derives from the operation-id
    prefix, so a future operation not following that convention would silently get `Gmail`. Nothing outside
    `jarvis-connectors` consumes `RetryDecision` yet — the recurring pipeline-side gap also recorded for
    `provider_request_id`, `batch_plan` and `calendar_signal`.
- [ ] `P5-005` **(continued — a declared output field is bounded by what the request can return)**:
  `gmail_messages_read` now renders every field it declares, and its schema no longer declares a field the
  request cannot return. **4 new tests (so 356 in the crate).** **`ADR-0083`.** Two guards falsified A-B-A with
  compiling mutants. **Adds Finding 7 to the record.**
  - **⭐⭐ THE FINDING: THE TOOL DECLARED FOUR OUTPUT FIELDS AND RENDERED ONE — AND ONE OF THE FOUR WAS
    UNDELIVERABLE.** `GMAIL_READ_OUTPUT` promised `message_id`, `thread_id`, `label_ids` and `snippet`, while
    the renderer emitted only `message_id` from `parse_single_id`, so **three of four declared fields were
    unreachable**. The existing schema test did not catch it because **a validity check is one-directional**: it
    proves the rendering is inside the schema (`{"message_id":"m1"}` satisfies `required: ["message_id"]`) and
    never that the schema's fields are inside the rendering. This is `ADR-0082`'s defect in the *output*
    direction — a declaration nothing produces.
  - **⭐ AND `snippet` COULD NOT BE DELIVERED EVEN IN PRINCIPLE.** The `Format` enum page defines each value:
    `minimal` is "only email message ID and labels" and `metadata` is "only email message ID, labels, and email
    headers" — neither returns `snippet`. The tool offers all three formats, so a declared `snippet` would be
    absent for two of them. **The fix for that is to stop declaring it, not to make it optional**: for a field
    two of three formats never return, "optional" reads as unreliable rather than impossible, and a model cannot
    tell which it is.
  - **The reverse direction is now a test, and it is the one that finds this class of defect.**
    `no_declared_output_property_is_undeliverable` compares the declared property **names** to the keys of a
    maximal rendering, so a field added to the schema and forgotten in the renderer fails — and it needs no
    maintenance when a field is added, because it fails until the renderer catches up. Written as name-set
    equality rather than a required-field check, since the latter passes for an optional field never emitted,
    which is exactly the `snippet` case.
  - **`parse_single_id` becomes `parse_single_message` returning `GmailMessage`** — three fields, the ones the
    output declares, **not** Gmail's nine-field `Message` resource. Same rule `IdPage` and `CalendarPage`
    follow, so the declaration and the rendering cannot drift.
  - **⭐ AN ABSENT LIST IS NOT AN EMPTY LIST.** `label_ids` is `Option<Vec<String>>` and the key is emitted
    **only when the provider returned it**: `None` = "the field was not in the response", `Some([])` = "the
    message has no labels". A `Vec` with `#[serde(default)]` collapses both to `[]` and renders "no labels" for
    a response that never mentioned labels — the two-values-three-situations defect. `thread_id` and `label_ids`
    are optional in the schema for the same reason: a `minimal` read may not carry them, so a `required` field
    would make an honest response fail the tool's own validation.
  - **NEW LIMITS:** no request has been sent, so what `minimal`/`metadata` return is taken from the Format page
    and not observed; if `minimal` returns more than "ID and labels" the optionality is looser than reality,
    which fails safe but is a gap the live smoke test would close. `GmailMessage` models three fields of the
    resource's nine; a future read wanting `historyId` or `internalDate` must add them to the type and the
    schema together. The reverse test is written for `gmail_messages_read` (where the defect was) and not yet
    swept across every operation.
- [ ] `P5-005` **(continued — an argument pair the provider forbids)**: `calendar_events_read` now refuses
  `sync_token` with a time bound, in the builder and in the input schema. **3 new tests (so 359 in the crate).**
  **`ADR-0084`.** Two guards falsified A-B-A with compiling mutants. **Adds Finding 8 to the record.**
  - **⭐⭐ THE FINDING: THE INPUT SCHEMA ADVERTISED A COMBINATION THE PROVIDER DOCUMENTS AS A `400`.**
    `calendar_events_read` offers `time_min`, `time_max` and `sync_token`, and the builder sent all three
    together. The `events.list` reference says, under `syncToken`: *"There are several query parameters that
    cannot be specified together with nextSyncToken … These are: `iCalUID` `orderBy` `privateExtendedProperty`
    `q` `sharedExtendedProperty` **`timeMin`** **`timeMax`** `updatedMin`."* The sync guide gives the reason (a
    time range belongs to the **initial full sync**, and each incremental sync repeats the initial filters) and
    the consequence ("The response code for list queries containing disallowed restrictions is `400`").
  - **⭐ THE LIST HAS EIGHT ENTRIES AND THE CONNECTOR OFFERS TWO — the count is the reason to record it.**
    `timeMin`/`timeMax` are the two it can send; the other **six** (`iCalUID`, `orderBy`, `q`,
    `privateExtendedProperty`, `sharedExtendedProperty`, `updatedMin`) are not offered by the operation at all,
    so they need no check. A future slice adding `q` or `orderBy` joins the disallowed set and must be checked
    against the note — which is why the whole list is written down rather than just the two that bite today.
  - **A `DisallowedCombination` variant, not `Argument`.** Both values can be individually valid and the
    *pair* is the fault; a caller told "`time_min` is unusable" would remove the legitimate part of a full sync.
    The two checks are separate so the message names **which** bound conflicts, and it states both remedies
    (drop the bounds to continue the sync, or drop the token for a filtered full read).
  - **The schema carries it twice, on purpose.** The property descriptions state the restriction, because the
    schema is what a **model** reads and a model that saw all three advertised could reasonably choose the
    combination; an `allOf`/`not` constraint enforces it for a validator. Both are present because they serve
    different readers, and the test asserts rejection by the **document** — so removing the constraint while
    leaving the prose fails. A description a validator does not enforce is the "documented but not applied"
    defect (`ADR-0077`).
  - **The control is a filtered FULL sync**, which the sync guide's own sample performs ("we are only syncing
    events up to a year old"). Without it a schema that rejected every `time_min` would pass.
  - **Why refuse locally rather than let Google answer.** `ADR-0082` already classifies a `400` as
    `Permanent`/`DoNotRetry`, so the outcome is identical — except the round trip, the quota unit, and the fact
    that the `400` still does not tell the caller which argument to drop.
  - **NEW LIMITS:** the restriction is taken from the reference and **not observed**, so if Google accepts the
    pairing the connector refuses a call that would have worked (the safe direction — the guide's sample never
    combines them). Nothing outside this module consumes `DisallowedCombination`; `request_for` is the only
    caller and the tests hold the behaviour in place. The refusal lives in one builder, so a second
    `events.list`-shaped operation would need its own check against the same note.
- [ ] `P5-005` **(continued — a rendered token needs an input that can consume it)**: `calendar_events_read`
  gains `page_token`, so the `next_page_token` it renders can actually be used. **3 new tests (so 362 in the
  crate).** **`ADR-0085`.** Two guards falsified A-B-A with compiling mutants. **Adds Finding 9 to the record.**
  - **⭐⭐ THE FINDING: THE TOOL EMITTED A PAGE TOKEN AND HAD NO ARGUMENT TO FETCH THE PAGE WITH.**
    `calendar_events_read` declared `next_page_token` in its output, the renderer produced it, a test asserted
    it — and the **input declared no `page_token`** and the builder never sent `pageToken`. So the caller was
    told "there is a next page" with no way to fetch it. Both Gmail list operations pair `next_page_token` out
    with `page_token` in; Calendar was asymmetric.
  - **⭐ AND THE MISSING PAGE IS THE COMMON CASE.** The sync guide: *"you may find a `pageToken` **instead of** a
    `syncToken` … you'll need to perform the exact same list query … (with the exact same `syncToken`), append
    the `pageToken` to it and paginate through all the following requests until you find another `syncToken` on
    the last page."* A sync of a busy calendar returns a page token instead of a cursor, so the walk could not
    finish — **the cursor the whole sync mechanism exists to advance was unreachable exactly when it mattered.**
  - **⭐⭐ THE SAME PROVIDER FACT POINTS THE OPPOSITE WAY FROM `ADR-0084`.** `pageToken` is **not** on the
    disallowed-with-`syncToken` list; the guide **requires** it *with* a sync token (its example is
    `…&syncToken=…&pageToken=…`). So the eight-parameter restriction must be applied **per parameter** and never
    by shape: a conflict check added "for symmetry" with the time-range one would refuse the documented walk.
  - **⭐ THE CHECK IS A PAIRING, NOT A PER-TOOL LITERAL.** Each schema was internally consistent, which is why
    nothing caught this — only the **pairing** of one tool's input and output halves exposes a renderer that
    emits a token no argument can consume. `every_output_that_can_return_a_page_token_accepts_one_as_input`
    walks **every** definition, so a future paginated read cannot ship one-directional. `ADR-0083`'s method
    (assert the reverse direction) applied across a single tool's two halves.
  - **The description says why the two tokens ride together**, not just "continues a paginated read": the guide
    requires the *exact same* query, so a model must repeat the sync token with the page token.
  - **NEW LIMITS:** the walk is **possible but nothing performs it** — no code loops on `next_page_token`, so a
    caller issues the follow-up call itself. The connector implements the *ability* to paginate rather than an
    automatic walk, which is `P5-010`'s "pagination" item. No request has been sent, so the large-change-set
    shape is from the guide and not observed.
- [ ] `P5-005` **(continued — a refusal names the argument the caller sent)**: `search_query` takes the field
  it validates, `time_min`/`time_max` get their own `time_bound` validator and a declared bound, and every
  declared input bound is asserted equal to its constant. **3 new tests (so 365 in the crate).** **`ADR-0086`.**
  Two guards falsified A-B-A with compiling mutants.
  - **⭐⭐ THE FINDING: A SHARED VALIDATOR HARD-CODED THE FIELD NAME IT REPORTED.** `search_query` always said
    `field: "query"`, but it had **three** callers — the Gmail list's `q`, and Calendar's `time_min` and
    `time_max`. So an oversized `time_min` on `calendar_events_read` was refused with *"the `query` argument is
    unusable: a search query may be at most 512 characters"*, **naming an argument the tool does not have** and
    calling an RFC 3339 instant a search query. A caller acting on that message hunts for a `query` parameter
    that does not exist. This is `ADR-0084`'s "remove the wrong argument" failure at a different layer: there a
    *combination* was reported against one field, here one field was reported as *another*.
  - **⭐ THE FIX IS A NEW VALIDATOR, NOT A RENAMED FIELD.** Passing the right field name through would still
    leave the *reason* wrong ("a search query" for a timestamp). `time_bound(field, value)` names its own noun
    and its own bound, because the fault's **kind** belongs in the message and not only the field it is about —
    the same distinction `DisallowedCombination` makes against `Argument`.
  - **⭐ `MAX_TIME_BOUND_CHARS = 64`, NOT THE QUERY'S 512.** An RFC 3339 instant is ~25 characters and at most
    ~30 with fractional seconds and an offset, so 64 admits every valid instant and still refuses a string that
    is plainly not a timestamp. The 512 was **the query's figure applied to a value of a different kind** —
    `ADR-0079`'s "a figure read as the wrong thing". Google publishes no length limit for `timeMin`/`timeMax`,
    so 64 is a **JARVIS** bound and the constant's doc says so.
  - **⭐⭐ THE SECOND DEFECT: THE DECLARED BOUNDS AND THE ENFORCED BOUNDS AGREED ONLY BY HAND.** The input
    schemas declare `maxLength`/`minimum`/`maximum`; `request.rs` enforces the same limits through constants;
    **nothing tied them**, and `time_min`/`time_max` had no declared bound at all. Now
    `every_declared_input_bound_matches_the_constant_that_enforces_it` reads each bound out of the schema by
    JSON pointer and compares it to the constant — 13 bounds plus 3 `minLength`s. The comparison is against the
    **constants**, so it cannot be satisfied by editing the schema alone, and it is the third instance this
    phase of one shape: two statements that must agree with nothing between them (`ADR-0083` output fields,
    `ADR-0085` token halves, this one numbers).
  - **NEW LIMITS:** `MAX_TIME_BOUND_CHARS` is a JARVIS bound, not a provider figure (no published limit). The
    drift test asserts **equality**, so it proves the two statements agree, not that either number is right — a
    bound changed in **both** places would pass with no provider evidence, which is why each constant carries
    its own justification rather than being asserted as "the value". The property table is explicit rather than
    a walk over every `maxLength`, so a new schema needs its row added.
- [ ] `P5-005` **(continued — a lease that lapses silently)**: new `google::watch` module — the watch lease's
  `expiration`, whether it is alive, and when to renew. **9 new tests (so 374 in the crate).** **`ADR-0087`.**
  Two guards falsified A-B-A with compiling mutants. **Adds Finding 10 to the record.**
  - **⭐⭐ THE FINDING: THE MANIFEST LINKED THE SEVEN-DAY BOUND AND NOTHING COULD APPLY IT.** The Webhooks doc
    link's purpose names "the seven-day renewal bound", but the crate had **no code that read a watch response
    or could decide whether a watch was still alive** — the "documented but not applied" defect (`ADR-0077`) on
    the one bound where it matters most, because **a Gmail watch fails silently**. The guide: *"You must call the
    `watch` method at least once every 7 days or you'll stop receiving updates for the user."* Nothing is raised
    and no notification announces that notifications stopped, so a lapsed watch is **indistinguishable from a
    quiet mailbox**.
  - **⭐⭐ `expiration` IS EPOCH MILLISECONDS IN A JSON STRING — two traps that raise nothing.** The reference:
    `"expiration": string (int64 format)`, *"epoch millis"*. A parser reading a JSON number refuses a
    **conforming** response; a parser scaling millis as seconds puts the watch's death **a thousand times too
    far in the future** — which is a valid instant, so nothing fails and the watch is silently dead. The unit is
    therefore pinned by a test against the reference's own value (`"1431990098200"` → 1,431,990,098 **seconds**
    *and* a rendered May-2015 instant), not by inspection.
  - **⭐ THE BOUND AND THE RECOMMENDATION ARE TWO FIGURES, ONE SENTENCE.** "At least once every 7 days" is when a
    watch *dies*; "We recommend calling `watch` once per day" is when to *renew*. `RenewalAdvice` is
    `Overdue | Recommended | NotYet` — ordered by **urgency**, which is why it is an enum and not a duration:
    a caller wants "renew now / soon / leave it", and a number would make it re-derive both thresholds here.
    Same split `ADR-0080` records for a rate limit and a recommendation.
  - **⭐ THE BOUNDARY IS DECIDED IN NANOSECONDS.** A lease ends at an *instant*, so comparing truncated seconds
    would call a watch with half a second left alive (keeping a dead watch) or lapsed (renewing early). The
    whole-second field is the magnitude computed **after** the direction, so `Lapsed { 0 }` is the exact expiry
    instant — **which counts as lapsed**, because the reference says the watch stops *at* that time and the safe
    direction is to treat the boundary as dead.
  - **`WatchLapse` is not a `bool`**: alive, just expired and long expired are three situations, and the elapsed
    time is what tells a caller whether this is a fresh problem or a mailbox unwatched for days (`ADR-0035`).
    **A renewal "in the future" is `NotYet`, not an error** — that is what a clock behind the renewal looks
    like, and the negative elapsed is carried rather than clamped so a caller can see the clock is ahead.
  - **FOUR unreadable shapes, four errors** (not JSON / absent-or-null / not a string / not an integer), because
    the remedies point at different layers. And a missing `expiration` is **refused**, not read as "never
    expires" — which would build a lease that never renews, the very failure being fixed.
  - **NEW LIMITS:** the module is **pure decisions with no caller** — nothing sends a `watch` request, so there
    is no response to parse and no scheduler to call `renewal_advice` (the same pipeline-side gap as
    `provider_request_id`, `batch_plan`, `calendar_signal`, `RetryDecision`). Neither the millis unit nor the
    inclusive boundary is **observed**; both are from the reference pages. `nanos_to_seconds` **truncates**, so a
    1.9-second difference reads as 1 — deliberate, since the direction is decided in nanoseconds first.
- [ ] `P5-005` **(continued — a field Google declares two encodings for)**: new `crate::base64` (extracted from
  `auth.rs`, decoder added) and `google::pubsub` (the notification envelope). **6 new tests (so 380 in the
  crate).** **`ADR-0088`.** Two guards falsified A-B-A with compiling mutants. **Adds Finding 11 to the
  record** and writes its "Pub/Sub envelope fixture" plan item.
  - **⭐⭐ THE FINDING: TWO OFFICIAL PAGES DECLARE `message.data`'s ENCODING DIFFERENTLY.** The Gmail push guide
    says it is *"a **Base64URL**-encoded string"*; the Cloud Pub/Sub `PubsubMessage` reference it links to types
    the field `string (bytes format)` and says *"A **base64**-encoded string"*. RFC 4648 §4/§5 differ in exactly
    two characters (`+`/`/` vs `-`/`_`), so **the disagreement is invisible on any value containing neither**.
  - **⭐⭐ AND ALMOST NO PAYLOAD CAN TELL THEM APART — which is why it is worth recording.** The guide's own
    example (`eyJlbWFpbEFkZHJlc3MiOiAidXNlckBleGFtcGxlLmNvbSIsICJoaXN0b3J5SWQiOiAiMTIzNDU2Nzg5MCJ9`) uses only
    `A-Za-z0-9` and decodes identically both ways — verified to yield exactly what the guide says. A sweep of all
    **95 printable ASCII** characters at all **four** base64 alignments inside a Gmail-shaped payload found only
    **three** (`>`, `?`, `~`, each after a one-character offset) that force the difference. So the readings agree
    on nearly every payload and diverge on the one that matters — where choosing wrongly **refuses a delivery**,
    which is a **missed change**.
  - **The decoder accepts both, tries URL-safe first, and says which matched.** URL-safe first because the guide
    is the more specific statement about *this* payload; both because the contradiction is the provider's and
    neither reading is provably wrong. `PubsubData` is returned rather than discarded so a real delivery can
    **settle the question from evidence** instead of the code resolving it silently in the provider's favour.
  - **⭐ THE FORCING FIXTURE IS SPELLED OUT, NOT GENERATED.** A second encoder would be a second thing that can be
    wrong, and the standard spelling is derived from the URL-safe one by RFC 4648 §5's substitution — with a test
    asserting the substitution actually changed the text, since a no-op substitution would test the same
    characters twice. Two earlier versions of the test failed here and each taught something: a hand-picked
    payload that happened to contain no `+`/`/` (so the test proved nothing), and an assertion that an unpadded
    value decoded (a fixture that did not contain the padding under test).
  - **Padding refused, not stripped**; **empty `data` refused as a shape**, not read as "nothing changed" (the
    silent direction); **four distinct errors** because the remedies point at different layers.
  - **Base64 now has one home.** The encoder moved from `auth.rs` to `crate::base64` **unchanged**, so the RFC 7636
    Appendix A test reaching it through `PkceVerifier` still guards the same lines. A `standard_padded` encoder
    was written and then **removed as unused** — a dead second implementation checks nothing.
  - **NEW LIMITS:** no delivery has been received, so which encoding a live subscription uses is **unknown** —
    exactly what reporting `PubsubData` leaves open. The envelope is decoded **only** as far as `message.data`;
    `messageId`, `publishTime`, `subscription` and the attributes map are not read, so there is no dedupe key and
    no ordering yet. `crate::base64` has no padded standard decoder, so a caller needing one must add it with a
    caller rather than as a spare function.
- [ ] `P5-005` **(continued — a rule from another context)**: padding is now **observed** rather than requested;
  the delivery envelope is parsed, with at-least-once dedupe and the five acknowledging statuses. **4 new tests
  (so 384 in the crate).** **`ADR-0089`.** Two guards falsified A-B-A with compiling mutants. **Adds Finding 12,
  which corrects Finding 11.**
  - **⭐⭐ THE FINDING: A RULE FROM ANOTHER CONTEXT WAS APPLIED TO A VALUE IT DOES NOT GOVERN.** `ADR-0088`
    refused `=` padding, justified by *"RFC 7636 requires padding omitted"* — an **OAuth PKCE** rule about the
    code verifier. But `message.data` is a **Cloud Pub/Sub** field, and the push page's **own minimum-value
    example** is `SGVsbG8gQ2xvdWQgUHViL1N1YiEgSGVyZSBpcyBteSBtZXNzYWdlIQ==` — **padded**, decoding to
    `Hello Cloud Pub/Sub! Here is my message!`. So the connector would have **refused Google's own published
    example**, which on a notification path is a **missed change**.
  - **⭐⭐ THE FIX REVEALS AN ASYMMETRY WORTH KEEPING: OBSERVABLE vs UNOBSERVABLE FACTS.** The alphabet needs to be
    a *parameter* because it is **unobservable** (the two differ in two characters, so a value containing neither
    decodes identically either way). The padding must **not** be a parameter because it is **observable** — it is
    the `=` at the end. Trying both paddings was *searching for something visible*, and it **cannot work**: a
    value whose unpadded length is already a multiple of four is valid either way, so the padded attempt is
    indistinguishable and can fail on a value that is legal. `decode_with(input, alphabet)` takes one axis;
    `Padding::of` reads the other.
  - **⭐ TWO DOCUMENTED FORMS ARE PREDICATES, NOT VARIANTS.** `PubsubData { alphabet, padding }` with
    `GMAIL_GUIDE = {UrlSafe, Absent}` and `PUBSUB_FIELD_TYPE = {Standard, Present}` as associated constants — a
    four-variant enum would make the documents into cases when they are points in one space.
  - **⭐ THE ENVELOPE, AND WHY `messageId` MATTERS.** Delivery is **at-least-once**: "A non-success response
    indicates that Pub/Sub must resend the messages", and a negative ack or an expired deadline resends. So
    `messageId` — "Guaranteed to be unique within the topic" — is the deduplication key, and **both spellings**
    (`messageId`/`message_id`, `publishTime`/`publish_time`) must be read, because the page's own examples show
    both and a `#[serde(default)]` field matching neither is `None` **with no error**.
  - **⭐ FIVE STATUSES ACKNOWLEDGE, NOT "2xx"**: `102`, `200`, `201`, `202`, `204`. A `203` or `206` is a success
    by HTTP's classification and a **negative acknowledgement** here, so a handler returning
    two-hundred-and-something would silently request redelivery of everything. `acknowledges_delivery` **fails
    closed**. Also recorded: **unwrapped** delivery (`payload-unwrapping`) has no `data` field, so it is refused
    as a shape rather than read as "no change".
  - **⭐ A FIELD IN THE WRONG PLACE IS SILENT.** The first attempt put `messageId`/`publishTime` at the **top
    level**; the provider puts them inside `message` (only `deliveryAttempt` and `subscription` are beside it).
    Every lookup returned `None` — silently, because these fields are `#[serde(default)]`. The test caught it by
    asserting `Some(...)` for the provider's own example rather than merely that the body parses.
  - **⭐ THE FIRST MUTANT SURVIVED, AND THAT WAS THE USEFUL PART.** Dropping the standard alphabet still passed
    the padded-example test, because `PUBSUB_PAGE_EXAMPLE` uses only `A-Za-z0-9` and decodes under **either**
    alphabet. The regression test now **forces the alphabet** as well, so a mutant cannot hide behind the
    provider's convenient example — the same lesson as Finding 11's sweep, applied to the test rather than the
    decoder. A dead-code smell was removed too: `decode_with` computed the padding and discarded it, so the first
    `Padding::of` mutant changed nothing.
  - **NEW LIMITS:** no delivery has been received, so which alphabet and padding a live subscription uses is
    **unknown** (which is what reporting `PubsubData` leaves open). `attributes` and `orderingKey` are not
    modelled — nothing in the Gmail path filters on an attribute and order is opt-in. `publish_time` is carried
    as **text**, not parsed, because nothing reads it and a parsed type would invite ordering logic the provider
    does not guarantee. Push **backoff** (100 ms–60 s, global, triggered by negative acks) is recorded in the
    research record but **not implemented**, since it is the provider's behaviour rather than a caller decision.
- [ ] `P5-005` **(continued — a refusal keeps the cursor)**: a refused cursor advance no longer discards the
  position. **2 new tests and 1 rewritten (so 386 in the crate).** **`ADR-0090`.** Two guards falsified A-B-A
  with compiling mutants, the second caught by **three** tests including a pre-existing one.
  - **⭐⭐ THE FINDING: A TRANSIENT FAILURE DISCARDED A WORKING CURSOR, SO A `429` COST THE WHOLE SYNC.**
    Both `advance_gmail_history` and `advance_calendar_sync` returned `cursor: None` for
    `SyncSignal::Refused(decision)` — a shape **copied from the `CursorUnusable` arm above them**, where it is
    right (the provider rejected the position) and here is wrong (the *request* failed and said nothing about the
    position). So a `429` on an incremental sync left the caller with no cursor: for Gmail a full resync of the
    mailbox, for Calendar a **full wipe of the store**.
  - **⭐⭐ AND IT IS THE EXACT MISTAKE `ADR-0067` WARNS AGAINST, COMMITTED IN THE ARM THAT HANDLES IT.**
    ADR-0067: *"a resync on a transient failure discards a working store, which is the opposite mistake and a
    much more expensive one."* That sentence is about the `404` heuristic; the `Refused` arm did worse, on the
    retryable family the sentence names. **The project's own table row contradicted the code**: it says
    `Refused` should "carry the classification; **never** a resync" — and dropping the cursor is what caused one.
  - **⭐ A TEST PINNED IT, WITH A COMMENT THAT WAS SOUND ABOUT THE WRONG SUBJECT.** The old test asserted
    `cursor.is_none()` justifying it as *"so a caller cannot store a new position on the strength of a failure"*.
    Correct reasoning, wrong referent: the previous cursor is **not a new position** — it is the caller's
    existing one, unchanged, and returning it unchanged satisfies the concern exactly. The test was
    **rewritten** with the old comment preserved so a reader sees why it changed.
  - **⭐ A REFUSAL AND AN UNCHANGED MAILBOX NOW GIVE THE SAME ANSWER, FOR ONE REASON.** `Refused` and
    `Advanced { None }` both return `Some(previous.clone())` because in both cases **nothing was learned about
    the position**. The three arms are now distinguishable and each is named: `CursorUnusable` → **none** (the
    position is dead), `Refused` → **the previous one** (the request failed), `Advanced { None }` → **the
    previous one** (nothing changed).
  - **⭐ A `429` HERE IS ROUTINE, NOT AN EDGE CASE.** `ADR-0089` established that push delivery is at-least-once
    and the push page documents backoff triggered by negative acknowledgements — so the arm that discarded the
    cursor sits on the common path of the feature this connector exists for.
  - **NEW LIMITS:** nothing consumes `CursorOutcome` outside this crate, so the benefit is proved by tests rather
    than observed in a deployment (the recurring pipeline-side gap). `Refused` still carries `RetryGuidance`
    inside it but **nothing reads the delay**, so a caller must schedule the retry itself.
- [ ] `P5-005` **(continued — a value redacted in one place and printed in another)**: four types stop printing
  what three other types already redact. **4 new tests (so 390 in the crate).** **`ADR-0091`.** Three guards
  falsified A-B-A with compiling mutants.
  - **⭐⭐ THE FINDING: THE CRATE HAD A REDACTION CONVENTION AND LATER TYPES OPTED OUT OF IT BY DEFAULT.**
    `AccessToken`, `FormRequest` and `VerifiedAccount` each hand-write `Debug` to print `[REDACTED]`/`<redacted>`,
    each with a test asserting the marker — and `PubsubNotification` (a **mailbox address**),
    `PubsubMessageBody` (base64 whose bytes **decode to** that address), `SyncCursor` (the **token**) and
    `SyncCursorParts` (the same token) all **derived** `Debug` and printed their sensitive field.
    `#[derive(Debug)]` is the default and hiding a field is a deliberate act, so nothing made the omission
    visible.
  - **⭐⭐ AND THE POLICY WAS ALREADY WRITTEN DOWN IN ANOTHER MODULE.** `DiagnosticField::CursorObservedAt`:
    *"the token itself is never a field, because a cursor is provider-issued text that can address another
    account's data."* The rule existed; the type printed it under `{:?}` anyway, which is how a token reaches a
    log line. **A rule recorded in one module and violated in another is not a rule** — the fix is a test per
    sensitive type, not a note.
  - **⭐⭐ FIXING THE THREE KNOWN TYPES WAS NOT ENOUGH — AUDIT BY SHAPE, NOT BY THE OFFENDER LIST.**
    `SyncCursorParts` (the parts struct `P3-006a` introduced, with the token **moved into it** by
    `From<SyncCursor>`) is not `SyncCursor`, so it was not covered by the redaction one screen up — and it is
    reached by the crate's own advertised idiom, `let parts: SyncCursorParts = cursor.into();`, which the
    existing test uses verbatim. **A sibling type is not covered by its sibling's redaction**, and it was found
    by grepping for `Debug`-deriving structs with a token-shaped field — a query naming none of the three.
  - **⭐ THE SUBTLEST ONE LOOKED SAFE.** `PubsubMessageBody::data` is base64 — an opaque-looking blob carrying no
    credential — so a reader could reasonably print it. Its bytes decode to `{"emailAddress": …}`, so the
    disclosure is exactly the one the decoded form is redacted for. **Opacity is not safety**, and the *name* of
    a field is no guide to what it carries.
  - **⭐ THE REDACTION IS ASYMMETRIC ON PURPOSE.** `PubsubNotification` hides the address and **keeps
    `history_id`**, because a position names nobody and is what an operator debugging a stuck sync needs.
    `PubsubMessageBody` hides `data` and prints its **length**, matching `AccessToken`'s `chars: N`. A `Debug`
    that redacted everything would pass "the value is absent" and make every diagnostic useless — `A10` requires
    diagnostics to *"remain useful"* as well as redacted, which is why every test here has a **control** asserting
    the non-sensitive fields are still printed.
  - **⭐ `None` PRINTS AS `None`, NOT AS A REDACTION.** `SyncCursor::new` refuses a token on a `Start` cursor, so
    "this kind carries no token" is a fact worth seeing; a marker would make a start cursor look like a redacted
    one — the "two situations, one rendering" defect in miniature.
  - **⭐ THE DERIVED `Debug` THAT STAYS HAS A COMMENT SAYING WHY.** `PubsubDelivery`'s derive is safe **because
    its only sensitive field is inside the redacted `PubsubMessageBody`** — so the comment records the property
    to re-check ("does every field it holds refuse to print one"), not the conclusion, because it changes when a
    neighbour changes.
  - **NEW LIMITS:** this is a **convention with tests, not a mechanism** — nothing stops the next struct deriving
    `Debug` while holding a sensitive field, and the remedy is one test per type rather than a lint (a lint
    cannot tell which fields are sensitive). And `SyncCursor`/`SyncCursorParts` now print `kind`/`account`/
    `version`/`instant`, which is deliberate: the account is an `AccountReference`, JARVIS's own local identifier
    rather than the provider's id, so it names nothing about the mailbox.
- [ ] `P5-005` **(continued — a response field with no reader)**: a `users.watch` response carries **two** facts;
  only the lease was read. **3 new tests (so 393 in the crate).** **`ADR-0092`.** Two guards falsified A-B-A with
  compiling mutants, one of them the original defect re-introduced.
  - **⭐⭐ THE FINDING: THE WATCH RESPONSE'S `historyId` — THE ANCHOR A FIRST SYNC STARTS FROM — HAD NO READER.**
    The reference gives the response as `{ "historyId": string, "expiration": string (int64 format) }`; the module
    read `expiration` (with its two traps) and read past `historyId` entirely. The push guide says what it is for:
    *"The response contains the current mailbox `historyId` … Your client receives notifications for all changes
    **after** that `historyId`."* So a caller using the crate's one watch reader got a **lease with no anchor** —
    the first sync after a `watch` had nowhere documented to start, and there was no function to call and no
    error to handle. **A reader named for one field is not a reader for its response**: the name
    `parse_watch_expiration` is accurate, so nothing looked mislabelled — the gap was in the *shape of the API*.
  - **⭐⭐ AND THE GUIDE'S WORKED EXAMPLE USES TWO DIFFERENT NUMBERS, WHICH IS WHAT MAKES THE CONFUSION
    FALSIFIABLE.** *"Pass `1234567890` as the `startHistoryId` to `history.list`. Afterward, you can persist
    `9876543210` as the last known `historyId`"* — the response's id at one end, the post-sync position at the
    other, both strings, both spelled `historyId`. **A one-value example cannot falsify a conflation of two
    same-typed fields: every reading of it type-checks.** The test asserts the anchor is `1234567890` **and**
    `assert_ne!`s it against `9876543210`. Worth re-reading a guide for its **example** rather than only its
    field list — that is where the second number was.
  - **⭐ THE FAILURE MODE WOULD NOT TERMINATE.** Reading the response's id as "the new position" produces a
    plausible `historyId` and satisfies every assertion written against a single-number fixture — and an anchor
    does **not move when the mailbox does**, so a sync that stored it as the position would re-read the same
    window on every run. `advance_gmail_history` already computes the real successor, which is why the anchor is
    named `anchor` and not `history_id`: the API's own name is ambiguous between the two ends.
  - **⭐ THE STRUCT IS THE FIX, NOT THE SECOND READER.** `WatchResponse { anchor, expires_at }` with
    `parse_watch_response` as the reader to use; the two field readers stay public because each has traps worth
    testing in isolation. A caller holding a `WatchResponse` **has read both fields**, because the only way to
    build one is the function that reads both — so the omission cannot recur without changing the type.
  - **⭐ TWO NEW ERRORS, AND THE ORDER OF THE TWO READS IS A DECISION.** `MissingHistoryId`/`HistoryIdNotAString`
    are separate from the expiration's, because the remedies differ (an absent anchor makes the first sync
    unanchorable; an absent expiration makes the lease unreadable). `parse_watch_response` reads `expiration`
    **first**: a caller that cannot tell when the lease ends cannot use the anchor either.
  - **⭐ A MISSING ANCHOR IS REFUSED, NOT DEFAULTED.** Treating "no anchor" as "sync from the beginning" takes
    the most expensive path exactly when the provider failed to supply the cheap one — the fail-open direction.
  - **NEW LIMITS:** **no `users.watch` request is built**, so the anchor's consumer is still a future sync loop —
    this makes the anchor *available and correct*, not read in production. The anchor is also unvalidated as a
    cursor: `SyncCursor::new` applies the empty/oversized/control-character bound, and nothing here calls it, so
    an unusable `historyId` is caught at the cursor rather than at the response.
- [ ] `P5-005` **(continued — a request the provider accepts and ignores)**: the `users.watch` **request** — the
  crate's first body-bearing operation. **4 new tests (so 397 in the crate).** **`ADR-0093`.** Two guards
  falsified A-B-A with compiling mutants; a third issue was found by a **failing test**, not by review.
  - **⭐⭐ THE FINDING: THE DANGEROUS ARGUMENT IS THE ONE THE PROVIDER ACCEPTS.** `POST …/users/me/watch` carries
    a JSON body whose `labelFilterAction` the reference calls *"deprecated because it caused incorrect behavior
    in some cases"* and says is *"ignored"* when `labelFilterBehavior` is set — so sending the stale spelling is
    a **`200`**, not a `4xx`. And `labelFilterBehavior` is the *"filtering behavior of `labelIds` list
    specified"*, so sent with **no** list it governs nothing: the provider registers an **unfiltered** watch
    rather than refusing. A connector would then receive **every** change while believing it scoped the set —
    valid notifications, a healthy-looking lease (`historyId` + `expiration`), and only the *set* wrong. **A
    `400` is self-reporting; a `200` that ignored what you sent is not.**
  - **⭐⭐ THE REMEDY FOR A DEPRECATED SPELLING IS UNREACHABILITY, NOT DISFAVOUR.** `LabelFilterBehavior` is a
    two-variant enum producing exactly one field name, and `gmail_watch` has **no parameter** that reaches
    `labelFilterAction` — so it cannot be sent by accident, by a duplicated call, or by a "send both to be
    safe" habit a bare `Option<String>` would invite. **An absent parameter cannot be passed; a refused value
    can later be widened — and the absence is asserted** (the tests parse the body and check the field is
    absent, in the unfiltered *and* the filtered case, where sending it would look most plausible).
  - **⭐⭐ A CHECK AND A NORMALIZATION ON THE SAME VALUE MUST BE ORDERED DELIBERATELY — AND THIS WAS FOUND BY A
    FAILING TEST.** `watch_json_body` did `trim()` and **then** `is_control()` on the result, so a trailing
    `\n` was **deleted before the check looked for it** and `"projects/p/topics/t\n"` was silently accepted and
    sent as the clean string. **The check that exists to catch a newline must run before the operation that
    deletes one.** Trim is right for the emptiness/length checks and wrong as the input to the control check;
    the fix moved the check to the **raw** value, and the test was left as it was.
  - **⭐ A NEW ERROR VARIANT, BECAUSE THE FAILURE IS NEW.** `RequestError::Ignored` is distinct from `Argument`
    and `DisallowedCombination`: both of those describe requests the provider **rejects**, and this one it
    accepts. The message names the argument and the remedy (send labels, or omit the filter).
  - **⭐ AN EMPTY LIST IS REFUSED, NOT READ AS "NO FILTER".** After the emptiness check both render the same
    body, so accepting the empty one would equate a **loop-over-zero-labels** mistake with a deliberate choice.
    "No filter" is expressed by *omitting* the argument — the one rendering that cannot be produced by accident.
  - **⭐ A SECOND REQUEST TYPE BECAUSE THE CREDENTIAL BOUNDARY IS THE AXIS, NOT THE BODY.** `WatchRequest` holds
    a URL and a rendered JSON body and **no header map** (so no field could hold a bearer token — the `ADR-0060`
    property). It is **not** `FormRequest`: that type's whole justification is the credential its body carries,
    which is why its `Debug` redacts. A watch body holds a topic name in the caller's own project and Gmail's
    label vocabulary, so redacting it would make every watch diagnostic useless while protecting nothing.
  - **NEW LIMITS:** **no request is sent**, so whether Google accepts this body rests on the live smoke test that
    does not exist; the topic name's **shape** is not validated — the reference requires
    `projects/{project}/topics/{topic}` whose project *"must exactly match your Google developer project id"*,
    and checking the second needs a project id the connector does not hold; and a **host discrepancy** between
    two official sources (`gmail.googleapis.com` in the method reference vs `www.googleapis.com` in this
    record's Verified Contract) is **recorded and deliberately not resolved**, because moving a base on one
    page's example rendering is churn.
- [ ] `P5-005` **(continued — a negative acknowledgement is charged to the subscription)**: the push handler's
  **answer** becomes a decision with a cost. **4 new tests (so 401 in the crate).** **`ADR-0094`.** Two guards
  falsified A-B-A with compiling mutants.
  - **⭐⭐ THE FINDING: THE ANSWER TO ONE DELIVERY IS PAID FOR BY EVERY DELIVERY ON THE SUBSCRIPTION.** The push
    page: *"Push backoff applies to all the messages in a subscription (global)"*, *"Push backoff can't be
    turned on or off"*, range **100 ms – 60 s**, *"calculated based on the number of negative acknowledgments"*.
    So `acknowledges_delivery → bool` read as "false: this one retries" while `false` also means **"and nothing
    else is delivered for up to a minute"**. A handler refusing a message it will *never* accept does not retry
    a message — it **slows every other mailbox on the subscription**, indefinitely, because the retry count is
    the subscription's policy and a push subscriber *"can't modify the acknowledgment deadline of individual
    messages"*. **Ask not "does this code acknowledge" but "who pays for this code".**
  - **⭐⭐ TWO TRIGGERS, AND ONE IS NOT A RESPONSE AT ALL.** An **expired acknowledgment deadline** triggers the
    same backoff — so a *slow* handler is indistinguishable, to the backoff, from a refusing one. That is why
    the decision type is about the delivery's fate rather than about a status code: the cost can be incurred
    without answering at all.
  - **⭐ `DeliveryAck` IS THREE ANSWERS, NOT A `bool`.** `Accept` / `Retry` / `AbandonAndAcknowledge` — exactly
    **one** of which refuses, which the test counts so a merge or a flipped arm fails loudly. A `u16` would let
    a caller re-derive `acknowledges_delivery` at the call site and hide the *reason* behind an integer.
  - **⭐ ABANDONING IS A NAMED CHOICE WITH ITS DOWNSIDE WRITTEN DOWN.** Acknowledge-and-record-the-drop loses
    the message — real cost — which is why it is a variant with the reasoning attached and not a default a
    caller falls into. `security.md`'s "fails closed" does not decide it: both answers are closed against
    *acting*, and the question is only whether the cost lands on this message or on the subscription.
  - **⭐ THE BOUND IS ON `deliveryAttempt`, AND `0` MEANS "NOT REPORTED" — NOT "EXHAUSTED".** No per-message
    deadline is readable, so the provider's own incremented count is the only per-message fact. Absent is not
    first, and **the direction is chosen**: `0` still gets a retry, keeping a possibly-new delivery alive,
    where the opposite reading would abandon a first delivery that merely arrived without an optional field.
    The push page's **minimum-value example omits `deliveryAttempt`**, so this is a shape that occurs.
  - **⭐ `MAX_RETRY_ATTEMPTS = 3` IS A JARVIS FIGURE AND SAYS SO.** The page publishes the backoff range and its
    global scope but **no retry count**, so this is this platform's policy — small and stated, because
    refusing without a bound is not a policy but the absence of one, paid for by every other mailbox.
  - **⭐ THE MIRROR OF `ADR-0093`, AND WORTH PAIRING.** There, an argument the provider **accepts and ignores**
    was dangerous because nothing reports it. Here, an answer the provider **honours** has a cost the sender
    cannot see — the same "real, untraceable from the call site" failure, from opposite directions.
  - **NEW LIMITS:** **nothing sends a response** — `DeliveryAck`/`decide_acknowledgement` are the *decision*, and
    the handler that maps a decision to a status code does not exist, so `acknowledges()` says *whether* to
    acknowledge and not *which* of the five codes to send (deliberate: they are interchangeable, and choosing
    one belongs with the response object); and **no dead-letter mechanism** is configured, so
    `AbandonAndAcknowledge` has nowhere to *send* the dropped delivery beyond recording it.
- [ ] `P5-005` **(continued — stopping notifications needs the grant that revoking destroys)**: account teardown
  is **two** operations with a forced order. **5 new tests (so 406 in the crate).** **`ADR-0095`.** Two guards
  falsified A-B-A with compiling mutants.
  - **⭐⭐ THE FINDING: THE REVOKE SUCCEEDS AND THE STOP THAT FOLLOWS IS THE CALL THAT FAILS — ONE STEP LATER,
    INVISIBLY.** `users.stop` is an ordinary authenticated API call (the reference lists the **same four
    scopes** `users.watch` needs), while revocation **removes exactly those scopes** (the identity page: it
    *"removes all OAuth 2.0 scopes previously granted to a project"*). So **stop then revoke** works and
    **revoke then stop** does not — and the reversed order fails at the *second* call, where a reader is
    looking at a different operation. **Same shape as `ADR-0094` (report and cost in different places), in a
    sequence rather than one call.**
  - **⭐⭐ AND THE DAMAGE IS A SILENT PRIVACY EXPOSURE, NOT A FAILED CALL.** `stop`'s *"All new notifications
    should stop within a few minutes"* only applies if the stop **happened**. With the watch still registered
    and the grant gone, nothing ends the stream but **the lease lapsing** — `WATCH_RENEWAL_BOUND_SECONDS`, i.e.
    **up to seven days** of the mailbox address arriving at an endpoint the user believes is disconnected, with
    **no credential left to turn it off**. The leaked value is exactly what `PubsubNotification` redacts
    (`ADR-0091`).
  - **⭐ THE RULE IS A PAIRING FUNCTION, NOT A SEQUENCE.** `may_precede(first, second)` refuses exactly one
    pairing — a step that `withdraws_access()` before one that `needs_a_live_grant()` — and permits the rest
    (including a step before itself), so a **third** step (Calendar `channels.stop`, a subscription deletion)
    is checked by the same rule rather than by a reader remembering a comment. `TEARDOWN_PLAN` is the safe
    order and a test asserts **the rule and the plan agree**.
  - **⭐ THE ASYMMETRY THAT MAKES THE MISTAKE EASY:** `needs_a_live_grant()` is `true` for `StopWatch` and
    **`false` for `RevokeGrant`** — revocation accepts an already-dead token (RFC 7009 §2.2's `200` covers
    "the client submitted an invalid token"). So a caller that revoked first sees its revoke **succeed** and
    gets **no signal** that it has just made the next step impossible.
  - **⭐ THE TWO HALVES CARRY DIFFERENT FAILURE POLICIES, AND THE ASYMMETRY IS ARGUED.** `StopWatch` is
    `BestEffort` (a failed stop costs a bounded privacy window; aborting would leave a **working credential**
    because a *notification preference* could not be changed — the larger harm to avoid the smaller);
    `RevokeGrant` is `Required` (a failed revoke means the account is **not** disconnected, so the caller is
    told).
  - **⭐ `notification_exposure` IS AN ENUM BECAUSE ONLY ONE ANSWER IS A FIGURE THIS CRATE MAY STATE.** With the
    stop accepted the provider says *"within a few minutes"* — **no number**, so inventing seconds would
    fabricate a provider rule. Without the stop the exposure is the lease's bound, which Google **does** state,
    and it is **reused from `watch`** rather than restated so the two cannot drift.
  - **⭐ A GENERALISATION: WHEN TWO TEARDOWN STEPS EXIST, ASK WHICH ONE THE OTHER DISABLES.** And narrower: *a
    cleanup you cannot retry after you remove access must happen before you remove access* — the window between
    them is the exposure, and its length is a provider figure the crate already holds.
  - **NEW LIMITS:** **no request is sent and nothing calls this plan** — the teardown executor does not exist
    (as the push handler and sync loop do not), so the ordering is a **value with tests, not a mechanism**: a
    caller that ignored `TEARDOWN_PLAN` and revoked first would get no refusal from this module, only a plan to
    read; and **deleting the Cloud Pub/Sub subscription** is not modelled — it is shared by every watched
    account, so silencing one mailbox that way would stop notifications for all of them (a candidate
    **connector-level** third step).
- [ ] `P5-005` **(continued — a requirement with no consumer)**: the account-identity requirement had no
  operation that returned it. **5 new tests (so 411 in the crate).** **`ADR-0096`.** Two guards falsified A-B-A
  with compiling mutants.
  - **⭐⭐ THE FINDING: `tools-and-connectors.md` REQUIRED PROVIDER-VERIFIED IDENTITY AND NOTHING RETURNED IT.**
    The requirement is "account identity verified from the provider, not user-entered labels", `P5-004`'s
    mapping names `users.getProfile`'s `emailAddress` as the value — and the connector declared **no operation
    that reads a profile**, only four that return mail and calendar *content*, none of which says **which
    mailbox answered**. So `VerifiedAccount::new`'s required `provider_account_id` had **no producer** and the
    requirement was **unimplementable, not merely unimplemented**. The gap was invisible because it was an
    **absence**: no wrong field, no failing test, just a requirement nothing could satisfy.
  - **⭐⭐ AND A COMMENT DISCHARGED THE WORK THE CODE HAD NOT DONE.** `SCOPE_OPENID`'s doc said *"the
    `users.getProfile` response carries an `emailAddress`, and that is the operation **this scope exists for**"*
    — while the `users.getProfile` reference accepts `mail.google.com/`, `gmail.modify`, `gmail.compose`,
    `gmail.readonly`, `gmail.metadata` and **NOT `openid`**. A reader following the comment concludes the
    identity path needs nothing more. **A plausible statement substituted for a working link** — the same shape
    as `ADR-0092` (a reader named for one field while its sibling went unread), one level up.
  - **⭐⭐ GENERALISATION: A REQUIREMENT'S EVIDENCE IS THE OPERATION THAT RETURNS IT, NOT THE DOCUMENTATION
    THAT NAMES IT.** "Operation X satisfies requirement Y" is a claim to verify against the API's own **scope
    and response tables** — here it was false about the *scope* as well as absent about the *operation*.
  - **⭐ THE SCOPE IS CORRECTED, NOT DELETED.** `openid` *is* requested and *does* have an effect — it makes
    Google return an `id_token`, which the exchange receives and deliberately does not verify — so removing the
    constant would remove a real declaration to hide a false explanation. The doc now says `getProfile` does not
    accept it, that the Gmail read scope is what makes the profile readable, and that the ID-token check is
    **unbuilt: a prepared seam, not a working feature** (the `nonce` `P5-002` carries is what a future check
    would compare).
  - **⭐ NAMING ANOTHER MAILBOX IS UNREPRESENTABLE, NOT DISCOURAGED.** `gmail_profile()` takes **no argument**
    — it hardcodes `me`, because a `user_id` field would let a caller aim at a mailbox its own token cannot
    address, refused as a `403` rather than as an error naming the argument. The input schema is
    `"properties": {}` with `additionalProperties: false`, so an invented field is refused rather than dropped.
  - **⭐ THE ADDRESS IS REQUIRED AND THE COUNTS ARE NOT DECLARED.** `parse_profile` refuses a response with no
    usable address — including a **whitespace-only** one, which satisfies "the field was present" while
    denoting nothing — because the operation exists to establish *which* mailbox answered. `messagesTotal` and
    `threadsTotal` are deliberately **absent** from both the output schema and the renderer: they are mailbox
    counts nothing reads, and declaring them is `ADR-0083`'s defect. A test asserts the output contains
    **neither**, so the omission is checked in both directions.
  - **⭐ TWO NEW TESTS ASSERT THE CORRECTION, NOT JUST THE FIX.** One asserts the profile operation carries no
    `openid` JARVIS scope and that the granted scopes contain both the Gmail read (which makes the profile
    answer) and `openid` (for the id token), with `assert_ne!` on the two strings — the divergence-assertion
    shape the revocation module uses. The other asserts the **declaration** (exists, `mail.read`, `ReadOnly`,
    cost `Documented(1)`) so a future edit removing it fails with the reason it must not be removed.
  - **NEW LIMITS:** **no request is sent and nothing calls the operation** — the connect-time identity flow
    (call `gmail_profile_read`, build `VerifiedAccount`, store `AccountReference`) does not exist, so the
    requirement is now *satisfiable* rather than *satisfied in production*; and the `id_token` remains
    **received and unverified** — this closes the *identity* gap, **not** the *token-verification* one, and the
    ADR says so rather than conflating them.
- [ ] `P5-005` **(continued — a delivery names a mailbox and nothing mapped it to an account)**: the push
  path's missing join, as a pure function. **7 new tests (so 418 in the crate).** **`ADR-0097`.** Two guards
  falsified A-B-A with compiling mutants (the first caught by **four** tests).
  - **⭐⭐ THE FINDING: EVERY PIECE OF THE PUSH PATH EXISTED AND THE JOIN DID NOT.** A delivery could be
    **decoded** (`parse_delivery`), its lease **read** (`watch::parse_watch_response`), its answer **decided**
    (`pubsub::decide_acknowledgement`), a stale cursor **classified** (`advance_gmail_history`) — and nothing
    connected the payload's `emailAddress` to one of the connector's accounts. So a delivery said "a mailbox
    changed" with no way to learn **which of yours**, and the sync it triggers needs one account's stored
    credential. **An unattributable notification is an unactionable one.**
  - **⭐⭐ AND THE VALUE THAT MUST BE JOINED ON IS UNTRUSTED.** Finding 1: neither Google mechanism fits
    `WebhookSupport::Push` (OIDC bearer JWT; echoed channel token over a zero-length body), so the connector
    **cannot authenticate a delivery at all** — the address is a string from whoever posts to the endpoint. Two
    facts bound what a forged delivery can do, and both are load-bearing: the route selects **a mailbox to
    read, never a credential to use** (the sync uses that account's own token, so it reaches only mailboxes
    already authorised), and the notified `historyId` is **not a position the sync trusts** (`history.list`
    runs from the **stored** cursor, so a too-high forged id cannot cause a **missed** change). **An identifier
    arriving over an unauthenticated channel may SELECT but must not AUTHORISE.**
  - **⭐ BYTE-EXACT, AND THAT IS SECURITY RATHER THAN STRICTNESS.** Six near-misses are asserted, each of which
    defeats a looser rule: a prefix (`starts_with`), a superstring (`contains`), a suffix-domain, a different
    local part, and leading/trailing whitespace (trimming). The control asserts the exact value **does** route.
  - **⭐ A CASE-ONLY NEAR-MATCH IS A STATE, NOT A MATCH.** Addresses are case-insensitive in practice, so
    `Person@example.invalid` is *probably* the same mailbox — but Google publishes **no canonicalisation rule**
    for `emailAddress` in a push payload, and if the two spellings were two accounts then applying the route
    reads the wrong mailbox. `DeliveryRoute::CaseDiffers { accounts }` reports it and a person decides.
  - **⭐ FOUR VARIANTS, NOT AN `Option`.** `Exact(AccountReference)` / `Ambiguous { accounts }` /
    `CaseDiffers { accounts }` / `Unknown` — an `Option` has two states and the decision has four, and a reader
    of `None` could not tell "not my account" from "my account, spelled differently". **`Exact` is the only
    variant carrying a reference**; `Ambiguous` and `CaseDiffers` carry **counts**, so no accessor can return an
    arbitrarily chosen account (picking first/oldest/most-recent would sync one mailbox under another's
    identity).
  - **⭐ EVERY UNROUTABLE DELIVERY IS ACKNOWLEDGED, AND `Retry` IS UNREACHABLE HERE.** None of the three is
    repaired by another attempt — the account set is a **local** fact — and `ADR-0094` makes that decisive: a
    negative acknowledgement triggers a **subscription-global** backoff of up to 60 seconds, so refusing would
    slow **every other mailbox on the subscription** for a message that can never become routable.
    `unroutable_acknowledgement()` is `AbandonAndAcknowledge` for all three and `None` for `Exact` (the latter
    because a routable delivery **can** be processed, not that it **was**).
  - **⭐ THE ARGUMENT IS `&[VerifiedAccount]`, NOT ADDRESSES.** The address and the reference must belong to the
    **same** account; a `&[(AccountReference, String)]` would let a caller pair one account's reference with
    another's address and route to the wrong mailbox with nothing able to notice. `VerifiedAccount` is the type
    that already binds the two, so it is the argument.
  - **NEW LIMITS:** **no delivery has been received and no account connected**, so routing runs on types the
    crate owns rather than observed data; and the **authentication gap is untouched** — this makes a forged
    delivery's consequence **small and bounded**, it does **not** make forging impossible (Unresolved
    Question 1, which needs a contract change or an OIDC/JWKS verifier `P5-001` kept out of a pure path
    deliberately).
- [ ] `P5-005` **(continued — one address, one account)**: the connect-time step that `ADR-0096` and `ADR-0097`
  both named as unbuilt, and where the router's `Ambiguous` is prevented. **6 new tests (so 424 in the crate).**
  **`ADR-0098`.** Two guards falsified A-B-A with compiling mutants; a **third** defect was found by a failing
  test rather than by review.
  - **⭐⭐ THE FINDING: THE TWO ENDS OF ONE STEP WERE MISSING, AND THE STATE THE ROUTER CANNOT ACT ON IS CREATED
    THERE.** `ADR-0096` produced an identity (`gmail_profile_read` → `emailAddress`) and named the connect flow
    as unbuilt; `ADR-0097`'s router consumes stored identities and named `Ambiguous` as caused by "a reconnect
    that mints a new reference without retiring the old row". Nothing turned one into the other, so **this step
    is where `Ambiguous` is prevented or created** — which is why its rule is a refusal, not a deduplication.
    A duplicate makes routing **undecidable** (every notification for that mailbox stops being acted on until a
    person resolves it) while being **invisible** (two cursors, two schedules, a quota budget paid twice look
    exactly like two mailboxes).
  - **⭐⭐ THE CASE COMPARISON RUNS THE OPPOSITE WAY, AND BOTH ARE THE SAME RESTRAINT.** `ADR-0097` refuses to
    **act** on a case-only near-match (no canonicalisation rule is published; acting could read the wrong
    mailbox). This refuses to **create** one, for the same uncertainty and because the directions differ in
    cost — refusing asks a person (recoverable), a duplicate is silent. **Neither acts on an uncertain
    case-match.** One test asserts **both halves against one pair of spellings**, so loosening either shows up
    as a contradiction rather than as policy drift.
  - **⭐⭐ A THIRD DEFECT, FOUND BY A FAILING TEST AND IT IS `ADR-0091`'s CLASS AGAIN: REDACTION APPLIED IN ONE
    PLACE IS NOT REDACTION APPLIED IN ANOTHER — AND THE SECOND PLACE IS USUALLY AN ERROR PATH.** The
    `IdentityUnusable` refusal first carried `error.to_string()`, propagating `ConnectorError::Identifier`'s
    `Display` — which is `"the connector identifier `{value}` is unusable: {reason}"`, **interpolating the
    value it rejected**. So an unstoreable address would have been printed in full by the refusal that says an
    identity could not be *stored* — while `VerifiedAccount`'s `Debug` **redacts that same value**. The test
    asserting the refusal does not contain the address caught it; the fix was to carry the error's
    **`&'static str` `reason`** instead of its rendering, making the leak **unrepresentable** (a `&'static str`
    has nowhere to put a runtime value). **The move is structural, not a rule about not printing.**
  - **⭐ A SEPARATE REFUSAL VARIANT, BECAUSE THE SUBJECT DIFFERS.** `IdentityUnusable` ≠
    `AddressAlreadyConnected`: one is about the account set, one about the identity, and the remedies differ
    (retire an account vs. look at the provider response). Reporting the second as the first sends a person
    hunting for a duplicate that does not exist. `holder()` returns `Option` — `None` here — rather than
    fabricating a holder to make the signature uniform.
  - **⭐ THE COUNTERFACTUAL IS DEMONSTRATED, NOT DESCRIBED.** The duplicate test constructs the duplicate
    **directly** and asserts the same notification becomes `Ambiguous`, so the consequence the refusal prevents
    is shown through the router rather than claimed in prose.
  - **⭐ NO DISPLAY NAME, BECAUSE THE PROVIDER SENDS NONE.** `users.getProfile` returns `emailAddress`,
    `messagesTotal`, `threadsTotal`, `historyId` — passing the address as a display name would invent a
    provider statement, which is the field `VerifiedAccount` exists to keep honest.
  - **⭐ `resume_from` CARRIES THE CURSOR'S KIND WITH ITS POSITION.** The kinds differ in exactly the way this
    matters: a `MonotonicMarker` has detectable staleness and a defined recovery; an `OpaqueToken` may not be
    validated at all and only the provider's refusal is authoritative — so a caller need not remember which API
    it holds. A `Start` cursor is `FullSync` rather than an error (`SyncCursorKind::Start`: a full sync "is a
    decision with consequences … an absent value would make it the default a caller stumbles into"), and its
    **two documented causes are deliberately not separated**, because the cursor cannot support the
    discrimination and the caller that discarded the position already knows why (`ADR-0067`'s restraint).
  - **NEW LIMITS:** **nothing calls `establish_account` or `resume_from`** — no account store, no executor, no
    sync loop — so both are decisions with tests rather than enforced behaviour (the "convention with tests,
    not a mechanism" limit `ADR-0091`/`ADR-0095` carry); the duplicate check is a **scan of a slice** rather
    than a store uniqueness constraint; and the two causes of `FullSync` stay unseparated by design.
- [ ] `P5-005` **(continued — a delivery can be authenticated without covering the body)**: the webhook contract
  gains the two authenticators Google's push mechanisms use — `SignatureAlgorithm::OidcIdToken` (an RS256 JWT in
  the `Authorization` header, Gmail via Cloud Pub/Sub) and `EchoedChannelToken` (a client-set string echoed in
  `X-Goog-Channel-Token`, Calendar channels) — plus `covers_the_body`/`is_body_independent` and a refusal of a
  signature encoding for a body-independent authenticator. **2 new tests (so 426 in the crate).** **`ADR-0099`.**
  This **resolves Unresolved Question 1** of the Google record, which two adjacent slices had named as
  *blocking*. Two guards falsified A-B-A with compiling mutants.
  - **⭐⭐ THE FINDING: ONE AXIS WAS BEING USED TO ANSWER TWO QUESTIONS, AND THE UNASKED ONE WAS THE PROVIDER'S.**
    `ADR-0054`'s contract had a single axis — *which MAC over the raw body* — and used it to answer a different
    question: *what authenticates this delivery*. A provider whose authentication is a **header token** (a
    bearer JWT, a shared string) therefore had **no representation at all**, and the finding was recorded for two
    rounds as "Gmail and Calendar push cannot be *expressed*". The finding's own name was the clue — it says
    "authenticated by an OIDC bearer JWT **or** an echoed channel token" — because **neither covers the body and
    both authenticate a delivery**. So the missing thing was not a value but an axis, and `covers_the_body` is
    it. **⭐ GENERALISATION: when a type answers two questions with one accessor, the unasked question is the one
    a new provider will need.** The variants are the consequence; the axis is the fix.
  - **⭐⭐ AND `ADR-0054`'s PROSE ALREADY KNEW THE AXIS, AND NAMED IT IN A SENTENCE WITH NO ACCESSOR.** Its doc
    for `is_keyed_mac` argues that "*a keyed MAC and an asymmetric signature both cover the body, but only the
    MAC requires the verifier to hold a secret*" — i.e. it states "*covers the body*" as a property in its own
    right, then exposes an accessor for only the second half. **A distinction stated in prose and absent from
    the code is the recurring defect of this phase (`0067`–`0098`)**, and this is the first instance where the
    prose was **correct and complete** and the code still could not answer the question: the sentence is a
    description, not an interface, and nothing forced it to be one.
  - **⭐ `covers_the_body` IS PROVABLY NOT `is_keyed_mac`, AND THE TWO DISAGREE IN BOTH DIRECTIONS.** Ed25519 is
    `is_keyed_mac == false` with `covers_the_body == true` (a public key still signs the bytes); an echoed
    channel token is `is_keyed_mac == false` with `covers_the_body == false` (a secret that covers no bytes).
    A single test asserts both cross-cases, so a later collapse of the two accessors fails rather than silently
    making one question stand for the other.
  - **⭐ `None` IS CORRECTED FROM "ANOTHER MECHANISM" TO "NO MECHANISM", AND THAT WAS THE TRAP.** Its doc had
    offered "*a bearer token in a header*" as an example of another way to authenticate that `None` made
    representable — but `None` means **nothing** authenticates the delivery, so the only honest way to represent
    a bearer-token authenticator was always a new variant. An author following the old doc would reach for
    `None` when the truth was "an authenticator I cannot name", producing a declaration that reads as *no
    control* — the webhook-spoof row with its control removed, in the one place the manifest refuses it.
  - **⭐ A BODY-INDEPENDENT AUTHENTICATOR MAY NOT CLAIM A SIGNATURE ENCODING (`SignatureError::Encoding`).**
    `encoding` describes how a **signature's** bytes are presented; a token presents an opaque header value, so
    there is nothing to hex-decode. `Raw` is the honest value and anything else is refused — `ADR-0057`'s rule
    (a field that cannot take an honest value for a variant is refused rather than defaulted) applied to a
    sibling field of the scheme. It is a distinct error from `Header` because the remedy differs: a bad name is
    a typo, an encoding on a token is a **misunderstanding of the mechanism**, and reporting it as a header
    problem sends a reviewer to the wrong half of the value.
  - **⭐ THE MANIFEST'S PUSH GUARD IS UNCHANGED AND STILL REFUSES ONLY `None`.** `!scheme.authenticates()`
    keeps its exact meaning ("no control is present"), and the acceptance test now round-trips **every**
    authenticator — including the two header tokens — so the guard is provably **not** a synonym for "not an
    HMAC". The falsification makes the point: mutating it to `!covers_the_body()` made a **truthful** OIDC push
    declaration refused, which is the over-broad-guard direction.
  - **⭐⭐ BUT THE CONNECTOR STILL DECLARES `Polling`, FOR A CORRECTED REASON, AND CORRECTING THE REASON WAS
    PART OF THE WORK.** Making the mechanisms expressible does **not** make Google's push declarable: one
    connector holds **one** `WebhookSupport` value while Google has **two** push mechanisms with different
    headers *and* different bindings, and **neither verifier is built** (no JWKS reader for the JWT, no stored
    value for the channel token). So flipping to `Push` would trade one incomplete declaration for another and
    would also demand webhook signature/replay readiness items nothing can satisfy. Three places of connector
    prose said the mechanisms "cannot be expressed" — now false — and each is corrected (`ADR-0074`/`ADR-0096`:
    a comment naming a module is a claim about code, and the code changed).
  - **⭐ A TEST'S PREMISE WAS CORRECTED RATHER THAN LEFT GREEN ON A STALE REASON.** `a_push_declaration_is_not_used_because_neither_google_mechanism_fits`
    asserted the *unexpressibility* that no longer holds. It is rewritten to **prove the mechanisms are
    expressible** (both schemes construct) and only then assert the manifest still declares `Polling` — so it
    fails for the reason that is still true rather than passing for one that is not.
  - **NEW LIMITS:** naming an authenticator is **not** verifying one — no JWKS fetching, certificate rotation,
    `aud`/`iss`/`exp` checking, or channel-token comparison exists, so the authentication gap `ADR-0097`
    recorded is **narrowed from "cannot be declared" to "is declared but unverified"** (Unresolved Question 9,
    which `P5-010` owns). `WebhookSupport` remains **one mechanism per connector**, which is why the two
    mechanisms cannot both be declared. And nothing *sends* or *receives* a delivery, so every rule here is a
    decision about values the crate owns.
- [ ] `P5-005` **(continued — a delivery is not always a change)**: new `google::channel` module — the
  **Calendar notification-channel push message**, read from `X-Goog-*` **headers** because the delivery has a
  **zero-length body**. `ResourceState { Sync, Exists, NotExists }`, `ChannelMessage` (with `is_sync()` and a
  redacted `channel_token()`), `parse_channel_message`, and `ChannelMessageError`. Two fixtures
  (`calendar_channel_message.json`, `calendar_channel_sync.json`) and 2 harness tests. **10 new lib tests (so
  436 in the crate).** **`ADR-0100`.** This writes the research record's own *"Calendar sync-message fixture"*
  item, which was open. Three guards falsified A-B-A with compiling mutants.
  - **⭐⭐ THE FINDING: A PUSH CONSUMER MUST DISTINGUISH "A MESSAGE ARRIVED" FROM "A RESOURCE CHANGED", AND BOTH
    GOOGLE MECHANISMS MAKE THOSE DIFFER ON THE FIRST MESSAGE.** Calendar sends a **`sync`** message when a
    channel is created — *"to indicate that notifications are starting"*, and *"It's safe to ignore"* — so the
    first delivery is a **handshake, not a change**. Gmail says the same in its own words (`ADR-0092`: a
    successful `watch` *"immediately sends a notification, so the first delivery is not a change"*). A consumer
    that equated the two would do one spurious read the moment it began watching, on **both** providers.
  - **⭐⭐ AND `ADR-0099` HAD JUST NAMED A CONTROL WHOSE INPUT NOTHING COULD PARSE.** That round taught the
    webhook contract to *name* an echoed channel token as an authenticator — over a header that **no reader
    looked at**, because `google::pubsub` models the Gmail envelope and **nothing modelled the Calendar
    mechanism at all**. That is `ADR-0092`'s "a value with no reader" inverted: the *contract* produced the
    requirement and no code consumed the wire form. This slice closes the **read** layer; the **compare** layer
    (the verifier) stays open and is named.
  - **⭐ AN UNKNOWN RESOURCE STATE IS REFUSED, BECAUSE NEITHER DEFAULT IS SAFE.** Treating an unrecognised
    `X-Goog-Resource-State` as `exists` (a change) acts on a message the connector does not understand; treating
    it as a non-change ignores a possible change. Refusing names the value instead of choosing a direction for
    the caller — and it is the mutant the test kills (`unwrap_or(Exists)`, the fail-open direction).
  - **⭐ DETECTION IS BY THE DECLARED DISCRIMINATOR, NOT THE ACCIDENTAL ONE.** `X-Goog-Message-Number` *"is
    always 1 for sync messages"* — but also *"not sequential"*, so `number == 1` classifies any early message as
    a handshake. `is_sync()` reads the **state**, and the fixtures deliberately pair a `sync` numbered `1` with
    an `exists` numbered `10`, so the number cannot be what is under test.
  - **⭐ TWO EXPIRATIONS WITH CONTRADICTORY ENCODINGS, READ BY DIFFERENT CODE.** `X-Goog-Channel-Expiration` is
    *"human-readable format"* (a date string) while the Gmail watch lease's `expiration` is an **epoch-millis
    string** (`ADR-0087`). Sharing a reader would force it to guess an encoding, so this module keeps the value
    **as text** and does not share one — the `ADR-0082` shape ("one classifier for two APIs") applied to a pair
    of values.
  - **⭐ THE ECHOED TOKEN IS SURFACED AND REDACTED, AND SURFACING IS NOT VERIFYING.** `channel_token()` returns
    it (a verifier needs the value), the field is **private**, and the hand-written `Debug` prints
    `[REDACTED], N chars` (`ADR-0091`) — because the token is the anti-spoofing control and a value in a log is
    a value an attacker could replay. Comparing it is `P5-010`'s work, and this crate does not hold the stored
    value the comparison needs.
  - **⭐ HEADER READS DISTINGUISH ABSENT, AMBIGUOUS, AND NON-UTF-8.** `single_header` collapses all three into
    `None`; this module separates them, because an absent header is the provider sending less than documented,
    an ambiguous one is a wire attack, and a non-UTF-8 one is an encoding fault. Ambiguity is **refused**, so
    the wrong value cannot win.
  - **⚠ A TEST-AUTHORING DEFECT FOUND BY A FAILING TEST: A HARDCODED LENGTH THAT DUPLICATED A COMPUTED VALUE.**
    The redaction test asserted `"34 chars"` (then `"25"`); the token `target=myApp-myChannelDest` is **26**
    characters, so it failed for a mistyped constant, not for the behaviour. Fixed by deriving the length from
    the token (`token.len()`). **A literal that restates a value the code already computes is a second source of
    truth, and it is wrong exactly when nothing else is.**
  - **NEW LIMITS:** **nothing receives a Calendar notification** — no delivery endpoint, no channel registration,
    and no channel-token comparison, so this reads a message from headers the crate owns rather than observed
    data. `WebhookSupport` still declares `Polling` for the connector (the cardinality reason `ADR-0099`
    records). And **`not_exists` semantics were not established by the page read** — the guide lists the value
    but does not define it for a caller — so treating it as actionable rests on the fail-safe direction rather
    than on a quoted rule.
- [ ] `P5-005` **(continued — a missing control and a failed one are not the same answer)**: the channel-token
  **verifier** — `verify_channel_token(stored, delivery)` returning a four-variant `ChannelTokenCheck`
  (`Verified`, `Absent`, `Mismatch`, `TokenRequired`) with `may_be_acted_on()`/`is_rejection()`, plus
  `MAX_CHANNEL_TOKEN_BYTES`. **4 new tests (so 440 in the crate).** **`ADR-0101`.** Completes the "surfaced,
  not verified" limit `ADR-0099`/`ADR-0100` each recorded, and **closes the constant-time-comparison half** of
  the research record's Unresolved Question 9. Two guards falsified A-B-A with compiling mutants.
  - **⭐⭐ THE FINDING: A `bool` CANNOT SEPARATE "A MISSING CONTROL" FROM "A FAILED ONE", AND THE TWO HAVE
    OPPOSITE READINGS.** A Calendar delivery has a **zero-length body**, so the echoed channel token is the
    **only** control — and `matches(stored, delivery) -> bool` would read `false` for both *"the channel was
    registered without a token, so there is nothing to check"* (`Absent`) and *"the channel is protected and
    this delivery failed the check"* (`Mismatch`/`TokenRequired`). One is a documented configuration that must
    **not** alert; the other is a forged or misrouted delivery that must. A predicate that cannot tell them
    apart makes a caller either **page on a correct configuration** or **accept a delivery that failed its only
    control**. `ADR-0035`'s "a boolean standing for more than two situations is an enum", where the collapsed
    states have **opposite operational readings**.
  - **⭐⭐ AND THE ANSWER IS TWO PAIRS, ONLY ONE MEMBER OF EACH A REFUSAL.** `Verified`/`Absent` may be acted on;
    `Mismatch`/`TokenRequired` may not. **`Absent` is not a refusal** — the token is optional (*"Only present if
    defined"*), so a delivery without one for an un-tokened channel is the documented shape, and refusing it
    would fail closed on a correct configuration. **`TokenRequired` ≠ `Mismatch`**: a wrong value versus no
    value, and a diagnostic saying "mismatch" for a delivery carrying nothing sends an operator hunting a value
    that was never sent.
  - **⭐⭐ ⚠ A GUARD IN MY OWN FIRST DRAFT COULD DECIDE NOTHING, AND ITS TEST COULD NOT TELL.** The first version
    checked `presented.chars().count() > MAX` and returned `Mismatch` before comparing — but
    `SecretValue::matches` **already** refuses a different-length candidate immediately, so the guard changed
    **no input's answer** while itself being an **unbounded `O(n)` walk of attacker input**: it added exactly the
    cost it claimed to prevent. The test I wrote for it **passed under a mutation removing the guard**, which is
    how the redundancy went unnoticed. **`ADR-0066`'s family in a new form: not a guard that can never *fire*,
    but one that can never *decide*.** Fix: removed the guard; renamed the constant to `MAX_CHANNEL_TOKEN_BYTES`
    and made its doc say **stated, not enforced, and why** (so a later reader does not add the guard back); and
    rewrote the test to assert the **observable** property — an over-long candidate is a `Mismatch` *because its
    length differs*, and a value **at** the documented maximum **verifies**, so the absence of a hidden ceiling
    is demonstrable. **⭐ Before adding a check, ask which input it changes the answer for — if none, it is not a
    check but a cost.**
  - **⭐ THE COMPARISON IS EXACT AND CONSTANT-TIME, REUSING THE CRATE'S OWN ROUTINE.** `SecretValue::matches`
    (`ADR-0055`, the OAuth `state`'s comparison) rather than `==`, because the stored token **is** a secret an
    attacker learns one byte at a time, and a short-circuiting compare leaks its prefix. Five near-misses
    pinned — prefix, superstring, case variant, trailing space, empty — each a rule a looser comparison would
    accept; and a **different but well-formed** token is a `Mismatch`, so `Verified` is not a constant.
  - **NEW LIMITS:** **nothing calls `verify_channel_token`** — no delivery endpoint and no channel store, so it is
    a decision with tests rather than enforced behaviour. The `OidcIdToken` (Gmail Pub/Sub) verifier is **still
    unbuilt** (no JWKS reader, no `aud`/`iss`/`exp` check), so Unresolved Question 9 is **half closed** (the
    constant-time comparison) and half open (the JWT). `WebhookSupport` still declares `Polling` (cardinality).
    And the verifier checks **only the token** — that a delivery names a resource this channel watches is a
    separate control it does not perform.
- [ ] `P5-005` **(continued — two pushes routed on keys of opposite provenance)**: `google::channel` gains the
  routing join — `ChannelRegistration` (channel id + account + optional token), `ChannelRoute`
  (`Exact`/`Ambiguous`/`Unknown`), and `route_channel`. **4 new tests (so 444 in the crate).** **`ADR-0102`.**
  Completes the attribution join `ADR-0100`/`ADR-0101` left open, the Calendar counterpart of `ADR-0097`. Two
  guards falsified A-B-A with compiling mutants.
  - **⭐⭐ THE FINDING: THE TWO GOOGLE PUSHES JOIN ON KEYS OF OPPOSITE PROVENANCE.** A Calendar delivery names the
    **channel** (`X-Goog-Channel-ID`) — a value the **connector chose** — while a Gmail delivery names the
    **`emailAddress`** — a value the **provider sent**. So the same "which of mine is this" question is a
    **lookup in the connector's own records** for Calendar and a **comparison against untrusted text** for
    Gmail. Both want **byte-exact** matching, for **opposite** reasons: an untrusted provider string may only be
    compared as sent, and a connector-generated id is compared against the bytes it stored. **⭐ A key's origin
    decides how strictly it may be compared — and a reader who generalised one mechanism's rule to the other
    would add either a spurious case-differs state or a dangerous case-fold.**
  - **⭐ ROUTING AND VERIFICATION ANSWER DIFFERENT QUESTIONS, AND BOTH MUST RUN.** A delivery can route `Exact`
    and still **fail** verification (a mismatched/absent token); a delivery for an **unregistered** channel is
    `Unknown` and cannot be verified **at all**, because there is no stored value to compare against. So the
    sequence is route → `verify_channel_token(registration.token(), message)`, and a test asserts routing
    **ignores** the token entirely, so the two controls are provably independent — fusing them would make an
    unroutable delivery unverifiable by construction and hide which control failed.
  - **⭐ THE BINDING IS ONE TYPE, SO A ROUTE CANNOT MISPAIR.** `route_channel` takes `&[ChannelRegistration]`
    (id + account + token bound together) rather than an `(id, account)` list, for the reason `route_delivery`
    takes `&[VerifiedAccount]`: separate lists let a caller pair one account's reference with another's channel
    and route to the wrong mailbox with nothing able to notice. `token()` reaches the stored value without the
    field being public, and the hand-written `Debug` redacts it (`ADR-0091`).
  - **⭐ `token: None` IS A RECORDED CHOICE, NOT A MISSING VALUE.** The guide makes the token optional, so a
    registration with no token is one the connector **chose** not to protect — read as `ChannelTokenCheck::Absent`
    (not a refusal), and an `Option<SecretValue>` with a sentinel would have made "I forgot the token" the same
    as "there is none".
  - **⭐ A REGISTRATION COLLISION IS `Ambiguous`, AND A COUNT IS NOT A PICK.** Reachable only if a channel id is
    reused — the guide *recommends* a UUID "so it is unique", a recommendation not an enforcement — and
    undecidable without a person. `ADR-0098`'s "one address, one account" argument applied to a channel: a
    duplicate makes the route unactionable and looks exactly like two legitimate channels.
  - **⚠ A COMPILE ERROR CAUGHT A DERIVE/IMPL CLASH BEFORE ANY TEST RAN** (`E0119`): `ChannelRegistration` derived
    `Debug` **and** hand-wrote it to redact the token. The **intent was the redaction**, so the fix was to drop
    the derive, not the hand-written impl — the compiler refused a type that would have printed the channel
    token. And a **test-helper defect** was fixed before it could pass for the wrong reason: the first
    `message_for_channel` built headers from a non-`'static` slice, so the helper leaks the id (test-only)
    rather than widening the fixture types. **The fixture layer is where a test stops testing the code.**
  - **NEW LIMITS:** **nothing calls `route_channel` and nothing registers a channel** — no `watch` executor, no
    channel store — so the route is a decision with tests rather than enforced behaviour, and a collision is only
    reachable if a caller hands in a duplicate slice (a store could enforce uniqueness, the revisit condition
    `ADR-0098` names). The Calendar push path is now end-to-end **on paper** (name → read → verify → route); what
    is missing is a handler that receives a delivery, and the Gmail `OidcIdToken` verifier (Unresolved Question
    9, still open).
- [ ] `P5-005` **(continued — composing the push path is what decides, and the seam changed a type)**:
  `ingest_channel_delivery(delivery, registrations) -> ChannelIngest` composes read → route → verify →
  classify; `ChannelIngest { Unreadable, Unroutable, Rejected, Handshake, Changed{account} }` with
  `is_accepted`/`account_to_sync`/`acknowledges`; and **`ChannelRoute::Exact` now carries the whole
  `ChannelRegistration`** (with a `registration()` accessor). **9 new lib tests + 1 harness test (so 453 in the
  crate, 18 fixture).** **`ADR-0103`.** Completes the Calendar push path the previous three slices built one
  piece at a time. Three guards falsified A-B-A with compiling mutants.
  - **⭐⭐ THE FINDING: THE SEAM IS WHERE A *TYPE* HAD TO CHANGE, AND NO PER-PIECE TEST COULD SHOW IT.** Read
    (`ADR-0100`), verify (`ADR-0101`) and route (`ADR-0102`) were each green alone and **nothing called them
    together** — `ADR-0069`'s "two tested halves do not test the seam", and `ADR-0098`'s "the join was the
    missing step", a **third** time. Composing them showed that `ChannelRoute::Exact` carried an
    `AccountReference` but **the token that proves the delivery lives on the registration** — so a caller would
    route, then scan the registrations **again** to get the token: two lookups deciding one match, and the
    account acted on and the token verified could come from two scans that merely happened to agree. `Exact` now
    carries the registration. **⭐ General rule: when a composition forces a value to be *fetched* rather than
    *carried*, the missing field is the finding — here, the control that proves what the account may act on.**
  - **⭐⭐ AND NO SINGLE ANSWER COULD EXPRESS THE OUTCOMES.** A `Result<Option<AccountReference>, _>` would encode
    *handshake* and *unroutable* as the same `None`, and those need **opposite** handling (accept-and-do-nothing
    vs drop); a `Result` also cannot separate a refusal from a retry. `ChannelIngest` has **five** variants for
    the three questions a push handler asks — *well-formed?* *really ours?* *a change?* — and only **one**
    (`Changed`) starts work. `is_accepted` and `account_to_sync` are **different questions**: a handshake is
    *accepted* (the sender keeps the message) but *syncs nothing*.
  - **⭐ ROUTING RUNS BEFORE VERIFICATION, SO AN UNREGISTERED CHANNEL IS `Unroutable`, NEVER `Rejected`.** The
    token cannot be checked without the registration that holds it, so for an unregistered channel **no
    comparison happened** — and `Rejected` would claim a control failed when none was present (`ADR-0101`'s
    "missing control ≠ failed control", at the composition). The order is also why the `sync` state is read
    **last**: an unauthenticated delivery must not steer whether work happens, even into the "no-op" branch.
  - **⭐ AN UN-TOKENED CHANNEL STILL SYNCS ON A REAL CHANGE.** `ChannelTokenCheck::Absent` **may be acted on**, so
    a correctly configured channel registered without a token is not silently dead — but its `sync` message is
    still a `Handshake`, so accepting a change does not accept everything.
  - **⭐ EVERY OUTCOME ACKNOWLEDGES.** None of the four non-`Changed` results is repaired by retrying, and a
    negative ack is **subscription-global** (`ADR-0094`), so refusing would slow every other channel for a
    message that can never become actionable. `acknowledges()` is a method so a future variant that *should* be
    retried has a place to say `false`. `Changed` carries only the **account**, not the registration — a sync
    needs the account, and handing the token onward puts the anti-spoofing control into a component with no use
    for it.
  - **NEW LIMITS:** **nothing receives the delivery** — no endpoint, no channel store — so this is a decision with
    tests, not a running handler. It does not record the drop, sync anything, or deduplicate a redelivery
    (`X-Goog-Message-Number` is read but unused, per `ADR-0100`). The Gmail `OidcIdToken` verifier is still
    unbuilt (Unresolved Question 9), so the Gmail path has no equivalent composition.
- [ ] `P5-005` **(continued — a cause is not a marker, and the two pushes do not share a state machine)**:
  `google::routing` gains `GmailIngest { Unreadable(GmailBodyError), Unroutable(DeliveryRoute),
  Changed{account, history_id, message_id} }`, `GmailBodyError { Envelope, Payload }`, `ingest_gmail_delivery`,
  and `acknowledgement()`. **5 new tests (so 458 in the crate).** **`ADR-0104`.** Composes the Gmail push path
  (`ADR-0103`'s counterpart) **and corrects a shipped claim** in `channel.rs`. Two guards falsified A-B-A.
  - **⭐⭐ THE FINDING: A *CAUSE* IS NOT A *MARKER*.** `ADR-0092`/`ADR-0100` recorded Gmail's rule as *"a
    successful `watch` immediately sends a notification, **so the first delivery is not a change**"*. Re-fetching
    the guide (footer **2026-09-15**) to build on it shows only the **first** clause is stated: the notification
    `watch` sends is an **ordinary** one — the same `{emailAddress, historyId}` payload a change produces, with
    **no state field**. Google **causes** an opening Gmail notification but **marks** the opening Calendar one
    (`X-Goog-Resource-State: sync`, *"safe to ignore"*). So *"X is not a Y"* was true of **what happens** and
    false of **what a consumer can detect**, and a state drawn from that confusion would be one the wire cannot
    produce. **⭐ Ask of any "X is not a Y" rule: a fact about what happens, or about what the message says
    happened?**
  - **⭐⭐ AND THE FALSE CLAIM HAD ALREADY SHIPPED — composition is what surfaced it.** `channel.rs`'s module doc
    said the Calendar `sync` rule was *"the Calendar counterpart of the Gmail rule"* — attributing to Gmail a
    detectable handshake it does not have (the `ADR-0074`/`ADR-0096` class, reached from a new direction: the
    wrong thing was a **comparison between two mechanisms**). Building the Gmail ingest forced the question
    *"which of `ChannelIngest`'s five variants apply here?"* — and the answer was *four do not, and one of the
    four is missing for a reason the doc got backwards*. Corrected in place and labelled as a correction.
  - **⭐ `GmailIngest` HAS THREE VARIANTS AND NO `Handshake`.** Copying `ChannelIngest` would have **invented a
    state**: with no marker, a `Handshake` variant is unreachable by any input (the "a variant nothing
    constructs" defect this phase keeps finding) *and* reachable-looking, so a caller would branch on it and
    believe it was skipping the opening notification. The outcome space is exactly read-failure,
    routing-failure, routed-change.
  - **⭐ `Unreadable` KEEPS THE LAYER.** `GmailBodyError::Envelope` (a broken Pub/Sub wrapper) vs `Payload` (a
    broken Gmail payload inside a good wrapper) — the two point at different layers, and a single "bad body"
    would send a caller to the wrong one. The failure is destructured back out of `PubsubDeliveryError::Payload`
    rather than flattened.
  - **⭐ `message_id` IS CARRIED AS AN `Option`, BECAUSE PUB/SUB IS AT-LEAST-ONCE.** It is the **only** field that
    tells a redelivery from a new change (`ADR-0094`); `None` means *"this may be a repeat I cannot detect"*,
    which is why it is not a defaulted string.
  - **⭐ `acknowledgement()`: `Accept` for a routed change, `AbandonAndAcknowledge` for both failures.** Neither
    failure is repaired by retrying (the payload is what it is; the account set is local) and a negative ack is
    subscription-global (`ADR-0094`). **No `Retry`** — this decides *what the delivery IS*, not whether acting on
    it succeeded, so the transient-failure answer belongs to the caller that acts.
  - **⭐ NO `Rejected` OUTCOME, AND THE REASON IS A NAMED GAP.** Gmail's delivery is authenticated by an OIDC
    bearer JWT (`ADR-0099`) and **that verifier is not built** (no JWKS reader, no `aud`/`iss`/`exp`), so there
    is no control to reject on — inventing the variant would claim a check that does not happen. Unresolved
    Question 9, still open.
  - **NEW LIMITS:** **nothing receives the delivery** (no endpoint), so this is a decision with tests; the JWT
    verification gap stands (the same trust limitation `ADR-0097` records — a forged delivery can select a
    mailbox to *read*, never a credential, and cannot cause a *missed* change); and the opening Gmail
    notification still costs **one** spurious sync per `watch`, which is the honest, documented cost of the
    absent marker rather than something this code can remove.
- [ ] `P5-005` / `P5-010` **(continued — the documented remedy is two steps, and only the second was a type)**:
  new `google::recovery` — `CallRecovery { Refresh, Reauth{reason}, NotCredential }`,
  `RefreshRecovery { RetryCall, RetryCallAndStore, RetryRefresh, Reauth{reason} }`, `TokenState
  { PossiblyStale, JustRefreshed }`, `recover_from_call`, `recover_from_refresh`. **8 new tests (so 466 in the
  crate).** **`ADR-0105`.** The first piece of the reauth lifecycle `P5-010` owns. Two guards falsified A-B-A.
  - **⭐⭐ THE FINDING: THE PROVIDER'S REMEDY IS TWO STEPS AND OUR VOCABULARY NAMED ONLY THE SECOND.** The errors
    guide, verbatim: *"To fix this error, **refresh the access token** … **If this fails, direct the user through
    the OAuth flow**."* The connector's `RetryGuidance::Reauthenticate` renders the **second** half as the whole
    of it — so read literally it sends a user through a consent screen as the **first** response to an **expired
    token**, exactly the failure a silent refresh repairs. The first step **had no type anywhere**, so the
    documented sequence was not expressible. **⭐ Generalisation: a remedy enum that names the *last* step of a
    documented sequence makes the earlier steps unrepresentable.** Ask of any remedy vocabulary: *does it cover
    the sequence the provider documents, or only its end?*
  - **⭐⭐ AND ONE ERROR CODE MEANS TWO CAUSES, WHICH ONLY THE PRESCRIBED *FIRST ACTION* SPLITS.** The same page:
    *"the access token … is either expired or invalid. **Missing authorization for the requested scopes can also
    cause this error.**"* So `authError` is an expired token (refresh fixes it) **or** a scope the grant never
    had (only re-consent does) — and the refusal cannot tell them apart. **A refresh can**, because it cannot
    grant a scope: a call refused with a *fresh* token is not a token problem at all. That is the guide's own
    *"if this fails"* branch from the other direction, and it is why the decision **takes `TokenState` as an
    input** rather than deciding from the refusal alone (the inference/decision split the cursor and pub/sub
    decisions already use).
  - **⭐ `TokenState` IS A TYPE, NOT A `bool`.** `PossiblyStale` (a refresh may help) vs `JustRefreshed` (a
    refresh was already tried **in response to this same failure** and the call was refused again). The
    distinction is a fact about the **attempt**, not about the token, and a `bool` named `refreshed` would read
    as the latter — which is how the second step gets taken first.
  - **⭐ `Rotated` CARRIES A STORE OBLIGATION `Refreshed` DOES NOT (`RetryCallAndStore` vs `RetryCall`).** A
    caller that treats a rotation as an ordinary refresh keeps using a refresh token the provider has already
    invalidated, and the **next** attempt then reads as a broken account rather than as a missed store.
    `RefreshOutcome::Rotated` was already its own variant; this is where it reaches a caller as an **action**.
  - **⭐ `Transient` RETRIES THE *REFRESH*, NOT THE CALL (`RetryRefresh`).** The one non-obvious direction:
    retrying the *call* with a token that was never obtained fails identically, while retrying the *refresh* is
    what a rate-limited token endpoint eventually honours.
  - **⭐ `JustRefreshed` NAMES `ScopeLoss`, NOT `ProviderRefused`.** With the token excluded by freshness, the
    guide's documented remaining cause is a missing scope; a persistent client-level problem is a
    *possibility* the refusal does not establish, so the reason named is the documented one rather than the
    graver-sounding one (`P3-008i`: do not assert a cause you cannot know). The code says the two are **not
    distinguishable here**.
  - **⭐ ONLY AN `Authentication` REFUSAL REACHES A REFRESH.** A `429`, a `403` and a `5xx` are `NotCredential`
    for **both** token states — refreshing a token that is not the cause spends the token endpoint's budget and
    changes nothing. `NotCredential` says *this module has an opinion and it is that the credential is not the
    problem*, which is different from "not considered".
  - **NEW LIMITS:** **nothing calls these functions and nothing performs a refresh** — no credential store and no
    refresh loop exist, so both are decisions with tests rather than enforced behaviour, and the
    `Rotated`→`RetryCallAndStore` obligation is only **expressed** (a store that could discharge it does not
    exist). `ProviderRefused` is **unreachable from `recover_from_call`** by design — only a caller that knows
    the client itself was refused can produce it, and this function cannot see that.
- [ ] `P5-005` **(continued — the same expiry in two encodings, and renewal is a replacement)**: new
  `google::channel` watch-lease half — `parse_channel_watch_response` (`ChannelWatchResponse { channel_id,
  resource_id, expires_at }`), `ChannelWatchError { NotJson, Missing, WrongType, OutOfRange }`, `ChannelLease
  { Lapsed, Alive }` + `channel_lease`, `ChannelRenewal { ReplaceNow, ReplaceSoon, NotYet }` +
  `renewal_decision`, and `CHANNEL_REPLACE_LEAD_SECONDS`. Plus the fixture
  `calendar_channel_watch_response.json` and a harness test that drives it. **7 new crate tests (so 470 in the
  crate; 1743 in the workspace).** **`ADR-0106`.** Two guards falsified A-B-A. The half of the channel path
  that happens *around* a channel rather than on a delivery.
  - **⭐⭐ THE FINDING: ONE QUANTITY, THREE ENCODINGS, AND THE FORM WE ALREADY HELD WAS THE UNUSABLE ONE.** A
    Calendar channel's expiry arrives as (a) the notification header `X-Goog-Channel-Expiration`, *"expressed in
    human-readable format"* (`Tue, 19 Nov 2013 01:13:52 GMT`) — which `ChannelMessage` already parsed and
    **could not** compare to a clock without a date parser, locale and timezone handling; (b) the `watch`
    **response body's** `expiration`, *"a Unix timestamp (in milliseconds)"*, typed `long` by the `events.watch`
    reference — which **was never read**; and (c) Gmail's `watch` response `expiration`, an epoch-millis
    **string**, per `ADR-0087`. So the connector held the expiry in the one form arithmetic cannot use, and the
    form it *can* use was the one nothing read. **⭐ Generalisation: when a quantity is available in several
    encodings, check which one the code already holds and whether it is the one the *decision* needs — "we have
    this value" is not "we have this value in a usable form".** Three encodings also means **three parsers**:
    sharing one would force the encoding to become a parameter, and the provider's own words would then live
    only at the call sites.
  - **⭐⭐ AND THE GUIDE CONTRADICTS ITS OWN REFERENCE ABOUT THE FIELD'S TYPE — in prose, with a table one click
    away.** The push guide's request section calls `expiration` *"An `expiration` **property string** set to a
    Unix timestamp (in milliseconds)"*; the `events.watch` reference the guide links to types it **`expiration |
    long`**. The reference wins (it is the schema for the method, and the guide uses the same "property string"
    phrasing for `params.ttl`, which the reference types `string`), so the parser reads a **number** and refuses
    a string **by name**. A parser that accepted both would have erased which document declares which form —
    and a *string* here is one of the other two encodings. **Same class as `ADR-0088`'s two-base64-alphabet
    contradiction, and the same resolution: prefer the specific statement, and make the choice visible.**
  - **⭐⭐ RENEWAL IS A *REPLACEMENT*, NOT A REFRESH — and reusing Gmail's vocabulary would have hidden it.**
    Verbatim from the guide: *"Currently, there's no automatic way to renew a notification channel. When a
    channel is close to its expiration, you must **replace it with a new one** by calling the `watch` method. As
    always, you must use a **unique value for the `id` property of the new channel**. Note that there's likely
    to be an **'overlap' period** of time when the two notification channels for the same resource are active."*
    Four facts, each of which changes a type: the channel is not extended (a *second* one is created), the new
    id must be unique, the old one **keeps delivering during an overlap** (duplicate deliveries for one resource
    are expected, not a bug), and **the overlap has no published duration**. `ADR-0087`'s `RenewalAdvice` takes
    a *last-renewed* instant because Gmail's rule is a **cadence**; Calendar has no cadence, only an **expiry**,
    so `renewal_decision(expires_at, now)` takes a different input and is a different function — the same split
    `ADR-0104` records for the two pushes, one layer down.
  - **⭐ `CHANNEL_REPLACE_LEAD_SECONDS` IS A JARVIS FIGURE AND SAYS SO.** Google publishes the overlap as
    *"likely"* with **no number**, so the margin for "close to its expiration" cannot be quoted from a page. It
    happens to equal `WATCH_RENEWAL_RECOMMENDED_SECONDS` and is **stated separately anyway**, because reusing
    the Gmail constant would tie two independently documented mechanisms together — a change to one provider's
    text would silently move the other's behaviour. `ADR-0080`: a figure with no source is neither a limit nor
    a recommendation.
  - **⭐ THE REFERENCE ALSO PUBLISHES THE SAME 7-DAY FIGURE FROM A SECOND MECHANISM.** *"params.ttl | string |
    The time-to-live in seconds for the notification channel. **Default is 604800 seconds.**"* Numerically
    identical to Gmail's `WATCH_RENEWAL_BOUND_SECONDS` (`604_800`), arrived at by a different mechanism and
    documented on a different page. Recorded because it makes the figure *look* like a shared constant when the
    two documents are separate — the same trap the two expirations set.
  - **⭐ `resourceId` IS READ BECAUSE A CONSUMER NEEDS IT; `token` IS NOT, BECAUSE ONE ALREADY EXISTS.**
    `channels.stop` *"requires that you provide at least the channel's `id` and the `resourceId` properties"*,
    so `resourceId` is a value with a **consumer** — and the response is the only place it appears *before* a
    notification arrives (the `sync` message carries it too, but the guide warns it can arrive *before* the
    `watch` response, so a stop built on `sync` alone has a race the response does not). The echoed `token`, by
    contrast, would be a second source for a value this connector chose; `kind` and `resourceUri` have no
    consumer. `ADR-0092`'s rule applied twice in one struct with opposite outcomes.
  - **⭐ THREE RENEWAL STATES, NOT TWO, BECAUSE ONE IS AN INCIDENT AND ONE IS A TASK.** A **lapsed** channel has
    already stopped delivering — notifications are being **lost** — while a **nearly-lapsed** one must be
    replaced *before* it does. Same action, different operator meaning, so collapsing them would hide whether a
    gap has already begun. The exact expiry instant counts as **lapsed**, in nanoseconds, for the same reasons
    `watch_lapse` does (`ADR-0035`, `ADR-0087`).
  - **⭐ TWO GUARDS FALSIFIED A-B-A, both compiling.** (1) `checked_mul(1_000_000)` → `checked_mul(1_000)` (ms
    read as µs) → **detected** by the test pinning Google's own `1426325213000` to `1426325213` seconds. (2) the
    renewal boundary `<= CHANNEL_REPLACE_LEAD_SECONDS` → `<` → **detected** by the test asserting a channel
    exactly one lead away is `ReplaceSoon`. **The scale factor is the one error here that does not announce
    itself**: `1426325213000` read as micro- or nanoseconds is a valid `UtcTimestamp` in the wrong century, and
    no type can catch a plausible unit — so the value is pinned to the provider's own example.
  - **⭐ A DEFECT FIXED IN THE RECORD ITSELF.** Finding 10 said Calendar's channel `expiration` is *"an RFC 3339
    date-time string"*. **No Calendar page says that** — it is epoch millis, and RFC 3339 is what Google
    **error** bodies' metadata and Pub/Sub's `publishTime` use. The claim was unquoted, plausible, and survived
    because nothing read the field. Corrected in place, with the reason: **a comparison drawn from memory reads
    exactly like one drawn from a page, and only the quoted form can be checked.**
  - **NEW LIMITS:** **no `watch` call is issued and no live channel has been read** — a hand-built fixture
    marked `_not_a_capture` stands in. `expiration` may come back **shorter than requested** (*"determined
    either by your request or by any Google Calendar API internal limits or defaults (the more restrictive value
    is used)"*), which no offline test can exercise. **The replacement itself is not built** — no second `watch`
    with a new unique `id`, so the documented overlap is never produced, and no deduplication key for the
    duplicate deliveries that overlap implies has been decided. Same shape as `ADR-0087`: the *decision* is
    tested and the *caller that acts* does not exist.
- [ ] `P5-005` **(continued — a teardown step whose arity the provider decides)**: `google::request` gains
  `calendar_channel_stop` (a body of exactly `id` + `resourceId`) and `WatchRequest` is **renamed `JsonRequest`**;
  `ChannelRegistration` gains `resource_id` (read by `ADR-0106` and previously discarded);
  `TeardownStep::StopCalendarChannel` joins the plan as a **third** step; `notification_exposure` splits into
  `gmail_exposure` + `calendar_exposure` and `NotificationExposure` gains `AlreadyEnded`. **5 new tests (so 475
  in the crate; 1748 in the workspace).** **`ADR-0107`.** Two guards falsified A-B-A, one mutation proved a
  **no-op** and corrected a doc claim. Completes the channel path at its far end.
  - **⭐⭐ THE FINDING: A VALUE READ FOR A CONSUMER THAT DID NOT EXIST.** `ADR-0106` read `resourceId` out of the
    `watch` response and said in its own doc that it is *"what the `channels.stop` call needs"* — and
    `ChannelRegistration` had **nowhere to put it**, so the value was dropped one function after it was obtained.
    That is `ADR-0092`'s "a response field with no reader" in a **stronger form: a field with a reader and no
    holder**, where the doc states the consumer and the type cannot carry the value to it. **⭐ A registration is
    the only value that survives between the `watch` and the teardown, so an identifier missing from it makes
    the channel unstoppable** — and the omission is discovered at teardown, the worst possible place.
  - **⭐⭐ AND ONE "THIRD STEP" IS NOT ONE STEP: THE PROVIDER DECIDES THE ARITY.** `teardown.rs` predicted a
    future third step *"is checked by the same rule"*, and `ADR-0095` asked whether `may_precede` *"generalises or
    needs a per-API argument"*. **It generalises** — the new step was admitted **unchanged and unedited**, because
    the rule tests *authority* (`withdraws_access`, `needs_a_live_grant`) rather than the operation. But the
    prediction missed the step's **shape**: `users.stop` ends **the** mailbox watch (one resource, one call, no
    arguments), while `channels.stop` ends **a** channel and has *no per-user form* — *"there's only one `stop`
    method"* — so an account watching three calendars needs **three** calls. One variant would have misreported
    whichever mechanism it skipped. **⭐ Generalisation: before reusing a step variant, check its ARITY — how many
    calls it stands for — because a step that is "one call per account" and one that is "one call per instance"
    are the same effect only until an account has two instances.**
  - **⭐⭐ AND THE EXPOSURE FIGURE CANNOT BE SHARED, for a reason about the *sources*.** `notification_exposure`
    returned Gmail's `WATCH_RENEWAL_BOUND_SECONDS` (7 days) — correct for Gmail *because Google publishes a bound
    for the mechanism*. Google publishes **no equivalent bound for a channel**: its life is *"determined either by
    your request or by any Google Calendar API internal limits or defaults"*. So the only honest figure is the
    expiry the channel's own `watch` response reported — **the two mechanisms are documented differently, so one
    function cannot compute both**, and a flag would drag an `Option<ChannelLease>` that is meaningless in one
    branch.
  - **⭐ `AlreadyEnded` REMOVES AN OVERSTATEMENT.** A channel whose lease had **already** lapsed exposes nothing,
    whether or not a stop was attempted — and it **outranks** the stop's success, because the answer no longer
    depends on the call. A `bool` function would have reported `UntilTheLeaseLapses { seconds: 0 }`, describing a
    clean teardown as an open window. Reachable only for Calendar, because Gmail's figure is a constant rather
    than a lease — a wiring gap the ADR records.
  - **⭐ THE STOP PERMISSION RULE IS DOCUMENTED AND LOCALLY UNENFORCEABLE, and the reason is worth stating.**
    *"If the channel was created by a regular user account, only the same user from the same client (as identified
    by the OAuth 2.0 client IDs from the auth tokens) who created the channel can stop the channel. If the channel
    was created by a service account, any user from the same client can stop the channel."* **Two rules keyed on
    how the channel was created**, and the discriminant is the **`client_id` inside the token** — which
    `credential` exposes only as a rendered header value (`ADR-0061`), and which a JWT access token would carry as
    the **unverified** `aud`/`azp` claims (`ADR-0064`). Enforcing it locally would mean trusting an unverified
    claim **to be stricter than the provider**, the failure direction `ADR-0064` warns about. So it is a recorded
    limit, and a violation is the provider's `403`.
  - **⭐ `WatchRequest` → `JsonRequest`: A NAME DERIVED FROM ITS FIRST CALLER BECAME FALSE.** `channels.stop` is
    the call that *ends* a channel a watch created — not a watch. The axis separating this type from
    `FormRequest` is the **credential boundary** (`ADR-0093`), which both bodies share (neither carries one), so
    the name follows the axis. `ADR-0093`'s revisit condition asked exactly this and pre-supplied the criterion.
    It is also the safe moment to rename: the type has **no consumers** — the builders are the public surface.
  - **⭐ THE TWO IDENTIFIERS ARE OPAQUE AND TRANSPOSABLE, SO THE TEST PARSES RATHER THAN SUBSTRING-MATCHES.** A
    swapped `id`/`resourceId` pair is well-formed and would stop the wrong channel or none. Asserted by
    deserializing the body and comparing each field to its expected value, plus an exact field count — so the
    optional `token` the reference permits is *asserted absent*, because sending it would put the anti-spoofing
    control into a body a diagnostic renders, for no effect.
  - **⭐ TWO GUARDS FALSIFIED A-B-A, AND A THIRD MUTATION WAS A NO-OP THAT DISPROVED MY OWN DOC.** (1) Writing the
    **channel id into both** body fields → **detected** (`left: "channel-alpha"`, `right: "o3hgv1538sdjfh"`).
    (2) Removing the **`stop_succeeded` guard** from `calendar_exposure`'s live arm → **detected**
    (`left: SettlingWithinMinutes`, `right: UntilTheLeaseLapses { seconds: 90000 }`). (3) **Reordering the arms
    so a successful stop matched first changed NOTHING** — `Lapsed` and `Alive` are different variants, so the
    arms are mutually exclusive and **no order decides anything**. The doc had said *"that is why the arms are
    ordered lease-first"*, which asserted a fact about the source's **layout** as though it were a fact about the
    **behaviour** — unfalsifiable, therefore uncheckable. **The claim was corrected rather than kept**, and the
    load-bearing part is the guard on the arm below.
  - **NEW LIMITS:** **no request is sent and no teardown executes** — still no caller, as for the push handler and
    the sync loop. `users.stop` is deliberately **not** built (a different method with an empty body would need a
    third request shape for no gain). The **channel count is a runtime fact**, so a plan names the effect and the
    caller performs it per channel; a caller that stops fewer than all of an account's channels gets no refusal
    from this module, only a plan to read — unchanged from `ADR-0095`. `gmail_exposure` cannot report
    `AlreadyEnded` even though the caller holds the watch's `expiration`: a wiring gap, not a missing fact.
- [ ] `P5-005` **(continued — a rule stated in another field's description)**: `google::client` gains
  `HistoryPosition { Storable, UnfinishedWalk, Unstated }` + `of_page`; `gmail_history_signal` takes the type
  rather than an `Option<&str>`; `HistoryPage.history_id` becomes `position`; the `gmail_history_list` **output
  schema's** description states the condition. **2 new tests (so 477 in the crate; 1750 in the workspace).**
  **`ADR-0108`.** Two guards falsified A-B-A. A fifth claim was found while writing the control for the second.
  - **⭐⭐ THE FINDING: THE RULE THAT GOVERNS STORING A VALUE IS IN ANOTHER FIELD'S DESCRIPTION.** The
    `history.list` response's `historyId` is described only as *"The ID of the mailbox's current history
    record."* The condition is stated **once**, in **`startHistoryId`'s** description: *"If you receive no
    `nextPageToken` in the response, there are no updates to retrieve and you can store the returned `historyId`
    for a future request."* So a page's id is the mailbox's position **at the moment that page was produced**, and
    a walk with more pages has not consumed the changes up to it. **⭐ Ask of a rule you are relying on: which
    field's description actually states it — the field you are storing, or one you only ever send?**
  - **⭐⭐ AND FOUR LAYERS OF THIS CRATE ASSERTED THE UNCONDITIONAL FORM, each true of a final page and false of a
    continuing one.** (1) a renderer comment (*"the durable cursor"*); (2) `HistoryPage`'s field doc (*"the next
    sync cursor"*); (3) the **output schema's description** (*"This is the next sync cursor…"*); (4)
    `gmail_history_signal`'s doc inferring *"the mailbox was unchanged"* from an **absent** id the reference does
    not document as optional. **The third is the one that decides the design: an output schema's `description`
    is not documentation for a repository reader — it is an INSTRUCTION TO A MODEL**, delivered in the model's
    own channel. A model told "this is the next sync cursor" stores it. **⭐ A schema description is
    executable-adjacent prose: it is read by the component that acts, so an error in it is a behaviour, not a
    typo.**
  - **⭐ THE DEFECT WAS NOT THE WORDING — IT WAS THAT NO TYPE ASKED THE QUESTION.** The parser produced
    `Option<String>`, the renderer emitted it whenever it was `Some`, and the signal producer took
    `Option<&str>` — so every layer could *describe* the field as a cursor without any layer having to *decide*
    whether it was one. **Four rounds of correcting prose would not have caught this; one type that makes the
    question unskippable does.** The producer now cannot be called with a bare id.
  - **⭐ THREE VARIANTS, NOT A `bool`.** `UnfinishedWalk` and `Unstated` both fail to yield a position and differ
    in *why*: a stated-but-unusable id is **information a diagnostic about a stalled walk wants**, and the second
    must not be read as "the mailbox is unchanged" — which is exactly claim (4). Collapsing them would restore
    that inference in the type system's own vocabulary (`ADR-0035`).
  - **⭐ THE RULE NEEDS BOTH FIELDS, SO ITS ENCODING TAKES BOTH.** `of_page(id, next_page_token)`: an id with no
    token is storable, an id with a token is not, and no id is not a position — which is why folding it into an
    `Option<String>` at the parser loses the distinction **whichever way the `Option` points**. A blank id is
    treated as absent (the reading `VerifiedAccount::new` already takes). `storable()` is named for the
    **question** rather than the value, and there is deliberately **no** accessor returning the id in every state.
  - **⭐ THE RENDERER STILL EMITS THE ID ON A CONTINUING PAGE.** Withholding it would make *"the page stated an id
    you may not use"* identical to *"the page stated nothing"*; the condition now travels with the value through
    the description **plus** the presence of `next_page_token` — the exact fact the provider's rule keys on.
  - **⭐ TWO GUARDS FALSIFIED A-B-A, both compiling.** (1) `of_page` returning `Storable` regardless of the token
    → **detected by two tests.** (2) `storable()` returning the id for `UnfinishedWalk` → **detected by three**,
    including the end-to-end signal assertion (`left: Advanced { history_id: Some("12347") }`,
    `right: … { None }`).
  - **⭐ A FIFTH CLAIM WAS MINE, FOUND BY WRITING THE CONTROL.** My first version of the end-to-end test stated an
    id **smaller** than the fixture cursor's, and it failed — not on the storable rule but on
    `advance_gmail_history`'s **monotonic** guard, which refused a backwards move. So the test would have been
    checking a different guard and *passing for the wrong reason* had the id happened to be larger. The id is now
    greater than the fixture's, **with the reason recorded in the test**: a fixture that triggers a different guard
    is the same defect as one that cannot separate two behaviours.
  - **NEW LIMITS:** **nothing stores a cursor** — there is no sync loop — so the rule is representable and tested
    rather than enforced on a store. No live walk has run: the path is exercised against the two hand-built
    fixtures. A `200` whose page states **no** id advances nothing and is `Unstated`, explicitly *not* "the
    mailbox is unchanged"; the reference does not mark `historyId` optional, so that shape is not one the provider
    documents — which is why the inference is refused rather than guessed.
- [ ] `P5-005` **(continued — a constraint three layers knew and the test contradicted)**: `google::client`
  gains `CalendarContinuation { MorePages, WalkComplete, NothingFurther, Rejected }` + `of_page`;
  `CalendarPage.next_page_token`/`next_sync_token` become `continuation`; `calendar_signal` takes the type; the
  renderer emits **at most one** token; the `calendar_events_read` output schema's two tokens gain descriptions.
  **1 new test (so 478 in the crate; 1751 in the workspace)**, plus a shipped test **corrected**. **`ADR-0109`.**
  Two guards falsified A-B-A — and one of them **survived `--lib`**, which is the second finding.
  - **⭐⭐ THE FINDING: A CONSTRAINT THREE LAYERS KNEW AND NONE ENFORCED.** The `events.list` reference documents
    the two continuation tokens as mutually exclusive **in each field's own description** (`nextPageToken`
    *"Omitted if no further results are available, in which case `nextSyncToken` is provided"*; `nextSyncToken`
    *"Omitted if further results are available, in which case `nextPageToken` is provided"*). **Three layers of
    this repository already stated it** — the research record, the fixture's own
    `_the_point_of_this_fixture` prose, and the renderer's test helper — while `CalendarPage` carried both as
    independent `Option<String>` fields, `calendar_signal` took a bare `Option<&str>`, and the **output schema**
    declared both with **no description at all**. **⭐ Writing a rule down three times is not enforcing it once;
    and the layers that repeat it read as corroboration, so nobody looks for the layer that is missing.**
  - **⭐⭐ AND THE ONE TEST STILL ASSERTING THE IMPOSSIBLE THING WAS THE PARSER'S — which the fixture's own notes
    had already flagged.** The fixture says: *"An earlier test of mine asserted a body carrying both, which
    Google cannot produce."* **The correction reached the fixture, the record and the renderer's helper, and
    missed the parser's test.** So the slice is not "add a type": it is "**the same defect was fixed four times
    in three places that do not run, and left in the one that does**". **⭐ Generalisation: when a correction is
    applied to several copies of a claim, enumerate the copies and check the ones that EXECUTE first.**
  - **⭐⭐ THE SECOND FINDING IS ABOUT THE TEST SUITE, NOT THE CODE: A GUARD WHOSE ONLY DETECTOR LIVED IN ANOTHER
    TEST BINARY.** The mutation routing a **page token through as the sync position** **survived the entire
    `cargo test --lib` run** (477 passed), because `calendar_signal`'s 200 arm had **no unit test** while the
    **Gmail** producer's arm did — the asymmetry was invisible and nobody had a reason to look for it. Only
    `tests/google_fixtures.rs` caught it. **`cargo test --lib` is what a developer runs while iterating, so a
    guard checked only by an integration test is one refactor from being unguarded.** The missing unit test was
    written, and re-running the *same* mutant then fails in `--lib` — **confirmed by mutation, not assumed**.
  - **⭐ FOUR VARIANTS, AND `Rejected` IS A VALUE RATHER THAN A PANIC OR A PRECEDENCE.** The pair is impossible
    against a conforming provider, so the state names a provider change or a hand-built fixture error. A panic
    would turn reportable provider input into a crash; **silently preferring one token is the defect the type
    exists for** — a caller handed the sync token from such a page would store a position for a walk that has not
    finished. Both accessors return `None` for it, so an uninterpretable response yields neither a position nor a
    next page, and `is_nonconforming()` makes the state actionable.
  - **⭐ IT IS NOT `HistoryPosition`, BECAUSE THE RULES HAVE DIFFERENT SHAPES.** Gmail's is a condition on **one**
    value (*"storable only when the page token is absent"*), which leaves an id present and unstorable;
    Calendar's is a relation between **two** (*"exactly one token"*), which leaves no such state. Sharing a type
    would give Calendar a variant its provider cannot produce — the defect being fixed. **⭐ `ADR-0108`'s lesson
    was "no type asked the question"; the answer is not therefore the same type.**
  - **⭐ THE OUTPUT SCHEMA WAS THE LAYER WITH THE LEAST INFORMATION AND THE MOST AUTHORITY.** `ADR-0108` taught
    the **Gmail** output schema to state its condition, and left this one — in the same tool family, in the
    **same slice** — with bare `["string", "null"]`. A model reading it saw two optional tokens and could
    persist the one that expires. **⭐ A lesson applied to one tool and not its neighbour is indistinguishable
    from a lesson not learned.**
  - **⭐ TWO GUARDS FALSIFIED A-B-A, both compiling.** (1) `of_page` **preferring the sync token** when both
    arrive → **detected** by the parser's test. (2) routing a page token through as the sync position →
    **detected only by the integration test** (which is the finding above); after the missing unit test was
    written, the same mutant is **detected by `--lib`**.
  - **⭐ A DUPLICATED VERIFICATION-LOG ROW WAS FOUND AND REMOVED** while appending the new one — the previous
    slice's edit inserted its row twice, and the record had been carrying both. Found by inspection while adding
    to the table, which is the only moment a duplicate of that shape is visible.
  - **NEW LIMITS:** no request sent, no live walk run (two hand-built fixtures); **nothing stores a sync token**,
    so `storable`/`page_token` have no production consumer and each doc says so explicitly (the
    `seconds_from_edge` convention, rather than deleting accessors whose absence would leave a page token
    unreadable while the sync token is not); `Rejected` is unreachable against a conforming provider by design,
    so it is exercised only by the test that deliberately keeps the impossible body.
- [ ] `P5-005` **(continued — a first sync with two documented branches and one expressible)**: `google::connection`
  gains `ResumePoint::FromAnchor`, `SyncOrigin { WatchResponse, Profile }`,
  `resume_anchored`/`resume_ignoring_anchor`, `AnchoredAccount`, and the `establish_account_with_anchor` /
  `establish_account_from_watch` entry points; `ResumePoint::position()` returns `None` for `FromAnchor` and a new
  `anchor()` accessor returns it. **2 new tests (so 480 in the crate; 1753 in the workspace).** **`ADR-0110`.**
  Two guards falsified A-B-A. Joins the anchor that two modules had parsed and described to the step that
  decides where a sync begins.
  - **⭐⭐ THE FINDING: A FIRST SYNC HAD TWO DOCUMENTED BRANCHES AND THE CONNECTOR COULD EXPRESS ONE.** The push
    guide, immediately after defining the anchor, **forks**: *"Your client receives notifications for all changes
    **after** that `historyId`. **If you need to process changes before this `historyId`**, refer to Synchronize
    clients with Gmail."* Branch one is `history.list` **from the anchor** — the changes since the `watch`, then
    a position of its own. Branch two is the mailbox's **existing contents**. `resume_from` answered `FullSync`
    for every `Start` cursor, so **only branch two existed**, and a just-connected mailbox was read end to end at
    `5 + 20N` quota units to discover nothing had happened since the `watch`. **⭐⭐ Generalisation: when a
    provider's sentence contains an "if you need X instead", that is a FORK — enumerate the branches and check
    each is expressible, because implementing the second one looks complete from the inside.**
  - **⭐⭐ AND THE ANCHOR HAD A PARSER, A FIELD, TWO MODULES OF JUSTIFICATION, AND NO CONSUMER.** `watch.rs` calls
    it *"the anchor a first sync starts from"*; `request.rs` says the profile's `historyId` *"yields the mailbox's
    current position without consuming a message"* and that **either can seed a first sync**. **Neither could
    seed anything.** This is `ADR-0092`'s "a response field with no reader" from the other side: a value that IS
    read, is documented as the input to a step, and whose step cannot accept it. **⭐ A doc comment that says what
    a value is FOR is a claim about a consumer — check the consumer exists before believing it.**
  - **⭐ `position()` RETURNS `None` FOR AN ANCHOR, AND THAT IS THE POINT.** An anchor is not a position —
    nothing has been synced from it. The guide's trap is exactly that the two are distinct strings which
    plausibly fit each other (`1234567890` vs `9876543210`, both spelled `historyId`), so the type now refuses
    the reading the guide warns against: `FromAnchor` carries no `position`, and `anchor()` is separate.
  - **⭐ `requires_full_sync()` IS TRUE ONLY FOR `FullSync`.** An anchored start does **not** read the mailbox
    from the beginning, so a caller asking "is this expensive" gets the right answer for both incremental cases
    without knowing which it holds — and the property is what makes the branch a decision rather than a default.
  - **⭐ A STORED POSITION OUTRANKS AN ANCHOR, AND THE WRONG DIRECTION IS A SILENT DUPLICATE.** Once anything has
    been synced the anchor is historical; preferring it would re-read the window between the `watch` and the
    first stored position, and `history.list` would simply return records the store already holds — a duplicate,
    not a miss, and therefore invisible. **The opposite failure direction from `ADR-0090`'s, which is why it
    needed its own assertion.**
  - **⭐ TWO BRANCHES ARE TWO FUNCTIONS, NOT A FLAG.** The difference in cost is orders of magnitude, and a
    `bool` at a call site (`resume_from(cursor, false)`) says nothing about which branch is which. The expensive
    branch is `resume_ignoring_anchor` — a **name**, so a caller cannot reach it by omitting an argument — and it
    takes the anchor and discards it, which is what keeps the two signatures alike enough that neither is called
    by accident.
  - **⭐ `SyncOrigin` IS AN ENUM, NOT A `bool`** (`ADR-0035`): the sources differ in *cost* — a profile read needs
    only a credential and 1 quota unit, a `watch` also needs a Pub/Sub topic — so a caller choosing between them
    is choosing a cost, and the origin is carried rather than inferred so a log line says **where** a starting
    point came from. `establish_account` now shares one body with the two anchored entry points, and carries no
    anchor of its own — a deliberate limit rather than an oversight.
  - **⭐ TWO GUARDS FALSIFIED A-B-A, both compiling.** (1) `resume_anchored` **never anchoring** → detected
    (`left: FullSync`, `right: FromAnchor { … }`). (2) the **position-outranks-anchor** check disabled →
    detected (`left: FromAnchor { … }`, `right: FromPosition { … }`).
  - **⭐ A DOC COMMENT THAT WAS AN OVERSTATEMENT IS NOW CHECKABLE.** `GmailProfile`'s own doc said *"**either**
    can seed a first sync"* about the profile and watch anchors — true of the **values** and false of the
    **connector**, which could seed nothing from either. It is now true of the code, and the code is what runs.
  - **NEW LIMITS:** no request sent, no account stored, no sync runs, so `FromAnchor` reaches a real
    `history.list` only when a sync loop exists. The anchor's **age** is not modelled — a long-held anchor used
    now may have been pruned, which arrives as the documented `404`/resync path rather than as a local refusal.
    And **nothing records which branch a deployment chose**: a caller picks per call, so a connector that used
    branch two for every account would still pay the cost the branch exists to avoid.
- [ ] `P5-005` **(continued — a bound and a lease are different inputs)**: `google::teardown` gains
  `gmail_watch_exposure(lease: WatchLapse, stop_succeeded)` beside the unchanged `gmail_exposure(stop_succeeded)`;
  the `AlreadyEnded` variant doc, the module doc and a stale `notification_exposure` reference are corrected.
  **2 new tests (so 482 in the crate; 1755 in the workspace).** **`ADR-0111`.** Two guards falsified A-B-A.
  Closes the wiring gap `ADR-0107` recorded as its own revisit condition.
  - **⭐⭐ THE FINDING: A BOUND AND A LEASE ARE DIFFERENT *INPUTS*, NOT TWO ENCODINGS OF ONE.** `ADR-0107` built
    `gmail_exposure` from Google's **mechanism bound** — *"at least once every 7 days"* — and
    `calendar_exposure` from a channel's **own lease**, then recorded that Gmail's figure was one-sided: the
    caller **does** hold the watch's `expiration` (`parse_watch_response` reads it; `watch_lapse` turns it into
    `WatchLapse`), so `AlreadyEnded` *could* be reachable but was not. **⭐ "We hold the value" ≠ "the value the
    function takes"** — the same shape `ADR-0106` records for one expiry in three encodings, where the form the
    code held was the unusable one.
  - **⭐⭐ A SECOND FUNCTION, NOT AN `Option` PARAMETER.** `gmail_exposure(Option<WatchLapse>, bool)` would give
    every existing call site a `None` to pass and would make the mechanism's limit and this watch's expiry two
    spellings of one call — the flag-with-a-meaningless-`Option` shape `NotificationExposure`'s own doc already
    rejects for the mechanism itself. **⭐ `ADR-0107` removed a `PushMechanism` enum because a value that only
    distinguishes two mechanisms is a consumerless value; the same argument applies to the INPUT, so the
    function choice names which input is held.**
  - **⭐ THE BOUND-ONLY FIGURE IS KEPT, NOT REPLACED.** A scheduler deciding when to renew holds the mechanism's
    limit and **no** particular watch — that is the caller `WATCH_RENEWAL_BOUND_SECONDS` exists for. Two inputs,
    two functions, both asserted reachable, so neither becomes dead code.
  - **⭐ THE LIVE ARM REPORTS THE LEASE'S OWN SECONDS, NOT THE BOUND.** The reference warns a watch's actual
    expiry *"may return shorter than requested"*, so falling back to the bound for a live lease overstates a
    nearly-expired watch in the exact direction the bound-only figure was already wrong. A falsification
    replaces `for_seconds` with `WATCH_RENEWAL_BOUND_SECONDS` and the test detects it.
  - **⭐ TWO GUARDS FALSIFIED A-B-A**, both compiling: (1) inverting the `stop_succeeded` guard → detected
    (`left: SettlingWithinMinutes`, `right: UntilTheLeaseLapses { seconds: 120000 }`); (2) the live arm's
    `for_seconds` → `WATCH_RENEWAL_BOUND_SECONDS` → detected (`left: … { seconds: 604800 }`, `right: … {
    seconds: 120000 }`). A third mutation — reordering the arms — changes nothing, because `Lapsed` and `Alive`
    are disjoint variants: `ADR-0107`'s lesson applied rather than re-learned.
  - **⭐⭐ A SIBLING TEST ASYMMETRY WAS CLOSED, AND THE SAME MUTANT IS NOW CAUGHT IN `--lib`.** `ADR-0109` found
    that `calendar_signal`'s 200 arm had no unit test while Gmail's did, so a mutant routing a **page token
    through as the sync position** survived the whole `--lib` suite. `CalendarContinuation::storable` and
    `page_token` were still exercised only *through* `calendar_signal`; a test now asserts directly that the two
    accessors are **disjoint** (no value offers both a page to fetch and a position to store) and that `Rejected`
    yields neither. Making `storable` return the page token is now **detected by `--lib`**, confirmed by
    mutation.
  - **⭐⭐ TWO DEFECTS WERE FOUND IN THE RECORD ITSELF WHILE APPENDING TO IT.** (1) A **dangling intra-doc link
    to the `PushMechanism` enum that `ADR-0107` removed** — in the doc paragraph that *says* the enum was
    removed; `cargo doc` warnings are not denied, so nothing compiled it. (2) **The findings were numbered
    `…20, 22, 23`** — `ADR-0108`'s commit renamed the then-existing Finding 21 to 23 and inserted a new Finding
    22, leaving **no Finding 21**, and two subsequent slices appended without noticing. **⭐ A gap in a numbered
    series reads as a DELETED finding**, and a duplicate/omission of that shape is only visible while appending —
    which is what found it. Renumbered to contiguous `21`/`22`; nothing outside the record cites a Finding by
    number, which is what made the renumber safe.
  - **NEW LIMITS:** no teardown executor, so nothing computes the `WatchLapse` and calls either exposure
    function; `users.stop` still deliberately absent; **the stop permission rule still cannot be checked
    locally** (the client id lives inside the credential). A convenience that reads a `WatchResponse`'s
    `expiration` into a `WatchLapse` is **not** built, because it would be a second reader of a value with one
    caller — the consumerless-value defect the same slice's own argument invokes.
- [ ] `P5-005` **(continued — a prescribed call with no builder)**: `google::request` gains
  `calendar_channel_watch(calendar_id, channel_id, address, token)`; `JsonRequest`'s redaction becomes
  body-dependent (hand-written `Debug`, two private named constructors `renderable`/`sensitive`); `webhook_address`
  and the channel-id bound are new. **5 new tests (so 487 in the crate).** **`ADR-0112`.** Four guards falsified
  A-B-A. Closes the **create** half of the channel path.
  - **⭐⭐ THE FINDING: A PRESCRIBED CALL WITH NO BUILDER.** `channels.stop` was built and `renewal_decision`
    prescribes the remedy — *"you must **replace it with a new one by calling the `watch` method`"* — while **no
    `events.watch` builder existed**. So the connector could **end** a channel and had no way to **make** one,
    and `parse_channel_watch_response` plus the whole `ChannelLease`/`ChannelRenewal` chain were reachable only
    from a hand-built fixture. **⭐ `ADR-0092`'s "a value with no reader" from a new direction: not a decision
    without a caller, but a call the documentation names and the connector's own decision function prescribes,
    with no operation behind it.**
  - **⭐⭐ THE THIRD BODY MADE A TYPE'S OWN DOC FALSE.** `JsonRequest`'s `rendered_body` said *"there is no
    credential here"* and the type **derived `Debug`** — true of its two bodies (a topic name with label ids; a
    channel id with a resource id) and false of a **creation** body carrying the webhook `address` and the
    channel **`token`** (the anti-spoofing control `verify_channel_token` compares against). **⭐ A derived
    `Debug` is a claim that every field is printable, and the claim expires when a new field arrives.** The
    redaction is now body-dependent, and the choice is **forced** rather than defaulted: no public constructor,
    two named private ones, so a fourth builder must say which kind of body it produces — the
    "unrepresentable rather than checked" shape this module already uses for a credential in a URL.
  - **⭐ THE `address` IS VALIDATED FOR WHAT A STRING CAN PROVE, AND THE CERTIFICATE RULE IS A RECORDED LIMIT.**
    The guide requires HTTPS *and* a valid (non-self-signed, non-revoked, subject-matching) certificate —
    but a chain is a fact about a TLS handshake a request value with no socket cannot perform. So a callback
    with a bad certificate passes the builder and fails at **delivery** time; pretending to check it would be an
    unverifiable claim. `http://` is refused rather than downgraded, and the scheme compare is case-insensitive
    because a URI scheme is.
  - **⭐ GOOGLE'S 64, NOT THE MODULE'S GENERIC 256.** The channel `id` is capped at *"64 characters"* by the
    push guide, while every other identifier here uses a JARVIS bound of 256. A 65-character id passes the
    generic `resource_id` and is refused by the provider, so the builder checks Google's figure **on top of**
    the shared validator, and the boundary is exercised at 64 (ok) and 65 (refused).
  - **⭐ THE REDUNDANT-GUARD RULE APPLIED.** The token's 256 bound is already enforced by `SecretValue::new`, so
    the builder does **not** re-check it — a second check of the same bound is the guard `ADR-0066` records as
    one that can never decide anything the first did not.
  - **⭐ FOUR GUARDS FALSIFIED A-B-A**, all compiling: (1) `sensitive` setting `false` → the token appeared in a
    `Debug` rendering → **detected**; (2) `strip_https_scheme` accepting any non-empty scheme → `http://`
    accepted → **detected**; (3) the channel-id bound relented to `MAX_RESOURCE_ID_CHARS` → a 65-character id
    accepted → **detected**; (4) **the control** — `renderable` setting `true` → a printable body rendered
    `[REDACTED]` → **detected**, which is what stops a redact-everything `Debug` from satisfying the redaction
    test while destroying the diagnostic value of the two bodies that hold no secret.
  - **NEW LIMITS:** no request is sent and no channel is registered — the **certificate** rule and reachability
    are unverifiable offline; the stop **permission** rule (`ADR-0107`) is still unenforceable locally; **no
    expiry parameter** is offered (the figure acted on is the response's), and **nothing renews on a schedule**
    yet, so `CHANNEL_REPLACE_LEAD_SECONDS` and this builder are not exercised together.
- [ ] `P5-005` **(continued — the value that survives is the one that did not hold it)**: `google::channel`'s
  `ChannelRegistration` gains `expires_at` (the provider's reported expiry), `from_watch_response`, `expires_at()`
  and `renewal(now)`; the three test fixtures and the integration test gain the argument. **2 new tests (so 489
  in the crate)**, plus the end-to-end fixture test extended. **`ADR-0113`.** Three guards falsified A-B-A.
  - **⭐⭐ THE FINDING: THE RECORD THAT SURVIVES THE CALL DID NOT CARRY A VALUE A LATER DECISION NEEDS AS ITS SOLE
    INPUT.** `parse_channel_watch_response` reads the response's `expiration` into `ChannelWatchResponse::expires_at`
    — whose own doc says it *"can drive a renewal decision"* — and `renewal_decision` takes **that expiry as its
    only input**. But `ChannelRegistration`, documented as *"the value that survives between the `watch` and the
    teardown"*, had **no field for it**, so nothing in production held the input and the decision was reachable
    only from a test. **⭐ The same "a value read and then dropped" shape `ADR-0107` found for `resourceId` one
    round earlier, from the same call.**
  - **⭐⭐ AND THE TYPE'S OWN DOC COUNTED ITS FACTS AND THE COUNT WAS WRONG.** It said *"two of the four facts are
    the provider's and two are not"* — there were **five** (three from the response: `id`, `resourceId`,
    `expiration`; two the connector's own: account, token), and the one the count had no room for was the
    expiry. **⭐ A doc that enumerates its inputs is a claim about the set, and an omission inside the
    enumeration is invisible because the arithmetic adds up.**
  - **⭐ `from_watch_response` IS THE CONSTRUCTOR, BECAUSE IT MAKES THE SPLIT UNNECESSARY.** Three of the five
    facts come from the one response; taking the response whole makes the expiry travel with the two identifiers
    beside it rather than leaving a caller to remember a third — which is exactly how it was dropped. The plain
    `new` stays for tests and a caller holding loose values.
  - **⭐ A STORED FACT, NOT A STORED DECISION.** The field is the `UtcTimestamp` and **not** a `ChannelLease` or
    `ChannelRenewal`: those are answers to a question asked at an instant, and storing one would pin it to the
    second it was computed. `renewal(now)` is the bridge and **delegates** to `renewal_decision`, asserted equal
    so the bridge is provably not a second opinion.
  - **⭐ `from_watch_response` REFUSES A BLANK `resourceId`.** The parser checks presence and type but not
    usability, so a blank second stop identifier would build a registration that cannot end its own channel.
  - **⭐ THREE GUARDS FALSIFIED A-B-A:** (1) `renewal()` ignoring the registration's expiry → detected; (2) the
    blank-`resourceId` refusal removed → detected (`left: Ok(… resource_id: "   " …)`, `right: Err(Missing {
    field: "resourceId" })`); (3) `from_watch_response` not carrying the expiry → detected.
  - **NEW LIMITS:** still no caller that renews on a schedule, so `CHANNEL_REPLACE_LEAD_SECONDS` and this bridge
    are not exercised together; a registration is **not persisted**, so the new `UtcTimestamp` field has no
    stored column yet; the expiry is read back from the response rather than recomputed from the request,
    because the guide's *"more restrictive value is used"* means a recomputation would disagree exactly when
    Google shortened it.
- [ ] `P5-005` **(continued — a path identifier validated but not encoded)**: `calendar_events_list` and
  `calendar_channel_watch` now apply `percent_encode` to the calendar id; `resource_id`'s doc is corrected to
  separate log forging from request structure. **1 new test (so 490 in the crate).** **`ADR-0114`.** Two guards
  falsified A-B-A.
  - **⭐⭐ THE FINDING: BOTH CALENDAR PATH BUILDERS INTERPOLATED `calendar_id` RAW, WHILE THE GMAIL ONE ENCODED.**
    `gmail_messages_get` percent-encodes its path identifier, with a comment and a test asserting a `/` becomes
    `%2F` — while `calendar_events_list` and `calendar_channel_watch` put a `calendar_id` in the path **raw**. A
    Gmail message id is an opaque hex string so the omission was invisible; **a Calendar id is routinely a
    mailbox address (`user@example.com`) and a holiday calendar's id contains a literal `#`
    (`en.usa#holiday@group.v.calendar.google.com`)**, which starts a URL **fragment** and truncates the path to
    `/calendars/en.usa` — a calendar that does not exist.
  - **⭐⭐ AND THE ASYMMETRY IS WHAT HID IT.** A reader asking "does this module encode path identifiers?" finds a
    `yes` in the sibling that was written with a message id in hand — the same shape `ADR-0107` records for two
    teardown steps that should have matched. **⭐ Two builders that should behave alike and do not are invisible
    until they are compared side by side.**
  - **⭐ A VALIDATOR THAT BOUNDS LENGTH AND REFUSES CONTROL CHARACTERS DOES NOT MAKE AN IDENTIFIER URL-SAFE**, and
    that is the second half: `resource_id` **accepts** `@`, `#`, `%` and `/` because a real calendar id contains
    them, so it addresses **log forging** and not **request structure**. Its doc implied the check covered URL
    safety; it now states what it does and names `percent_encode` as the control for the other. **⭐ A doc that
    implies coverage is what makes the next builder omit the control.**
  - **⭐ THE WATCH HALF MATTERS MORE.** A wrong path on a read addresses the wrong calendar and fails; a wrong
    path on a `watch` **registers a channel against the wrong resource**, and the channel id is the join key a
    delivery routes on — so the mistake propagates past the call.
  - **⭐ THE SAME `percent_encode`, NOT A PATH VARIANT.** The unreserved set is identical for a query value and a
    path segment, so one function is correct in both positions and a second would be two implementations of one
    rule that could drift.
  - **⭐ TWO GUARDS FALSIFIED A-B-A:** (1) removing the encoding from `calendar_events_list` → detected
    (`…/calendars/en.usa#holiday@…/events`); (2) removing it from `calendar_channel_watch` → detected. The test
    asserts each structural character's **exact** encoding (`#`→`%23`, `?`→`%3F`, `/`→`%2F`, `%`→`%25`) on both
    builders, plus the control that `primary` is left alone, so a partial fix (escaping only the `#`, or only one
    builder) fails rather than passing.
  - **NEW LIMITS:** no request is sent, so the encoding is proved against the builder's own output rather than a
    provider's response; a fourth builder would justify extracting the encoding into a shared URL helper.
- [ ] `P5-005` **(continued — one acknowledgement vocabulary for both push mechanisms)**: `ChannelIngest`'s
  `acknowledges() -> bool` becomes `acknowledgement() -> DeliveryAck`, the same type `GmailIngest` returns; the
  four existing assertions are tightened to name the specific `DeliveryAck`. **2 new tests (so 492 in the
  crate).** **`ADR-0115`.** Three guards falsified A-B-A.
  - **⭐⭐ THE FINDING: ONE INGEST ANSWERED WITH A DECISION AND ITS TWIN WITH A `bool` THAT WAS ALWAYS `true`.**
    `GmailIngest::acknowledgement()` returns a three-state `DeliveryAck` (`Accept` / `Retry` /
    `AbandonAndAcknowledge`); `ChannelIngest::acknowledges()` returned `bool` and **always `true`**, so a caller
    could not tell **a delivery it acted on** from **one it deliberately dropped** — opposite downstream
    consequences (work done versus work irrecoverably discarded), and only one should be recorded as a success.
    The `bool`'s doc even said *"a future variant that should be retried … has a place to say `false`"*: it
    reasoned about the **retry** dimension (always `false` here) and never noticed the other dimension, *which
    kind of yes*, that its sibling carries. **⭐ The same sibling-asymmetry shape `ADR-0107` and `ADR-0114` found
    — two things that should behave alike, each internally consistent, invisible until read side by side.**
  - **⭐ A METHOD WHOSE RETURN IS CONSTANT IS THE SHAPE TO CHECK.** "All variants return `true`" is true of the
    *acknowledgement* and false of the *reason*, and the reason is the half a caller acts on. **A value that is
    always the same is either a fact worth asserting or a type that is missing a variant.**
  - **⭐ ONE METHOD, NOT TWO NAMES.** The richer type loses nothing because `DeliveryAck::acknowledges()` is the
    predicate, so a caller wanting "does this acknowledge" asks `outcome.acknowledgement().acknowledges()` — the
    old name is **not** kept as a shim, because two names for one question is the duplication this repository
    removes everywhere else.
  - **⭐ THE `ADR-0094` ARGUMENT IS UNCHANGED AND NOW CARRIED BY A TYPE.** Every variant still acknowledges
    (none of the three refusals is repaired by another attempt, so refusing would be charged to the whole
    subscription) — only **which reason** becomes visible, and a call site now reads `AbandonAndAcknowledge`
    instead of trusting a `true`.
  - **⭐⭐ THE PARITY IS ASSERTED AS A CROSS-MECHANISM EQUIVALENCE, NOT A COMMENT.** Two tests drive the **same**
    outcome kind through **both** mechanisms and assert the acknowledgements are equal — accepted
    (`Changed` ↔ `Changed`) → `Accept`; unroutable and refused → `AbandonAndAcknowledge` — so a future
    divergence fails a test. Reverting to the "always `Accept`" behaviour is **detected by that parity test**,
    confirmed by mutation.
  - **⭐ THREE GUARDS FALSIFIED A-B-A:** (1) the refusals reporting `Accept` → detected; (2) an accepted delivery
    reporting a drop → detected; (3) reverting to always-`Accept` → detected by the parity test.
  - **⭐ THE FOUR EXISTING ASSERTIONS WERE TIGHTENED, NOT RELAXED** — each `assert!(outcome.acknowledges())`
    became an `assert_eq!(outcome.acknowledgement(), …)` naming the **specific** value, so the tests pin the
    reason as well as the acknowledgement (the "assert the specific value, not `is_ok`" rule).
  - **NEW LIMITS:** no handler sends an acknowledgement yet, so the `Accept`/`AbandonAndAcknowledge` distinction
    reaches a metric only when a push handler exists; a transient store failure is still not an ingest outcome,
    so `DeliveryAck::Retry` has no user in either mechanism.
- [ ] `P5-005` **(continued — a field with a producer and no reader)**: `ConnectorHealth` gains
  `missing_scopes()`; `diagnostics_for` consults the state's list as well as the caller's and emits one finding;
  a dangling `is_stale_at` doc link is corrected. **2 new tests (so 494 in the crate).** **`ADR-0116`.** Two
  guards falsified A-B-A.
  - **⭐⭐ THE FINDING: `NeedsReauth`'s `missing_scopes` WAS WRITTEN EVERYWHERE AND READ NOWHERE.**
    `ConnectorHealth::NeedsReauth` carries the list, documented as *"the scopes that are missing, when the
    reason is a scope loss"* — and there was **no accessor**, while `diagnostics_for` matched
    `NeedsReauth { reason, .. }` (discarding the field) and emitted `MissingScopes` — described as *"the list a
    reauth prompt needs"* — from a **separate argument**. So a caller that built a `ScopeLoss` state carrying
    scopes and passed an empty shortfall produced a report with **no** missing-scope finding at all.
    **⭐ `ADR-0092`'s "a value with a producer and no reader", and the field's own doc made a claim about a
    consumer that did not exist.**
  - **⭐⭐ AND THE TWO LISTS HAD TO AGREE WITH NOTHING MAKING THEM** (`ADR-0021`): the state's list and the
    caller's described one fact, only one reached the report, and they could differ.
  - **⭐ BOTH SOURCES ARE CONSULTED, DELIBERATELY** — a shortfall **without** a reauth (a partial consent the
    account still runs under, `AccountStatus::ScopeShortfall`) is only ever in the caller's list, while a
    `ScopeLoss` records the scopes in the **state** (the value a persisted health record holds). The union is
    the honest answer and it closes the drop. `MissingScopes` is emitted **once**, because it describes the
    condition rather than each scope.
  - **⭐ THE ACCESSOR IS THE `reauth_reason()` SHAPE** — a slice, not an `Option<&Vec>`, so a caller branches on
    `is_empty` rather than matching every variant; an absent list and an empty one call for the same action.
  - **⭐ TWO GUARDS FALSIFIED A-B-A:** (1) `missing_scopes()` returning empty for `NeedsReauth` → detected by
    **both** the accessor test and the diagnostics test; (2) `diagnostics_for` reading only the caller's argument
    → detected. A `let _ = missing_scopes;` was removed — the tell that the parameter decided nothing.
  - **NEW LIMITS:** nothing renders the report yet, so the union's contents are asserted rather than shown; a
    shortfall without a reauth still has no `ConnectorHealth` variant, so the caller's argument is not yet
    redundant.
- [ ] `P5-005` **(continued — a diagnostic that contradicts the predicate the platform gates on)**:
  `DiagnosticField::HealthStale` added; `diagnostics_for` takes `now` and `freshness_seconds` and derives the
  health severity from `permits_calls_at`. **2 new tests (so 496 in the crate).** **`ADR-0118`.** Three guards
  falsified A-B-A.
  - **⭐⭐ THE FINDING: THE REPORT RANKED A STATE BY THE WRONG PREDICATE, AND THE TWO FACTS NEEDED TO SEE THE
    CONTRADICTION WERE NOT IN THE REPORT.** `security.md`'s "missing or stale evidence fails closed" is enforced
    by `ConnectorHealth::permits_calls_at(now, bound)` — whose module doc calls it *"the method a caller should
    use"* and says calling `permits_calls` on an unfresh state *"is the defect this exists to prevent"* — and
    `diagnostics_for` derived `HealthState`'s severity from **`permits_calls()`** alone. So a `Connected`
    observation from yesterday was reported as `connected` at **`Info`** (the severity that means *nothing to
    do here*) about a state that permits **no call**.
  - **⭐⭐ THE REPORT CARRIED `HealthObservedAt` AND NEITHER THE BOUND NOR *NOW*.** So an operator could not even
    *see* the contradiction: the rule turns on `elapsed` versus a bound, and one of the two operands was in the
    report and the other two were not. **⭐ `ADR-0092`'s "a value with a producer and no reader" — the instant
    was there and nothing about it.** **⭐ A diagnostic that contradicts the predicate the rest of the platform
    gates on is worse than a missing one: it is read *instead of* the truth.**
  - **⭐ `DiagnosticField` IS DOCUMENTED AS A CLOSED SET OF FACTS A DIAGNOSTIC MAY REPORT, CHOSEN SO
    `is_loggable()` IS TRUE FOR ALL OF IT.** "This state is too old to act on" is such a fact, and the set had
    no variant — so the platform's central rule was the one thing the report could not say. The count assertion
    (20 → 21) was updated in the same change, so the new field is covered by the loggability, model-exposure,
    renderability and distinctness assertions.
  - **⭐ `now` IS A PARAMETER, NOT A CLOCK READ** — the reason the health module's own `is_fresh_at` records:
    a value that asked the system clock about its own age could not be checked against a supplied instant, and
    `jarvis_core::Clock` exists so time is injectable. The daemon owns the clock; this function owns the policy.
  - **⭐⭐ A MUTANT SURVIVED THE FIRST VERSION OF THIS CHANGE, AND THAT IS THE SECOND FINDING.** With
    `stale = !health.permits_calls_at(now, bound)` in place of `!is_fresh_at(now, bound)` **the entire suite
    passed** — every state the staleness test used was `Connected`, where the two predicates **agree**. They are
    different facts: a `NeedsReauth` observed a moment ago **permits no call** and **is fresh**, and the mutant
    would have told an operator "nobody has checked since" about a state that was just checked. The missing
    detector is a state that **refuses and is fresh**; `staleness_and_unusability_are_two_dimensions_and_a_
    mutant_is_why_this_exists` is it, and it was confirmed to fail under that mutant **after** being written.
    **⭐ A predicate with two conjuncts is exercised by a value where they DIFFER, and a fixture where they
    agree cannot see the difference.**
  - **⭐ THREE GUARDS FALSIFIED A-B-A:** (1) the freshness inputs ignored entirely → detected by **both** tests;
    (2) staleness conflated with unusability → detected by the **second** test only, after the gap was closed;
    (3) the supplied bound ignored (`u64::MAX`) → detected by both.
  - **⭐ THE SEVERITY IS DERIVED, NOT STORED, AND THE TWO FINDINGS NEITHER SUPPRESS NOR IMPLY EACH OTHER** — a
    stale healthy state is an error **and** stale; a fresh refusal is an error and **not** stale; a stale
    refusal reports both. `HealthStale` is emitted **only when true** (the `MissingScopes` rule), because a
    negative finding that is always present is one a reader stops seeing.
  - **NEW LIMITS:** nothing renders the report yet, so the two severities are asserted rather than shown together;
    the report has no ordering rule beyond emission order; a second source of staleness (a cursor's age, a
    lease's expiry) would repeat "value plus bound plus now" a third time and may deserve one type.
- [ ] `P5-006` Research Microsoft identity platform and Microsoft Graph mail/calendar, subscriptions, delta queries, and limits; record findings.
- [ ] `P5-007` Implement Microsoft connection setup and Outlook/Calendar read tools with recorded wire fixtures.
- [ ] `P5-008` Research and implement GitHub authentication and read tools.
- [ ] `P5-009` Add draft/write operations behind policy and approval with provider idempotency where available.
- [ ] `P5-010` Add reauth, token expiry, revoked scope, pagination, throttling, webhook replay, and redacted diagnostics tests.

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

## P10: Server And Multi-Device

- [ ] `P10-001` Define server deployment profile, TLS termination, trusted proxy rules, backup/restore, and operator runbooks.
- [ ] `P10-002` Complete PostgreSQL parity and concurrency tests for every canonical repository.
- [ ] `P10-003` Implement device enrollment, pairing proof, scoped credentials, rotation, revocation, and lost-device response.
- [ ] `P10-004` Implement remote access with explicit enablement, secure defaults, rate limits, and exposure audit.
- [ ] `P10-005` Add team membership and roles only after workspace isolation is proven.
- [ ] `P10-006` Establish service-level objectives, load profiles, recovery objectives, restore drills, and capacity evidence.
- [ ] `P10-007` Evaluate optional infrastructure only with measured bottlenecks and an ADR per adoption.
