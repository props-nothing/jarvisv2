//! The backend contract, and the backend this host can actually offer.
//!
//! # The trait
//!
//! [`SandboxBackend`] is deliberately small — three methods — because everything a sandbox *is* lives in
//! [`crate::SandboxPolicy`]. A backend answers one question ("what can this machine enforce?"), names one
//! facility, and performs one operation: "launch, confined or not at all".
//!
//! # Why the launch returns a boxed future rather than being `async fn`
//!
//! `dyn SandboxBackend` has to be object-safe: the composition root picks a backend at **runtime**, because
//! whether a host has a delegated cgroup is not known at compile time (see [`backend_for_host`]). An `async fn`
//! in a trait has no `dyn`-compatible form, so the launch method returns [`BackendFuture`] explicitly.

use std::future::Future;
use std::pin::Pin;
use std::process::Stdio;

use crate::policy::{Guarantee, SandboxError, SandboxPolicy};

/// A boxed future, so the trait stays object-safe.
pub type BackendFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// What a backend can enforce on this machine.
///
/// A value rather than a per-guarantee query, because the answer is the same for every request: computing it
/// once and carrying it makes `doctor` cheap and makes the policy check an in-memory comparison rather than a
/// syscall per requirement.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GuaranteeSupport {
    /// Sorted and deduplicated, so two backends supporting the same guarantees compare equal regardless of how
    /// they were constructed, and `doctor`'s output is stable.
    guarantees: Vec<Guarantee>,
}

impl GuaranteeSupport {
    /// Builds a support set from an iterator, sorting and deduplicating it.
    #[must_use]
    pub fn from_guarantees(guarantees: impl IntoIterator<Item = Guarantee>) -> Self {
        let mut guarantees: Vec<Guarantee> = guarantees.into_iter().collect();
        guarantees.sort_unstable();
        guarantees.dedup();
        Self { guarantees }
    }

    /// Returns whether this set contains a guarantee.
    #[must_use]
    pub fn supports(&self, guarantee: Guarantee) -> bool {
        self.guarantees.contains(&guarantee)
    }

    /// Returns every guarantee in the set, sorted.
    #[must_use]
    pub fn guarantees(&self) -> &[Guarantee] {
        &self.guarantees
    }

    /// Returns whether the set is empty, i.e. this machine enforces nothing.
    ///
    /// The **stronger** of the two possible empty readings is the one named: an empty set here means no
    /// confinement is available at all, not that nothing was required. A caller who meant the latter has an
    /// empty `required` list on its request, which is a different value.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.guarantees.is_empty()
    }
}

/// The facility a backend uses, so a report can name *what* confined a child rather than only assert that
/// something did.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Support {
    /// The Linux cgroup-v2 backend.
    CgroupV2,
    /// No OS primitive, so nothing is enforced.
    Unconfined,
}

impl Support {
    /// Returns the label `doctor` prints.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CgroupV2 => "cgroup_v2",
            Self::Unconfined => "unconfined",
        }
    }
}

impl std::fmt::Display for Support {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A child process that has been launched under a policy.
///
/// # Why this is a trait rather than a struct holding a `Child`
///
/// The confinement has to live **inside the same value** as the child, because on both platforms it is a
/// resource whose release changes what happens to the child: closing the last handle to a job object set to
/// kill on close terminates the tree, and a cgroup can only be removed once no process is left in it. A struct
/// that held a bare `Child` would put the confinement somewhere a caller could drop early, leaving a process
/// that dies for a reason no caller could see.
///
/// Process id and kill are the whole surface. Anything else a caller needs — the child's pipes, to drive an MCP
/// server — is on [`Launched`], so this trait does not pretend every backend produces the same child.
pub trait LaunchedProcess: Send {
    /// Returns the child's process id, for correlation in a log line.
    ///
    /// `0` when the id is no longer available, which is the state after the child has been reaped. A sentinel
    /// rather than an `Option` because every caller's use is a log field, and a `0` there reads as "gone" while
    /// an `Option` would push a decision nobody needs to make at each call site.
    fn pid(&self) -> u32;

    /// Kills the confined tree.
    ///
    /// Takes `self` by value because termination ends the confinement: whatever keeps the tree captured is
    /// released here, so the value cannot be used afterwards to observe a process it no longer owns.
    fn kill(self: Box<Self>) -> BackendFuture<Result<(), SandboxError>>;
}

/// The result of a successful [`SandboxBackend::launch`].
pub struct Launched {
    /// The launched child.
    pub process: Box<dyn LaunchedProcess>,
    /// Which facility confined it.
    pub support: Support,
    /// The guarantees in force for **this** launch.
    ///
    /// Recorded rather than recomputed at the call site so a live child and an audit record cannot disagree
    /// about what was promised. By construction this is the policy's `required` set, which
    /// [`SandboxPolicy::new`] already proved the backend supports.
    pub enforced: Vec<Guarantee>,
    /// The child's standard input, present when the launcher captured it.
    ///
    /// Separate from `process` because it is caller-facing rather than confinement-related: a caller that only
    /// supervises a child ignores it, and one wiring an MCP server takes it.
    pub stdin: Option<tokio::process::ChildStdin>,
    /// The child's standard output, under the same rule as [`Self::stdin`].
    pub stdout: Option<tokio::process::ChildStdout>,
    /// The child's standard error, under the same rule as [`Self::stdin`].
    pub stderr: Option<tokio::process::ChildStderr>,
}

impl std::fmt::Debug for Launched {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Launched")
            .field("pid", &self.process.pid())
            .field("support", &self.support)
            .field("enforced", &self.enforced)
            .field("stdin", &self.stdin.is_some())
            .field("stdout", &self.stdout.is_some())
            .field("stderr", &self.stderr.is_some())
            .finish()
    }
}

/// Builds the concrete child, so stdio wiring stays a caller decision.
///
/// A one-shot boxed closure rather than a pre-built [`Command`](tokio::process::Command), because the spawn has
/// to happen **after** the confinement exists but from inside the launch future. That ordering is the point:
/// the Linux backend writes `pids.max` and `memory.max` before the child can create its first process, so a
/// spawn performed any earlier would be a child that ran unbounded for a while.
pub type ProcessLauncher = Box<dyn FnOnce() -> std::io::Result<tokio::process::Child> + Send>;

/// Builds a launcher that starts a request's program with its exact environment.
///
/// The environment is **cleared and replaced** rather than extended: `env_clear` plus the request's pairs is
/// what makes the request's map an allowlist in fact rather than in intention. There is no shell — program and
/// arguments go in as an argv vector — so no metacharacter is ever interpreted.
///
/// `piped` decides whether the three standard streams are captured, which is the one thing the request cannot
/// state, because it is a property of how the caller intends to use the child rather than of the confinement.
#[must_use]
pub fn launcher_for(request: &crate::SandboxRequest, piped: bool) -> ProcessLauncher {
    let program = request.program.clone();
    let arguments = request.arguments.clone();
    let working_directory = request.working_directory.clone();
    let environment = request.environment.clone();
    Box::new(move || {
        let mut command = tokio::process::Command::new(&program);
        command.args(&arguments).env_clear().envs(&environment);
        if piped {
            command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
        }
        if let Some(directory) = &working_directory {
            command.current_dir(directory);
        }
        command.spawn()
    })
}

/// A launcher that never starts a process, for asserting that a refusal happens **before** one exists.
///
/// Public because it is the assertion mechanism for the crate's most important ordering property. A test that
/// checked "no child was created" by inspecting a process table would be racy; a launcher that records whether
/// it was called is exact, and it fails when the ordering is wrong instead of only when a race loses.
#[must_use]
pub fn refusing_launcher(
    observed: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> ProcessLauncher {
    Box::new(move || {
        observed.store(true, std::sync::atomic::Ordering::SeqCst);
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "the refusing launcher was called",
        ))
    })
}

/// Launches confined children.
///
/// # What an implementation must not do
///
/// It must not launch first and confine second. A child that ran before its limits were applied has already
/// done whatever it was going to do with no ceiling, so `launch` returns a **confined** child or an error —
/// never a running unconfined one. Where a platform forces a window between spawn and confinement, that window
/// is documented at the backend and recorded as a limit rather than described as closed.
pub trait SandboxBackend: Send + Sync {
    /// Returns what this backend can enforce on this machine.
    ///
    /// The set is what the host was **probed** for, not what the target can compile: see [`backend_for_host`].
    fn support(&self) -> GuaranteeSupport;

    /// Returns the facility label for reports.
    fn facility(&self) -> Support;

    /// Spawns and confines a child.
    ///
    /// # Errors
    ///
    /// Returns [`SandboxError::Setup`] when the OS facility cannot be created — a cgroup the operator has not
    /// delegated, for example — and [`SandboxError::Launch`] when the child cannot be started or placed under
    /// the limits. A launch that cannot apply a required guarantee is a failure here; there is no path that
    /// returns `Ok` for a partially confined child.
    ///
    /// The launcher is **injected** rather than built from [`crate::SandboxRequest`] because only the caller
    /// knows whether it needs the child's pipes. It is called exactly once, and only after the confinement is
    /// in place.
    fn launch(
        &self,
        policy: &SandboxPolicy,
        launcher: ProcessLauncher,
    ) -> BackendFuture<Result<Launched, SandboxError>>;
}

/// Returns the strongest backend this host can offer.
///
/// # Why this is a function rather than a constant or a `#[cfg]` type alias
///
/// The answer depends on the **running machine**, not on the compiled target. A Linux host with no delegated
/// cgroup — a stock install, most containers, essentially every interactive development shell — cannot offer
/// the same guarantees as one that has them. Reporting the compiled target would claim a guarantee the host
/// does not provide, which is exactly the parity claim `docs/architecture/security.md` forbids.
#[must_use]
pub fn backend_for_host() -> Box<dyn SandboxBackend> {
    #[cfg(target_os = "linux")]
    {
        // `self::linux`, not `crate::linux`: the module is a child of *this* file, declared at the bottom, so
        // the path has to match where it actually lives.
        Box::new(self::linux::CgroupV2Backend::probe())
    }
    #[cfg(not(target_os = "linux"))]
    {
        // Windows could use a job object and macOS a seatbelt profile. Both need `unsafe` FFI that this
        // workspace's `unsafe_code = "forbid"` rejects, and macOS's facility expresses filesystem and network
        // policy rather than the resource ceilings this crate models. Reporting an unconfined backend is the
        // honest answer, and it is what makes `doctor` on those hosts say so rather than imply parity.
        Box::new(UnconfinedBackend)
    }
}

/// A backend that enforces nothing, for a platform whose facilities do not express these guarantees.
///
/// It exists so [`backend_for_host`] has an answer everywhere rather than an `Option` every caller must handle.
/// Its support set is empty, so [`SandboxPolicy::new`] refuses any request that requires something — the
/// refusal happens in one place instead of at every call site.
#[derive(Debug)]
pub struct UnconfinedBackend;

impl SandboxBackend for UnconfinedBackend {
    fn support(&self) -> GuaranteeSupport {
        GuaranteeSupport::default()
    }

    fn facility(&self) -> Support {
        Support::Unconfined
    }

    fn launch(
        &self,
        policy: &SandboxPolicy,
        launcher: ProcessLauncher,
    ) -> BackendFuture<Result<Launched, SandboxError>> {
        // Taken before the future is created, because `enforced()` borrows the policy and a `BackendFuture` is
        // `'static`. Cloning here is also the honest encoding of what happens: a launch reads its policy once.
        let enforced = policy.enforced();
        Box::pin(async move {
            let mut child = launcher().map_err(|error| SandboxError::Launch {
                reason: format!("spawn failed: {}", error.kind()),
            })?;
            let stdin = child.stdin.take();
            let stdout = child.stdout.take();
            let stderr = child.stderr.take();
            Ok(Launched {
                process: Box::new(ChildHandle { child }),
                support: Support::Unconfined,
                enforced,
                stdin,
                stdout,
                stderr,
            })
        })
    }
}

/// The plain child handle: a process with nothing confining it.
struct ChildHandle {
    child: tokio::process::Child,
}

impl LaunchedProcess for ChildHandle {
    fn pid(&self) -> u32 {
        self.child.id().unwrap_or(0)
    }

    fn kill(self: Box<Self>) -> BackendFuture<Result<(), SandboxError>> {
        Box::pin(async move {
            let mut child = self.child;
            child.kill().await.map_err(|error| SandboxError::Launch {
                reason: format!("kill failed: {}", error.kind()),
            })
        })
    }
}

#[cfg(target_os = "linux")]
#[path = "linux.rs"]
mod linux;

/// The guarantees the cgroup v2 backend provides **when** a delegation exists.
///
/// This lives here, in the always-compiled module, rather than inline in `linux.rs`, because that is what makes
/// the claim checkable on a host that is not Linux. Two different questions are being asked and only conflating
/// them would hide one: *which guarantees can this facility express?* is a fixed fact about cgroup v2, while
/// *does this host have a delegation?* is what `probe()` answers. Keeping the list in a `#[cfg(target_os]`
/// module would mean the first question could only ever be checked by running the suite on Linux — and a claim
/// that cannot be checked where it is written is exactly the claim that goes wrong, especially for the entry
/// that is deliberately **absent**.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) const fn cgroup_v2_guarantees() -> [Guarantee; 4] {
    [
        Guarantee::TreeTermination,
        Guarantee::ProcessCountCeiling,
        Guarantee::MemoryCeiling,
        Guarantee::CpuRateCeiling,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The cgroup v2 guarantee list excludes `CpuTimeCeiling`, and that exclusion is the whole point.**
    ///
    /// cgroup v2 **accounts** CPU time in `cpu.stat` but has no limit file for a cumulative total — only the
    /// rate form in `cpu.max`. Reporting `CpuTimeCeiling` as supported would let a caller require "stop this
    /// child after N seconds of CPU" and receive a child that is *throttled* instead, forever. The two are not
    /// the same promise, and a caller who stopped checking at construction would never learn the difference.
    ///
    /// This test exists on every platform on purpose: it is the one assertion about the Linux backend that a
    /// non-Linux host can still falsify, and it is written against the deliberately-absent entry rather than the
    /// present ones, because an omission is what a well-meaning edit reintroduces.
    ///
    /// Falsified by mutation: adding `Guarantee::CpuTimeCeiling` to `cgroup_v2_guarantees` fails here, and would
    /// fail on Linux too — where this suite does not run during development.
    #[test]
    fn the_cgroup_v2_list_omits_the_guarantee_that_facility_cannot_provide() {
        let listed = cgroup_v2_guarantees();
        assert!(
            !listed.contains(&Guarantee::CpuTimeCeiling),
            "cgroup v2 has no cumulative CPU limit file, so it must never be claimed: {listed:?}"
        );
        assert_eq!(
            listed.len(),
            4,
            "the list is exactly the four guarantees cgroup v2 expresses"
        );
        let support = GuaranteeSupport::from_guarantees(listed);
        for expected in [
            Guarantee::TreeTermination,
            Guarantee::ProcessCountCeiling,
            Guarantee::MemoryCeiling,
            Guarantee::CpuRateCeiling,
        ] {
            assert!(support.supports(expected), "{expected} must be claimed");
        }
    }

    /// **An empty support set is `is_empty`, and a set built from the cgroup list is not.**
    ///
    /// `GaranteeSupport::is_empty` decides whether `doctor` reports "nothing enforced" versus a facility, so a
    /// wrong answer there misreports a host in the direction an operator would not check. The positive control
    /// is included because `is_empty` returning `true` unconditionally would satisfy the negative case alone.
    #[test]
    fn support_emptiness_reflects_the_set() {
        assert!(GuaranteeSupport::default().is_empty());
        assert!(!GuaranteeSupport::from_guarantees(cgroup_v2_guarantees()).is_empty());
    }

    /// **A support set is order-independent and deduplicated.**
    ///
    /// Built from a set of guarantees in arbitrary order, it must compare equal to one built in sorted order —
    /// otherwise two backends that enforce the same things would report differently depending on construction,
    /// and `doctor`'s output would drift for no reason an operator could act on.
    ///
    /// Falsified by mutation: removing the `sort_unstable`/`dedup` in `from_guarantees` fails here.
    #[test]
    fn support_is_order_independent_and_deduplicated() {
        let a = GuaranteeSupport::from_guarantees([
            Guarantee::MemoryCeiling,
            Guarantee::TreeTermination,
            Guarantee::MemoryCeiling,
        ]);
        let b = GuaranteeSupport::from_guarantees([
            Guarantee::TreeTermination,
            Guarantee::MemoryCeiling,
        ]);
        assert_eq!(
            a, b,
            "the same guarantees in any order are the same support"
        );
        assert_eq!(a.guarantees().len(), 2, "the duplicate must be removed");
    }
}
