# ADR-0117: A skill is a procedure, not a permission

- **Status:** Accepted
- **Date:** 2026-10-01
- **Slice:** `P4-011` (procedural memory and the skill format). Recorded **before** implementation, because
  the decision fixes a trust boundary that both the memory schema and the context envelope depend on, and
  the migration that carries it cannot be edited once applied.
- **Relates to:** `ADR-0004` (canonical memory is JARVIS-owned), `ADR-0049` (untrusted content is fenced, and
  the fence is not a promise the model will obey), `ADR-0005` (canonical tools and MCP), `ADR-0017` (policy
  evaluation outcomes and ownership), `ADR-0018` (approvals bind to a digest and store no bearer token),
  `ADR-0045` (a correction is declared, not inferred), `ADR-0035` (a documented invariant with no test is a
  convention).

## Context

`docs/product/requirements.md` lists `procedural` among the memory types (`FR-MEM-001`), and `ROADMAP.md`
names "event-triggered skills" in Phase 6. The two are not connected by any format, lifecycle, or record:
there is no skill shape, no creation step, and no statement of what a skill *is*.

That gap is not a missing feature list. It is an unmade trust decision, because "skill" is the name two
other designs in this class give to **text that is loaded and then acted on**. The pattern is:

1. the agent authors or is given a procedure;
2. the procedure is injected into the prompt beside the tool list;
3. the agent follows it, and its effects are gated by a pattern list or a coarse approval.

Step 3 is the incompatible part. `AGENTS.md` states the boundary this platform is built on — *"the model may
request an effect; deterministic Rust policy decides whether it may happen"* — and `ADR-0049` already
records the honest limit that a fence does not stop a model from following an instruction it reads. A skill
is therefore a *new route by which an instruction reaches the model*, and the only question that matters is
what authority that instruction can carry.

The tempting answer is the dangerous one: store the procedure, load it, and let it run.

## Decision

**1. A skill is a procedure: an ordered statement of steps that name already-granted tools.**

It describes *how* to do something the user has already permitted. It is not a grant, cannot widen one, and
carries no authority of its own. A skill names tools; it never names a permission.

**2. Loading a skill authorizes nothing, and each step re-enters the same pipeline.**

There is no "skill execution" path. A step becomes a tool request through the ordinary gateway — schema
validation, authentication, authorization, risk classification, approval policy, timeout, idempotency where
relevant, execution, and audit. A skill is a *sequence of requests*, not a unit of authority, so an
unavailable or refused tool stops the skill at that step exactly as it would stop a direct call.

**3. A skill is self-authored context, so the envelope's trust rules apply to it.**

A skill the model produced is `Derived` at best, and is rendered through `IsolatedText` (`ADR-0049`) like any
other retrieved content — fenced, neutralised, and marked as data rather than instruction. A skill the user
authored carries the user's provenance. The distinction is required, because a self-authored procedure
re-entering a later prompt as though it were the user's instruction is the self-feeding loop the inference
boundary (`ADR-0049`, §6) exists to prevent.

**4. Promotion is an approval.**

An agent that authors a skill produces a **proposal**, not an active procedure — the `Proposed` state
`P4-001` already defines for an unsupported claim. A proposal becomes usable only through a durable,
attributable decision that names its approver (`ADR-0043`). An agent that could author a procedure *and*
promote it would have authored its own effect, which is precisely the boundary `AGENTS.md` draws.

**The second half of that rule — that the approver must not *be* the author — took three slices to
enforce, and nothing about the gap was visible in the code.** The construction rule refuses a model-authored
revision recorded `Active`, and the `skill_revisions` schema refuses to store one without a named promoter:
both were in place, and the author of a proposal could reach `Active` simply by calling `promote` with its own
identifier as the approver, which satisfied the schema's own `CHECK`. So this paragraph's sentence was quoted
in the migration, in the constructor, and in `promote`'s doc comment — three descriptions of a boundary and no
check of it. `SkillRevision::promote` now refuses `approver == created_by_actor_id`, compared after trimming,
with its own error variant separate from an unattributed promotion because the two remedies differ (name
somebody, versus have somebody *else* decide). It is deliberately the same rule, at the same scope,
`ApprovalRequest` applies to a tool call: the approval's rule is unconditional, so a user may not approve a
request it made either. The two shapes the guard separates are a **run promoting a proposal it authored**
(refused — this paragraph's boundary) and a **user promoting a proposal a run produced** (the ordinary path,
and the control its test asserts). **A decision that states a rule as reasoning has described it, not
enforced it; the enforcement is a `Result`, and this one did not exist.**

**5. A skill is versioned and provenance-carrying, and correction is declared.**

A stored procedure carries its source, its version, and the tools it names at that version. Replacing one is
a declared supersession (`ADR-0045`), not an inferred one, so "which procedure ran" is answerable from the
record rather than reconstructed.

**6. A step's effect is unchanged by living inside a skill.**

The effect vocabulary, risk level, and approval requirement of a step are properties of the *tool*, decided
by policy at execution. A skill cannot lower them, cannot pre-select an approver, and cannot carry an
approval. Whether the current grant still covers a step is decided when that step runs, not when the skill
was loaded — a grant revoked between load and run must refuse the step, and only an execution-time check can
see that.

**7. Interoperating with an external skill format is allowed only where it maps onto these rules.**

An external format may be adopted for its prose and its step list. A field that would grant authority,
preselect a tool the actor was not granted, or pre-approve an effect is **dropped**, and the drop is
recorded rather than silently ignored — the same "a field that is accepted-then-ignored is worse than a
removed one" rule `ADR-0022` applies to a request body. Which fields are dropped is a recorded limit, so a
reader can tell a skill's behaviour from the format's intent.

## Consequences

- **An agent can author a procedure; it cannot author an effect.** The worst a hostile or mistaken skill can
  do is propose steps, each of which policy evaluates on its own. Nothing in the skill path is new authority.
- **A skill is a new prompt-injection surface, and it is fenced like the others.** A procedure is retrieved
  content; treating it as such is what keeps "the model wrote this" from meaning "the model decided this".
- **What a skill buys is repeatability, token economy, and an inspectable artifact** — not capability. The
  capability was already present as granted tools; the skill states one way to use them.
- **A skill is not a workflow.** A workflow step (`P6-004`) is a durable, resumable, policy-evaluated unit
  with its own retry and compensation semantics; a skill is prose-plus-references. Keeping them separate
  means a skill never silently acquires workflow guarantees it does not implement.
- **Rejected alternatives, each for a reason the platform already holds:** skill-as-injected-text-with-
  pattern-gated-effects (the pattern-authority model `AGENTS.md` forbids); skill-as-grant ("loading grants
  its tools" — privilege escalation through memory, and it would make one decision cover a set of undecided
  intents); skill-with-preselected-approval (an approval binds to one intent's digest, `ADR-0018`, so a
  pre-approval is not an approval); auto-promotion after first use (the author would be the sole decider of
  both the procedure and its promotion).

## Conditions that would justify revisiting

- **Skills become shareable across users or workspaces.** Provenance, trust, and authority across a
  workspace boundary is then a new decision, not a field — the same shape `P4-010`'s isolation work covers
  for claims.
- **A skill needs a step whose arguments are computed without a model** (a deterministic sequence). That is
  a workflow step rather than a skill, and the question is whether the two representations should merge
  rather than whether a skill should gain execution semantics.
- **An external skill format becomes a de-facto standard for procedures.** Adoption still holds §7: the
  format is read for its prose and steps, and any authority-bearing field remains dropped and recorded.
