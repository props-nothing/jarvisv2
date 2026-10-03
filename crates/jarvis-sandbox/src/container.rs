//! The container backend: a restricted worker with no FFI and no image of its own.
//!
//! # Why this backend exists
//!
//! `P3-011` recorded two limits that together leave the **development host** with nothing: the Windows and macOS
//! facilities are `unsafe` FFI (and this workspace sets `unsafe_code = "forbid"`), and the Linux cgroup backend
//! refuses on any host without a delegated cgroup — "a stock install, most containers, essentially every
//! interactive development shell". A container runtime is the one facility that reaches those hosts without FFI,
//! because it is driven by spawning a CLI.
//!
//! That is the entire reason this is possible, and it is also the reason it is dangerous to write: **every flag
//! is a third party's interface, and every claim about what the kernel enforces is that third party's claim
//! rather than this workspace's.** So the backend claims three of five guarantees and refuses the other two, and
//! the three are the ones whose flag the documentation describes as **container-scoped**.
//!
//! # The two it refuses, and why the reasons differ
//!
//! | Guarantee | Verdict | Reason |
//! | --- | --- | --- |
//! | [`Guarantee::CpuTimeCeiling`] | **cannot** | the only flag is `--ulimit cpu` = `RLIMIT_CPU`, **per process** — a tree of N processes gets N budgets |
//! | [`Guarantee::CpuRateCeiling`] | **not yet** | `--cpus` **can** express it; the rate-to-`--cpus` translation is simply unimplemented |
//!
//! The first is a fact about the facility, the second is a fact about this file, and conflating them would be the
//! worse mistake: claiming `CpuRateCeiling` before emitting the flag is the declared-but-unconstructed shape
//! `AGENTS.md` forbids, while *recording* it as unimplemented is the truth. `container_guarantees` in `backend`
//! holds the list, where a host with no container runtime can still falsify it.
//!
//! # Two decisions that are not guarantees, and are therefore not optional
//!
//! **`--read-only`, `--cap-drop ALL`, `--network none`, and `-i` are always passed.** None is a `Guarantee`,
//! because this crate models *resource ceilings* rather than a filesystem or network policy — but a container
//! without them is not "a restricted worker", so they are part of what the facility means rather than a policy a
//! caller selects. By contrast `--security-opt no-new-privileges` is real hardening that is deliberately **not**
//! passed: it is unmodelled, and enforcing something a caller never asked for and cannot see makes the sandbox's
//! behaviour a function of a flag they do not control.
//!
//! **The environment is replaced, not extended.** `SandboxRequest::environment` is the **complete** environment,
//! and `-e` **adds** to the image's own — probed, not assumed: `docker run -e FOO=bar alpine:3 env` still shows
//! the image's `PATH` and `HOME`. The launch therefore runs `env -i <pairs> <program> <args>`, which needs no
//! shell and so does not depend on any image shipping a particular `/bin/sh`.
//!
//! # The kill path is the part that would have been wrong
//!
//! `Child::kill` on the `docker` CLI would leave the **container** running, because the CLI is a client and the
// work happens in the daemon. `kill` therefore runs `docker rm -f <name>` — removing the container stops it —
// and then reaps the CLI. The container's generated name is a **handle** for that path, which is why the name
// must be unique per launch: a shared name would let one launch's kill stop another's container.

use std::process::Stdio;

use crate::backend::{
    BackendFuture, Completion, GuaranteeSupport, Launched, LaunchedProcess,
    MINIMUM_CONTAINER_MEMORY_BYTES, ProcessLauncher, SandboxBackend, Support, container_guarantees,
    container_image,
};
use crate::policy::{Limits, SandboxError, SandboxPolicy, SandboxRequest};

/// The runtime's program name.
///
/// A fixed name resolved through `PATH` rather than a configurable path, for the same reason the probe is a
/// runtime question: an operator who needs a different binary puts it on `PATH`, and a configured absolute path
/// is a second place the two can disagree.
const RUNTIME: &str = "docker";

/// The program that establishes the environment boundary, resolved **inside** the image.
const ENVIRONMENT_BOUNDARY: &str = "env";

/// A container backend, holding the server version it found.
///
/// The version is `Option` and the support set is derived from it at the same moment, so this type cannot carry
/// a claim a later `launch` disagrees with — the same shape as the cgroup backend's delegation root.
pub struct ContainerBackend {
    server_version: Option<String>,
}

impl std::fmt::Debug for ContainerBackend {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ContainerBackend")
            .field("server_version", &self.server_version)
            .finish()
    }
}

impl ContainerBackend {
    /// Probes the host for a reachable container runtime.
    ///
    /// # What the probe asks, and why it is that question rather than a simpler one
    ///
    /// It runs `docker info --format "{{.ServerVersion}}"` and requires **exit 0 with non-empty stdout**.
    /// Asking `docker --version` would answer a different question: the client binary can be installed with no
    /// daemon running, no permission to reach it, or a runtime that cannot start a container — and "Docker
    /// Desktop is installed" is not "a container can be launched". `docker info` is the cheapest command that
    /// **requires a server round trip**, and `--format` keeps the output a single parseable value rather than a
    /// report to be scraped (probed: it emits the version with no trailing warning).
    ///
    /// `stdout` is read and **not** inherited, so the probe is silent: this runs inside `doctor` and a launch
    /// path, and a child process writing to the daemon's console is not something an operator asked for.
    /// `stderr` is inherited rather than piped, because a run-time-level failure to reach a runtime is worth
    /// seeing once at probe time, and capturing it to discard it would be work whose only effect is to hide it.
    ///
    /// A probe cannot report *which* of those causes failed without parsing a message that is not a stable
    /// interface, so it reports the one thing it knows: that the runtime is not usable. [`Self::reason`] carries
    /// the distinction a caller needs to act.
    #[must_use]
    pub fn probe() -> Self {
        let probed = std::process::Command::new(RUNTIME)
            .args(["info", "--format", "{{.ServerVersion}}"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .output();
        match probed {
            Ok(output) if output.status.success() => {
                let version = String::from_utf8_lossy(&output.stdout).trim().to_owned();
                // A version string that is empty means the format rendered nothing, which is not an answer even
                // though the exit status was zero. `None` rather than `Some("")`, because a support set derived
                // from an empty string would be a claim derived from nothing.
                if version.is_empty() {
                    Self {
                        server_version: None,
                    }
                } else {
                    Self {
                        server_version: Some(version),
                    }
                }
            }
            _ => Self {
                server_version: None,
            },
        }
    }

    /// Returns why no runtime was usable, for a `Setup` refusal's `reason`.
    ///
    /// `Setup` rather than `Launch`, because everything it can say is about the **facility**: whether the probe
    /// ran at all, and whether it succeeded.
    fn reason() -> String {
        format!("no reachable container runtime: `{RUNTIME} info` did not report a server version")
    }

    /// Returns whether this backend found a usable runtime.
    #[must_use]
    pub(crate) fn available(&self) -> bool {
        self.server_version.is_some()
    }
}

impl SandboxBackend for ContainerBackend {
    fn support(&self) -> GuaranteeSupport {
        if !self.available() {
            return GuaranteeSupport::default();
        }
        // Derived from the same value `launch` reads, so a support claim and a launch cannot disagree. The list
        // lives in `backend`, where a host without a runtime can still test it — see `container_guarantees`.
        GuaranteeSupport::from_guarantees(container_guarantees())
    }

    fn facility(&self) -> Support {
        Support::Container
    }

    fn launch(
        &self,
        policy: &SandboxPolicy,
        launcher: ProcessLauncher,
    ) -> BackendFuture<Result<Launched, SandboxError>> {
        let enforced = policy.enforced();
        if !self.available() {
            return Box::pin(async move {
                Err(SandboxError::Setup {
                    facility: FACILITY,
                    reason: Self::reason(),
                })
            });
        }
        // Checked here rather than in `SandboxPolicy::new` because the requirement belongs to *this* facility:
        // the cgroup backend has no use for an image and must not be made to require one. The complement — a
        // request **without** an image reaching this backend — is refused for the mirror-image reason: a host path
        // is not an image reference, and substituting one would make the two interchangeable.
        let image = match container_image(policy.request()) {
            Ok(image) => image.to_owned(),
            Err(error) => return Box::pin(async move { Err(error) }),
        };
        let arguments = match container_arguments(policy.request()) {
            Ok(arguments) => arguments,
            Err(error) => return Box::pin(async move { Err(error) }),
        };
        // The runtime's argv is built **here, once**. A container's name is a handle the kill path holds, and it
        // is part of this argv, so building it a second time anywhere would generate a second name — which is
        // precisely the defect the acceptance test found: the container was launched under one name while `kill`
        // removed another, leaving it behind in state `Created` while the kill reported success.
        let command = runtime_command(&arguments);
        Box::pin(async move {
            let mut child = launcher(command).map_err(|error| SandboxError::Launch {
                reason: format!("could not start `{RUNTIME}`: {}", error.kind()),
            })?;
            let stdin = child.stdin.take();
            let stdout = child.stdout.take();
            let stderr = child.stderr.take();
            Ok(Launched {
                process: Box::new(ContainerChild {
                    child,
                    // The handle the kill path holds. Read back out of the argv the launch actually used — the
                    // only place the name exists — so the two cannot be two spellings that must be kept equal.
                    container: container_name(&arguments, &image),
                }),
                support: Support::Container,
                enforced,
                stdin,
                stdout,
                stderr,
            })
        })
    }
}

/// The facility label used in refusals, matching [`Support::Container::as_str`].
const FACILITY: &str = "container";

/// Builds the command that runs a container for a request.
///
/// # Why the backend builds this rather than the caller's launcher
///
/// A container request's `program` names something **inside the image**, so the executable on this host is the
/// **runtime** and the program appears in the container's argv. The first implementation left the command to the
/// caller's launcher, which started `request.program` and failed with a bare `entity not found` — and, worse,
/// forced this argv to be built a second time inside that launcher, running the name generator twice. See
/// [`crate::ProcessLauncher`] for the three failures that came from splitting the command from the confinement.
///
/// # What is deliberately not inherited from the host
///
/// The runtime's own environment is **not** cleared, unlike a host-process child's. Its environment is not the
/// confined child's — the container's is, established by `env -i` inside the image — and a runtime that could not
/// find its socket or credential helper would fail for a reason no caller could act on. That asymmetry is
/// deliberate and is the one place the two kinds of launch differ beyond the argv.
///
/// A request's `working_directory` is likewise not applied: it names a directory inside the image, and the
/// runtime's own working directory has no bearing on that. Honouring it here would silently change the runtime's
/// context instead, which is not what a caller asked for.
#[must_use]
pub(crate) fn runtime_command(arguments: &[String]) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(RUNTIME);
    command.args(arguments);
    command
}

/// Builds the `docker run` argv for a request, refusing anything the backend cannot express.
///
/// # Errors
///
/// Returns [`SandboxError::Launch`] for a memory ceiling outside what the runtime accepts, naming the value. It is
/// a **launch** refusal rather than a setup one because the facility is fine and the request is not, and it is
/// raised here rather than in [`SandboxPolicy::new`] because the bound is the runtime's rather than the crate's.
pub(crate) fn container_arguments(request: &SandboxRequest) -> Result<Vec<String>, SandboxError> {
    // The image was already checked by `container_image`, so a missing one cannot reach here; this re-read is
    // what lets the argv be built from the request alone, and it is deliberately the *same* accessor.
    let image = container_image(request)?;
    let mut arguments = vec![
        "run".to_owned(),
        // Auto-removes the container on exit, so a completed launch leaves nothing accumulating on the host.
        "--rm".to_owned(),
        "--name".to_owned(),
        container_name_for(request, image),
        // Not a `Guarantee`: a container that can reach the network is not an isolated worker. `none` is the
        // only network mode that is not a policy choice, which is why it is fixed rather than configurable.
        "--network".to_owned(),
        "none".to_owned(),
        // The root filesystem read-only, again not a `Guarantee` but part of what the facility means. Probed:
        // `touch /x` fails with `Read-only file system`.
        "--read-only".to_owned(),
        // The default already drops most capabilities; this drops all of them. A confined worker needs none, and
        // a capability is authority this crate's model has no way to express, so keeping any would be unmodelled
        // privilege.
        "--cap-drop".to_owned(),
        "ALL".to_owned(),
        // Keeps stdin open. Without it `run` closes stdin immediately, so a child that reads stdin — an MCP
        // server, which is the caller this crate exists for — sees EOF and exits.
        "-i".to_owned(),
        // No image is ever pulled. Pulling is a network fetch of content no operator chose, and a sandbox that
        // reaches a registry on demand is a sandbox whose contents nobody reviewed. Probed: an absent image is
        // refused (`No such image`), which is the correct outcome.
        "--pull".to_owned(),
        "never".to_owned(),
    ];
    apply_limits(request.limits, &mut arguments)?;
    arguments.push(image.to_owned());
    arguments.extend(container_command(request));
    Ok(arguments)
}

/// Appends the limit flags a request asks for, refusing a memory ceiling the runtime will not accept.
///
/// Each flag is passed **only** when the corresponding limit is present, mirroring `apply_limits` in `linux.rs`:
/// an unconditional flag would express a limit the caller did not ask for, and `--memory=0` in particular means
/// *unlimited* rather than "none".
fn apply_limits(limits: Limits, arguments: &mut Vec<String>) -> Result<(), SandboxError> {
    if let Some(maximum) = limits.max_processes {
        // Container-scoped, which is the whole reason this guarantee is claimed: the container's own pid
        // namespace and cgroup make `--pids-limit` a tree-wide counter, unlike `--ulimit nproc`, which Linux
        // applies **per user** so four containers under one uid share one budget.
        arguments.push("--pids-limit".to_owned());
        arguments.push(maximum.to_string());
    }
    if let Some(maximum) = limits.max_memory_bytes {
        // # Two refusals that look like one, and are not
        //
        // **Below the floor** the runtime rejects the launch itself (probed: exit 125, "Minimum memory limit
        // allowed is 6MB"), so passing it on would turn a policy error into an opaque launch failure an operator
        // could not act on.
        //
        // **Zero** is the case that *looks* like it is covered by that check and deserves separate words. The
        // runtime **accepts** `--memory=0` and reads it as *unlimited* (probed: exit 0), so the caller who wrote
        // zero gets the opposite of what they wrote. It is arithmetically inside the "below the floor" branch, and
        // it is still refused for its own reason — and a mutation that deleted the explicit zero test left every
        // assertion passing, which is why the *message* is where the distinction is now made rather than a
        // redundant disjunct. The check is written against the floor alone so there is one condition to be wrong,
        // and the zero case is named in the message so an operator who made the mistake is told which one it was.
        //
        // The general rule this is an instance of: an absent limit in this crate is *absent* rather than zero,
        // everywhere, so the two are never conflated — and a zero arriving here is not a small ceiling but the
        // inversion of the field's meaning.
        if maximum < MINIMUM_CONTAINER_MEMORY_BYTES {
            let zero_note = if maximum == 0 {
                " (zero is refused because the runtime reads it as *unlimited*, which is the opposite of what a ceiling of zero means)"
            } else {
                ""
            };
            return Err(SandboxError::Launch {
                reason: format!(
                    "a container memory ceiling must be at least {MINIMUM_CONTAINER_MEMORY_BYTES} bytes, got {maximum}{zero_note}"
                ),
            });
        }
        arguments.push("--memory".to_owned());
        arguments.push(maximum.to_string());
    }
    // `limits.max_cpu_rate` is deliberately not translated — see the module docs. `max_cpu_time` is deliberately
    // not translated either, and cannot be: `--ulimit cpu` is `RLIMIT_CPU`, per process, so a tree of N
    // processes gets N budgets rather than one. Both are refused before this point if the caller *required* the
    // corresponding guarantee, because neither is in `container_guarantees`.
    Ok(())
}

/// The command the container runs: the environment boundary, then the program and its arguments.
///
/// A request with a **non-empty** environment is run as `env -i <pairs> <program> <args…>`, which starts from an
/// empty environment and applies exactly the request's pairs. A request with an **empty** environment passes no
/// `env` at all and runs the program directly.
///
/// # Why the empty case is different rather than handled uniformly
///
/// `env -i <program>` and `<program>` differ only when the image's own environment is non-empty, and then they
/// differ a lot: `env -i` removes the image's `PATH`, which breaks any program that resolves a helper by name.
/// The two branches are not an optimisation — they encode which the caller stated. An empty `environment` on a
/// request says "I stated nothing", and the honest reading of that is the image's default rather than
/// "I stated nothing, therefore remove everything"; a caller who wants an empty environment states a request
/// whose semantics this crate documents as complete. It is a real asymmetry and it is recorded here rather than
/// left for a caller to infer from behaviour.
fn container_command(request: &SandboxRequest) -> Vec<String> {
    let program = request.program.to_string_lossy().into_owned();

    if request.environment.is_empty() {
        let mut command = vec![program];
        command.extend(request.arguments.iter().cloned());
        return command;
    }
    let mut command = vec![ENVIRONMENT_BOUNDARY.to_owned(), "-i".to_owned()];
    for (name, value) in &request.environment {
        // `env` takes `NAME=VALUE` arguments, so the two are joined rather than passed separately. `env` itself
        // splits on the **first** `=`, so a value containing `=` is carried correctly; the name cannot contain
        // one, and a caller that supplied such a name would produce an argument `env` treats as a program name —
        // which fails loudly rather than mis-assigning a variable.
        command.push(format!("{name}={value}"));
    }
    command.push(program);
    command.extend(request.arguments.iter().cloned());
    command
}

/// A per-launch container name, unique within this process.
///
/// The same shape as `linux.rs`'s `unique_leaf`, for the same reasons: a monotonic counter cannot repeat within
/// a process, where a timestamp can collide at the resolution a virtualised host reports; the pid disambiguates
/// **across** processes, which matters when two daemons share a host and both hold a runtime.
///
/// The name is a handle rather than a label. `--rm` removes a container when it exits, so a name is only live
/// while a launch is, and the kill path depends on the name it was launched with.
fn container_name_for(request: &SandboxRequest, image: &str) -> String {
    format!("{PREFIX}{}-{}", std::process::id(), leaf(image, request))
}

/// The prefix every generated name carries, so an operator can see which containers JARVIS owns.
const PREFIX: &str = "jarvis-sandbox-";

/// A monotonic per-process sequence, shared by naming and by nothing else.
fn next_sequence() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Builds the sequence-bearing part of a container name.
fn leaf(image: &str, request: &SandboxRequest) -> String {
    // The image and program are included so an operator running `docker ps` sees *what* is running rather than
    // an opaque number. Sanitised because a container name is restricted to `[a-zA-Z0-9][a-zA-Z0-9_.-]*` — an
    // image reference may contain `/` and a `:`, which the runtime rejects — and a rejected name would turn a
    // naming detail into a launch failure.
    let digest = format!("{image}-{}", request.program.to_string_lossy());
    let sanitised: String = digest
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric()
                || character == '_'
                || character == '.'
                || character == '-'
            {
                character
            } else {
                '-'
            }
        })
        .collect();
    format!("{}-{}", sanitised, next_sequence())
}

/// Recovers the container name from a built argv.
///
/// # Why this reads the argv instead of being passed the name separately
///
/// The name must appear in `--name` **and** be held by the kill path. Threading it separately would make those
/// two a pair that has to be kept equal by hand, and a divergence there is exactly the failure that leaves a
/// container running while `kill` reports success. Reading it back out of the argv makes them one value by
/// construction — the argv is the only place the name exists until the child is spawned.
///
/// `--name` is emitted immediately after `run` for every argv this module builds, and the index is *found*
/// rather than assumed so a future reordering cannot silently return the wrong element (the image, say, or a
/// limit value) as a container name.
pub(crate) fn container_name(arguments: &[String], image: &str) -> String {
    let position = arguments.iter().position(|argument| argument == "--name");
    match position.and_then(|index| arguments.get(index + 1)) {
        Some(name) => name.clone(),
        // Unreachable for an argv this module builds, and a fallback rather than a panic: this function's result
        // is only used by the kill path, whose worst case is removing nothing, and a panic inside a process
        // supervisor would take down the daemon rather than one confined child. The image is a safe placeholder
        // because `docker rm -f <image>` cannot remove an image — it names no container.
        None => image.to_owned(),
    }
}

/// A container under a policy, which must remove its container to be killed.
struct ContainerChild {
    child: tokio::process::Child,
    /// The `--name` this launch used, which is the handle `docker rm -f` needs.
    container: String,
}

impl LaunchedProcess for ContainerChild {
    fn pid(&self) -> u32 {
        // The CLI's pid, because the CLI **is** the process this value owns: `docker run` waits for the
        // container and propagates its exit status, so the two are the same lifetime. The container's own pid
        // would name something this process cannot reap.
        self.child.id().unwrap_or(0)
    }

    fn wait(self: Box<Self>) -> BackendFuture<Result<std::process::ExitStatus, SandboxError>> {
        Box::pin(async move {
            let Self {
                mut child,
                container,
            } = *self;
            // The CLI's status **is** the container's status, because `run` waits for the container and
            // propagates its exit code (probed: `exit 7` → shell `7`). That is the property that makes a launch
            // supervisable, and it is asserted by the acceptance test rather than assumed here.
            //
            // A failed wait is **not** a failed program: the status carries the program's outcome, so an error
            // here means this process could not collect the child. The container name is included because a
            // child that cannot be collected may still be running, and the name is what an operator needs to
            // find it.
            child.wait().await.map_err(|error| SandboxError::Launch {
                reason: format!(
                    "could not collect the status of container {container}: {}",
                    error.kind()
                ),
            })
        })
    }

    fn wait_for(
        self: Box<Self>,
        limit: std::time::Duration,
    ) -> BackendFuture<Result<Completion, SandboxError>> {
        Box::pin(async move {
            let Self {
                mut child,
                container,
            } = *self;
            if let Ok(status) = tokio::time::timeout(limit, child.wait()).await {
                return status
                    .map(Completion::Exited)
                    .map_err(|error| SandboxError::Launch {
                        reason: format!(
                            "could not collect the status of container {container}: {}",
                            error.kind()
                        ),
                    });
            }
            // The limit passed. Removing the **container** is what stops the work — killing the CLI alone would
            // leave it running in the daemon, a confined process outliving its confinement. The CLI is reaped
            // afterwards, and a failed removal is the failure worth reporting.
            let removed = removal(&container).await;
            let _ = child.kill().await;
            removed.map(|()| Completion::TimedOut)
        })
    }

    fn kill(self: Box<Self>) -> BackendFuture<Result<(), SandboxError>> {
        Box::pin(async move {
            let Self {
                mut child,
                container,
            } = *self;
            // Removing the container is what stops it, and it is deliberately not `child.kill()` alone. The CLI
            // is a client: killing it leaves the container running in the daemon, which is a confined process
            // outliving its confinement — worse than no sandbox, because the caller believes it stopped.
            //
            // `-f` is required rather than merely convenient: without it a *running* container is refused and
            // only a stopped one is removed, which is the opposite of what this call is for.
            let removed = removal(&container).await;
            // Reaping the CLI is still needed and is not redundant: this process owns the zombie, so without a
            // wait the exit status is never collected. It is also the fallback when removal failed.
            let reaped = child.kill().await.map_err(|error| SandboxError::Launch {
                reason: format!("could not stop the container run: {}", error.kind()),
            });
            // Every `Err` is returned regardless of whether the removal worked, so the two error rows are one
            // arm. The order encodes the policy: a failure to remove the container is the failure worth
            // reporting, because it is the one that leaves work running.
            match (removed, reaped) {
                (Ok(()), Ok(())) => Ok(()),
                (Err(error), _) | (Ok(()), Err(error)) => Err(error),
            }
        })
    }
}

/// Runs `docker rm -f <name>`, whose success means the container is gone.
///
/// `stdout` and `stderr` are **inherited** rather than captured: this command runs in a kill path that has
/// nowhere to put a message, and a runtime that cannot remove a container prints the reason an operator needs.
/// Capturing it to build a `reason` string would replace a diagnostic with a summary of one.
async fn removal(container: &str) -> Result<(), SandboxError> {
    let status = tokio::process::Command::new(RUNTIME)
        .args(["rm", "-f", container])
        .stdin(Stdio::null())
        .status()
        .await
        .map_err(|error| SandboxError::Launch {
            reason: format!("could not run `{RUNTIME} rm -f`: {}", error.kind()),
        })?;
    if status.success() {
        return Ok(());
    }
    Err(SandboxError::Launch {
        reason: format!(
            "`{RUNTIME} rm -f {container}` failed, so the container may still be running"
        ),
    })
}
