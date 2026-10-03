# ADR-0130: A pending approval holds what it is waiting on, so a person can decide it

**Status:** Accepted
**Date:** 2026-10-03
**Slice:** `P3-028`

## Context

A held tool call parks its run until a person decides. The pipeline, the decision route and the nonce delivery all
existed, and the first time a **real model** drove a held call end to end they were found to add up to a flow no
person could complete:

1. **Nothing listed what was waiting.** There was no route and no CLI verb for pending approvals.
2. **A person could not see what they were approving.** `0006` deliberately stored no arguments ("the hash is the
   binding ... the hash already makes unnecessary"). The hash does make them unnecessary for *binding*; it cannot
   make them unnecessary for *deciding*. An approver shown "`jarvis.web.fetch` 1.0.0" approves a sentence, and the
   hash then binds the decision to a URL nobody read.
3. **The held call could be released only by a client that kept its own copy of the arguments**, because
   `POST /calls/{id}/resume` needs them and `tool_calls` stores none. No human-facing client does.
4. **A denial left the run parked forever.** `deny` settled the approval and nothing told the run.
5. **A daemon holding one parked run would not start.** Restart recovery tried to settle every non-terminal run as
   `failed`; the state table forbids `awaiting_approval → failed`, so the settlement was refused and, because it
   runs at startup, the daemon exited. (Amends `ADR-0013`.)

Items 1–4 are one gap and 5 is a defect the same walk through the product exposed.

## Decision

1. **A pending approval holds the call's arguments, and a decision clears them.** Migration `0013` adds a nullable
   `approvals.arguments_json`, bounded to 8 KiB. The guarded decision `UPDATE` sets it to `NULL`, so a *decided*
   approval keeps no tool payload — which is what `0006` was protecting. The hash is still the binding, and the
   resume route still recomputes it from the supplied arguments, so a stored payload is not an authority and a wrong
   one is refused. A payload that does not fit is not held, and the approval is then **not decidable from a client
   that must show what it approves**: nobody can approve what they cannot see.
2. **`GET /api/v1/approvals` lists pending approvals with their arguments and the held call's identifier, never the
   nonce.** The nonce still reaches a human only through the profile-private file (`ADR-0042`); a route that returned
   it would hand it to whatever can call the route.
3. **`jarvis approvals list|approve|deny`.** `approve` prints the tool and its arguments and asks; without a terminal
   it refuses unless `--yes` says the caller already looked. It reads the nonce from the private file (read, not
   taken: the daemon discards it after a successful decision, and a retryable failure must not also destroy the means
   to retry), decides, resumes the call with the arguments it was shown, and follows the run to its answer. It names
   no approver — the daemon records its own identity.
4. **A refusal continues the run.** `decide_approval` hands a `deny` or `cancel` to the executor, which tells the model
   the person declined, that nothing ran, and not to ask again, and lets it answer. The tool is never executed.
5. **`jarvis ask` stops when a run needs a person**, with its own exit status (11) and the approval's identifier,
   instead of reporting a parked run as "stalled, not working".
6. **Recovery leaves a run at `awaiting_approval` alone**, because its approval row and nonce file are durable; it
   settles one only when cancellation was already requested (`awaiting_approval → cancelled` is legal).

## Consequences

- The human flow works end to end: ask, see what is held, approve or deny, get the answer — including across a daemon
  restart. Verified live against a real model (Ollama's `glm-5.3:cloud`) and the real network.
- A pending approval now stores model-authored arguments for up to its lifetime (one hour by default). They are
  readable by one authenticated route and by nothing a model can call, and they are never logged.
- A pending approval that **expires** keeps its payload until a retention sweep exists. Recorded as a limit.
- The declined call's `tool_calls` row stays `requested`: no transition records a refusal against a call that never
  ran. The run is correct; the call row is not yet a faithful audit of the decline.
- Postgres has no `approvals` migration yet (its directory holds only the embeddings table), so this is SQLite-only
  like the table itself.

## Amendment (2026-10-03, `P3-030`): arguments are kept until the call has run

Decision 1 said a decision clears the arguments. That left one hole, recorded as a limit: the arguments were gone the
moment a person approved, so a daemon that died between the decision and the release left a call that was approved,
never run, and impossible to finish (the CLI had to re-send a payload it no longer had). The rule is now:

- A **denial or cancellation** clears the arguments at once — there is no call to finish.
- An **approval keeps them until the call has executed**, when the release clears them. "Approved and still holding
  arguments" is therefore exactly "released by a person but not yet run".
- `POST /api/v1/approvals/{id}/decision` takes an opt-in `resume: true`: the daemon releases the approved call itself,
  in a task, from the arguments it held. The CLI uses it, so it never re-sends a payload and what runs is what was
  shown. Clients that predate it keep deciding and then resuming themselves.
- `POST /api/v1/approvals/{id}/resume` (no body) finishes an approved call whose release did not happen, and
  `GET /api/v1/approvals` lists such approvals with `state: approved`. `jarvis approvals resume ID` is the CLI form.
- Both release paths share one function with `POST /calls/{id}/resume`, and the intent digest recomputed from the held
  arguments is still the last check.

Verified live: approve over REST without `resume`, kill the daemon, restart, `jarvis approvals list` shows the call as
approved-not-run, `jarvis approvals resume` fetches the page, and the run completes.

## Falsification

- `a_run_waiting_on_approval_survives_a_restart` failed before the recovery change, with the daemon's own error.
- `a_run_whose_held_call_was_declined_answers_without_running_it` asserts the adapter ran **zero** times and the model
  was told not to retry.
- `a_pending_approval_holds_its_arguments_until_it_is_decided` asserts a decided approval holds no payload and that a
  payload cannot be attached afterwards.
- `a_pending_approval_is_listed_with_its_arguments_and_never_its_nonce` asserts the nonce is absent from the reply.
