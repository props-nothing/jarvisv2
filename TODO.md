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
        **The limit every tool slice restated, closed.** `P3-005`, `P3-006`, `P3-006d`, and each of
        `P3-008a`..`P3-009c-b` recorded a version of "nothing persists an `ApprovalRequest`": an
        `AwaitingApproval` outcome returned a call id and a strength and wrote **nothing**, so the call row
        stayed truthfully `requested` forever and the documented resume path (`security.md`: "a trusted
        desktop/mobile/CLI approval may resume a voice-originated run") had no record to resume against.
        `ToolPipeline::authorize_and_admit` now returns an `Admission::{Runnable,Held}` and a held decision
        writes an `ApprovalRequest` bound to the **call's own canonical intent**, so a decision binds to the
        action rather than to a description of it. `AwaitingApproval` carries the `approval_id`, and the
        gateway replies with it beside the call id.
        **A held call has no receipt, and that is why the value types differ.** `AuthorizationReceipt::new`
        refuses a `RequireApproval` with no citation (`P3-006a`), because a receipt is what an adapter treats
        as **permission** and none exists yet — so a hold could only produce one by inventing a citation for a
        decision nobody made. `Admission` is therefore two variants rather than `Option<PreparedCall>`: the two
        cases carry different values, and filling a hold's facts into the runnable type would mean writing
        placeholders for fields only the runnable case has.
        **⚠ The call is now admitted BEFORE the receipt is built, and that reordering is safe for a stated
        reason.** `CallBinding` takes the receipt identifier as a parameter, and the pipeline passes the
        call's own id for both fields. That is not a fabrication: a call that is never authorized cannot cite
        a receipt, and the ledger key is `(run_id, idempotency_key)` rather than the receipt, so the binding
        value is not part of any uniqueness constraint. The alternative — keeping the old order — meant
        building a receipt whose constructor refuses exactly this decision.
        **The identity decision, which is the security content of the slice.** The domain refuses an approval
        whose approver equals its requester, and `security.md` names the threat as **model self-approval /
        confused deputy**. For that refusal to do any work in a single-owner profile the requester must be the
        **agent acting for the run**, because the person is the only identity eligible to approve anything
        here — recording the user as the requester would make the two equal by construction and turn the guard
        into a check that can only refuse real work, the shape `ADR-0022` removed from cancellation. So the
        requester is the run, which the call's own origin already records for the same reason, and the
        approver is chosen by the decide path and never travels in a request body.
        **The preview names the tool and version, not its arguments** — deliberately, and the reason is the
        bound rather than tidiness. The arguments are already bound by the intent digest, so a preview is
        *additional* context; rendering arbitrary model-authored arguments into a field `P3-004` caps at 512
        characters would fail the hold for a tool whose arguments are simply large. A rich, effect-shaped
        preview is `A11`'s job, where the content is a message or an event this layer can summarize.
        **Falsified, one property each.** Removing the `create_approval` call fails with `ApprovalNotFound`
        (the test reads the row back by the `approval_id` the outcome carried, so it cannot pass on any other
        row). Replacing the requester with `LOCAL_USER_ID` fails with the run id vs the user id, so the
        identity assertion is not vacuous. Both mutations were restored and the suite re-run green.
        **⚠ Two traps hit during this slice, both already recorded and both worth restating.** A
        PowerShell `-replace` on a multi-line anchor **silently no-opped** because the working copy has LF
        while the anchor used `r`n`, so the first falsification passed for the wrong reason — the fix is to
        assert the string actually changed (and prefer a single-line anchor). And `Copy-Item` restoring a
        `.bak` **restores the older timestamp**, so cargo skipped the rebuild and the restored suite looked
        red; the working copy was correct and `cargo` was running the mutated artifact. *A falsification that
        passes is a finding about the harness, not a reprieve.*
        **Limits, recorded rather than glossed:** nothing decides the approval yet and nothing resumes the
        call, which is `P3-012b`. A held call has **no durable link to its approval** (the `tool_calls` row's
        `approval_id` column is a migration away), so the association is carried in an outcome value and a
        log rather than a join — `P3-012c`. Nothing writes a `run_events` row, and a run does not park in
        `awaiting_approval`: the hold is a **tool-level** fact until the executor path (`P3-012c`) makes it a
        run-state change. Gates: fmt, clippy `-D warnings`, 44 suites / `--all-features` / `--locked` green,
        `cargo deny check` ok.
  - [x] `P3-012b` Deliver the decision nonce so a stored approval can be decided. (The decision route and the
        resumption of the held call remain, recorded below.)
        **Recon done, and it found the crux before any code was written.** A stored approval **could not
        be decided by anything**, and the reason is a design tension rather than a missing function:
        `DecisionNonce` is generated into `record_hold`, stored only as a SHA-256 **digest**
        (`ADR-0018`), and the plaintext was **dropped** when the hold returned. `record_decision` requires
        the plaintext and compares it against that digest, so `expose()`/`nonce_for_storage()` had **no
        production reader**. The nonce is meant to be **presented**, which needs an out-of-band delivery
        channel that did not exist.
        **DELIVERED: the delivery channel (`ADR-0042`).** New `crates/jarvis-storage/src/secret_store.rs`
        holds the plaintext nonce in a **profile-private file** and `ToolPipeline::record_hold` writes it
        immediately after the approval row. This is the shape the daemon already uses for its own client
        credential: the daemon **issues** a secret into a file the human's account can read, and the
        secret never travels in a request body and is never returned to whoever asked for the action.
        Permission hardening reuses `paths::secure_private_file`, so `0600` on unix and the hardened DACL
        on Windows keep one home.
        **⭐ Why not the obvious place.** Returning the nonce in the hold's response would look like a fix
        today and become a **self-approval primitive** the moment `P3-012c` routes model→tool: that
        response travels the tool-call path, whose requester is the **run** — and the nonce exists to
        defeat *model self-approval*. The one identity that must never receive it is the one that would.
        Deliberately **not** "derive it from a server secret" either; `ADR-0018` already rejected that
        because a derived value stays valid until the secret rotates and so is not one-time.
        **The one-time property is enforced by the filesystem.** `SecretStore::take` removes the file
        **before** returning the value, so two concurrent decisions race on one `remove_file` and exactly
        one wins; the failure direction is the safe one — a crash loses the nonce and makes the approval
        **undecidable** rather than **reusable**. A corrupt stored value is consumed as well as reported,
        so a retry cannot repeat the read.
        **`ApprovalRequest::nonce() -> &DecisionNonce`, never `&str`.** The nonce and its digest are both
        fixed-length lowercase hex, so a caller holding a `&str` cannot tell them apart and a **digest
        passed where a nonce belongs would be written out as the secret** — recreating exactly the flaw
        `P3-004` found. The typed return makes "the digest is not the nonce" a property of the type system.
        **The identifier is parsed, not sanitized**: `path_for` refuses anything that is not an
        `ApprovalId` *before* joining a path, so `../../escape` has to be unrepresentable rather than
        escaped.
        **Proved end to end, which no unit test could show:** hold a call, take the nonce the way the
        operator's client would, decide as the **human** and land `approved`. The gap was *between* a
        correct store and a correct pipeline. Also asserted: deciding as the **run** is refused (so
        `P3-012a`'s requester choice now has an executable consequence), and a refused decision does
        **not** burn the nonce (the domain refuses after the digest matches and the guarded UPDATE rotates
        only when the write lands) — so one honest mistake does not destroy an approval.
        **Falsified, one property each:** deleting the `secrets.store(..)` call fails with `Absent`;
        deleting the `remove_file` in `take` fails `a nonce must not be presentable twice` **and** the
        corrupt-value test. Both mutations restored, both suites re-run green.
        **⚠ The stale-mtime trap again:** `Copy-Item` restoring a `.bak` restores the **older**
        timestamp, so cargo skipped the rebuild and the restored suite appeared red while the working copy
        was correct. Touch the file before believing a post-restoration failure.
        **DELIVERED: the decision route, and it forced a correction to the control itself.**
        `POST /api/v1/approvals/{id}/decision` (`jarvis-protocol::ApprovalDecisionBody`) closes the slice;
        the approver is the **local identity**, never a request field, and `deny_unknown_fields` makes an
        attempt to supply one a `422` rather than a silently ignored value.
        **⭐ The first cut was wrong, and the failing test is what exposed it.** It had the daemon `take`
        the nonce from the file and present *that*. But the file is a **delivery channel** — the operator's
        client is its legitimate reader — so the daemon consuming it would refuse a decision the operator
        had every right to make, and the route's answer would depend on filesystem state rather than on the
        caller's credential. Fixed: the daemon now verifies the **presented** nonce via `record_decision`
        (which compares it to the stored digest and rotates that digest on a landed write) and then
        `discard`s the file. **The digest rotation is the one-time mechanism; the file removal is defence
        in depth.** A `discard` failure is **logged, not reported** — telling an operator a decision *did
        not happen* when it did is a worse answer than a stale file.
        **A refused decision therefore consumes nothing**: a wrong nonce, a self-approval, or a lapsed
        approval leaves the nonce valid and the operator retries. Burning an approval on a retryable error
        turns a typo into a fresh tool call, which is how an approval flow gets routed around.
        **Falsified through the route, one guard each:** forcing `stored != digest(presented_nonce)` false
        made a **forged nonce decide an approval** (`200` where `403` was asserted), and removing
        `deny_unknown_fields` made an `approver_id` field **silently accepted** (`200` where `422` was
        asserted). Both restored byte-identically (`git status` clean) and re-run green: 81 daemon tests.
        Gates: fmt, clippy `-D warnings`, 44 suites with the application binaries absent and **zero
        skips**, both phase gates with `ACCEPTANCE_REQUIRE_BINARIES=1`, `cargo deny` ok. Limits recorded
        in `ADR-0042`: the secret is cleartext protected by filesystem permissions only; nothing sweeps a
        lapsed approval's nonce file.
  - [x] `P3-015` Honor a tool's own `ApprovalPolicy::Ask` declaration (**production defect, found by a test**).
        `evaluate`'s step 7 consulted the **workspace** policy and the risk threshold but **never the
        tool's own `approval()`**, so a tool declaring "always ask me" was auto-allowed whenever the
        workspace did not require approval for that risk. Every existing `Ask` fixture happened to be
        risk 2 or 3, where the risk threshold holds the call anyway — so the missing check was **masked by
        the fixtures**, and only a **risk-0 `Ask`** tool exposes it. Found because the route test could not
        produce a held call from a read-only tool. Fixed: step 7 now ORs the tool's declaration with the
        risk and external requirements, and `ApprovalPolicy::Policy` is deliberately **not** treated as
        `Ask` (deferring to policy is the opposite of always asking). Two tests: a tool declaring `Ask` at
        risk 0 is held; a tool deferring to policy at risk 0 runs. **Falsified**: forcing
        `tool_requires_approval = false` returns `Decision::Allow` and the new test fails.
        **The lesson recorded here:** a fixture that shares the bug's assumption cannot catch the bug, and
        a threshold in front of a check can hide that the check is absent.
  - [x] `P3-016` Link a held call to its approval and to the run's event stream.
        **The gap was a column with no writer.** `tool_calls.approval_id` existed since `0007`, nullable and
        with its foreign key, and **nothing ever set it**: `admit_tool_call` runs before `create_approval`
        (the write order `P3-012a` fixed so a crash leaves an undecidable approval rather than a dangling
        secret), so the identifier did not exist at insert time and no second statement set it either. The
        consequence was that a decision moved the approval to `approved` and nothing could find the call
        waiting on it — the two rows shared no column, so there was **no join from a decision back to its
        subject**. `link_tool_call_approval` is that join, written by `ToolPipeline::record_hold` after the
        row and the secret.
        **⭐ Write-once, and the falsification is what made that worth asserting.** The `UPDATE` requires
        `approval_id IS NULL`, so a link cannot be **moved**. A re-pointable link would let a call
        authorized under one approval be resumed under another — a way to obtain an effect the operator
        never decided, because their decision was about a *different* action. Linking again to the **same**
        approval is a no-op, following `record_tool_outcome`'s rule that a retried write of one fact is not
        an error while a changed fact is.
        **⚠ The falsification found a coverage gap, which is the point of running it.** Deleting the
        `AND approval_id IS NULL` guard left **every one of the 149 tests green** — no test linked a call
        twice, so the guard was unprotected. Two tests were added (`a_call_links_to_its_approval_exactly_once`,
        `a_link_requires_both_ends_to_exist`) and the same mutation then **failed** with
        `ToolCallApprovalLinkConflict` expected. *A guard with no test is a guard the next person deletes.*
        **The event stream now shows a call, in order.** `ToolPipeline` appends `tool_requested` before
        admitting a call and `approval_requested` when a hold is recorded, so a client replaying the stream
        sees the run ask for something and then block on a human, without reading a second table. The
        `P3-012` limit "no `run_events` row is written for a tool call" is closed for this path.
        **A refusal writes nothing, deliberately.** The event is emitted after the policy decision allows,
        so a `Deny` produces no row — otherwise a caller could fill the run's log by asking for tools it may
        not use, making a refusal a write primitive. Asserted separately
        (`a_refused_call_writes_nothing_to_the_run_stream`) so a future change that emits on refusal fails
        there.
        **Migration `0008` adds an index and nothing else, after a first attempt was withdrawn.** The first
        version rebuilt the whole table to "tidy" the column, which re-stated every `CHECK` by hand and
        **silently widened two byte bounds into character bounds** — `evidence` 256→512 and `output` from
        bytes to chars, in the column holding untrusted provider text. Those bounds are written in **bytes**
        on purpose, so a multi-byte payload cannot smuggle past a character count. The column was already
        correct; only a partial index (`WHERE approval_id IS NOT NULL`) was missing. *Re-typing a constraint
        is how a bound changes.* No backfill: a guessed link could connect a call to a decision that was
        never about it, and an unlinked call (visible, never resumes) is the safe direction.
        Gates: fmt, clippy `-D warnings`, 44 suites with the application binaries absent and **zero skips**,
        both phase gates with `ACCEPTANCE_REQUIRE_BINARIES=1`, `cargo deny` ok. 151 storage tests, 83 daemon
        tests.
        **Recorded as a limit:** the link makes a held call **findable**, and nothing yet **resumes** it. A
        decision moves the approval to `approved` and the call stays `requested`; the resumption needs the
        approver identity from a decided approval, which `ApprovalRequest` does not currently expose (the
        row stores `decided_by`; the domain type returns only the decision), so it is a domain change rather
        than a call-site one. That is `P3-012c`'s remainder and is where a duplicate delivery would become a
        second effect.
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
        server and saying nothing else — holds a call, reads the delivered nonce out of the profile's private
        store, **kills the daemon**, restarts it, decides the approval, resumes the call, and asserts the
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
      tombstone table. The same technique `ADR-0018` uses for a decision nonce.
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
    none, so it exists for `P5-009`); **the output rendering is not validated back through the schemas it claims
    to match**; and **nothing constructs the adapter in a binary** — no registry, executor, or route offers
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
  - **The research record's Verification Plan is now auditable.** It had eleven items and said "none exist yet";
    each is now marked **WRITTEN** or **Not written**, with the one written item named and the six fixtures
    listed as hand-built. A plan where every line is unmarked reads as done; a plan where every line is marked
    can be checked. The live smoke test remains **not written**.
  - **NEW LIMITS:** **no fixture is a capture and no live call has been made** — no credential exists, no Cloud
    project was created, no Google API was contacted; **a hand-built fixture cannot reveal a constraint nobody
    documented**, so a rule Google enforces but does not write down would still be invisible; **the fixtures do
    not cover every declared operation** — `history.list` (the operation Finding 2 most depends on), the batch
    endpoint, and the `format=metadata` envelope have **no** fixture at all; **the sweep checks provenance and
    not shape**, so nothing validates a fixture against the output schema `ADR-0059` derives; and
    **`_shape_documented_at` is a URL nothing re-checks**, so a fixture can drift from the page it cites with no
    assertion failing.
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