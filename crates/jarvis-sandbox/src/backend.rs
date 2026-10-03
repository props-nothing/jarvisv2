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
    /// A container runtime, driven through its CLI.
    Container,
    /// No OS primitive, so nothing is enforced.
    Unconfined,
}

impl Support {
    /// Returns the label `doctor` prints.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CgroupV2 => "cgroup_v2",
            Self::Container => "container",
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
/// Process id, waiting, and kill are the whole surface. Anything else a caller needs — the child's pipes, to
/// drive an MCP server — is on [`Launched`], so this trait does not pretend every backend produces the same
/// child.
///
/// # Why `wait` was added by `P3-020`, and why that is a finding rather than a detail
///
/// The first version of this trait had `pid()` and `kill()` — a handle that could be **destroyed but never
/// observed**. Writing the container acceptance test made the gap plain: proving that a container's exit status
/// reaches the caller requires reading that status, and there was no method to read it, so *no* live property of
/// any backend could be asserted at all. A supervisor's two questions are "is it done?" and "was it fine?", and
/// neither was answerable.
///
/// It takes `self` by value for the reason [`LaunchedProcess::kill`] does: the method returns a `'static` future,
/// which cannot borrow from `self`. The limitation that follows is real and recorded rather than hidden: a
/// consuming wait cannot be cancelled, so "wait with a timeout, then kill" is not expressible through this
/// trait. A caller that needs it must use [`Self::kill`], and a slice that needs both will have to redesign this
/// signature — which is a smaller change than it sounds, and better made against a caller that exists.
pub trait LaunchedProcess: Send {
    /// Returns the child's process id, for correlation in a log line.
    ///
    /// `0` when the id is no longer available, which is the state after the child has been reaped. A sentinel
    /// rather than an `Option` because every caller's use is a log field, and a `0` there reads as "gone" while
    /// an `Option` would push a decision nobody needs to make at each call site.
    fn pid(&self) -> u32;

    /// Waits for the confined child to exit and returns its status.
    ///
    /// # Errors
    ///
    /// Returns [`SandboxError::Launch`] when the wait itself fails — which means this process could not collect
    /// the child rather than that the child did badly. A child that exited non-zero is a **success** here, with
    /// the status carrying that: conflating "the wait failed" with "the program failed" would make every
    /// ordinary failure look like a sandbox problem.
    fn wait(self: Box<Self>) -> BackendFuture<Result<std::process::ExitStatus, SandboxError>>;

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

/// Applies the caller's stdio policy to a command and spawns it.
///
/// # Why the launcher no longer builds the command, and what that fixed
///
/// The injected value used to be a closure that started `request.program` itself, which meant **two authorities
/// constructed one process**: the caller's launcher chose the executable and environment, while the backend
/// chose the confinement. That looked fine and was wrong in three ways at once, all of which the `P3-020`
/// acceptance test found rather than the unit tests:
///
/// 1. A container request's `program` names something **inside the image**, so the launcher spawned a path that
///    is not on this host and failed with a bare `entity not found`.
/// 2. The container argv had to be rebuilt inside the launcher, because only the launcher knew it was starting
///    the runtime. Building it twice ran the **name generator twice**, so the container was launched under one
///    name while `kill` removed another — leaving a container behind and, worse, reporting success. The leftover
///    was in state `Created`, which is why nothing looked wrong.
/// 3. The container's `--name` is a handle the kill path needs, and it is part of the argv — so it can only be
///    owned by whoever builds the argv **once**.
///
/// Now the backend builds the command and the caller decides only whether to capture the standard streams, which
/// is the one thing a request cannot state because it is a property of how the caller uses the child. The
/// observable ordering property is unchanged: the backend builds the command **after** the confinement exists and
/// asks the launcher to spawn it, so a launcher that refuses still proves no process was created.
pub type ProcessLauncher =
    Box<dyn FnOnce(tokio::process::Command) -> std::io::Result<tokio::process::Child> + Send>;

/// Returns a launcher that captures the standard streams when asked, and spawns.
///
/// `piped` is the whole of the caller's influence on the process, which is the point of the split: a caller
/// decides whether it intends to **drive** the child or only to supervise it, and the backend decides what the
/// child is and what confines it.
#[must_use]
pub fn stdio_launcher(piped: bool) -> ProcessLauncher {
    Box::new(move |mut command| {
        if piped {
            command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
        }
        command.spawn()
    })
}

/// Builds the command for a request the **host process** backends serve.
///
/// The environment is **cleared and replaced** rather than extended: `env_clear` plus the request's pairs is what
/// makes the request's map an allowlist in fact rather than in intention, and `docs/architecture/security.md`
/// asks for exactly that. There is no shell — program and arguments go in as an argv vector — so no metacharacter
/// is ever interpreted.
///
/// Shared by the unconfined and cgroup backends because they differ in **what confines the child**, not in how a
/// child is started. A container backend does not use this: its executable is the runtime rather than the
/// request's program, and its environment boundary is established inside the image.
#[must_use]
pub fn host_command(request: &crate::SandboxRequest) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(&request.program);
    command
        .args(&request.arguments)
        .env_clear()
        .envs(&request.environment);
    if let Some(directory) = &request.working_directory {
        command.current_dir(directory);
    }
    command
}

/// Refuses a request that carries an image, for a backend that runs host processes.
///
/// # Why the process backends need this and the container backend needs its complement
///
/// `image` selects the **backend**, so exactly one backend can serve any request, and each names the other in its
/// refusal. Without this the pairing would be silently ineffective rather than refused: a cgroup backend handed
/// an image-bearing request would confine the `docker` **CLI** with `pids.max`, while the container's own
/// processes live in the daemon's cgroup — so the limits would be applied to a client and the confined worker
/// would run unbounded, with `doctor` reporting four guarantees. That is the failure this crate exists to
/// prevent, and it is invisible from the outside.
///
/// The complement, in `container.rs`, refuses a request **without** an image for the same reason in reverse: a
/// host path is not an image reference and must not be substituted for one.
///
/// # Errors
///
/// Returns [`SandboxError::Launch`], because the facility is fine and the request belongs to another backend.
pub(crate) fn reject_container_request(
    request: &crate::SandboxRequest,
    facility: &'static str,
) -> Result<(), SandboxError> {
    match request.image.as_deref() {
        Some(image) => Err(SandboxError::Launch {
            reason: format!(
                "the {facility} backend runs a host process, so it cannot serve a request carrying the image \
                 `{image}`; that request belongs to the container backend"
            ),
        }),
        None => Ok(()),
    }
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
    Box::new(move |_command| {
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
/// the same guarantees as one that has them, and a Windows host with a reachable container runtime can offer
/// more than a Windows host without one. Reporting the compiled target would claim a guarantee the host does not
/// provide, which is exactly the parity claim `docs/architecture/security.md` forbids.
///
/// # Which of two available backends wins, and why it is not the one with the longer list
///
/// On Linux a delegated cgroup and a reachable container runtime can both exist, and the cgroup wins. The reason
/// is not the guarantee counts, which are close (four against three), but **what each needs to be trusted**: a
/// cgroup is the kernel's own interface, reached through the filesystem, with no third party in the path and no
/// image to choose; a container adds a runtime daemon and an image between the caller and the child. Where the
/// host needs neither, adding both buys nothing. So the container is the answer where the host process backend
/// has **nothing** — which is every Windows and macOS host, and a Linux host whose cgroup was not delegated.
///
/// Note that the container is deliberately **not** chosen by a feature flag or a configuration key. An operator
/// changes the answer by making a runtime reachable or by not, which is the same fact `doctor` reports — so a
/// report and a launch can never disagree about which facility is in use.
#[must_use]
pub fn backend_for_host() -> Box<dyn SandboxBackend> {
    // The container is probed **first** on every platform, because it is the one facility that reaches a host
    // whose own primitives are unavailable. Probed rather than assumed: `probe()` asks the runtime for its server
    // version, so "Docker Desktop is installed" and "a container can be launched" are not conflated.
    #[cfg(not(target_os = "linux"))]
    {
        // Windows could use a job object and macOS a seatbelt profile. Both need `unsafe` FFI that this
        // workspace's `unsafe_code = "forbid"` rejects, and macOS's facility expresses filesystem and network
        // policy rather than the resource ceilings this crate models — so a container is the *only* restriction
        // available on these hosts. Where no runtime is reachable, an unconfined backend is the honest answer,
        // and it is what makes `doctor` say so rather than imply parity.
        prefer_container(self::container::ContainerBackend::probe())
    }
    #[cfg(target_os = "linux")]
    {
        // `self::linux`, not `crate::linux`: the module is a child of *this* file, declared at the bottom, so
        // the path has to match where it actually lives.
        let host = self::linux::CgroupV2Backend::probe();
        if !host.support().is_empty() {
            return Box::new(host);
        }
        // No delegation, so the host process backend can enforce nothing. A container can still confine a child —
        // with fewer guarantees, which `support()` reports — and a caller who requires what it cannot provide is
        // refused by `SandboxPolicy::new` as always.
        prefer_container(self::container::ContainerBackend::probe())
    }
}

/// Returns the container backend when a runtime is reachable, else the one that enforces nothing.
///
/// The alternative is not a weakened container backend: it is [`UnconfinedBackend`], whose support set is
/// **empty**, so a request requiring any guarantee is refused rather than served without the confinement the
/// caller asked for. Returning a container backend that could not launch would be worse than either — it would
/// report a facility and then fail at the point a child was meant to exist.
#[must_use]
fn prefer_container(container: self::container::ContainerBackend) -> Box<dyn SandboxBackend> {
    if container.available() {
        Box::new(container)
    } else {
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
        // A request carrying an image belongs to the container backend, and this one names it rather than
        // confining the wrong process — see `reject_container_request` for why that distinction is a security
        // property rather than tidiness.
        if let Err(error) = reject_container_request(policy.request(), "unconfined") {
            return Box::pin(async move { Err(error) });
        }
        // Taken before the future is created, because `enforced()` borrows the policy and a `BackendFuture` is
        // `'static`. Cloning here is also the honest encoding of what happens: a launch reads its policy once.
        let enforced = policy.enforced();
        // The command is built here rather than by the caller, so the launcher decides only whether to capture the
        // streams. That is what makes the executable and the confinement a single decision — see `ProcessLauncher`
        // for the three failures that came from splitting them.
        let command = host_command(policy.request());
        Box::pin(async move {
            let mut child = launcher(command).map_err(|error| SandboxError::Launch {
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

    fn wait(self: Box<Self>) -> BackendFuture<Result<std::process::ExitStatus, SandboxError>> {
        Box::pin(async move {
            let mut child = self.child;
            child.wait().await.map_err(|error| SandboxError::Launch {
                reason: format!("could not collect the child's status: {}", error.kind()),
            })
        })
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

/// The guarantees the cgroup v2 facility can express, when a delegation exists.
///
/// # Why this is public, and why it is not the same question as `support()`
///
/// Two different questions, and conflating them would hide one: **which guarantees can this facility express?**
/// is a fixed fact about cgroup v2, while **does this host have a delegation?** is what `probe()` answers and
/// `support()` reports. Only the second depends on the machine.
///
/// It is published because the first question is the one worth checking, and it can only be checked by asking a
/// facility that may not exist here. A caller — or `doctor`, or a test — that wants to know what a cgroup host
/// *would* enforce has no other way to ask on a host without cgroups, and a claim that can only be verified where
/// it happens to be true is the claim that goes wrong. That matters most for the entry that is deliberately
/// **absent**, since an omission is what a well-meaning edit reintroduces.
#[must_use]
pub const fn cgroup_v2_guarantees() -> [Guarantee; 4] {
    [
        Guarantee::TreeTermination,
        Guarantee::ProcessCountCeiling,
        Guarantee::MemoryCeiling,
        Guarantee::CpuRateCeiling,
    ]
}

/// The guarantees a container runtime can express, when one is reachable.
///
/// Public for the reason [`cgroup_v2_guarantees`] gives: this is the list a host with a runtime enforces, and a
/// host without one is exactly where it cannot be verified unless it is readable. The omission is again the
/// load-bearing entry.
///
/// # Why the two CPU guarantees are absent, and separately
///
/// `CpuTimeCeiling` is absent because the only flag that expresses it, `--ulimit cpu`, sets `RLIMIT_CPU` — a
/// **per-process** limit, so a tree of N processes gets N budgets and the ceiling is not one. Docker's own
/// documentation makes the same point about the neighbouring `nproc` option: it "sets the maximum number of
/// processes available to a user, not to a container", to the point that four containers under one uid share
/// one budget. That is the identical argument `P3-011` used to reject `RLIMIT_NPROC` as a fallback, so accepting
/// the container's equivalent would contradict a decision this crate already made.
///
/// `CpuRateCeiling` is absent for a **weaker** reason and is recorded as such rather than dressed up: `--cpus`
/// expresses the guarantee exactly (it "is the equivalent of setting `--cpu-period='100000'` and
/// `--cpu-quota='150000'`"), and converting [`crate::Limits::max_cpu_rate`] to a `--cpus` value is arithmetic a
/// later slice could add. It is not claimed because the flags are not emitted — a guarantee claimed before its
/// flag exists is the declared-but-unconstructed shape this repository removes — and
/// `every_claimed_container_guarantee_has_a_flag_and_every_flag_a_guarantee` asserts that direction so the list
/// cannot drift ahead of the argv. The entry appears here when the translation does.
#[must_use]
pub const fn container_guarantees() -> [Guarantee; 3] {
    [
        Guarantee::TreeTermination,
        Guarantee::ProcessCountCeiling,
        Guarantee::MemoryCeiling,
    ]
}

/// The smallest memory ceiling a container runtime accepts, in bytes.
///
/// Six mebibytes, and the value is Docker's rather than a round number this workspace picked: the
/// documentation states "the minimum allowed value is `6m` (6 megabytes)". It is a `pub(crate)` constant here,
/// in the always-compiled module, so a **non-container** host can still assert the refusal path and so the
/// number exists once.
pub(crate) const MINIMUM_CONTAINER_MEMORY_BYTES: u64 = 6 * 1024 * 1024;

/// The image a container request must carry, or a refusal naming the field.
///
/// Returns the image rather than `()` so a caller cannot read `is_ok()` and then reach for
/// [`crate::SandboxRequest::image`] again — the value that passed the check is the value the launch uses.
///
/// # Errors
///
/// Returns [`SandboxError::Launch`] when the request carries no image. It is a **launch** refusal rather than a
/// setup one because the facility is fine and the request is not, and it is raised by the backend rather than
/// by [`crate::SandboxPolicy::new`] because the requirement belongs to the container facility: the cgroup
/// backend has no use for an image and must not require one.
#[must_use = "a request's image is required, not advisory; ignoring the result skips the refusal"]
pub(crate) fn container_image(request: &crate::SandboxRequest) -> Result<&str, SandboxError> {
    match request.image.as_deref() {
        Some(image) if !image.trim().is_empty() => Ok(image),
        _ => Err(SandboxError::Launch {
            reason: "a container sandbox requires the request's image".to_owned(),
        }),
    }
}

// A child of *this* file, and declared with an explicit path for the same reason `linux` is: Rust's module
// resolution would otherwise look for `backend/container.rs`, and the alternative — a `backend/` directory
// holding two files whose siblings live at the crate root — is a layout difference with no meaning.
#[path = "container.rs"]
mod container;

pub use container::ContainerBackend;

// The argv builder and the handle helpers, exposed only to this crate's tests: the interesting container
// assertions are about the **argv** rather than about a launch, because spawning a runtime in a unit test would
// make the suite depend on a daemon — and an argv can be asserted exactly, where a live launch can only be
// observed. `container_name` is included because the claim that the name is a *handle* is a claim about two
// values agreeing, which needs both of them readable.
#[cfg(test)]
pub(crate) use container::{container_arguments, container_name};

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

    /// **The container list omits both CPU guarantees, and each omission is a different claim.**
    ///
    /// `CpuTimeCeiling` is absent because the facility **cannot** provide it: the only flag is `--ulimit cpu`,
    /// which is `RLIMIT_CPU` and therefore per **process**, so a tree of N processes gets N budgets rather than
    /// one ceiling. That is the same argument `P3-011` used to reject `RLIMIT_NPROC` — and Docker's own note on
    /// the sibling `nproc` option confirms the class: four containers under one uid share one budget.
    ///
    /// `CpuRateCeiling` is absent for the opposite reason: `--cpus` **can** express it, and the rate-to-`--cpus`
    /// translation is merely unimplemented. Asserting both omissions in one test is deliberate, because the
    /// risk is that a later edit "fixes" the second by adding the first — the two are adjacent in the list and
    /// only one of them is a fact about the facility.
    ///
    /// This runs on every platform, which is the point: it is the container backend's central claim and the
    /// development host has a runtime while a CI host may not.
    ///
    /// Falsified by mutation: adding either guarantee to `container_guarantees` fails here.
    #[test]
    fn the_container_list_omits_both_cpu_guarantees_for_two_different_reasons() {
        let listed = container_guarantees();
        assert!(
            !listed.contains(&Guarantee::CpuTimeCeiling),
            "`--ulimit cpu` is per process, so a tree of N processes would get N budgets: {listed:?}"
        );
        assert!(
            !listed.contains(&Guarantee::CpuRateCeiling),
            "`--cpus` can express this, but the translation is unimplemented, so it must not be claimed yet: {listed:?}"
        );
        assert_eq!(
            listed.len(),
            3,
            "exactly the three guarantees whose flags are container-scoped"
        );
    }

    /// **Every claimed container guarantee is expressed by a flag in the built argv, and no other.**
    ///
    /// The claim and the argv are two different values that must agree, and a list that outran its flags would
    /// be exactly the declared-but-unconstructed shape `AGENTS.md` forbids — a caller requiring
    /// `ProcessCountCeiling` and receiving no `--pids-limit`. Asserting the mapping in **both** directions is
    /// what makes it an equivalence rather than a promise: a flag passed for a guarantee that is *not* claimed
    /// would mean the backend enforces something no caller can require and no report will mention.
    ///
    /// Falsified by mutation: dropping the `--pids-limit` push while leaving `ProcessCountCeiling` claimed fails
    /// the first half; adding a `--cpus` push while leaving `CpuRateCeiling` unclaimed fails the second.
    #[test]
    fn every_claimed_container_guarantee_has_a_flag_and_every_flag_a_guarantee() {
        let request = crate::SandboxRequest {
            program: std::path::PathBuf::from("/bin/echo"),
            image: Some("alpine:3".to_owned()),
            arguments: vec!["hello".to_owned()],
            working_directory: None,
            environment: std::collections::BTreeMap::new(),
            isolation: crate::Isolation::Restricted,
            limits: crate::Limits {
                max_processes: Some(32),
                max_memory_bytes: Some(64 * 1024 * 1024),
                // Both CPU limits set, so an implementation that *did* translate them would reveal itself: the
                // flags are absent below precisely because these values are present.
                max_cpu_rate: Some(std::time::Duration::from_millis(500)),
                max_cpu_time: Some(std::time::Duration::from_secs(5)),
            },
            required: container_guarantees().to_vec(),
        };
        let argv = container_arguments(&request).unwrap_or_else(|error| {
            panic!("a request with an acceptable memory ceiling must build: {error}")
        });
        let flag = |name: &str| argv.iter().any(|argument| argument == name);
        assert!(
            flag("--pids-limit"),
            "ProcessCountCeiling is claimed, so its flag must be passed: {argv:?}"
        );
        assert!(
            flag("--memory"),
            "MemoryCeiling is claimed, so its flag must be passed: {argv:?}"
        );
        assert!(
            flag("--rm"),
            "TreeTermination is claimed through the remove-on-kill path, and `--rm` is what keeps a completed \
             launch from leaving a container behind: {argv:?}"
        );
        // The unclaimed guarantees must have **no** flags, because a flag for a guarantee no caller can require
        // is enforcement nobody can see, select, or report.
        for unclaimed in ["--cpus", "--cpu-quota", "--cpu-period", "--ulimit"] {
            assert!(
                !flag(unclaimed),
                "{unclaimed} expresses a guarantee this backend does not claim, so passing it would enforce \
                 something unmodelled: {argv:?}"
            );
        }
    }

    /// **The container argv always confines, regardless of what the request asked for.**
    ///
    /// `--read-only`, `--cap-drop ALL`, `--network none`, `-i`, and `--pull never` are **not** `Guarantee`s —
    /// they are part of what "a container isolation" means, and a container without them is not a restricted
    /// worker. So they are asserted against a request that required **nothing**, which is the case where an
    /// implementation that treated them as opt-in would omit them.
    ///
    /// `--network none` and `--pull never` are the two with a security consequence rather than a convenience
    /// one: a container that can reach the network is not isolated, and a runtime that pulls on demand fetches
    /// content no operator chose.
    ///
    /// Falsified by mutation: removing any of these flags fails here.
    #[test]
    fn the_container_argv_is_confined_even_when_the_request_requires_nothing() {
        let request = crate::SandboxRequest {
            program: std::path::PathBuf::from("/bin/echo"),
            image: Some("alpine:3".to_owned()),
            arguments: Vec::new(),
            working_directory: None,
            environment: std::collections::BTreeMap::new(),
            isolation: crate::Isolation::None,
            limits: crate::Limits::default(),
            required: Vec::new(),
        };
        let argv = container_arguments(&request)
            .unwrap_or_else(|error| panic!("a request requiring nothing must build: {error}"));
        assert_eq!(argv.first().map(String::as_str), Some("run"));
        for required in [
            "--read-only",
            "--cap-drop",
            "--network",
            "--pull",
            "-i",
            "--rm",
            "--name",
        ] {
            assert!(
                argv.iter().any(|argument| argument == required),
                "{required} is part of what a container isolation means, not a policy a caller selects: {argv:?}"
            );
        }
        // The values matter as much as the flags: `--network bridge` and `--pull always` would each satisfy a
        // check for the flag alone while doing the opposite of what was intended, which is the failure a
        // flag-presence assertion cannot see.
        assert!(
            argv.windows(2).any(|pair| pair == ["--network", "none"]),
            "the network must be `none` rather than merely unmentioned: {argv:?}"
        );
        assert!(
            argv.windows(2).any(|pair| pair == ["--pull", "never"]),
            "an image must never be pulled, because pulling fetches content no operator chose: {argv:?}"
        );
    }

    /// **A memory ceiling below the runtime's floor is refused rather than adjusted, and zero is refused with
    /// its own reason.**
    ///
    /// Two values, two distinct mistakes. `--memory=1m` is rejected **by the daemon** (probed: exit 125,
    /// "Minimum memory limit allowed is 6MB"), so passing it on would turn a policy error into an opaque launch
    /// failure. `--memory=0` is *accepted* and read as **unlimited** (probed: exit 0) — the exact inverse of what
    /// a caller who wrote zero means, and this crate says everywhere that an absent limit is absent rather than
    /// zero so the two are never conflated.
    ///
    /// The **6 MiB floor passes**, which is the control: an implementation that refused every memory ceiling
    /// would satisfy both refusals above while making the guarantee unusable, and the boundary is inclusive
    /// because the runtime accepts exactly that value (probed: exit 0).
    ///
    /// **Falsified in three separate mutations, and the third is the interesting one.** Replacing `<` with `<=`
    /// fails the inclusive-floor case. Deleting the `== 0` disjunct from the condition **survives** — correctly,
    /// because zero is arithmetically below the floor, so that disjunct was dead code and was removed. What the
    /// zero case actually needs is its *explanation*, so the assertion below is about the message: removing the
    /// zero-specific note fails with `got: a container memory ceiling must be at least 6291456 bytes, got 0`.
    #[test]
    fn a_container_memory_ceiling_below_the_runtime_floor_is_refused_by_value() {
        let with_memory = |bytes: u64| crate::SandboxRequest {
            program: std::path::PathBuf::from("/bin/echo"),
            image: Some("alpine:3".to_owned()),
            arguments: Vec::new(),
            working_directory: None,
            environment: std::collections::BTreeMap::new(),
            isolation: crate::Isolation::Restricted,
            limits: crate::Limits {
                max_memory_bytes: Some(bytes),
                ..crate::Limits::default()
            },
            required: Vec::new(),
        };
        for refused in [0_u64, 1, MINIMUM_CONTAINER_MEMORY_BYTES - 1] {
            let error = match container_arguments(&with_memory(refused)) {
                Ok(argv) => {
                    panic!("{refused} bytes cannot be expressed as a ceiling, but built: {argv:?}")
                }
                Err(error) => error,
            };
            let message = error.to_string();
            assert!(
                message.contains(&refused.to_string()),
                "the refusal must name the value so an operator can fix it, got: {message}"
            );
            assert!(
                message.contains(&MINIMUM_CONTAINER_MEMORY_BYTES.to_string()),
                "the refusal must name the floor it is enforcing, got: {message}"
            );
        }
        // The **zero** case is asserted separately from the two small-value cases, and this is not redundancy.
        // A mutation that deleted the explicit zero test from the floor check left every assertion above passing,
        // because zero is arithmetically below the floor — so a disjunct there is dead code. What is *not*
        // redundant is telling the two apart: `0` and `1` are refused by one condition for two different reasons,
        // and an operator who wrote `0` needs to hear that the runtime reads it as *unlimited* rather than as a
        // very small ceiling. This assertion is about that explanation, which is the part a deletion can break.
        let zero = match container_arguments(&with_memory(0)) {
            Ok(argv) => panic!("zero must be refused, but built: {argv:?}"),
            Err(error) => error.to_string(),
        };
        assert!(
            zero.contains("unlimited"),
            "zero must be refused with its *own* reason, because the runtime reads it as unlimited rather than \
             as a small ceiling, got: {zero}"
        );
        let one = match container_arguments(&with_memory(1)) {
            Ok(argv) => panic!("one byte must be refused, but built: {argv:?}"),
            Err(error) => error.to_string(),
        };
        assert!(
            !one.contains("unlimited"),
            "a non-zero sub-floor value must not be described as being read as unlimited, or the two refusals \
             would be indistinguishable, got: {one}"
        );
        let argv = container_arguments(&with_memory(MINIMUM_CONTAINER_MEMORY_BYTES))
            .unwrap_or_else(|error| panic!("the floor itself is accepted by the runtime: {error}"));
        assert!(
            argv.windows(2).any(|pair| pair
                == [
                    "--memory",
                    MINIMUM_CONTAINER_MEMORY_BYTES.to_string().as_str()
                ]),
            "the accepted ceiling must reach the argv: {argv:?}"
        );
    }

    /// **The request's environment replaces the image's rather than adding to it.**
    ///
    /// Probed, not assumed: `docker run -e FOO=bar alpine:3 env` still shows the image's `PATH` and `HOME`, so
    /// `-e` **adds**. That would make the same request mean two different variable sets depending on which
    /// backend served it, while `SandboxRequest::environment` is documented as the **complete** environment and
    /// the process backend implements that as `env_clear().envs(…)`.
    ///
    /// The argv therefore runs `env -i <pairs> <program> <args>`, and the assertions below are about the
    /// **boundary** rather than the intent: `env -i` is present, every request pair precedes the program, and
    /// the program is the first element after the pairs — a `-i` in the wrong position, or a pair emitted after
    /// the program, would leave the image's environment partly intact while still containing the right strings.
    ///
    /// The empty-environment case is asserted separately because it is deliberately **different**: a request that
    /// states nothing runs the program directly, so an image's own `PATH` survives. That is the reading of
    /// "I stated nothing" as the image's default rather than as "remove everything", and it is a real asymmetry
    /// that this test pins so it cannot drift silently.
    ///
    /// Falsified by mutation: dropping `-i`, or pushing the pairs after the program, fails here.
    #[test]
    fn the_request_environment_replaces_the_images_environment() {
        let with_environment =
            |environment: std::collections::BTreeMap<String, String>| crate::SandboxRequest {
                program: std::path::PathBuf::from("/bin/echo"),
                image: Some("alpine:3".to_owned()),
                arguments: vec!["a b".to_owned(), "c$d".to_owned()],
                working_directory: None,
                environment,
                isolation: crate::Isolation::Restricted,
                limits: crate::Limits::default(),
                required: Vec::new(),
            };
        let stated = std::collections::BTreeMap::from([
            ("FOO".to_owned(), "bar".to_owned()),
            ("BAZ".to_owned(), "q u x".to_owned()),
        ]);
        let argv = container_arguments(&with_environment(stated))
            .unwrap_or_else(|error| panic!("a request with no limits must build: {error}"));
        let image_at = argv
            .iter()
            .position(|argument| argument == "alpine:3")
            .unwrap_or_else(|| panic!("the image must be the command's first argument: {argv:?}"));
        let command = &argv[image_at + 1..];
        assert_eq!(
            command.first().map(String::as_str),
            Some("env"),
            "the environment boundary must be established inside the image, not by the runtime's `-e`: {argv:?}"
        );
        assert_eq!(
            command.get(1).map(String::as_str),
            Some("-i"),
            "`-i` is what clears the environment; without it the image's own variables survive: {argv:?}"
        );
        let program_at = command
            .iter()
            .position(|argument| argument == "/bin/echo")
            .unwrap_or_else(|| panic!("the program must be present: {argv:?}"));
        for pair in ["FOO=bar", "BAZ=q u x"] {
            let pair_at = command
                .iter()
                .position(|argument| argument == pair)
                .unwrap_or_else(|| panic!("{pair} must be passed to `env`: {argv:?}"));
            assert!(
                pair_at < program_at,
                "{pair} must precede the program, or `env` would treat it as an argument to it: {argv:?}"
            );
        }
        // The arguments are passed through `env` to the program, so their element boundaries must survive: an
        // implementation that joined the command into a shell string would split `a b` into two arguments.
        assert_eq!(
            &command[program_at + 1..],
            ["a b", "c$d"],
            "argv element boundaries must reach the program unmodified: {argv:?}"
        );

        let bare = container_arguments(&with_environment(std::collections::BTreeMap::new()))
            .unwrap_or_else(|error| panic!("a request with no environment must build: {error}"));
        let bare_image_at = bare
            .iter()
            .position(|argument| argument == "alpine:3")
            .unwrap_or_else(|| panic!("the image must be present: {bare:?}"));
        assert_eq!(
            bare[bare_image_at + 1].as_str(),
            "/bin/echo",
            "a request that states no environment must run the program directly, so the image's default \
             environment is not removed by a caller who said nothing about it: {bare:?}"
        );
    }

    /// **Two launches never share a container name, and the name is recoverable from the argv.**
    ///
    /// The name is a **handle** rather than a label: the kill path runs `docker rm -f <name>`, so a repeated name
    /// would let one launch's kill stop another's container — a confined process outliving its confinement, which
    /// is worse than no sandbox because the caller believes it stopped.
    ///
    /// Asserting uniqueness alone would not be enough, because a name that never appears in the argv would satisfy
    /// it while making the kill path remove nothing. So the name is read back **out of the argv the launch would
    /// run** and compared with the handle the value holds, which is the property that matters and the one a
    /// separate `--name` argument would break by drifting.
    ///
    /// Falsified by mutation: making `leaf` ignore the sequence counter fails uniqueness; emitting the name in a
    /// field other than the one the handle reads fails the match.
    #[test]
    fn a_container_name_is_a_unique_handle_matching_the_argv() {
        let request = crate::SandboxRequest {
            program: std::path::PathBuf::from("/bin/echo"),
            image: Some("alpine:3".to_owned()),
            arguments: Vec::new(),
            working_directory: None,
            environment: std::collections::BTreeMap::new(),
            isolation: crate::Isolation::Restricted,
            limits: crate::Limits::default(),
            required: Vec::new(),
        };
        let first = container_arguments(&request)
            .unwrap_or_else(|error| panic!("a request with no limits must build: {error}"));
        let second = container_arguments(&request)
            .unwrap_or_else(|error| panic!("a request with no limits must build: {error}"));
        let name_at = |argv: &[String]| {
            argv.iter()
                .position(|argument| argument == "--name")
                .and_then(|index| argv.get(index + 1))
                .cloned()
                .unwrap_or_else(|| panic!("every argv must carry `--name`: {argv:?}"))
        };
        let (first_name, second_name) = (name_at(&first), name_at(&second));
        assert_ne!(
            first_name, second_name,
            "two launches must never share a name, or one kill would stop the other's container"
        );
        // The runtime rejects a container name outside `[a-zA-Z0-9][a-zA-Z0-9_.-]*`, and an image reference
        // contains a `:` and usually a `/`. So the charset is asserted through an image that has both: a name
        // built by interpolating the reference unsanitised would make a *naming* detail a launch failure, which
        // is the failure this assertion exists to catch.
        let qualified = crate::SandboxRequest {
            image: Some("docker.io/library/alpine:3".to_owned()),
            ..crate::SandboxRequest {
                program: std::path::PathBuf::from("/bin/echo"),
                image: None,
                arguments: Vec::new(),
                working_directory: None,
                environment: std::collections::BTreeMap::new(),
                isolation: crate::Isolation::Restricted,
                limits: crate::Limits::default(),
                required: Vec::new(),
            }
        };
        let qualified_argv = container_arguments(&qualified)
            .unwrap_or_else(|error| panic!("a request with no limits must build: {error}"));
        let qualified_name = name_at(&qualified_argv);
        for name in [&first_name, &second_name, &qualified_name] {
            assert!(
                !name.is_empty()
                    && name
                        .chars()
                        .all(|character| character.is_ascii_alphanumeric()
                            || character == '_'
                            || character == '.'
                            || character == '-'),
                "a container name the runtime would reject turns a naming detail into a launch failure, got: \
                 {name}"
            );
        }
        assert!(
            qualified_name.contains("alpine"),
            "the name must still identify what is running, rather than sanitising into an opaque number: \
             {qualified_name}"
        );
        // The name must be the value the kill path will hold, which is the whole reason this is asserted against
        // the argv rather than by inspecting the string.
        assert_eq!(
            container_name(&first, "alpine:3"),
            first_name,
            "the handle the kill path uses must be the name the argv was launched with"
        );
    }
}
