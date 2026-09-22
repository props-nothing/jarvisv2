# ADR-0019: A tool call is a durable lifecycle row, and a terminal outcome is final

**Status:** Accepted

**Date:** 2026-09-22

## Context

`P3-005` requires "tool execution lifecycle, bounded output, idempotency ledger, audit receipt, and
honest outcome states (`requested`, `submitted`, `confirmed`, `failed`, `unknown`)". Four documents
constrain it, and one of them forces a crate boundary.

`docs/architecture/tools-and-connectors.md` gives the pipeline: schema validation, authentication,
authorization, risk classification, approval policy, timeout, idempotency, execution, and audit,
ending at "Persist result and decision receipt". It states the rule the whole slice turns on:
"Do not turn a success-sounding string into proof. Preserve provider IDs and receipts separately from
user-facing text."

`docs/architecture/events-and-workflows.md` states that "Approval is not a permanent bearer token" and
that on approval, execution "revalidates current identity, policy, connector/account health, target
state, and intent hash" — so a call carries the evidence it was authorized against, not a flag saying
it was authorized.

`docs/architecture/overview.md` requires that "all tool calls pass through schema validation,
authentication, authorization, risk classification, approval policy, timeout, idempotency where
relevant, execution, and audit", and `AGENTS.md` adds that "the model may request an effect;
deterministic Rust policy decides whether it may happen".

`docs/architecture/repository-layout.md` sets the dependency direction: domain crates define
interfaces and policy, infrastructure crates implement them, and provider SDK types must not cross
domain boundaries. `jarvis-storage` is an adapter. `jarvis-tools` is a domain-pipeline crate. Those two
facts are what produced the first decision below.

The requirement that made this slice more than a table was **honest outcomes**. The five states are not
a status enum with a happy path; they encode what is *known* about an effect, and the difference
between `failed` and `unknown` is the difference between "nothing happened, retry" and "something may
have happened, do not retry". Getting that distinction wrong is how one sent message becomes two.

## Decision

**1. `ToolOutcome` and `ToolOutcomeRecord` live in `jarvis-core`, not `jarvis-tools`.** They were
written in `jarvis-tools` first and moved. The reason is the adapter direction: a durable call row must
carry its outcome, so `jarvis-storage` needs the type, and `jarvis-storage` is an adapter that cannot
depend on `jarvis-tools`. A state machine about what is known to have happened is domain vocabulary, so
core is where it belongs rather than duplicating the enum in storage. This is recorded because it is
counter-intuitive: `P3-001` put the outcome vocabulary in `jarvis-tools`, and the move looks like
churn without the layering argument.

**2. The idempotency ledger is the `UNIQUE (run_id, idempotency_key)` index, not a second table.**
`admit_tool_call` is one `INSERT ... ON CONFLICT DO NOTHING`. When zero rows are affected, the existing
call is read and returned as `DatabaseError::ToolCallDuplicate { existing_call_id }` so the caller
**adopts** the existing call. A separate ledger table would need its own write ordering against the call
row and would be a second thing to keep consistent with an effect that already happened.

**3. The idempotency key is generated at admission and is not derived from the intent.** The intent
digest must be deterministic, because an approval binds to it (`P3-004`, ADR-0018). The key must be
**unique per logical call**. Deriving it from the intent would collapse "archive this folder, then
archive it again after a new message arrived" into one call, which is the exact case the key exists to
distinguish. Two calls with an identical intent hash and different keys are asserted to be two calls.

**4. A terminal outcome cannot be replaced, and a repeat of the same outcome is a no-op.** Enforced in
`record_tool_outcome`: a terminal stored outcome equal to the incoming one returns the row unchanged
(so a retried write is not an error), and a terminal stored outcome that differs is refused with
`ToolCallAlreadyResolved`. The case that matters is `Unknown` quietly becoming `Failed` after a
re-drive: `Unknown` means the effect may have happened, so overwriting it with "nothing happened" is
how a second message gets sent.

**5. The domain owns the transition table and the store re-uses it.** `ToolOutcome::can_advance_to`
defines the legal edges and `record_tool_outcome` calls it rather than restating the edges in SQL. The
table is strict — `requested -> submitted` is **refused**, because a call that never passed
`authorized` has no receipt and an adapter must never be handed one. That edge was added to the
falsification tests after it was found to be the one a test author gets wrong.

**6. `reported_at` is stamped by `ToolOutcome::is_reported()`, not by `is_terminal()`.** `Submitted` is
an adapter report — the adapter has been handed the request and may have reached the provider — while
`Authorized` is a pipeline state that has not. A predicate phrased as "terminal outcomes were reported"
would leave `Submitted` unstamped, so a row would say it was sent with no time for when. The predicate
lives in core so the migration's `CHECK` and the write path cannot drift.

**7. Output is bounded and a truncation flag is a flag, not text.** `BoundedOutput` has two
constructors with different contracts: `strict()` refuses an oversized value and `truncating()` always
succeeds by cutting on a char boundary. `truncated` is a separate boolean because a marker embedded in
the content could be produced by the tool itself, so a reader could not tell truncation from a tool that
wrote the words. The 32 KiB bound is enforced **twice** — by `BoundedOutput` before the write and by the
migration's `length(CAST(output AS BLOB)) <= 32768` — so a caller bypassing the type is still refused.

**8. Provider evidence is a separate, much smaller field than output.** `MAX_PROVIDER_EVIDENCE_CHARS` is
256 and output is 32 KiB, and the migration's `CHECK` refuses a `confirmed` row with no evidence. This is
the mechanical form of "preserve provider IDs and receipts separately from user-facing text": a
success-sounding sentence in `output` cannot become proof, because proof is a different column with a
different bound and a `NOT NULL` requirement.

**9. A failed outcome has no output.** `ToolOutcome::failed` carries a bounded **reason**; `confirmed`
carries **evidence**; neither can be constructed empty, and `bounded()` reports "you supplied none" and
"yours is too long" as different errors, because collapsing them sends a reader looking for a length
problem in an empty string.

**10. The executor port is deliberately missing two things.** `ToolExecutor` receives a
`ToolExecutionRequest` with no secret resolver and no cancellation token. Connector token lifecycle is
`P3-008` and cancellation needs a run to signal, which is `P3-011`; adding fields now would create a
capability with no owner. `AdapterError` distinguishes `RefusedBeforeReaching` from
`AmbiguousAfterReaching` because that distinction is what the caller maps onto `failed` versus
`unknown` — the port makes the honest answer expressible rather than leaving the adapter to guess.

## Consequences

- A re-driven pipeline cannot send a message twice, and the mechanism is one unique index rather than a
  check a caller must remember to write.
- A crash between "sent" and "recorded" leaves a row that `must_not_repeat()` is true for, because
  `Submitted` and `Unknown` both may have had an effect. A recovery path has one predicate to ask.
- `Submitted` is not a transient state that must be resolved before the row is useful. It is a complete
  statement: the adapter has the request, and the outcome is not yet known.
- The five outcome states each have a test asserting the distinct thing it means, so a missing state is
  visible rather than implied by the enum having five variants.
- `jarvis-tools` gained an implementation file per concern (`execution.rs` for the record,
  `executor.rs` for the port) rather than one large module, so the durable vocabulary and the adapter
  contract are separately reviewable.
- **Nothing drives a tool yet.** See the limits section below; this slice is the lifecycle and the port,
  not a tool.

## Honest limits at the time of this decision

- No adapter implements `ToolExecutor`. The only implementation is a `ScriptedExecutor` in tests.
- No caller admits a tool call. No run reaches `authorized`, `submitted`, or any outcome. The repository
  and the port exist and are exercised by tests only.
- `apps/jarvisd` has no tool pipeline: no schema validation, no policy evaluation call, no approval
  request, no execution. `P3-006` (the read-only filesystem tool) is the first real adapter and the
  first consumer, and `P3-012` is where the Phase 3 gate proves the whole path.
- The audit receipt is stored as a document produced upstream; nothing here generates one, and no
  `run_events` row is written for a tool call. Linking calls to the durable event log is not yet done.
- `MAX_POLICY_VERSION_CHARS` and `MAX_PROVIDER_EVIDENCE_CHARS` are enforced by the types that carry
  them, and `policy_version` is additionally bounded by the migration; the two bounds are equal today
  but are separate numbers, so a divergence would be possible rather than impossible.

## Alternatives considered

- **Keep `ToolOutcome` in `jarvis-tools` and have `jarvis-storage` store the outcome as a string.**
  Rejected: the decoder would then re-implement the honesty rules (`confirmed` requires evidence,
  `failed` requires a reason) in an adapter, which is a second home for a domain rule — and the copy in
  the adapter would be the one a reader of the storage code trusts.
- **A separate `tool_call_ledger` table keyed by the idempotency key.** Rejected as two writes where
  one will do: the ledger row and the call row could disagree after a partial failure, and the ledger
  would need its own ordering rule against the call.
- **Derive the idempotency key from the intent digest.** Rejected: it makes a deliberate second
  identical call impossible to express, silently collapsing it into the first. The user would archive a
  folder twice and be told the second call was a duplicate.
- **Allow a terminal outcome to be overwritten when the new one is "more specific".** Rejected: it
  reintroduces exactly the `Unknown`-to-`Failed` downgrade the rule exists to prevent, and "more
  specific" is not decidable.
- **Store truncation as an appended marker in the output text.** Rejected: a tool's own output is
  attacker-influenced content, so a marker in it cannot be trusted to mean truncation.
- **Compare the deadline in SQL against the stored timestamp.** Rejected for the reason recorded in
  ADR-0018 and pinned by a core test: `UtcTimestamp`'s text form omits a zero fraction, so
  `"...00Z"` sorts after `"...00.5Z"`. `is_past_deadline` and `is_valid_at` compare `unix_nanos()` in
  Rust.
- **One combined `output: Option<Content>` carrying both the text and the provider identifier.**
  Rejected: the two have different bounds and different trust levels, and the architecture requires them
  preserved separately.
- **Put the secret resolver on the execution request now.** Rejected: connector token lifecycle is
  `P3-008`, and a field with no owner is a field whose semantics get invented by its first caller.

## Conditions that would justify revisiting

- A second adapter reveals that `AdapterError`'s four cases are not enough to map onto the five outcome
  states, which would mean the port is missing a distinction or carrying a spurious one.
- `P3-012`'s gate shows a recovery path needing more than `must_not_repeat()` to decide.
- The 32 KiB output bound proves wrong in use, which would change `MAX_TOOL_OUTPUT_BYTES` and the
  migration's `CHECK` together — the two must move in the same change.
- A remote or multi-process control plane appears, which would make the guarded-`UPDATE` conflict path
  a per-request concern rather than a rare one.
- Tool calls need to appear in the durable run event log with the same ordering guarantees, which would
  make the current "stored document, no event" split a gap rather than a boundary.
