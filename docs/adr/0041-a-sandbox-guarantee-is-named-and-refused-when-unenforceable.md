# ADR-0041: A sandbox guarantee is a named capability that is refused when it cannot be enforced

**Status:** Accepted

**Date:** 2026-09-24

## Context

`P3-011` asks for sandbox contracts and **one** restricted process backend, "before exposing code execution".
The pressure behind it is concrete: `crates/jarvis-mcp-transport/src/client.rs` builds an MCP server over stdio
with `tokio::process::Command` and hands it to `TokioChildProcess`. That server is **operator-configured
third-party code**, and it runs with the daemon's own authority — the same files, the same network, the same
memory. Nothing in the transport layer bounds any of it.

`docs/architecture/security.md` requires the answer to be honest rather than uniform: `doctor` must "report
effective guarantees rather than claiming parity", because the platforms differ fundamentally. It also names
the minimum-viable-sandbox-per-OS question as open, and lists shell/command injection as a threat to be mitigated
with "structured commands where possible; no host shell by default".

Two constraints shaped the outcome, and both were discovered rather than assumed.

**The first is `unsafe_code = "forbid"`** in `[workspace.lints.rust]`. Every Windows job-object call —
`CreateJobObjectW`, `SetInformationJobObject`, `AssignProcessToJobObject` — is `unsafe` FFI, so a job-object
backend cannot exist in this workspace without relaxing a repo-wide lint. Linux **cgroup v2**, by contrast, is a
**filesystem interface**: limits are files, and killing a tree is a write. It needs no FFI at all.

**The second is that the two platforms cannot express the same CPU policy.** A Windows job object has
`PerJobUserTimeLimit`, a *cumulative* ceiling, and no rate form. cgroup v2 has `cpu.max`, a *rate*
(`$MAX $PERIOD`), and **no limit file for a cumulative total** — `cpu.stat` only accounts for the time already
used. There is no single "CPU limit" that both can honour.

## Decision

**1. A guarantee is an enum, and there is no "sandboxed" boolean.**

`Guarantee` names `TreeTermination`, `ProcessCountCeiling`, `MemoryCeiling`, `CpuRateCeiling`, and
`CpuTimeCeiling`. A request states which ones it **requires**, and `SandboxPolicy::new` resolves that against
the backend's probe at construction. A backend that cannot enforce a required guarantee produces a refusal that
**names it**; there is no variant meaning "applied, but weaker than asked".

The security argument is that degradation is undetectable downstream. A caller that required a bounded process
tree has, by construction, stopped checking — that is what making the requirement declarative buys. If the
launch quietly returned a weaker confinement, the caller's own logic would have no reason to notice, and the
failure would surface as a fork bomb rather than as an error. **A sandbox that silently enforces less than
asked is worse than no sandbox**, because it is the one configuration nobody audits.

**2. CPU is two guarantees, and the split is the crate's central evidence.**

Collapsing them into one variant would force a backend to claim a capability it does not have on one platform.
Instead, `cgroup_v2_guarantees()` **omits** `CpuTimeCeiling`, and that omission is the load-bearing assertion:
a caller who requires a cumulative CPU ceiling on Linux is refused rather than given an indefinite throttle that
looks like a stop.

The list lives in the always-compiled `backend.rs` rather than in `linux.rs`, so a non-Linux host can still
falsify it. A claim checkable only on the platform it describes is a claim that stops being checked the moment
development happens elsewhere — and this crate is developed on Windows, where the Linux tests are compiled but
never run.

**3. The implemented backend is cgroup v2; every other host reports that it has none.**

Linux gets the real backend. Windows and macOS get an empty support set, which makes every requirement on them
a refusal. This is recorded as the deliberate consequence of `unsafe_code = "forbid"` plus the fact that
macOS's seatbelt facility expresses **filesystem and network** policy rather than resource ceilings, i.e. it
does not implement this crate's guarantees at all. Reporting a guarantee the host cannot provide would be the
parity claim the architecture forbids.

`backend_for_host` **probes the running host** — `/sys/fs/cgroup/cgroup.controllers` for v2, then the process's
own cgroup from `/proc/self/cgroup`, then the mount root — rather than reporting the compiled target. A Linux
host with no delegated cgroup, which includes most containers and essentially every interactive shell, offers
nothing: a unit only controls a cgroup the operator delegated to it.

**4. `RLIMIT_NPROC` is not a `ProcessCountCeiling`, and is deliberately unused.**

The obvious fallback on a host without cgroups is per-process `RLIMIT_NPROC`/`RLIMIT_AS`/`RLIMIT_CPU`. It is
rejected because it would be a lie by construction: `RLIMIT_NPROC` is counted **per user id**, so a child that
reaches its own limit can still `fork` — every process it creates gets a fresh budget under the same uid. Only a
**tree-scoped** counter makes a fork bomb fail, and only cgroups provide one.

**5. The child is confined before it exists, and the one window that cannot be closed is documented.**

Linux writes `pids.max`, `memory.max`, and `cpu.max` **before** the spawn, because a limit applied afterwards
may already have been exceeded — for `memory.max`, the allocation has happened. The spawn itself is **injected**
by the caller (`ProcessLauncher`) because only the caller knows whether it needs the child's pipes, and
injecting it is also what makes the ordering testable: a `refusing_launcher` records whether it was called at
all, which is exact where a process-table inspection would be racy.

cgroup v2 has **no atomic spawn-into-cgroup** for a `fork`/`exec` pair, so there is a window between the spawn
and the `cgroup.procs` write. That window is stated as a limit rather than described as closed. A test asserts
the child really lands in the launch's cgroup by reading `/proc/<pid>/cgroup` from the kernel, because a
migration that addressed the wrong pid would still report `Ok` while every limit was inert.

**6. `doctor` reports the guarantees in force, and an absent capability is `Info`.**

`check_sandbox` emits `SandboxAvailable` or `SandboxUnavailable` with the facility and the guarantee list as
evidence — the **list**, not a boolean, because a boolean would let a host enforcing one of four read
identically to one enforcing all four. Both codes are `Severity::Info`, following `ServiceNotApplicable`:
a capability absent **by design** is informational, and `SandboxUnavailable`'s remediation names the
`systemd Delegate=yes` action that would change the answer.

The branch selection was extracted into `sandbox_finding(support, facility)` so **both** branches are testable
on any host. That was not tidiness: a falsification sweep mutated the available branch's evidence and the suite
stayed green, because no Windows host can reach it.

**7. There is no shell, and the environment is an allowlist in fact.**

`SandboxRequest` carries the program and an argv **vector**, so no metacharacter is ever interpreted — the
structured-command mitigation the architecture asks for, made structural. `launcher_for` applies `env_clear()`
followed by exactly the request's pairs, so no host credential or `PATH` entry is inherited by omission. A test
asserts the **absence** of an inherited variable as well as the presence of the request's own, because an added
sentinel is visible whether or not `env_clear` ran and would pass on its own.

## Consequences

- **The first caller is a report, not an execution path.** `doctor` is wired; `connect_stdio` is **not**. The
  slice says "implement one restricted process backend **before exposing code execution**", and that is the
  state reached: the contract exists, refusal works, and no code-execution surface depends on it yet. Wiring the
  MCP stdio launcher needs a trait seam so `jarvis-mcp-transport` does not depend on `jarvis-sandbox`, and it is
  recorded in `TODO.md` as the next step rather than implied to be done.
- **On Windows, every guarantee is refused.** Anyone enabling code execution on Windows must first either
  relax `unsafe_code` for a job-object crate or delegate the work to a separate process that does, and that is a
  decision with its own ADR. Reporting an empty support set is what makes `doctor` say so.
- **Cross-compiling the Linux target is part of this crate's gate.**
  `cargo clippy -p jarvis-sandbox --all-targets --all-features --target x86_64-unknown-linux-gnu -- -D warnings`
  found six defects invisible on Windows — an import unused only on Linux, two collapsible `if let`s, identical
  `match` arms, an error-type mismatch in the kill path, and a duplicated `linux` module from declaring it in
  two places. A `#[path]` module declared at both the crate root and a child includes the file **twice**, as two
  modules with distinct copies of every type.
- **The Linux tests are compiled but have not been executed here.** They skip **loudly** when no delegation
  exists, printing what could not be checked, because a silently skipped assertion is indistinguishable from one
  that ran. This gap is recorded in `TODO.md`.
- **No network policy, no filesystem confinement, no privilege reduction.** `Isolation::Restricted` bounds
  *resources*; a confined child can still open any file and reach any host this process can. Seccomp and
  Landlock would each be their own slice with their own evidence, and `Isolation::Restricted` must never be
  presented as "sandboxed" without the guarantee list beside it.
- **`cpu.max` is a rate, and the field name says so.** `Limits` has separate `max_cpu_rate` and `max_cpu_time`
  fields rather than one duration interpreted two ways; a single field would have let a caller's cumulative
  ceiling be silently applied as a rate.
