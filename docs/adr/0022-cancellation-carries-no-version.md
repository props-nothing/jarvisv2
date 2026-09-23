# ADR-0022: A cancellation request carries no version, because operator intent cannot be stale

**Status:** Accepted

**Date:** 2026-09-22

## Context

`POST /api/v1/runs/{id}/cancel` required `expected_version`, and
`docs/quality/acceptance-tests.md` gave the justification: "The request also carries
`expected_version`, so a client cannot cancel a run it has not read."

That reasoning is wrong, and the cost of it was **observable rather than theoretical**.

`apps/jarvisd`'s executor walks a run through its state machine (`received`, `context_building`,
`planning`, `executing`, `observing`, `responding`) with a version-guarded write at **each** step. A
running run's version therefore advances several times per second. A client's `expected_version` is
stale almost immediately, so:

- **a user asking to stop a run was refused with `409 CONFLICT`** — a failure they cannot act on and
  cannot resolve, because the version they would need is changing while they read it. The system
  refused a request to stop a run *because the run was running*;
- **a concurrency test failed intermittently**: `a_cancellation_requested_in_flight_settles_the_run_once`
  read the version, slept 80 ms while the model call was parked, and wrote it — and the executor's own
  progress write landed in that window. It failed once under full-workspace load, then passed 6/6 in
  isolation and on the next full run. A nondeterministic test is itself a defect, and this one was
  reporting a real user-facing one.

> **CORRECTION (2026-09-23, `P3-008j`): removing the expectation from the *request* did not remove the
> race, and this ADR's claim that it did is struck rather than quietly edited.** The same test failed again
> under full-workspace load, and the cause was the sentence in decision 4 below. `request_run_cancellation`
> still advanced `version`, and a version exists to guard a write that depends on a version it read — this
> one depends on nothing, so advancing it protected nothing while invalidating the expectation every other
> writer held. See decision 7, which supersedes decision 4.

`apps/jarvisd/src/run_service.rs` made it worse rather than better: it **re-read the run and then
discarded what it read**, passing the caller's version instead of the fresh one. So the read was pure
cost with no effect.

Two documents decide the direction of the fix.

`docs/adr/0013-restart-settles-interrupted-runs.md`: a run whose `cancellation_requested_at` is set
settles **`cancelled`**, "because the operator's intent **already outranks** the interruption". Operator
intent is the highest-priority fact the system holds about a run.

`docs/architecture/events-and-workflows.md` and acceptance test `A04`: "new work stops, state settles
once" — cancellation is a **request**, not a transition, so recording it cannot be a state change that
would need guarding against another state change.

## Decision

**1. `request_run_cancellation` takes no `ExpectedRunState`.** It is `(database, id, at)`. The statement
is guarded by the state rather than by the version:

```sql
UPDATE agent_runs
SET cancellation_requested_at = COALESCE(cancellation_requested_at, ?2),
    updated_at = ?2, version = version + 1
WHERE id = ?1 AND state NOT IN ('completed', 'cancelled', 'failed')
```

**2. The guard that remains is the one that cannot go stale.** A **settled** run refuses a cancellation,
because there is no work left to stop. That is a fact about the run rather than about who is asking or
when they looked, so no amount of concurrency can invalidate it. This is the whole point: the old design
had one guard that was correct-but-unusable mixed with one that was meaningful, and they were applied by
the same predicate.

**3. Zero rows affected is resolved by reading, not by guessing.** A non-match has two causes — the run
is absent, or it has settled — and those are different answers for a caller. The follow-up `find_run`
supplies `RunNotFound` when identity is unknown, and the settled case returns
`RunTransitionRefused { TerminalStateImmutable }`, which is what the old code returned by *pre-checking*
a caller-supplied state. The pre-check is gone because it trusted the caller for a fact the store knows.

**4. It is idempotent.** `COALESCE` keeps the **first** request time, so a repeated request is not an
error and the interval between asking and stopping stays measurable. The old code's
"no-op must not advance the version" concern does not arise: this write records intent, and a second
identical intent is harmless.

> **SUPERSEDED by decision 7 (2026-09-23).** "Harmless" was half-true in the way that matters: the extra
> version is harmless to *this* write, and harmful to **every other writer**, whose expectation it
> invalidates. A statement that looks only at its own effect will not notice that.

**5. `CancelRunRequest` is deleted from the protocol rather than left with a permissive decoder.** It had
`deny_unknown_fields`, so a client still sending `expected_version` would now get a `422` for a field the
daemon ignores — an error for a request that should succeed. Deleting the type makes an old client fail
to compile, which is the honest outcome for a deliberate contract change. The endpoint takes no body.

**6. The gateway test was replaced rather than adjusted.** It asserted a stale version produced `409`,
which was the assertion that encoded the wrong design. The new test asserts a bodyless cancel is
accepted, that a repeat is accepted and preserves the first request time, and the storage test covers the
settled refusal where it lives. The daemon test no longer reads or supplies a version at all.

**7. A write that changes no state does not advance the version. (`P3-008j`, 2026-09-23)**

`request_run_cancellation` sets `cancellation_requested_at` and `updated_at` and **leaves `version`
untouched**. This supersedes decision 4.

**A version is a guard for a write that depends on the version it read.** This write depends on nothing:
its predicate is a **state**, its effect is idempotent under `COALESCE`, and the state machine does not
mention the version. Advancing it therefore protects nothing, while invalidating the version every other
writer is holding — including the executor, which holds one across the whole model call.

**The defect it caused was a lost update, which is worse than the refusal this ADR was written to fix.**
The executor's `advance` re-reads the run and then writes it guarded on what it read (`P2-009a` finding
3). A cancellation landing between the two trips the guard, so the progress write reports `RunConflict`
— a concurrency bug to a reader — and the run carries on. The intent does survive that write, because the
transition's own `COALESCE` re-preserves it, but it survives **only** for a caller that re-reads and acts.

**It was found by the gate, not by theory, and it is now pinned deterministically.**
`a_progress_write_racing_a_cancellation_neither_conflicts_nor_loses_the_request` stages the interleaving
explicitly instead of sleeping, because the defect is an **ordering** rather than a timing — which is why
this ADR's "12 consecutive passes" could not have found it and why a sleep-based test cannot pin it.

**Both properties were falsified, and the second falsification corrected an expectation.** Removing the
version bump makes the new test fail with `RunConflict`. Removing the transition's `COALESCE` makes it fail
with `left: None` — the cancellation silently gone — so the request-preservation assertion is load-bearing
and is not merely a restatement of the first. The author's first draft of the test comment claimed the
version bump alone was what discarded the intent; running the falsification showed that was **false**, and
the comment was corrected to the mechanism that actually holds.

**Why the version is not instead made monotonic some other way.** The alternative was to keep bumping it
and have every guard retry on conflict. Rejected: retrying an optimistic guard hides which writer was
overwriting what, and a run's progress writes are frequent enough that a cancellation would be retried
against a moving target forever. Removing the bump removes the conflict rather than living with it.

## Consequences

- A client can always cancel a run it can identify. The one refusal left is actionable: "it already
  stopped", which a client can report plainly.
- ~~The intermittent test failure is gone **structurally**, not by loosening a timeout: there is no
  read-then-write window left to lose. Verified by 12 consecutive passes after the change.~~ **CORRECTION
  (2026-09-23): this was FALSE, and the sentence is struck rather than quietly edited.** The failure
  recurred under full-workspace load. Two things were wrong with it: the claim, and the **method** that
  produced it. Twelve isolated passes cannot sample a window that only opens under load, so "verified by 12
  consecutive passes" was evidence of nothing — the same weak evidence this ADR elsewhere criticises when a
  vendor presents it. What replaced it is a test that stages the interleaving explicitly and therefore
  fails **every** time without the fix and passes **every** time with it. That is the shape a concurrency
  regression needs; a repeated run is not.
- A `RunConflict` from this path no longer means "you were too slow"; the only way to reach it is a
  racing writer between the write and the follow-up read, which is reported honestly rather than
  silently succeeding — claiming a request that was not recorded would be worse than a conflict.
- `docs/quality/acceptance-tests.md` carried the wrong justification for the guard and now records the
  correction inline, so the reasoning cannot be re-derived from a stale paragraph.
- The protocol lost a type and a route lost its body, so this is a **breaking API change**. It is
  acceptable now because no released client exists; `docs/api/contracts.md` is where a frozen version
  would need a deprecation path instead.

## Honest limits at the time of this decision

- ~~**`expected_version` remains on run *start* and on approval decisions.**~~ **CORRECTION (2026-09-22):
  this claim was FALSE and is struck rather than quietly edited.** Neither case exists in this codebase.
  `ApprovalDecision` holds `outcome`, `channel`, `strength`, and `decided_at` — there is no version field,
  and there never was. Starting a run creates a row, so there is no prior version to expect. `grep` for
  `expected_version` across the workspace now finds only this ADR, a doc quotation, and the message of the
  test that replaced the removed cancel field.

  The claim is worth recording as a **defect of this ADR, not of the code**: it was written as a
  deferral, so it read as "the same problem may exist elsewhere". It did not — the opposite is true. **An
  approval is inherently *not* an optimistic-concurrency case, and for a reason the cancellation did
  share.** An approval is addressed by a stable `ApprovalId` and its own transition rule refuses a second
  decision, so the *identity plus the state machine* already exclude a lost update. No version token is
  needed to protect a decision whose subject cannot be raced. That is the general form of the rule this
  ADR applied to cancellation only in its narrow instance: **where a durable identity already names the
  subject of the decision, a version expectation adds no safety — it can only add a refusal.** The
  cancellation had an `id` too; what it lacked was a settled-guard that could express "there is work left
  to stop" without a version, which is what the state predicate supplied.

  The lesson is about how a limit gets written: "neither has been re-examined" invited a future reader to
  treat an unverified claim as a live risk. Stating it as a question ("does an approval need a version
  guard?") would have been honest; asserting that one exists was not.
- **This is not the composition work.** `apps/jarvisd` still has no tool pipeline: nothing reads a tool
  definition from a registry, calls `evaluate`, admits a tool call, or calls an adapter. `P3-012` owns
  that. *(Superseded: the pipeline was composed in `P3-006d` and the MCP translation in `P3-008a`/
  `P3-008b`.)*
- **The client's view is unchanged in one respect**: a cancel still returns the run in its current,
  non-terminal state, so a caller that wants to know when work actually stopped must watch the stream or
  re-read. That is `A04`'s semantics and is not altered here.
- **No CLI verb was added.** `jarvis cancel` does not exist; the endpoint and the e2e gate exercise it,
  and adding a verb is a separate slice with its own exit-status mapping (`ExitStatus::Cancelled` is `9`).
  *(Still true as of 2026-09-22: the verbs are `status`, `health`, `ask`, `chat`, `logs`, and `doctor`.)*
- `run_events` is untouched by this change: settling still writes the terminal event, and the request
  does not write one. Whether a *request* should emit an event is a question this decision does not
  answer.

## Alternatives considered

- **Keep the version and have the executor stop bumping it during a run.** Rejected: version monotonicity
  is what the optimistic-concurrency guards on every other write depend on. Removing it to accommodate
  one endpoint would weaken all of them.
- **Keep the version and have `RunService::cancel` retry on a conflict.** Rejected: it hides a design
  error behind a loop, cannot be bounded honestly (the run keeps moving while the client retries), and
  leaves the client-facing contract claiming a version matters when it does not.
- **Make the version optional and ignore it when present.** Rejected: an accepted-then-ignored field is
  worse than a removed one, because a client would keep sending something that appears to be honoured.
- **Return `200` with the run for any cancellation, including a settled one.** Rejected: a settled run has
  no work to stop, and reporting success would claim the request had an effect. `A04`'s honesty standard
  applies to a refusal as much as to an outcome.
- **Keep `CancelRunRequest` with `deny_unknown_fields` removed so old bodies are tolerated.** Considered,
  and it is the right choice for a *released* protocol — but there is no released client, and a type that
  invites sending a meaningless field is a worse default than a compile error.
- **Remove the version only from the HTTP layer and keep it in the repository.** Rejected: the repository
  is where the race lives, so the flaky test and the lost cancellation would both survive.

## Conditions that would justify revisiting

- A second client (desktop, a remote daemon) needs a cancellation that is refused when it is not based on
  current knowledge — for example a queue of pending cancellations to review. That would need a different
  mechanism than a version, such as a target-state assertion.
- The approval-decision endpoint or run start is re-examined under the same reasoning and found to share
  the problem, which would make this decision one instance of a broader one.
- A cancellation is found to have an effect that must be guarded — if, for example, cancelling during
  `awaiting_approval` needs to invalidate a pending approval atomically, that write would need its own
  guard.
- The absence of a `jarvis cancel` verb turns out to leave `A04` untestable from the CLI, which would move
  it from a later slice into a requirement of this one.
