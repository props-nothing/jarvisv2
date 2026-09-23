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
//!   `unsafe_code = "forbid"`. The Windows backend is therefore **recorded as unavailable rather than
//!   half-built**: its support set is empty, so a policy requiring anything is refused on Windows instead of
//!   silently running unconfined.
//!
//! The two platforms also disagree about CPU, which is why the guarantee is split in two rather than named
//! once: a Windows job object has `PerJobUserTimeLimit`, a **cumulative** ceiling, and no rate form, while
//! cgroup v2 has `cpu.max`, a **rate**, and no cumulative limit file at all. A single "CPU limit" guarantee
//! would force one backend to claim something it cannot enforce.
//!
//! So the crate models a guarantee as an enum, and a backend that cannot enforce a requested guarantee
//! **refuses** rather than degrading quietly. That is the whole security property: a sandbox that silently
//! enforces less than asked is worse than no sandbox, because by then the caller has stopped checking.
//!
//! # What is deliberately NOT here
//!
//! - **No network policy.** Neither facility can express "this child may not open a socket" without a
//!   namespace or a filter that this crate does not build. A child under `Isolation::Restricted` can still
//!   reach the network.
//! - **No filesystem confinement** of the child. Both facilities express *resource* limits, not a filesystem
//!   view, so a confined child can still open any file this process can.
//! - **No privilege reduction.** The child is not re-uid'd, so it keeps this process's authority.
//! - **No seccomp, no Landlock.** Each would be its own slice with its own evidence.
//!
//! Because of those, `Isolation::Restricted` must never be presented to a user as "sandboxed" without the list
//! of guarantees actually in force. [`SandboxPolicy::supported`] is that list, and it is what `doctor` reports.

// `linux` is deliberately **not** declared here: it is a child of `backend`, which is the module that uses it.
// Declaring `mod linux;` at the crate root as well would include the same file **twice**, as two distinct
// modules with two distinct copies of every type — so `crate::backend::linux::CgroupV2Backend` would be a
// different type from `crate::linux::CgroupV2Backend`, and the unused copy would be reported as dead code.
mod backend;
mod policy;

pub use backend::{
    BackendFuture, GuaranteeSupport, Launched, LaunchedProcess, ProcessLauncher, SandboxBackend,
    Support, UnconfinedBackend, backend_for_host, launcher_for, refusing_launcher,
};
pub use policy::{Guarantee, Isolation, Limits, SandboxError, SandboxPolicy, SandboxRequest};
