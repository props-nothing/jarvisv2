# ADR-0013: A Restart Settles Interrupted Runs Truthfully, It Does Not Resume Them

- Status: Accepted
- Date: 2026-09-23

## Context

`FR-RUN-003` requires the platform to support "cancellation, timeouts, retries, resumable waits, and
crash recovery at documented boundaries". Acceptance scenario `A03` phrases the same requirement as an
either/or: after termination at every persisted run transition, a restart must "either resume from the
documented boundary **or** mark the run with a truthful terminal/recoverable state".

`P2-009` gives a run something to lose. Before it, nothing in `jarvisd` invoked a model, so a run never
left `received` and a crash could not interrupt anything. Now the executor walks the documented state
machine, streams from a model, appends an event per fragment, records usage, and writes the answer to
the transcript. A process killed mid-run therefore leaves **durable** state that no process will ever
advance:

- an `agent_runs` row in one of seven non-terminal states, with a `version` and possibly a recorded
  `cancellation_requested_at`;
- its `run_events` up to the last committed transition, and no terminal event;
- the user's message, already committed in the same transaction as the run;
- possibly `output_delta` events whose fragments formed a partial answer.

What cannot be left behind is the rest of it. The model stream, the assembled context beyond its
recorded manifest, and the answer text as a whole live in the executor's memory. A restart also cannot
learn whether an in-flight provider call was accepted, so a run that appears to have stopped between
two events may in fact have been charged for and answered.

Meanwhile the daemon has one hard obligation: whatever a client can observe, a process must be able to
honour. A run reported as active that no executor will ever drive is a lie a client cannot detect — it
would wait on a stream that will never advance.

## Decision

**At startup, before the transport binds, the daemon settles every non-terminal run and reports what it
did.**

`jarvis_storage::recover_interrupted_runs` scans the non-terminal rows and, for each, writes the
terminal state and its terminal event in **one transaction** through `settle_run`:

- a run whose `cancellation_requested_at` is set settles **`cancelled`**, because the operator's intent
  already outranks the interruption — reporting a daemon fault for work that was meant to stop would be
  wrong;
- every other run settles **`failed`** with `error_code = interrupted_by_restart`, a code that names the
  restart as its cause rather than implying a provider or model fault;
- the settled run gains exactly one terminal event, so a client replaying the stream sees the
  settlement once;
- the identifiers are logged, not merely a count, so an operator can look up what was recovered.

The scan happens **before the listener binds**, so there is no window in which a client can read a run
that the daemon has not yet accounted for. Failure to persist the accounting is **fatal**: the daemon
stops rather than serving runs it cannot explain, which is the fail-closed direction for a system whose
whole claim is that its records are truthful.

### Recovery is truthfulness, not resumption, and that is deliberate

Resumption was the alternative and is rejected for this build.

**It is not implementable honestly yet.** The answer text is streamed, and the only durable record of it
is one `output_delta` event per fragment. A resume would have to either re-send the objective and
produce a second answer — which the transcript would then hold alongside the fragments of the first,
presenting two answers to one question — or reconstruct the partial answer and continue from it, which
requires the provider to accept a pre-filled assistant prefix and to be charged as a second call.
Neither is a property this build can verify.

**And the ambiguity is unresolvable from the daemon's side.** Whether the interrupted call was accepted
is knowable only by the provider, and this build has no provider adapter wired to credentials. A
resumed run therefore cannot state whether the work it repeats was already done, which is exactly the
guarantee "resume" is supposed to provide.

So the second branch of `A03` is taken explicitly, and the record of it is what makes the choice
reviewable. A run that was interrupted is reported as interrupted.

### What this does not cover

- **Resumption.** When a provider adapter can report whether a call was accepted, a resume is a new
  decision with new evidence, and it needs its own record.
- **`awaiting_approval`.** No run reaches that state until `P3` provides approvals. It is non-terminal,
  so recovery would settle it `failed`, which is correct today (nothing can resume it) and will need
  revisiting when a durable approval request can outlive a restart. `A13` owns the workflow version of
  the same problem.
- **Partial answers.** The `output_delta` events of an interrupted run remain in its stream and are
  neither replayed to a client as a finished answer nor deleted. They are what was observed, and
  deleting history to tidy an outcome is worse than reporting the outcome.

## Consequences

- A client that reconnects after a restart sees a terminal state and a reason, never a run that waits
  forever. This is `A03`'s "truthful terminal state".
- Operators can distinguish a restart interruption from a provider failure by the error code, which
  matters because the responses differ: an interrupted run is worth retrying, a refused one may not be.
- The daemon's startup is no longer purely local: it now reads and possibly writes run state before it
  serves. That is a real cost, bounded by the `agent_runs_active_idx` predicate, and it is the price of
  never serving an unaccountable run.
- Recovery is idempotent by construction: it targets only non-terminal runs, and a settled run is not
  one. A second startup finds nothing to do.
- This is the **second** place the settlement primitive is required, which is evidence for ADR-0011's
  conclusion that a terminal transition and its terminal event have no valid order as separate calls.
