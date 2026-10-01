# ADR-0120: A decided approval continues the run that parked on it

- **Status:** Accepted
- **Date:** 2026-10-01
- **Slice:** the approval continuation — closing `P3-022`'s recorded limit that a held call *parks* a run
  and nothing drove it forward, so a conversation that paused for approval never produced its answer.
- **Relates to:** `ADR-0119` (the model may request a tool; policy decides), `ADR-0019` (tool calls are a
  durable lifecycle), `ADR-0018` (approvals bind to a digest), `ADR-0049` (untrusted content is fenced),
  `ADR-0017` (policy evaluation outcomes), `ADR-0022` (cancellation carries no version). The run state
  table is `docs/architecture/runtime-and-models.md`.

## Context

`ADR-0119` made the model a caller of the tool pipeline: a turn that requests a tool runs it through
`ToolPipeline::call_tool`, and a call policy **holds** for human approval moves the run to
`AwaitingApproval` and stops. The recorded limit was explicit:

> A call policy holds for human approval moves the run to `AwaitingApproval` and **stops** … The decision
> and resume arrive through the approval and resume routes, which complete the call directly rather than
> through this loop — so the executor parks a run truthfully and does not hold a promise those routes
> already keep.

That is true of the **call** and false of the **run**. The approval and resume routes executed the decided
effect and settled the call row, but nothing advanced the run: it stayed at `AwaitingApproval` with its
effect already made and its user still waiting for an answer. A conversation that paused for approval
never finished.

Building the continuation surfaced two constraints that had to be respected rather than worked around.

**The state table does not allow `Executing → AwaitingApproval`.** `docs/architecture/runtime-and-models.md`
has `Planning → AwaitingApproval` and no edge from `Executing`. A model call runs in `Executing`, so a held
tool call arrives with the run in `Executing`; the first implementation advanced straight to
`AwaitingApproval` and was refused by the domain. **A hold is decided when the run plans a step, not while
a step is running** — the model call has finished by the time the hold is known.

**`tool_calls` stores no arguments.** Migration `0007` deliberately keeps the intent digest and not the
payload (`ADR-0018`), so a resumed run cannot reconstruct the assistant tool-call turn a provider needs to
accept a `tool` result. The observation therefore cannot be shaped as the provider's tool-result
structure.

## Decision

**1. A held call parks the run over the documented edges, not a shortcut.**

`park_for_approval` walks `Executing → Observing → Planning → AwaitingApproval`: interpret what the model
asked for, decide the next step is that capability, hold the step. Every edge is in the table, and the
sequence narrates the hold honestly rather than reaching a legal state by an illegal path.

**2. The run continuation reads the decided call's outcome; it does not run the call again.**

The resume **route** owns the effect: it calls `ToolPipeline::resume`, which refuses any call past
`requested` (`P3-012c`). The run continuation therefore *reads* the stored call and reports its outcome.
Re-running the call from the executor would either be refused (failing the run for no reason) or — if the
duplicate guard were ever weakened — produce the second effect the guard exists to prevent. The effect
lives in exactly one place.

**3. A resumed run re-assembles its own context, then appends the observation.**

A resumed run begins with an empty loop state. Without re-assembly the resumed model call would be sent
*only* the observation — no policy, no objective, no history — and the model would answer a message about a
tool with no idea what was asked. So the continuation reuses the same context assembly the fresh-run path
uses, then appends the outcome. To make that reuse correct the assembly is a **function that returns the
messages**, separated from the transition that moved the run; the loop's `ContextBuilding` arm now owns
that transition.

**4. A tool observation is fenced data, not a provider `tool` message.**

Because the arguments are not stored (constraint above), the assistant turn that requested the call is not
reconstructible, so a `tool` result cannot be correlated to it. The outcome is reported as fenced data
(`ADR-0049`) — the vocabulary built for content originating outside JARVIS — which is the correct
classification for tool output and is what the model is already instructed to treat as data.

**5. The run's own parked arm never advances on a decision nobody took.**

Reaching `AwaitingApproval` **without** a pending call leaves the run parked. `recover_interrupted_runs`
settles every non-terminal run at daemon startup, so a parked run does not survive a restart; reaching the
arm without a pending call therefore means a caller re-drove a parked run it has no decision for. Advancing
there would be acting on a decision that was never made, which is the one thing the arm must never do.

**6. A resume only continues a run that is actually parked.**

The route's continuation reads the run first and spawns the drive only when the state is
`AwaitingApproval`. A resume of a call from a run that has since settled, or that was never parked, is a
no-op rather than a re-drive the domain would refuse — reaching for the drive at all is the mistake.

## Consequences

- **A conversation that pauses for approval finishes.** Hold, decide, resume, and the run answers — the
  end-to-end property `P3-022` recorded as a limit, now a test.
- **The effect still happens exactly once.** The route owns execution and the continuation reads; the
  duplicate guard and the single reader are two mechanisms, and the test asserts the adapter's own call
  count rather than a status.
- **Restart-resumable approvals remain `P6`.** This continuation is in-memory; because a restart settles
  the run, a decision that arrives after a restart currently finds no parked run. Persisted approval across
  a restart is the workflow/event slice's work and is unchanged here.
- **Context assembly is now a reusable function.** Splitting it from its transition is what let the resume
  reuse it; a second reader of the same assembly is exactly the shape that keeps two request-building paths
  from drifting.

## Alternatives

- **Add `Executing → AwaitingApproval` to the state table.** Rejected: the table is the documented contract
  and this edge is not the semantics anyone wants — a hold is a *planning* decision, and widening the table
  to match a shortcut would encode the shortcut as design.
- **Have the executor run the decided effect itself.** Rejected: the resume route already owns that, and a
  second execution path is what the `P3-012c` duplicate guard exists to make impossible; reading the
  stored outcome is the single-effect design.
- **Reconstruct a provider `tool` result from the stored arguments.** Rejected: `tool_calls` stores no
  arguments by design (`0007`), and storing them to make the transcript reconstructible would put tool
  payloads into a durable row, which is the privacy decision `ADR-0018` declined.
- **Settle the run the moment its call is held, as `failed`.** Rejected: a hold is not a failure; the
  documented `AwaitingApproval` state exists so the run resumes.

## Conditions that would justify revisiting

- A requirement that an approval decided **after a daemon restart** continue its run, which needs the run
  and its pending call to be reconstructible from durable state and belongs to the workflow slice (`P6`).
- A provider integration that requires a real `tool`-result message for a resumed call, which would need
  the arguments to be stored (with their privacy implications) and is a superseding decision to `0007`.
- A measured need to re-plan after an approval (rather than answer over the observed outcome), which would
  use `AwaitingApproval → Executing` and risks re-requesting the very tool that was held.
