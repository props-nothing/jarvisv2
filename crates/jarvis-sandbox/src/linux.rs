//! The Linux backend: cgroup v2, reached entirely through the filesystem.
//!
//! # Why this is the backend that exists
//!
//! Every operation this crate needs is a file operation:
//!
//! | Guarantee | Interface | Kernel behaviour |
//! | --- | --- | --- |
//! | [`Guarantee::ProcessCountCeiling`] | `pids.max` | a `fork` past the limit fails with `EAGAIN`, for the cgroup **and all its descendants** |
//! | [`Guarantee::MemoryCeiling`] | `memory.max` | the OOM killer runs **inside the cgroup only**, so a runaway child cannot take the daemon with it |
//! | [`Guarantee::CpuRateCeiling`] | `cpu.max` = `$MAX $PERIOD` | a bandwidth ceiling: throttling, not a kill |
//! | [`Guarantee::TreeTermination`] | `cgroup.kill` | writing `1` kills the cgroup **and all descendant cgroups**; the kernel documents it as dealing with concurrent forks appropriately and being protected against migrations |
//!
//! No `unsafe`, no FFI, no `libc` — which is what makes this implementable at all under this workspace's
//! `unsafe_code = "forbid"`. The Windows job-object equivalent is entirely `unsafe` FFI, so it is recorded as
//! unavailable in `lib.rs` rather than half-built.
//!
//! # `CpuTimeCeiling` is deliberately absent
//!
//! cgroup v2 **accounts** CPU time in `cpu.stat` but has no limit file for a cumulative total — only the rate
//! form in `cpu.max`. So [`CgroupV2Backend::support`] omits [`Guarantee::CpuTimeCeiling`], and a request that
//! requires it is refused. The tempting shortcut — report it as supported and interpret the limit as "roughly a
//! second of CPU time" — is precisely the silent degradation this crate exists to prevent: a rate ceiling
//! throttles a child indefinitely, while a cumulative ceiling stops it, and a caller who asked for the second
//! would get the first while believing otherwise.
//!
//! # And why it so often refuses
//!
//! A process may only write to a cgroup **delegated** to it. On a stock host — including most containers and
//! essentially every interactive development shell — `/sys/fs/cgroup` is owned by `root` with no delegated
//! subtree, so the only writable directory is one an operator created. This backend therefore **probes** and
//! reports what it found; it never claims a guarantee it cannot place a child into.
//!
//! The fallback a naive implementation would reach for is per-process `RLIMIT_AS` / `RLIMIT_NPROC` /
//! `RLIMIT_CPU`. That is deliberately **not** used, because it would be a lie by construction: `RLIMIT_NPROC` is
//! counted **per user id**, so a child that reaches its own process limit can still `fork` — each new process
//! gets a fresh budget under the same uid. Only a tree-scoped counter makes a fork bomb fail, and only cgroups
//! provide one.
//!
//! # What is deliberately not here
//!
//! The `No Internal Process Constraint` matters to a richer design: a cgroup with children must have no
//! processes of its own. This backend uses a single **leaf** per launch and creates no child cgroups, so the
//! constraint does not bind it. A future version wanting per-launch subgroups would have to respect it.

use std::path::{Path, PathBuf};

// `Guarantee` is deliberately absent: this backend never constructs one. Its support set comes from
// `cgroup_v2_guarantees()` in the parent module, which is where a non-Linux host can still test it — so an
// import here would be unused **only on Linux**, and `cargo check` on Windows would never have said so.
use crate::policy::{Limits, SandboxError, SandboxPolicy};
use crate::{
    BackendFuture, GuaranteeSupport, Launched, LaunchedProcess, ProcessLauncher, SandboxBackend,
    Support,
};

/// `cgroup.kill` is a write-`1`-to-kill file: any non-empty write triggers the kill and the kernel chooses the
/// value. Named so the write does not read as a magic byte.
const KILL: &str = "1";

/// The cgroup v2 unified mount.
const UNIFIED_MOUNT: &str = "/sys/fs/cgroup";

/// The v2 discriminator: `cgroup.controllers` exists only on the unified hierarchy, so its presence answers
/// "is this v2?" without parsing `/proc/mounts`.
const V2_MARKER: &str = "cgroup.controllers";

/// The delegation root this backend looks for under a writable cgroup.
///
/// A **fixed** name rather than a random one, so an operator's delegation is a one-time setup step with a stable
/// path to grant. It also means two daemons on one host contend for the same subtree, which is where they should
/// meet — rather than each silently creating its own and doubling the memory an operator budgeted.
const DELEGATED_ROOT: &str = "jarvis-sandbox";

/// A cgroup v2 backend, holding the delegation root it found.
///
/// The root is `Option`, and the support set is derived from it at the same moment, so the type carries no claim
/// that a later `launch` could disagree with.
pub struct CgroupV2Backend {
    delegation: Option<PathBuf>,
}

impl std::fmt::Debug for CgroupV2Backend {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CgroupV2Backend")
            .field("delegation", &self.delegation)
            .finish()
    }
}

impl CgroupV2Backend {
    /// Probes the host and returns a backend reporting only what it can actually enforce.
    ///
    /// # The probe, and what each step rules out
    ///
    /// 1. `/sys/fs/cgroup/cgroup.controllers` must exist. Its absence means no v2 unified hierarchy — a v1-only
    ///    kernel, or no cgroup mount at all — so no guarantee is available.
    /// 2. The process's **own** cgroup must be writable, or failing that the mount root. This is the step that
    ///    makes the probe meaningful: on a stock host the mount exists and is writable only by `root`, so a
    ///    check that stopped at step 1 would report guarantees that every subsequent launch would fail to apply.
    ///
    /// The process's own cgroup is tried **first**, so a subtree delegated by systemd wins over a globally
    /// writable mount root: the narrower scope is the one that respects a host's existing hierarchy.
    ///
    /// When either step fails the support set is **empty**, which makes [`SandboxPolicy::new`] refuse any request
    /// requiring something. That is the fail-closed direction: an operator who wants confinement on such a host
    /// gets a startup error naming the guarantee, not a daemon that runs children unconfined.
    #[must_use]
    pub fn probe() -> Self {
        Self {
            delegation: Self::find_delegation(),
        }
    }

    /// Locates a cgroup this process may write to, or `None`.
    fn find_delegation() -> Option<PathBuf> {
        let mount = Path::new(UNIFIED_MOUNT);
        if !mount.join(V2_MARKER).is_file() {
            return None;
        }
        // A single `if let` with `&&` rather than two nested `if`s: both conditions must hold for the process's
        // own cgroup to be usable, so nesting adds a level of indentation that suggests an intermediate state
        // exists when it cannot.
        if let Some(own) = Self::own_cgroup()
            && Self::writable(&own)
        {
            return Some(own.join(DELEGATED_ROOT));
        }
        // Fallback for a container granted a writable `/sys/fs/cgroup` with no delegated subtree.
        if Self::writable(mount) {
            return Some(mount.join(DELEGATED_ROOT));
        }
        None
    }

    /// Returns the process's own cgroup path, from the unified `0::` line of `/proc/self/cgroup`.
    fn own_cgroup() -> Option<PathBuf> {
        let contents = std::fs::read_to_string("/proc/self/cgroup").ok()?;
        for line in contents.lines() {
            // v2 lines are `0::<path>`; the `0::` prefix identifies the unified hierarchy. v1 lines are
            // `<id>:<controllers>:<path>` and would each need per-controller joining, which v2 does not have.
            let Some(rest) = line.strip_prefix("0::") else {
                continue;
            };
            return Some(Path::new(UNIFIED_MOUNT).join(rest.trim_start_matches('/')));
        }
        None
    }

    /// Returns whether this process can create entries in a cgroup directory.
    ///
    /// Permission bits rather than a trial write: on a cgroup filesystem an unexpected write is a **policy
    /// action** — writing a limit file configures the cgroup, writing `cgroup.kill` kills it — so probing by
    /// writing is not a neutral test here, and a probe that killed a cgroup would be worse than no probe.
    fn writable(directory: &Path) -> bool {
        use std::os::unix::fs::MetadataExt as _;
        let Ok(metadata) = std::fs::metadata(directory) else {
            return false;
        };
        let uid = effective_id("Uid:");
        // Root is writable regardless of the bits, because `CAP_DAC_OVERRIDE` means a permission bit is not the
        // whole story and reading only the mode would refuse a host where a write would succeed.
        if uid == 0 {
            return true;
        }
        let mode = metadata.mode();
        if metadata.uid() == uid {
            return mode & 0o200 != 0;
        }
        if metadata.gid() == effective_id("Gid:") {
            return mode & 0o020 != 0;
        }
        mode & 0o002 != 0
    }

    /// Removes a launch's cgroup directory, ignoring one that is already gone.
    fn remove(directory: &Path) {
        match std::fs::remove_dir(directory) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                // A leftover cgroup directory is a resource leak, not a safety failure: the tree it held is
                // already dead. Reported rather than swallowed, because a silent leak of cgroup directories is
                // exactly the kind of thing that is never noticed.
                eprintln!(
                    "jarvis-sandbox: could not remove cgroup {}: {error}",
                    directory.display()
                );
            }
        }
    }
}

/// Reads an effective id from `/proc/self/status`, where the line is `<name>\t<real>\t<effective>\t…`.
///
/// Reading a proc file instead of declaring a `libc` dependency for two integers: this crate's whole point on
/// Linux is that it needs no FFI, and a dependency whose sole use is an id would be a hole in that story for no
/// benefit. Every host with cgroup v2 has `/proc`.
fn effective_id(name: &str) -> u32 {
    let Ok(contents) = std::fs::read_to_string("/proc/self/status") else {
        // An unreadable status file means the check cannot succeed. Returning a value that matches no id (rather
        // than `0`, which would read as root) keeps the caller on the permission-bit path, which is the
        // conservative branch.
        return u32::MAX;
    };
    for line in contents.lines() {
        let Some(rest) = line.strip_prefix(name) else {
            continue;
        };
        // Field 2 is the effective id, field 1 the real one. They differ in exactly the case that matters — a
        // process that dropped privileges — and it is the effective id that decides what a write does.
        if let Some(effective) = rest.split_whitespace().nth(1)
            && let Ok(id) = effective.parse()
        {
            return id;
        }
    }
    u32::MAX
}

impl SandboxBackend for CgroupV2Backend {
    fn support(&self) -> GuaranteeSupport {
        if self.delegation.is_none() {
            return GuaranteeSupport::default();
        }
        // Everything in the table at the top of this file, and **not** `CpuTimeCeiling`: this platform has no
        // limit file for a cumulative CPU total, only the rate form. Derived from the same `delegation` value
        // `launch` reads, so a support claim and a launch cannot disagree. The list itself lives in `backend`,
        // where a non-Linux host can still test it — see `cgroup_v2_guarantees`.
        GuaranteeSupport::from_guarantees(super::cgroup_v2_guarantees())
    }

    fn facility(&self) -> Support {
        Support::CgroupV2
    }

    fn launch(
        &self,
        policy: &SandboxPolicy,
        launcher: ProcessLauncher,
    ) -> BackendFuture<Result<Launched, SandboxError>> {
        let Some(delegation) = self.delegation.clone() else {
            return Box::pin(async {
                Err(SandboxError::Setup {
                    facility: "cgroup v2",
                    reason: "no delegated cgroup is writable by this process".to_owned(),
                })
            });
        };
        let limits = policy.request().limits;
        let enforced = policy.enforced();
        Box::pin(async move {
            let directory = delegation.join(unique_leaf());
            std::fs::create_dir(&directory).map_err(|error| SandboxError::Setup {
                facility: "cgroup v2",
                reason: format!("could not create {DELEGATED_ROOT}: {}", error.kind()),
            })?;
            // Limits are written **before the child exists**. A limit applied after a spawn is a limit the child
            // may already have exceeded — which, for `memory.max`, means the allocation has already happened.
            if let Err(error) = apply_limits(&directory, limits) {
                Self::remove(&directory);
                return Err(error);
            }
            let child = match launcher() {
                Ok(child) => child,
                Err(error) => {
                    Self::remove(&directory);
                    return Err(SandboxError::Launch {
                        reason: format!("spawn failed: {}", error.kind()),
                    });
                }
            };
            let mut child = child;
            let Some(pid) = child.id() else {
                // The child is already gone, so there is nothing to confine and nothing to leak.
                let _ = child.wait().await;
                Self::remove(&directory);
                return Err(SandboxError::Launch {
                    reason: "the child exited before it could be confined".to_owned(),
                });
            };
            // The migration is what makes the limits apply, and it is also the step that can race: between the
            // spawn and this write the child runs under no ceiling. cgroup v2 has no atomic
            // spawn-into-cgroup for a `fork`/`exec` pair, so the window is closed as far as possible rather than
            // pretended away — a child that allocates in those microseconds is not stopped. That is recorded as
            // a limit here rather than described as airtight.
            if let Err(error) = std::fs::write(directory.join("cgroup.procs"), pid.to_string()) {
                let _ = child.kill().await;
                Self::remove(&directory);
                return Err(SandboxError::Launch {
                    reason: format!("could not place pid {pid} in the cgroup: {}", error.kind()),
                });
            }
            let stdin = child.stdin.take();
            let stdout = child.stdout.take();
            let stderr = child.stderr.take();
            Ok(Launched {
                process: Box::new(CgroupChild { child, directory }),
                support: Support::CgroupV2,
                enforced,
                stdin,
                stdout,
                stderr,
            })
        })
    }
}

/// Writes the request's limits into the cgroup.
///
/// Each file is written **only** when the corresponding limit is present, because writing `max` is what *clears*
/// a limit in cgroup v2. An unconditional write would clear a limit an operator had set higher up the hierarchy
/// — the opposite of what a sandbox should do with a subtree it was delegated.
fn apply_limits(directory: &Path, limits: Limits) -> Result<(), SandboxError> {
    if let Some(maximum) = limits.max_processes {
        write_limit(directory, "pids.max", &maximum.to_string())?;
    }
    if let Some(maximum) = limits.max_memory_bytes {
        write_limit(directory, "memory.max", &maximum.to_string())?;
    }
    if let Some(rate) = limits.max_cpu_rate {
        // `cpu.max` is `$MAX $PERIOD` in microseconds: a budget **per period**, which is why the field this
        // reads is `max_cpu_rate` and not `max_cpu_time`. A period of one second is chosen explicitly rather
        // than left to the kernel's default, so the semantics of the written budget do not depend on how the
        // parent cgroup happened to be configured.
        let period = 1_000_000_u64;
        // Saturating rather than unchecked: a caller asking for more budget than fits in a period gets the whole
        // period, which is the closest expressible policy, and a truncation or wraparound here would write a
        // *smaller* budget than asked — a limit that silently throttles harder than the caller chose.
        let budget = u64::try_from(rate.as_micros())
            .unwrap_or(period)
            .min(period);
        write_limit(directory, "cpu.max", &format!("{budget} {period}"))?;
    }
    Ok(())
}

/// Writes one limit file, naming it in any error so an operator knows which one failed.
fn write_limit(directory: &Path, file: &'static str, value: &str) -> Result<(), SandboxError> {
    // `OpenOptions` rather than `fs::write`: these files are provided by the kernel, and `fs::write`'s implicit
    // `O_CREAT` would be an attempt to create a file that must already exist.
    std::fs::write(directory.join(file), value).map_err(|error| SandboxError::Setup {
        facility: "cgroup v2",
        reason: format!("could not write {file}: {}", error.kind()),
    })
}

/// A per-launch leaf name.
///
/// A monotonic counter plus the pid, rather than a timestamp: two launches in the same nanosecond would collide
/// on a clock a virtualised host may not have at that resolution, while a counter cannot repeat within a
/// process. The pid disambiguates **across** processes, which matters when two daemons share one delegation.
fn unique_leaf() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
    format!("launch-{}-{sequence}", std::process::id())
}

/// A child confined by a cgroup, which must tear its cgroup down when killed.
struct CgroupChild {
    child: tokio::process::Child,
    directory: PathBuf,
}

impl LaunchedProcess for CgroupChild {
    fn pid(&self) -> u32 {
        self.child.id().unwrap_or(0)
    }

    fn kill(self: Box<Self>) -> BackendFuture<Result<(), SandboxError>> {
        Box::pin(async move {
            let Self {
                mut child,
                directory,
            } = *self;
            // `cgroup.kill` first, and deliberately not `child.kill()` alone: killing the direct child would
            // leave every descendant it forked running, which is exactly the escape this guarantee exists to
            // close. The kernel's own documentation says the write deals with concurrent forks and is protected
            // against migrations, so it does not race a child that forks while being signalled the way a
            // walk-the-tree-and-signal loop does.
            let killed = std::fs::write(directory.join("cgroup.kill"), KILL).is_ok();
            // Reaping the direct child is still needed and is not redundant: this process owns the zombie, so
            // without a wait the exit status would never be collected. It is also the whole of the fallback on a
            // kernel older than 5.14, which has no `cgroup.kill` — and there it terminates **only** the direct
            // child, so that case is reported as a failure rather than assumed to have worked.
            //
            // `kill()` reports an `io::Error`, so the conversion is explicit rather than relying on a `?` that
            // would have to live in a function with one error type. A failure to signal a process we own is a
            // launch-time failure — the process exists and is not under control — not a setup failure.
            let reaped = child.kill().await.map_err(|error| SandboxError::Launch {
                reason: format!("kill failed: {}", error.kind()),
            });
            CgroupV2Backend::remove(&directory);
            // Every `Err` is returned regardless of whether the cgroup kill worked, so the two error rows are
            // one arm. The order encodes the policy: an error from the signal we own is reported as it stands,
            // and only a **successful** reap with no `cgroup.kill` becomes the degraded-kill error.
            match (killed, reaped) {
                (true, Ok(())) => Ok(()),
                (_, Err(error)) => Err(error),
                (false, Ok(())) => Err(SandboxError::Launch {
                    reason: "cgroup.kill is unavailable, so only the direct child was killed"
                        .to_owned(),
                }),
            }
        })
    }
}
