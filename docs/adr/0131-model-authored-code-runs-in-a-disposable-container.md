# ADR-0131: Model-authored code runs in a disposable container, and is always held for a person

**Status:** Accepted
**Date:** 2026-10-03
**Slice:** `P3-029`

## Context

A model that can only talk, read files and fetch pages cannot calculate, transform data, or check its own claims.
Running code is the capability that changes that, and the one most worth getting wrong slowly: the code is
model-authored, and the model may be quoting a web page it just fetched.

`ADR-0128` built a container backend and `ADR-0041` made its guarantees named and refusable, but nothing used it to
run anything a model wrote. Two things stood between the backend and a tool: `LaunchedProcess` could not express
"wait, and kill if it takes too long" (its header predicted this: *"a slice that needs both will have to redesign
this signature"*), and there was no way for an operator to say which image a snippet runs in.

## Decision

1. **`jarvis.code.run` runs one snippet per call in a fresh container**, through `jarvis-sandbox`'s container
   backend: no network, read-only root filesystem, all capabilities dropped, at most 64 processes and 256 MiB, an
   environment that is exactly `PATH`, no image ever pulled, nothing kept afterwards. `TreeTermination`,
   `ProcessCountCeiling` and `MemoryCeiling` are *required*, so a host that cannot enforce them refuses the launch
   rather than running with less.
2. **`LaunchedProcess::wait_for(limit)`** is added: wait, and if the limit passes, kill. The container backend
   removes the **container** (killing the `docker` CLI alone leaves the work running in the daemon) and the
   unconfined backend kills the child. The cgroup backend inherits a default that **refuses** instead of waiting
   without a bound — an unbounded wait presented as a bounded one is the quiet failure this crate exists to prevent.
   It is not implemented there yet and is recorded as a limit.
3. **The operator names the image and the interpreter** — `daemon.code_sandbox_image` and
   `daemon.code_sandbox_interpreter` (e.g. `["python3", "-c"]`), with `JARVIS_CODE_SANDBOX_*` overrides. Both or
   neither: half a sandbox is refused at startup. The interpreter is an argument vector with the snippet appended as
   one argument — there is no shell. The tool is **absent**, not present-and-failing, unless both are set and the
   host can run a container; a configured sandbox with no reachable runtime is reported once at startup.
4. **Every run is held for a person, whatever the workspace allows** — *amended by [ADR-0133](0133-approval-is-for-what-can-hurt-and-the-owner-can-decide-once.md): a threshold never lifts the hold, but the owner's explicit `policy.trust` entry does.* The tool is `code_execution` (risk floor 3)
   with `ApprovalPolicy::Ask`, which is unconditional; a workspace can only tighten an approval (`ADR-0122`), so no
   `approval_threshold` makes model-authored code run unattended. The person sees the code first, because a pending
   approval carries its arguments (`ADR-0130`) and `jarvis approvals` prints a multi-line program as lines.
   A sandbox bounds what code can do; it does not decide whether to run it.
5. **Output is untrusted and fenced.** Standard output and error each go through `IsolatedText` inside the adapter.
   The exit status and the timeout are the adapter's own statements and are not fenced. A non-zero exit is an answer
   about the *program* and is reported in `exit_code`, not as a failure of the tool.
6. **The per-result budget is a shared constant.** `jarvis_tools::MAX_MODEL_FACING_RESULT_CHARS` (12,000, raised from
   4,000) is what the executor truncates to and what `jarvis.web.fetch` and `jarvis.code.run` size their output
   against — both with compile-time assertions that the worst JSON-escaped result still fits, because a truncation
   that cut a closing fence would hand the model unterminated untrusted data.

## Consequences

- The model can calculate. Verified live: asked for the sum of the first 100 primes, it wrote JavaScript, a person
  approved the code after reading it, the code ran in `node:22-alpine`, and the answer (24,133) was correct.
- No files in or out, no network, no state between runs. That is the safe place to start, and it is a limit.
- The 30-second run limit and the resource ceilings are constants, not configuration.
- Host-side output is read to 64 KiB per stream and the rest discarded, so a program that floods a pipe cannot block
  itself into a misleading timeout.
- A snippet longer than 8 KiB of arguments (4,000 characters, possibly more bytes) cannot be shown to the approver and
  therefore cannot be approved from the CLI.

## Falsification

`the_snippet_cannot_reach_the_network_or_write_the_filesystem` runs real escape attempts in a real container and
reports what happened to each; `a_child_past_its_limit_is_killed_and_its_container_removed` asks the runtime whether
the container is gone, not the return value; `the_snippet_inherits_none_of_the_hosts_environment` checks for host
variables in the container's `env`; `code_is_held_for_a_person_under_the_most_permissive_policy` raises both the
ceiling and the threshold to `High` and still gets a hold. The live tests skip **loudly** on a host without a runtime
or without the image, which the sandbox never pulls.
