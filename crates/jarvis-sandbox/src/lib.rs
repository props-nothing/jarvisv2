//! OS-level confinement for child processes: the contract, and the backend this host can offer.
//!
//! # The limit this closes
//!
//! An MCP server configured over stdio is **arbitrary third-party code** that the daemon starts and that runs
//! with the daemon's full authority: the same files, the same network, the same memory. Nothing in the transport
//! layer bounds what it does. This crate defines what a caller may *require* of such a child, what each platform
//! can actually enforce, and refuses the difference.
//!
//! # The decision that shapes everything else: a guarantee is an enum, not a bool
//!
//! `docs/architecture/security.md` requires that `doctor` "reports effective guarantees rather than claiming
//! parity", because what a platform can enforce differs fundamentally:
//!
//! - **Linux** gives **cgroup v2**, which is a **filesystem** interface: `pids.max`, `memory.max`, `cpu.max`,
//!   and `cgroup.kill`. The kernel documents the last as dealing with concurrent forks appropriately and being
//!   protected against migrations, which is what makes it a *tree* kill rather than a race against a child that
//!   forks while being signalled. Because it is a filesystem, this backend needs **no FFI**. But a unit only
//!   controls a cgroup the operator **delegated** to it, so on a stock host it is frequently unavailable — and
//!   there is **no** exact per-tree resource limit as a fallback, only per-process `RLIMIT`s that a child
//!   simply forks past.
//! - **Windows** gives a **job object**: a kernel object that owns a *process tree*. `AssignProcessToJobObject`
//!   cannot be undone, children join the job by default, and closing the last handle with
//!   `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` terminates the whole tree. That is **exact** tree membership — but
//!   every call that creates, limits, or assigns one is `unsafe` FFI, and this workspace sets
//!   `unsafe_code = "forbid"`. There is therefore **no Windows job-object backend**, and a policy requiring a
//!   guarantee is refused on Windows unless a container runtime is reachable — see below.
//! - **A container runtime** gives a restricted worker on **any** platform, because it is driven by spawning a
//!   CLI rather than by FFI. That is the only reason this backend exists, and it is why it is the default
//!   wherever the host's own primitives are unavailable: a Windows or macOS host has no job object or seatbelt
//!   backend here, and a Linux host whose cgroup was not delegated has nothing either. It claims **three** of the
//!   five guarantees — the ones whose flags the runtime documents as container-scoped — and refuses the two CPU
//!   guarantees, one because the facility cannot express it (`--ulimit cpu` is per *process*) and one because
//!   the flag is not emitted yet (`--cpus` could express it). `README`-level detail is in
//!   `docs/research/integrations/container-sandboxing.md` and `docs/adr/0128`.
//!
//! The platforms also disagree about CPU, which is why the guarantee is split in two rather than named
//! once: a Windows job object has `PerJobUserTimeLimit`, a **cumulative** ceiling, and no rate form, while
//! cgroup v2 has `cpu.max`, a **rate**, and no cumulative limit file at all. A single "CPU limit" guarantee
//! would force one backend to claim something it cannot enforce — and the container backend, which has flags
//! for both, claims neither, for two further reasons of its own.
//!
//! So the crate models a guarantee as an enum, and a backend that cannot enforce a requested guarantee
//! **refuses** rather than degrading quietly. That is the whole security property: a sandbox that silently
//! enforces less than asked is worse than no sandbox, because by then the caller has stopped checking.
//!
//! # What is deliberately NOT here
//!
//! - **No network policy** as a *guarantee*. The container backend always passes `--network none`, so a
//!   container cannot reach the network — but that is a property of the backend rather than something a caller
//!   can require or `doctor` can report, and the cgroup backend has no equivalent. A child launched by the
//!   **host-process** backends can still reach the network under `Isolation::Restricted`.
//! - **No filesystem confinement** as a *guarantee*, by the same distinction: a container's root is always
//!   read-only, and a cgroup confines nothing about the filesystem. Both facilities express resource ceilings,
//!   so a host-process child can still open any file this process can.
//! - **No privilege reduction.** No child is re-uid'd, so a host-process child keeps this process's authority.
//!   A container runs as the image's user with all capabilities dropped, which is stronger — and again not a
//!   `Guarantee`.
//! - **No seccomp, no Landlock.** Each would be its own slice with its own evidence.
//!
//! One consequence worth stating, because it cuts against the usual direction: the container backend is
//! **stronger than the request says** in several ways it does not claim. That is the reverse of the failure this
//! crate exists to prevent — a caller is not misled into thinking they required more — but it does mean
//! `Isolation::Restricted` and `Support::Container` together describe less than what is actually in force. The
//! alternative, claiming filesystem and network guarantees, would require modelling a policy this crate has no
//! way to express, so the difference is recorded rather than narrowed by inventing a claim.
//!
//! Because of those, `Isolation::Restricted` must never be presented to a user as "sandboxed" without the list
//! of guarantees actually in force. [`SandboxPolicy::supported`] is that list, and it is what `doctor` reports.

// `linux` is deliberately **not** declared here: it is a child of `backend`, which is the module that uses it.
// Declaring `mod linux;` at the crate root as well would include the same file **twice**, as two distinct
// modules with two distinct copies of every type — so `crate::backend::linux::CgroupV2Backend` would be a
// different type from `crate::linux::CgroupV2Backend`, and the unused copy would be reported as dead code.
// `container` is a child of `backend` for the same reason, and is declared exactly once — there.
mod backend;
mod policy;

pub use backend::{
    BackendFuture, ContainerBackend, GuaranteeSupport, Launched, LaunchedProcess, ProcessLauncher,
    SandboxBackend, Support, UnconfinedBackend, backend_for_host, cgroup_v2_guarantees,
    container_guarantees, host_command, refusing_launcher, stdio_launcher,
};
pub use policy::{Guarantee, Isolation, Limits, SandboxError, SandboxPolicy, SandboxRequest};
