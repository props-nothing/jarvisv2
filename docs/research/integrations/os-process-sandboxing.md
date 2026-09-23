# OS process sandboxing (Linux cgroup v2, Windows job objects)

**Researched:** 2026-09-24
**Used by:** `P3-011` — `crates/jarvis-sandbox`
**Status:** Linux cgroup v2 backend implemented and type-checked for `x86_64-unknown-linux-gnu`; **not executed**
on a Linux host in this development environment. Windows backend **deliberately not implemented**; see
"Unresolved and rejected" below.

## Sources

| What | URL | Fetched | Notes |
| --- | --- | --- | --- |
| cgroup v2 (unified hierarchy) | https://docs.kernel.org/admin-guide/cgroup-v2.html | 2026-09-23 | Official kernel documentation. The authority for every file name and semantic below. |
| Windows Job Objects | https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects | 2026-09-23 | Official Microsoft documentation. |
| `JOBOBJECT_EXTENDED_LIMIT_INFORMATION` | https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-jobobject_extended_limit_information | 2026-09-23 | Field-level detail for the limit structure. |
| `JOBOBJECT_BASIC_LIMIT_INFORMATION` | https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-jobobject_basic_limit_information | 2026-09-23 | `LimitFlags`, `ActiveProcessLimit`, `PerJobUserTimeLimit`. |

No `llms.txt` applies to either: both are operating-system interfaces rather than an SDK or service, and there is
no vendor documentation index to prefer. The kernel documentation and Microsoft Learn are the primary sources.

## Linux: cgroup v2 is a filesystem interface

The decisive property is that **every operation this crate needs is a file operation**, which is why this backend
needs no `unsafe`, no FFI, and no `libc` — and therefore why it is implementable at all under this workspace's
`unsafe_code = "forbid"`.

| Guarantee | File | Documented behaviour |
| --- | --- | --- |
| `ProcessCountCeiling` | `pids.max` | A `fork` past the limit fails with `EAGAIN`, for the cgroup **and all its descendants**. |
| `MemoryCeiling` | `memory.max` | The OOM killer runs **inside the cgroup only**, so a runaway child cannot take the daemon with it. |
| `CpuRateCeiling` | `cpu.max` = `$MAX $PERIOD` | A bandwidth ceiling: time allowed per period. Throttling, not a kill. |
| `TreeTermination` | `cgroup.kill` | Writing any non-empty value kills the cgroup **and all descendant cgroups**. The documentation states it "deal[s] with concurrent forks appropriately and is protected against migrations". |

Supporting facts used by the implementation:

- **`cgroup.procs` is the migration interface.** "On creation, all processes are put in the cgroup that the parent
  process belongs to", so a child must be explicitly written into the target cgroup's `cgroup.procs`.
- **Writing `max` clears a limit.** Therefore each limit file is written **only** when the corresponding limit is
  present; an unconditional write would clear a limit an operator had set higher in the hierarchy.
- **A unit only controls a cgroup delegated to it.** This is why the backend probes rather than assumes: a stock
  host, most containers, and essentially every interactive shell have `/sys/fs/cgroup` owned by `root` with no
  delegated subtree.
- **`No Internal Process Constraint`:** a cgroup with children must have no processes of its own. The backend uses
  a single leaf per launch and creates no subgroups, so the constraint does not bind it. A future design with
  per-launch subgroups would have to respect it.
- **`cgroup.controllers` is the v2 discriminator**, which avoids parsing `/proc/mounts`.

### `CpuTimeCeiling` is NOT available on Linux, and this is the load-bearing omission

cgroup v2 **accounts** CPU time in `cpu.stat` but exposes **no limit file for a cumulative total**. Only the rate
form (`cpu.max`) exists. `crate::backend::cgroup_v2_guarantees()` therefore omits `CpuTimeCeiling`, and a request
that requires it is **refused by name**.

Interpreting a cumulative ceiling as "roughly a second of CPU per period" would be a different policy — a rate
ceiling throttles a long-running child indefinitely, while a cumulative ceiling stops it once — so the two are
separate `Guarantee` variants rather than one, and the unit test asserting the **omission** lives in
`backend.rs` (always compiled) so a non-Linux host can still falsify it.

### `RLIMIT_*` is not an acceptable fallback, by construction

`RLIMIT_NPROC` is counted **per user id**. A child that reaches its own process limit can still `fork`, because
every process it creates receives a fresh budget under the same uid. Only a **tree-scoped** counter makes a fork
bomb fail, and only cgroups provide one. `RLIMIT_AS`/`RLIMIT_CPU` are likewise per-process and do not compose
into a tree limit. Reporting `ProcessCountCeiling` on the strength of `RLIMIT_NPROC` would be a guarantee a fork
bomb walks straight through, so the fallback is rejected rather than used.

### The one window that cannot be closed

There is **no atomic spawn-into-cgroup** for a `fork`/`exec` pair. Limits are written **before** the spawn — which
is the part that matters for `memory.max`, since a limit applied afterwards is applied after the allocation — but
a brief window remains between the spawn and the `cgroup.procs` write. This is **documented as a limit** in the
backend and asserted indirectly: a test reads `/proc/<pid>/cgroup` to prove the child really landed in the launch's
cgroup, because a migration that addressed the wrong pid would still report `Ok` while every limit was inert.

## Windows: job objects can express what cgroup v2 cannot, but are all `unsafe` FFI

Documented behaviour that a future backend would rely on:

- Processes created by a process in a job are **also in that job by default**, unless
  `JOB_OBJECT_LIMIT_BREAKAWAY_OK` or `JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK` is set. "Prevent breakaways of any
  kind by setting neither."
- **"After a process is associated with a job, the association cannot be broken."**
- `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`: closing the last handle to the job **terminates all associated
  processes**; for a nested job it terminates the job *and its child jobs*.
- `TerminateJobObject` terminates all processes in the job; `IsProcessInJob` reports membership.
- `JOBOBJECT_BASIC_LIMIT_INFORMATION` carries `LimitFlags`, `PerJobUserTimeLimit`, `PerJobUserLimit`, and
  `ActiveProcessLimit`; `JOBOBJECT_EXTENDED_LIMIT_INFORMATION` adds `ProcessMemoryLimit` and `JobMemoryLimit`.
- Accounts survive termination: `QueryInformationJobObject` returns `PeakJobMemoryUsed`, so a tree's peak memory
  can be read **after every process is gone**. This has no cgroup v2 equivalent.
- Jobs nest since Windows 8, so a test process that is already in a job is still fully captured.

**CPU is the documented asymmetry**, and it is why `Guarantee` has two CPU variants: a job object has
`PerJobUserTimeLimit` (cumulative) and no rate form, while cgroup v2 has `cpu.max` (rate) and no cumulative limit
file.

**Why it is not implemented here:** `CreateJobObjectW`, `SetInformationJobObject`, and `AssignProcessToJobObject`
are all `unsafe` FFI, and `[workspace.lints.rust] unsafe_code = "forbid"`. Implementing a job-object backend means
either relaxing a repo-wide lint for this crate or delegating the work to a separate helper process — a decision
with its own ADR. Until then, `backend_for_host()` reports an **empty** support set on Windows, so every
requirement is refused there rather than silently unmet.

**The assign-race, recorded for whoever implements it:** the standard mitigation is a *permissive* job assigned to
the child first (so the child's own `AssignProcessToJobObject` calls succeed), then a *restrictive* inner job in
which the child is created. `windows-sys` does not generate `PROC_THREAD_ATTRIBUTE_JOB_LIST`, which is the cleaner
documented route, so the two-job sequence is what remains.

## Unresolved and rejected

- **Windows job objects:** documented above, **not implemented** (requires `unsafe`). No guarantee is claimed.
- **macOS:** no job objects and no cgroups. The facility that exists (`sandbox_init`'s seatbelt profile) expresses
  **filesystem and network** policy rather than the resource ceilings this crate models, so it does not implement
  these guarantees at all. `backend_for_host()` reports none.
- **No network policy and no child filesystem confinement.** Both facilities bound *resources*, not a filesystem
  view or a socket, so a child under `Isolation::Restricted` can still open any file and reach any host this
  process can. Seccomp and Landlock would each be their own slice.
- **No privilege reduction.** The child is not re-uid'd and keeps this process's authority.
- **`memory.max` is not a total across time**, only a concurrent commitment ceiling, so it does not bound a child
  that allocates and frees repeatedly. A cumulative memory bound is not available.
- **`cpu.max` semantics depend on the period**, which this implementation fixes at one second explicitly so the
  written budget does not depend on how a parent cgroup happened to be configured.
- **Delegation on a stock host requires operator action.** The Linux tests skip **loudly** when no delegation
  exists, printing what could not be checked. The path they cover has not been executed in this development
  environment, and `TODO.md` records that.
