# ADR-0143: JARVIS can run a command in your folder, and a run that waits for you remembers what it was doing

Status: Accepted
Date: 2026-10-08

## Context

A live "create a Next.js app" run ended with JARVIS handing over a site that did not build, because nothing it could call ran a
build or a test; `jarvis.code.run` needs Docker and an image, so a coding task could not check itself. Giving it that tool then
showed two faults in the approval path that no earlier test reached, because no earlier tool was one a run calls many times:

- Running the **same approved command again** was refused ("this exact call was already asked about in this run"): a unique index
  on `(run_id, intent_hash)` held for every approval state.
- After **every approval** the run resumed from the stored conversation plus the approved result, so it had forgotten every step it
  took this run. In one session it approved the same `npm run build` about fourteen times, each time starting over, and ended with an
  empty answer.

## Decision

1. **`jarvis.command.run`** (scope `command.run`, risk 3, effect `CodeExecution`, approval `Ask`) runs a program in a folder under a
   granted root, only when a folder is granted. There is **no shell**: the program is a bare name resolved on the `PATH` (and
   `PATHEXT` on Windows), its arguments are a list (at most 24, 240 characters each), so nothing is parsed, expanded or chained.
   The directory is canonicalised and must be inside a granted root. The environment is an allowlist plus fixed variables
   (`CI=1`, `NO_COLOR`); no `JARVIS_*` variable or secret is inherited. A timeout (default 180 s, at most 540 s) kills the **whole
   process tree**, and so does dropping the call. Output keeps its head and tail (3,000 characters per stream), has terminal escapes
   removed, and reaches the model fenced as untrusted data.
2. **It is not a sandbox.** It runs on the host with your privileges; the approval is the guard, and the card shows the full command
   line. You may mark it "run without asking" in Permissions (`ADR-0133`) for a project you trust. `jarvis.code.run` remains the
   isolated, Docker-only way to run a snippet. This is a trust decision, recorded here so it is not mistaken for isolation.
3. **An approved call may be repeated.** The unique index is partial: only a `pending` or `denied` approval blocks the same intent
   again in a run (so a model cannot nag past a "no", or stack duplicate questions). An approved, cancelled or expired one is asked
   afresh, so you are still asked every time and the audit trail shows each decision (migration 0015).
4. **A parked run keeps its turn.** When a run waits for an approval, the messages it has produced since context assembly (its
   assistant turn with all its calls, and the results so far) are saved in `run_transcripts` (migration 0016, at most 1 MiB,
   trimmed by whole turns with a note, never half of one). On resume they are put back, followed by the real result of the call that
   waited, and an honest "not run, the run paused for an approval" for each later call in the same turn, because a provider requires
   a result for every call an assistant turn made. The kept turn is deleted when it is consumed. With none saved (a run parked
   before this change) resume falls back to the old path. Counters (model calls, repeat guard, reasoning effort) still restart on
   resume; they are safety nets, not memory.

## Consequences

A coding task can now build, read the error, edit, and build again, asking each time (or never, for a trusted project). The model
is told to run the project's build or tests and fix what fails before claiming it is finished. Verified live with an injected type
error: the run built, found it, removed the line, rebuilt, and answered in one sentence.

Not done: the command tool on Linux and macOS is checked by type only (tracked in `P9-022`); trusting the tool from the approval
card still needs a restart to apply; there is no per-program allowlist (the approval is the policy).